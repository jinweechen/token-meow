use super::super::adapter::Counts;
use super::*;
use std::collections::BTreeMap;

fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
        .unwrap();
    db
}

fn payload() -> Value {
    json!({
        "model":"test-model", "providerId":null,
        "effectiveFrom":"2026-10-01T00:00:00Z",
        "inputPerMillion":"2", "outputPerMillion":"10",
        "cacheReadPerMillion":"0.2", "cacheWritePerMillion":"1"
    })
}

fn response() -> ResponseUsage {
    ResponseUsage {
        source_id: "test-source".into(),
        channel_id: "log".into(),
        namespace: "codex-thread-response-v1".into(),
        thread_id: "thread".into(),
        response_id: "response".into(),
        turn_id: None,
        root_turn_id: None,
        event_time: "2026-10-16T12:00:00Z".into(),
        collected_at: "2026-10-16T12:01:00Z".into(),
        requested_model: Some("test-model".into()),
        observed_model: None,
        provider_id: None,
        account_scope_id: None,
        native_counts: BTreeMap::from([
            ("input_tokens".into(), Metric::Valid(1000)),
            ("output_tokens".into(), Metric::Valid(200)),
            ("cached_input_tokens".into(), Metric::Valid(600)),
            ("cache_write_input_tokens".into(), Metric::Valid(100)),
            ("total_tokens".into(), Metric::Valid(1200)),
        ]),
        counts: Counts {
            input: Metric::Valid(1000),
            output: Metric::Valid(200),
            cache_read: Metric::Valid(600),
            cache_write: Metric::Valid(100),
            reasoning: Metric::Missing,
            total: Metric::Valid(1200),
        },
        semantics_version: SEMANTICS.into(),
        adapter_version: "codex-0.147-v1".into(),
        completeness: "source-recorded".into(),
        phase: "final".into(),
        revision: None,
        live: false,
    }
}

#[test]
fn cache_buckets_are_subsets_of_input_and_never_added_twice() {
    let db = database();
    let version = save(&db, &payload()).unwrap();
    let cost = estimate(&[version.clone()], &response()).unwrap();
    // 300 fresh * $2 + 200 output * $10 + 600 read * $.2 + 100 write * $1.
    assert_eq!(cost.estimated_cost_usd.as_deref(), Some("0.00282"));
    assert_eq!(cost.units, Some(2_820_000_000));
    assert_eq!(cost.pricing_version_id, Some(version.id));
    assert_eq!(cost.reason, None);
    let value = serde_json::to_value(cost).unwrap();
    assert!(value.get("units").is_none());
    assert_eq!(value["estimatedCostUsd"], "0.00282");
}

#[test]
fn event_time_and_revision_choose_price_not_collection_time() {
    let db = database();
    let old = save(&db, &payload()).unwrap();
    let mut changed = payload();
    changed["effectiveFrom"] = json!("2026-10-16T20:00:00+08:00");
    changed["inputPerMillion"] = json!("4");
    let current = save(&db, &changed).unwrap();
    let all = versions(&db).unwrap();
    let mut r = response();
    r.event_time = "2026-10-16T11:59:59Z".into();
    assert_eq!(estimate(&all, &r).unwrap().pricing_version_id, Some(old.id));
    r.event_time = "2026-10-16T12:00:00Z".into();
    assert_eq!(
        estimate(&all, &r).unwrap().pricing_version_id,
        Some(current.id)
    );
    changed["inputPerMillion"] = json!("5");
    let revised = save(&db, &changed).unwrap();
    assert_eq!(revised.revision, 3);
    assert_eq!(
        estimate(&versions(&db).unwrap(), &r)
            .unwrap()
            .pricing_version_id,
        Some(revised.id)
    );
    assert_eq!(versions(&db).unwrap().len(), 3);
}

#[test]
fn exact_provider_precedes_generic_without_cross_provider_leakage() {
    let db = database();
    let generic = save(&db, &payload()).unwrap();
    let mut exact = payload();
    exact["providerId"] = json!("provider-a");
    exact["effectiveFrom"] = json!("2026-09-01T00:00:00Z");
    let scoped = save(&db, &exact).unwrap();
    let all = versions(&db).unwrap();
    let mut r = response();
    r.provider_id = Some("provider-a".into());
    assert_eq!(
        estimate(&all, &r).unwrap().pricing_version_id,
        Some(scoped.id.clone())
    );
    r.provider_id = Some("provider-b".into());
    assert_eq!(
        estimate(&all, &r).unwrap().pricing_version_id,
        Some(generic.id.clone())
    );
    r.provider_id = None;
    assert_eq!(
        estimate(&all, &r).unwrap().pricing_version_id,
        Some(generic.id)
    );
    assert_eq!(
        estimate(&[scoped], &r).unwrap().reason.as_deref(),
        Some("no_matching_price")
    );
}

#[test]
fn observed_model_is_authoritative_and_requested_model_is_only_fallback() {
    let db = database();
    save(&db, &payload()).unwrap();
    let mut r = response();
    r.observed_model = Some("unpriced-actual-model".into());
    assert_eq!(
        estimate(&versions(&db).unwrap(), &r)
            .unwrap()
            .reason
            .as_deref(),
        Some("no_matching_price")
    );
    r.observed_model = Some(String::new());
    assert!(estimate(&versions(&db).unwrap(), &r)
        .unwrap()
        .units
        .is_some());
    r.requested_model = None;
    assert_eq!(
        estimate(&versions(&db).unwrap(), &r)
            .unwrap()
            .reason
            .as_deref(),
        Some("unknown_model")
    );
}

#[test]
fn zero_price_and_missing_price_are_distinct() {
    let db = database();
    let mut free = payload();
    for field in [
        "inputPerMillion",
        "outputPerMillion",
        "cacheReadPerMillion",
        "cacheWritePerMillion",
    ] {
        free[field] = json!("0");
    }
    save(&db, &free).unwrap();
    let priced = estimate(&versions(&db).unwrap(), &response()).unwrap();
    assert_eq!(priced.estimated_cost_usd.as_deref(), Some("0"));
    assert_eq!(priced.units, Some(0));
    assert!(priced.pricing_version_id.is_some());
    let missing = estimate(&[], &response()).unwrap();
    assert_eq!(missing.estimated_cost_usd, None);
    assert_eq!(missing.units, None);
    assert_eq!(missing.reason.as_deref(), Some("no_matching_price"));
}

#[test]
fn missing_unknown_invalid_and_negative_counts_cannot_become_free_usage() {
    let db = database();
    save(&db, &payload()).unwrap();
    let all = versions(&db).unwrap();
    for metric in [
        Metric::Missing,
        Metric::Unknown,
        Metric::Invalid,
        Metric::Valid(-1),
    ] {
        for field in 0..5 {
            let mut r = response();
            match field {
                0 => r.counts.input = metric.clone(),
                1 => r.counts.output = metric.clone(),
                2 => r.counts.cache_read = metric.clone(),
                3 => r.counts.cache_write = metric.clone(),
                _ => r.counts.total = metric.clone(),
            }
            let cost = estimate(&all, &r).unwrap();
            assert!(cost.units.is_none());
            assert_eq!(
                cost.reason.as_deref(),
                Some("incomplete_or_invalid_token_counts")
            );
        }
    }
}

#[test]
fn native_missing_or_disagreeing_counts_are_excluded() {
    let db = database();
    save(&db, &payload()).unwrap();
    let all = versions(&db).unwrap();
    for key in [
        "input_tokens",
        "output_tokens",
        "cached_input_tokens",
        "cache_write_input_tokens",
    ] {
        let mut r = response();
        r.native_counts.remove(key);
        assert_eq!(
            estimate(&all, &r).unwrap().reason.as_deref(),
            Some("incomplete_or_invalid_native_counts")
        );
        r.native_counts.insert(key.into(), Metric::Valid(9999));
        assert!(estimate(&all, &r).unwrap().units.is_none());
    }
}

#[test]
fn absent_native_total_uses_adapter_derived_total_but_bad_native_total_stays_unpriced() {
    let db = database();
    save(&db, &payload()).unwrap();
    let all = versions(&db).unwrap();
    let mut r = response();
    r.native_counts.remove("total_tokens");
    assert_eq!(
        estimate(&all, &r).unwrap().estimated_cost_usd.as_deref(),
        Some("0.00282")
    );
    r.native_counts
        .insert("total_tokens".into(), Metric::Missing);
    assert!(estimate(&all, &r).unwrap().units.is_some());
    for metric in [
        Metric::Unknown,
        Metric::Invalid,
        Metric::Valid(-1),
        Metric::Valid(1201),
    ] {
        r.native_counts.insert("total_tokens".into(), metric);
        let cost = estimate(&all, &r).unwrap();
        assert!(cost.units.is_none());
        assert_eq!(
            cost.reason.as_deref(),
            Some("incomplete_or_invalid_native_counts")
        );
    }
}

#[test]
fn unknown_semantics_intermediate_records_and_inconsistent_totals_are_excluded() {
    let db = database();
    save(&db, &payload()).unwrap();
    let all = versions(&db).unwrap();
    for field in 0..3 {
        let mut r = response();
        match field {
            0 => r.semantics_version = "future-v99".into(),
            1 => r.phase = "streaming".into(),
            _ => r.completeness = "unknown".into(),
        }
        assert_eq!(
            estimate(&all, &r).unwrap().reason.as_deref(),
            Some("unsupported_usage_semantics")
        );
    }
    let mut r = response();
    r.counts.total = Metric::Valid(1201);
    assert_eq!(
        estimate(&all, &r).unwrap().reason.as_deref(),
        Some("inconsistent_token_counts")
    );
    r = response();
    r.counts.cache_write = Metric::Valid(500);
    assert_eq!(
        estimate(&all, &r).unwrap().reason.as_deref(),
        Some("inconsistent_token_counts")
    );
    r = response();
    r.event_time = "not-a-time".into();
    assert_eq!(
        estimate(&all, &r).unwrap().reason.as_deref(),
        Some("invalid_event_time")
    );
}

#[test]
fn sub_micro_dollar_costs_remain_exact_and_large_counts_do_not_overflow() {
    let db = database();
    let mut p = payload();
    p["inputPerMillion"] = json!("0.000001");
    let small = save(&db, &p).unwrap();
    let mut r = response();
    r.counts.input = Metric::Valid(1);
    r.counts.output = Metric::Valid(0);
    r.counts.cache_read = Metric::Valid(0);
    r.counts.cache_write = Metric::Valid(0);
    r.counts.total = Metric::Valid(1);
    r.native_counts
        .insert("input_tokens".into(), Metric::Valid(1));
    r.native_counts
        .insert("output_tokens".into(), Metric::Valid(0));
    r.native_counts
        .insert("cached_input_tokens".into(), Metric::Valid(0));
    r.native_counts
        .insert("cache_write_input_tokens".into(), Metric::Valid(0));
    r.native_counts
        .insert("total_tokens".into(), Metric::Valid(1));
    assert_eq!(
        estimate(&[small], &r)
            .unwrap()
            .estimated_cost_usd
            .as_deref(),
        Some("0.000000000001")
    );
    p["inputPerMillion"] = json!("1000000");
    let maximum = save(&db, &p).unwrap();
    r.counts.input = Metric::Valid(i64::MAX);
    r.counts.total = Metric::Valid(i64::MAX);
    r.native_counts
        .insert("input_tokens".into(), Metric::Valid(i64::MAX));
    r.native_counts
        .insert("total_tokens".into(), Metric::Valid(i64::MAX));
    let large = estimate(&[maximum], &r).unwrap();
    assert_eq!(
        large.estimated_cost_usd.as_deref(),
        Some("9223372036854775807")
    );
    assert_eq!(large.units, Some(i128::from(i64::MAX) * USD_UNITS));
    assert_eq!(format_usd(USD_UNITS + 1), "1.000000000001");
}

#[test]
fn malformed_prices_and_unknown_form_fields_never_mutate_saved_history() {
    let db = database();
    save(&db, &payload()).unwrap();
    let original = get_setting(&db, SETTING_KEY).unwrap();
    for invalid in [
        "-1",
        "NaN",
        "Infinity",
        "1e3",
        "1.0000001",
        "1000000.000001",
        "999999999999999999999999999999",
        " 1",
        "1 ",
        ".1",
        "1.",
        "+1",
        "01",
    ] {
        let mut p = payload();
        p["inputPerMillion"] = json!(invalid);
        assert!(save(&db, &p).is_err(), "accepted {invalid}");
    }
    let mut p = payload();
    p["outputPerMillion"] = json!(3.0);
    assert!(save(&db, &p).is_err());
    p = payload();
    p["deleteExisting"] = json!(true);
    assert!(save(&db, &p).is_err());
    p = payload();
    p["revision"] = json!(0);
    assert!(save(&db, &p).is_err());
    assert_eq!(get_setting(&db, SETTING_KEY).unwrap(), original);
}

#[test]
fn invalid_identity_dates_and_lengths_are_rejected() {
    let db = database();
    for (field, value) in [
        ("model", "".into()),
        ("model", "unknown".into()),
        ("model", " model".into()),
        ("model", "line\nbreak".into()),
        ("model", "x".repeat(161)),
        ("providerId", "".into()),
        ("effectiveFrom", "2026-10-16".into()),
        ("effectiveFrom", "2026-10-16T12:00:00".into()),
    ] {
        let mut p = payload();
        p[field] = json!(value);
        assert!(save(&db, &p).is_err(), "accepted {field}: {value}");
    }
    assert!(versions(&db).unwrap().is_empty());
}

#[test]
fn future_or_corrupt_persistent_schema_fails_closed_and_preserves_original() {
    let db = database();
    for raw in [
        json!({"schemaVersion":2,"revision":5,"versions":[],"future":"preserve"}),
        json!({"schemaVersion":1,"revision":5,"versions":[]}),
        json!({"schemaVersion":1,"revision":0,"versions":[],"unknownField":true}),
        json!({"versions":[]}),
        Value::Null,
    ] {
        put_setting(&db, SETTING_KEY, &raw).unwrap();
        assert!(versions(&db).is_err());
        assert!(save(&db, &payload()).is_err());
        assert_eq!(get_setting(&db, SETTING_KEY).unwrap(), Some(raw));
    }
}

#[test]
fn duplicate_ids_or_non_monotonic_revisions_in_saved_store_are_rejected() {
    let db = database();
    save(&db, &payload()).unwrap();
    save(&db, &payload()).unwrap();
    let original = get_setting(&db, SETTING_KEY).unwrap().unwrap();
    let mut duplicate = original.clone();
    duplicate["versions"][1]["id"] = duplicate["versions"][0]["id"].clone();
    put_setting(&db, SETTING_KEY, &duplicate).unwrap();
    assert!(versions(&db).is_err());
    let mut non_monotonic = original;
    non_monotonic["versions"][1]["revision"] = json!(1);
    put_setting(&db, SETTING_KEY, &non_monotonic).unwrap();
    assert!(versions(&db).is_err());
}

#[test]
fn append_only_versions_survive_database_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pricing.sqlite3");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
        .unwrap();
    let first = save(&db, &payload()).unwrap();
    drop(db);
    let reopened = Connection::open(&path).unwrap();
    let second = save(&reopened, &payload()).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second.revision, first.revision + 1);
    let all = versions(&reopened).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(
        serde_json::to_value(&all[0]).unwrap(),
        serde_json::to_value(first).unwrap()
    );
    assert_eq!(
        estimate(&all, &response()).unwrap().pricing_version_id,
        Some(second.id)
    );
}

fn sync_item(model: &str) -> Value {
    let mut item = payload();
    item["model"] = json!(model);
    item["catalogProvider"] = json!("catalog-provider");
    item["catalogModel"] = json!(format!("api/{model}"));
    item
}

fn synchronize(
    db: &mut Connection,
    items: &[Value],
    at: &str,
    overwrite: bool,
) -> Result<(usize, usize)> {
    let tx = db.transaction().map_err(err)?;
    let counts = sync_prices(&tx, items, at, overwrite)?;
    tx.commit().map_err(err)?;
    Ok(counts)
}

#[test]
fn unknown_cache_prices_are_preserved_and_only_block_nonzero_cache_usage() {
    let db = database();
    let mut p = payload();
    p["cacheReadPerMillion"] = Value::Null;
    p["cacheWritePerMillion"] = Value::Null;
    let version = save(&db, &p).unwrap();
    assert!(version.cache_read_per_million.is_none());
    assert!(version.cache_write_per_million.is_none());
    assert!(version.origin.is_none());
    let mut r = response();
    assert_eq!(
        estimate(&[version.clone()], &r)
            .unwrap()
            .pricing_version_id
            .as_deref(),
        Some(version.id.as_str())
    );
    assert_eq!(
        estimate(&[version.clone()], &r).unwrap().reason.as_deref(),
        Some("missing_cache_read_price")
    );
    r.counts.cache_read = Metric::Valid(0);
    r.native_counts
        .insert("cached_input_tokens".into(), Metric::Valid(0));
    assert_eq!(
        estimate(&[version.clone()], &r).unwrap().reason.as_deref(),
        Some("missing_cache_write_price")
    );
    r.counts.cache_write = Metric::Valid(0);
    r.native_counts
        .insert("cache_write_input_tokens".into(), Metric::Valid(0));
    assert_eq!(
        estimate(&[version.clone()], &r)
            .unwrap()
            .estimated_cost_usd
            .as_deref(),
        Some("0.004")
    );
    r.counts.cache_write = Metric::Missing;
    r.native_counts
        .insert("cache_write_input_tokens".into(), Metric::Missing);
    assert_eq!(
        estimate(&[version], &r).unwrap().reason.as_deref(),
        Some("incomplete_or_invalid_token_counts")
    );
    let stored = serde_json::to_value(versions(&db).unwrap()).unwrap();
    assert!(stored[0]["cacheReadPerMillion"].is_null());
    assert!(stored[0]["cacheWritePerMillion"].is_null());
}

#[test]
fn sync_appends_origin_and_server_effective_time_with_monotonic_revisions() {
    let mut db = database();
    let at = "2026-10-16T20:00:00+08:00";
    let mut first = sync_item("model-a");
    first["cacheWritePerMillion"] = Value::Null;
    let second = sync_item("model-b");
    assert_eq!(
        synchronize(&mut db, &[first, second], at, false).unwrap(),
        (2, 0)
    );
    let all = versions(&db).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].revision, 1);
    assert_eq!(all[1].revision, 2);
    assert_eq!(all[0].effective_from, "2026-10-16T12:00:00Z");
    assert_eq!(all[0].created_at, "2026-10-16T12:00:00Z");
    assert!(all[0].cache_write_per_million.is_none());
    let origin = all[0].origin.as_ref().unwrap();
    assert_eq!(origin.kind, "models.dev");
    assert_eq!(origin.catalog_provider, "catalog-provider");
    assert_eq!(origin.catalog_model, "api/model-a");
    assert_eq!(origin.synced_at, "2026-10-16T12:00:00Z");
    assert!(all[0].provider_id.is_none());
}

#[test]
fn sync_is_idempotent_for_numeric_prices_and_distinguishes_unknown_from_zero() {
    let mut db = database();
    let mut item = sync_item("model-a");
    item["cacheWritePerMillion"] = Value::Null;
    assert_eq!(
        synchronize(&mut db, &[item.clone()], "2026-10-16T00:00:00Z", false).unwrap(),
        (1, 0)
    );
    let before = get_setting(&db, SETTING_KEY).unwrap();
    item["inputPerMillion"] = json!("2.000000");
    item["cacheReadPerMillion"] = json!("0.200000");
    assert_eq!(
        synchronize(&mut db, &[item.clone()], "2026-10-17T00:00:00Z", false).unwrap(),
        (0, 1)
    );
    assert_eq!(get_setting(&db, SETTING_KEY).unwrap(), before);
    item["cacheWritePerMillion"] = json!("0");
    assert_eq!(
        synchronize(&mut db, &[item], "2026-10-17T00:00:00Z", false).unwrap(),
        (1, 0)
    );
    let all = versions(&db).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].revision, 2);
    assert_eq!(all[1].cache_write_per_million.as_deref(), Some("0"));
}

#[test]
fn sync_preserves_latest_manual_price_unless_explicitly_overwritten() {
    let mut db = database();
    let manual = save(&db, &payload()).unwrap();
    let mut item = sync_item("test-model");
    item["inputPerMillion"] = json!("5");
    assert_eq!(
        synchronize(&mut db, &[item.clone()], "2026-10-16T00:00:00Z", false).unwrap(),
        (0, 1)
    );
    assert_eq!(versions(&db).unwrap().len(), 1);
    assert_eq!(
        synchronize(&mut db, &[item], "2026-10-16T00:00:00Z", true).unwrap(),
        (1, 0)
    );
    let all = versions(&db).unwrap();
    assert_eq!(all[0].id, manual.id);
    assert!(all[0].origin.is_none());
    assert!(all[1].origin.is_some());
    let mut manual_again = payload();
    manual_again["inputPerMillion"] = json!("8");
    save(&db, &manual_again).unwrap();
    assert_eq!(
        synchronize(
            &mut db,
            &[sync_item("test-model")],
            "2026-10-17T00:00:00Z",
            false
        )
        .unwrap(),
        (0, 1)
    );
    assert_eq!(versions(&db).unwrap().len(), 3);
}

#[test]
fn sync_protects_manual_prices_per_exact_target_provider_only() {
    let mut db = database();
    let mut manual = payload();
    manual["providerId"] = json!("local-provider-a");
    save(&db, &manual).unwrap();
    let mut a = sync_item("test-model");
    a["providerId"] = json!("local-provider-a");
    let mut b = sync_item("test-model");
    b["providerId"] = json!("local-provider-b");
    assert_eq!(
        synchronize(&mut db, &[a, b], "2026-10-16T00:00:00Z", false).unwrap(),
        (1, 1)
    );
    let all = versions(&db).unwrap();
    assert_eq!(all[1].provider_id.as_deref(), Some("local-provider-b"));
    assert_eq!(
        all[1].origin.as_ref().unwrap().catalog_provider,
        "catalog-provider"
    );
}

#[test]
fn sync_validates_the_whole_batch_before_any_write_even_if_caller_commits_on_error() {
    let mut db = database();
    save(&db, &payload()).unwrap();
    let before = get_setting(&db, SETTING_KEY).unwrap();
    let good = sync_item("model-a");
    let mut bad = sync_item("model-b");
    bad["outputPerMillion"] = json!("-1");
    let tx = db.transaction().unwrap();
    assert!(sync_prices(&tx, &[good, bad], "2026-10-16T00:00:00Z", false).is_err());
    tx.commit().unwrap();
    assert_eq!(get_setting(&db, SETTING_KEY).unwrap(), before);
    assert!(sync_prices(&db, &[sync_item("model-a")], "2026-10-16T00:00:00Z", false).is_err());
}

#[test]
fn sync_rejects_unknown_fields_duplicate_targets_and_oversized_batches() {
    let mut db = database();
    let item = sync_item("model-a");
    let mut unknown = item.clone();
    unknown["unknownField"] = json!(true);
    assert!(synchronize(&mut db, &[unknown], "2026-10-16T00:00:00Z", false).is_err());
    let mut origin = item.clone();
    origin["origin"] = json!({"kind":"manual"});
    assert!(synchronize(&mut db, &[origin], "2026-10-16T00:00:00Z", false).is_err());
    assert!(synchronize(
        &mut db,
        &[item.clone(), item.clone()],
        "2026-10-16T00:00:00Z",
        false
    )
    .is_err());
    assert!(synchronize(&mut db, &vec![item; 101], "2026-10-16T00:00:00Z", false).is_err());
    assert!(versions(&db).unwrap().is_empty());
}

#[test]
fn invalid_sync_origin_and_corrupt_stores_fail_without_resetting_history() {
    let mut db = database();
    let mut item = sync_item("model-a");
    item["catalogProvider"] = json!("");
    assert!(synchronize(&mut db, &[item], "2026-10-16T00:00:00Z", false).is_err());
    synchronize(
        &mut db,
        &[sync_item("model-a")],
        "2026-10-16T00:00:00Z",
        false,
    )
    .unwrap();
    let mut raw = get_setting(&db, SETTING_KEY).unwrap().unwrap();
    raw["versions"][0]["origin"]["kind"] = json!("future-source");
    put_setting(&db, SETTING_KEY, &raw).unwrap();
    assert!(versions(&db).is_err());
    assert!(synchronize(
        &mut db,
        &[sync_item("model-b")],
        "2026-10-16T00:00:00Z",
        false
    )
    .is_err());
    assert_eq!(get_setting(&db, SETTING_KEY).unwrap(), Some(raw));
}
