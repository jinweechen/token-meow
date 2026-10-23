use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T09:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(seconds)
}
fn reading(seconds: i64) -> Reading {
    Reading {
        utc: at(seconds),
        mono_ms: (1_000_000 + seconds * 1000) as u64,
    }
}
fn schema(db: &Connection) {
    db.execute_batch("CREATE TABLE plan_period_state(owner_kind TEXT NOT NULL,owner_id TEXT NOT NULL,period_instance_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(owner_kind,owner_id,period_instance_id));").unwrap();
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    schema(&db);
    db
}
fn plan() -> Value {
    json!({"id":"local-plan-a","revision":1,"type":"tokens","title":"synthetic local plan",
        "sourceId":"source-a","modelDimension":"response","model":null,"target":1000,
        "effectiveFrom":timestamp(at(0)),"endsAt":timestamp(at(3600)),"endPolicy":"time",
        "windowId":null,"periodInstanceId":null,"remindersEnabled":true,"lifecycle":"active","endedAt":null})
}
fn view(p: &Value, seconds: i64, used: f64) -> Value {
    let mut v = p.clone();
    v["used"] = json!(used);
    v["remaining"] = json!((p["target"].as_f64().unwrap() - used).max(0.0));
    v["evaluationState"] = json!("ready");
    v["coverage"] = json!("recorded-local");
    v["reasons"] = json!([]);
    v["measuredUntil"] = json!(timestamp(at(seconds)));
    v["forecastLedgerFingerprint"] = json!("synthetic-unchanged-ledger");
    v["forecastLedgerHeadFingerprint"] = json!("synthetic-unchanged-ledger");
    v["ledgerRevision"] = json!(42);
    v
}
fn scan_at(db: &Connection, p: &Value, v: &Value, runtime: &str, r: &Reading, reliable: bool) {
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    observe(db, p, v, runtime, r, reliable).unwrap();
    db.execute_batch("COMMIT").unwrap();
}
fn scan_with_fresh_append(
    db: &Connection,
    p: &Value,
    v: &Value,
    r: &Reading,
    reliable: bool,
    proof: Option<&FreshAppendProof>,
) {
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    observe_with_fresh_append(db, p, v, "runtime-a", r, reliable, proof).unwrap();
    db.execute_batch("COMMIT").unwrap();
}

fn fractional_view(p: &Value, ms: i64, used: f64) -> (Value, Reading) {
    let mut v = view(p, ms / 1000, used);
    let r = Reading {
        utc: at(0) + Duration::milliseconds(ms),
        mono_ms: 1_000_000 + ms as u64,
    };
    v["measuredUntil"] = json!(timestamp(r.utc));
    (v, r)
}
fn fresh_proof(
    p: &Value,
    db: &Connection,
    v: &Value,
    r: &Reading,
    cutoff: DateTime<Utc>,
) -> FreshAppendProof {
    // This is a typed algorithm fixture only. Core integration must independently
    // establish the append-only SQLite evidence before invoking verified().
    let head = capture(db).head.unwrap();
    FreshAppendProof::verified(
        p,
        "runtime-a",
        head.utc,
        head.ledger_fingerprint.as_deref().unwrap(),
        instant(&v["measuredUntil"]).unwrap(),
        text(&v["forecastLedgerFingerprint"]).unwrap(),
        cutoff,
        v["ledgerRevision"].as_u64().unwrap(),
        r,
    )
    .unwrap()
}
fn scan(db: &Connection, p: &Value, seconds: i64, used: f64) {
    scan_at(
        db,
        p,
        &view(p, seconds, used),
        "runtime-a",
        &reading(seconds),
        true,
    );
}
fn warm(db: &Connection, p: &Value) {
    for seconds in (0..=60).step_by(10) {
        scan(db, p, seconds, seconds as f64 * 2.0);
    }
}
fn project(db: &Connection, p: &Value, seconds: i64, used: f64) -> Value {
    projection(
        db,
        p,
        &view(p, seconds, used),
        "runtime-a",
        &reading(seconds),
    )
    .unwrap()
}
fn capture(db: &Connection) -> Capture {
    load(&state(db, "local-plan-a").unwrap()).unwrap()
}
fn assert_unavailable(v: &Value) {
    for field in ["localPlanEta", "periodEndUsage"] {
        assert_eq!(v[field]["state"], "unavailable", "{v}");
        assert!(v[field]["empiricalScenarioRange"].is_null(), "{v}");
        assert_eq!(v[field]["notifyEligible"], false);
    }
}

#[test]
fn sqlite_forecast_is_explicitly_local_and_anchored_to_the_persisted_measurement() {
    let db = database();
    let mut p = plan();
    p["endsAt"] = json!(timestamp(at(300)));
    warm(&db, &p);
    let before = state(&db, "local-plan-a").unwrap();
    let result = project(&db, &p, 60, 120.0);
    assert_eq!(result["localPlanEta"]["state"], "conditional");
    assert_eq!(
        result["localPlanEta"]["empiricalScenarioRange"]["earliestAt"],
        timestamp(at(480))
    );
    assert_eq!(
        result["localPlanEta"]["empiricalScenarioRange"]["latestAt"],
        timestamp(at(540))
    );
    assert_eq!(
        result["periodEndUsage"]["empiricalScenarioRange"]["minimum"],
        600.0
    );
    assert_eq!(
        result["periodEndUsage"]["empiricalScenarioRange"]["maximum"],
        600.0
    );
    assert_eq!(result["localPlanEta"]["scope"], "local-plan");
    assert!(result["localPlanEta"]["uncertaintyBounds"].is_null());
    assert_eq!(result["localPlanEta"]["asOf"], timestamp(at(60)));
    assert_eq!(result["localPlanEta"]["notifyEligible"], false);
    assert_eq!(result, project(&db, &p, 65, 120.0));
    assert_eq!(
        state(&db, "local-plan-a").unwrap(),
        before,
        "reads must not renew or modify persisted state"
    );
    assert_unavailable(&project(&db, &p, 76, 120.0));
}

#[test]
fn recent_baseline_extrema_are_empirical_scenarios_with_minute_and_token_rounding() {
    let db = database();
    let mut p = plan();
    p["endsAt"] = json!(timestamp(at(300)));
    for s in (0..=120).step_by(10) {
        scan(
            &db,
            &p,
            s,
            if s <= 60 {
                s as f64
            } else {
                60.0 + (s - 60) as f64 * 3.0
            },
        );
    }
    let v = project(&db, &p, 120, 240.0);
    assert_eq!(v["localPlanEta"]["speedScenarios"]["minimum"], 2.0);
    assert_eq!(v["localPlanEta"]["speedScenarios"]["maximum"], 3.0);
    assert_eq!(
        v["periodEndUsage"]["empiricalScenarioRange"]["minimum"],
        600.0
    );
    assert_eq!(
        v["periodEndUsage"]["empiricalScenarioRange"]["maximum"],
        780.0
    );
    assert_eq!(
        v["localPlanEta"]["empiricalScenarioRange"]["earliestAt"],
        timestamp(at(360))
    );
    assert_eq!(
        v["localPlanEta"]["empiricalScenarioRange"]["latestAt"],
        timestamp(at(540))
    );
    assert!(v["localPlanEta"]["rangeMeaning"]
        .as_str()
        .unwrap()
        .contains("not-confidence-or-guaranteed"));
}

#[test]
fn eta_after_or_straddling_the_fixed_end_does_not_claim_target_within_the_plan_period() {
    for (end, relation, judgment) in [
        (300, "after", json!(false)),
        (510, "overlapping", Value::Null),
        (600, "before", json!(true)),
    ] {
        let db = database();
        let mut p = plan();
        p["endsAt"] = json!(timestamp(at(end)));
        warm(&db, &p);
        let v = project(&db, &p, 60, 120.0);
        assert_eq!(v["localPlanEta"]["relationToPeriodEnd"], relation);
        assert_eq!(v["localPlanEta"]["willReachBeforePeriodEnd"], judgment);
        assert_eq!(
            v["localPlanEta"]["periodEndJudgmentBasis"],
            "empirical-scenarios-only-not-a-guarantee"
        );
    }
}

#[test]
fn missing_short_or_two_distinct_samples_do_not_produce_a_forecast() {
    let db = database();
    let p = plan();
    assert_unavailable(&project(&db, &p, 0, 0.0));
    for s in (0..=50).step_by(10) {
        scan(&db, &p, s, s as f64);
    }
    assert_unavailable(&project(&db, &p, 50, 50.0));
    let db = database();
    for s in (0..=60).step_by(10) {
        scan(&db, &p, s, if s == 60 { 100.0 } else { 0.0 });
    }
    assert_unavailable(&project(&db, &p, 60, 100.0));
}

#[test]
fn zero_speed_and_ten_minutes_without_a_change_are_unavailable_not_infinite() {
    let db = database();
    let p = plan();
    for s in (0..=60).step_by(10) {
        scan(&db, &p, s, 0.0);
    }
    assert_unavailable(&project(&db, &p, 60, 0.0));
    let db = database();
    warm(&db, &p);
    for s in (70..=660).step_by(10) {
        scan(&db, &p, s, 120.0);
    }
    assert_unavailable(&project(&db, &p, 660, 120.0));
}

#[test]
fn achieved_target_has_no_fabricated_eta_even_with_no_speed_samples() {
    let db = database();
    let p = plan();
    scan(&db, &p, 0, 1100.0);
    let v = project(&db, &p, 0, 1100.0);
    assert_eq!(v["localPlanEta"]["state"], "reached");
    assert!(v["localPlanEta"]["empiricalScenarioRange"].is_null());
    assert_eq!(v["localPlanEta"]["asOf"], timestamp(at(0)));
    assert_eq!(v["periodEndUsage"]["state"], "unavailable");
    assert_eq!(
        v["localPlanEta"],
        project(&db, &p, 5, 1100.0)["localPlanEta"]
    );
}

#[test]
fn reminder_pause_and_forward_target_or_title_revisions_reuse_speed_without_rewriting_history() {
    let db = database();
    let mut p = plan();
    warm(&db, &p);
    p["revision"] = json!(2);
    p["target"] = json!(2000);
    p["remindersEnabled"] = json!(false);
    p["title"] = json!("renamed");
    scan(&db, &p, 70, 140.0);
    assert_eq!(capture(&db).points.len(), 8);
    let v = project(&db, &p, 70, 140.0);
    assert_eq!(v["localPlanEta"]["state"], "conditional");
    assert_eq!(v["localPlanEta"]["asOfRevision"], 42);
    assert_eq!(v["localPlanEta"]["planConfigurationRevision"], 2);
    assert_eq!(v["localPlanEta"]["target"], 2000.0);
    assert_eq!(v["localPlanEta"]["notifyEligible"], false);
    p["revision"] = json!(3);
    p["remindersEnabled"] = json!(true);
    scan(&db, &p, 80, 160.0);
    assert_eq!(capture(&db).points.len(), 9);
}

#[test]
fn revision_regression_and_usage_corrections_break_the_segment() {
    let db = database();
    let mut p = plan();
    p["revision"] = json!(2);
    warm(&db, &p);
    p["revision"] = json!(1);
    scan(&db, &p, 70, 140.0);
    assert_eq!(capture(&db).points.len(), 1);
    assert_eq!(
        capture(&db).break_reason.as_deref(),
        Some("local_forecast_revision_regressed")
    );
    assert_unavailable(&project(&db, &p, 70, 140.0));
    let db = database();
    warm(&db, &p);
    scan(&db, &p, 70, 100.0);
    assert_eq!(capture(&db).points.len(), 1);
    assert_eq!(
        capture(&db).break_reason.as_deref(),
        Some("local_forecast_usage_reduced")
    );
    assert_unavailable(&project(&db, &p, 70, 100.0));
}

#[test]
fn backfill_or_missing_coverage_invalidates_without_becoming_a_zero_sample() {
    for reliable in [false, true] {
        let db = database();
        let p = plan();
        warm(&db, &p);
        let mut v = view(&p, 70, 500.0);
        if reliable {
            v["used"] = Value::Null;
            v["remaining"] = Value::Null;
            v["evaluationState"] = json!("unknown");
            v["coverage"] = json!("unknown");
            v["reasons"] = json!(["collection_incomplete"]);
        }
        scan_at(&db, &p, &v, "runtime-a", &reading(70), reliable);
        assert!(capture(&db).points.is_empty());
        assert!(capture(&db).head.is_none());
        assert_unavailable(&project(&db, &p, 70, 500.0));
        scan(&db, &p, 80, 500.0);
        assert_eq!(capture(&db).points.len(), 1);
        assert_unavailable(&project(&db, &p, 80, 500.0));
    }
}

#[test]
fn restart_source_model_period_or_runtime_changes_cannot_join_prior_samples() {
    for field in [
        "runtime",
        "sourceId",
        "model",
        "modelDimension",
        "effectiveFrom",
        "endsAt",
    ] {
        let db = database();
        let mut p = plan();
        warm(&db, &p);
        let runtime = if field == "runtime" {
            "runtime-b"
        } else {
            "runtime-a"
        };
        match field {
            "sourceId" => p[field] = json!("source-b"),
            "model" => p[field] = json!("model-b"),
            "modelDimension" => p[field] = json!("request"),
            "effectiveFrom" => p[field] = json!(timestamp(at(10))),
            "endsAt" => p[field] = json!(timestamp(at(4000))),
            _ => {}
        }
        scan_at(&db, &p, &view(&p, 70, 140.0), runtime, &reading(70), true);
        assert_eq!(capture(&db).points.len(), 1, "{field}");
        assert_unavailable(
            &projection(&db, &p, &view(&p, 70, 140.0), runtime, &reading(70)).unwrap(),
        );
    }
}

#[test]
fn sleep_gap_wall_clock_jumps_and_monotonic_rollback_break_the_segment() {
    for r in [
        reading(76),
        Reading {
            utc: at(73),
            mono_ms: reading(70).mono_ms,
        },
        Reading {
            utc: at(70),
            mono_ms: reading(59).mono_ms,
        },
        Reading {
            utc: at(59),
            mono_ms: reading(70).mono_ms,
        },
    ] {
        let db = database();
        let p = plan();
        warm(&db, &p);
        let seconds = (r.utc - at(0)).num_seconds();
        scan_at(&db, &p, &view(&p, seconds, 140.0), "runtime-a", &r, true);
        assert_eq!(capture(&db).points.len(), 1, "{r:?}");
        assert_unavailable(
            &projection(&db, &p, &view(&p, seconds, 140.0), "runtime-a", &r).unwrap(),
        );
    }
}

#[test]
fn read_clock_jump_unobserved_usage_and_future_measurement_fail_closed() {
    let db = database();
    let p = plan();
    warm(&db, &p);
    let bad = Reading {
        utc: at(65),
        mono_ms: reading(61).mono_ms,
    };
    assert_unavailable(&projection(&db, &p, &view(&p, 65, 120.0), "runtime-a", &bad).unwrap());
    assert_unavailable(&project(&db, &p, 61, 121.0));
    let mut v = view(&p, 62, 120.0);
    assert_unavailable(&projection(&db, &p, &v, "runtime-a", &reading(61)).unwrap());
    v["remaining"] = json!(100.0);
    assert_unavailable(&projection(&db, &p, &v, "runtime-a", &reading(62)).unwrap());
}

#[test]
fn utc_gap_over_fifteen_seconds_is_expired_even_if_monotonic_is_two_seconds_behind() {
    let db = database();
    let p = plan();
    warm(&db, &p);
    let r = Reading {
        utc: at(76),
        mono_ms: reading(74).mono_ms,
    };
    assert_unavailable(&projection(&db, &p, &view(&p, 76, 120.0), "runtime-a", &r).unwrap());
    scan_at(&db, &p, &view(&p, 76, 140.0), "runtime-a", &r, true);
    assert_eq!(capture(&db).points.len(), 1);
}

#[test]
fn small_clock_skews_cannot_accumulate_into_a_false_long_baseline() {
    let db = database();
    let p = plan();
    for index in 0..=5 {
        let r = Reading {
            utc: at(index * 10) + Duration::milliseconds(index * 500),
            mono_ms: reading(index * 10).mono_ms,
        };
        let mut v = view(&p, index * 10, index as f64 * 20.0);
        v["measuredUntil"] = json!(timestamp(r.utc));
        scan_at(&db, &p, &v, "runtime-a", &r, true);
    }
    let c = capture(&db);
    assert_eq!(c.points.len(), 1);
    assert_eq!(
        c.break_reason.as_deref(),
        Some("local_forecast_clock_discontinuous")
    );
}

#[test]
fn pending_partial_ended_and_percent_views_have_no_numeric_scenarios() {
    for mode in ["pending", "partial", "ended", "percent"] {
        let db = database();
        let mut p = plan();
        warm(&db, &p);
        let mut v = view(&p, 60, 120.0);
        match mode {
            "pending" => v["evaluationState"] = json!("pending"),
            "partial" => v["reasons"] = json!(["window_ended_partial"]),
            "ended" => v["lifecycle"] = json!("ended"),
            "percent" => {
                p["type"] = json!("percent");
                v["type"] = json!("percent");
            }
            _ => {}
        }
        assert_unavailable(&projection(&db, &p, &v, "runtime-a", &reading(60)).unwrap());
    }
    let db = database();
    let mut p = plan();
    p["endsAt"] = json!(timestamp(at(60)));
    warm(&db, &p);
    assert_unavailable(&project(&db, &p, 60, 120.0));
}

#[test]
fn real_scans_keep_dense_head_but_sample_at_most_once_per_ten_seconds_and_cap_storage() {
    let db = database();
    let mut p = plan();
    p["endsAt"] = json!(timestamp(at(20_000)));
    for s in 0..=60 {
        scan(&db, &p, s, s as f64 * 2.0);
    }
    let c = capture(&db);
    assert_eq!(c.points.len(), 7);
    assert_eq!(c.head.unwrap().utc, at(60));
    scan(&db, &p, 61, 122.0);
    assert_eq!(capture(&db).points.len(), 7);
    assert_eq!(
        project(&db, &p, 61, 122.0)["localPlanEta"]["asOf"],
        timestamp(at(61))
    );
    for s in (70..=6000).step_by(10) {
        scan(&db, &p, s, s as f64 * 2.0);
    }
    let c = capture(&db);
    assert_eq!(c.points.len(), 512);
    for pair in c.points.windows(2) {
        assert!((pair[1].utc - pair[0].utc).num_milliseconds() >= 10_000);
    }
    scan(&db, &p, 6001, 12002.0);
    let c = capture(&db);
    assert_eq!(
        c.points.len(),
        511,
        "head must not create a 513th retained observation"
    );
    assert_eq!(c.head.unwrap().utc, at(6001));
}

#[test]
fn state_merges_siblings_and_participates_in_real_transaction_rollback() {
    let db = database();
    let p = plan();
    db.execute("INSERT INTO plan_period_state VALUES('plan','local-plan-a','local-plan-a',?1)",[json!({"latched":[80],"windowPlan":{"proof":"keep"},"demandHistory":{"fixture":"keep"}}).to_string()]).unwrap();
    warm(&db, &p);
    let before = state(&db, "local-plan-a").unwrap();
    assert_eq!(before["latched"], json!([80]));
    assert_eq!(before["windowPlan"]["proof"], "keep");
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    observe(
        &db,
        &p,
        &view(&p, 70, 140.0),
        "runtime-a",
        &reading(70),
        true,
    )
    .unwrap();
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(state(&db, "local-plan-a").unwrap(), before);
    assert!(observe(
        &db,
        &p,
        &view(&p, 70, 140.0),
        "runtime-a",
        &reading(70),
        true
    )
    .is_err());
    assert_eq!(state(&db, "local-plan-a").unwrap(), before);
}

#[test]
fn sqlite_reopen_cannot_resurrect_forecast_in_another_runtime() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "token-meow-local-forecast-{}-{}.sqlite",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let p = plan();
    {
        let db = Connection::open(&path).unwrap();
        schema(&db);
        warm(&db, &p);
        assert_eq!(
            project(&db, &p, 60, 120.0)["localPlanEta"]["state"],
            "conditional"
        );
    }
    {
        let db = Connection::open(&path).unwrap();
        assert_unavailable(
            &projection(&db, &p, &view(&p, 61, 120.0), "runtime-b", &reading(61)).unwrap(),
        );
        scan_at(
            &db,
            &p,
            &view(&p, 61, 120.0),
            "runtime-b",
            &reading(61),
            true,
        );
        assert_eq!(capture(&db).points.len(), 1);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn native_window_token_plan_uses_local_coverage_without_requiring_official_capacity() {
    let db = database();
    let mut p = plan();
    p["endPolicy"] = json!("window");
    p["windowId"] = json!("bucket-a");
    p["periodInstanceId"] = json!("window-instance-a");
    warm(&db, &p);
    let v = project(&db, &p, 60, 120.0);
    assert_eq!(v["localPlanEta"]["state"], "conditional");
    p["periodInstanceId"] = json!("window-instance-b");
    assert_unavailable(&project(&db, &p, 61, 120.0));
}

#[test]
fn malformed_unknown_version_and_unsafe_counts_do_not_create_nan_or_infinity() {
    let db = database();
    let p = plan();
    warm(&db, &p);
    let mut original = state(&db, "local-plan-a").unwrap();
    original["localForecast"]["algorithmVersion"] = json!("future-v999");
    db.execute(
        "UPDATE plan_period_state SET data=?1",
        [original.to_string()],
    )
    .unwrap();
    assert_unavailable(&project(&db, &p, 60, 120.0));
    for used in [-1.0, 0.5, SAFE + 1.0] {
        assert_unavailable(&project(&db, &p, 60, used));
    }
}

#[test]
fn ledger_correction_with_unchanged_total_invalidates_read_and_rebaselines_writer() {
    let db = database();
    let p = plan();
    warm(&db, &p);
    let ready = project(&db, &p, 60, 120.0);
    assert_eq!(ready["localPlanEta"]["asOfRevision"], 42);
    assert_eq!(ready["localPlanEta"]["planConfigurationRevision"], 1);
    let mut changed = view(&p, 61, 120.0);
    changed["forecastLedgerHeadFingerprint"] = json!("same-total-corrected-facts");
    changed["forecastLedgerFingerprint"] = json!("same-total-corrected-facts");
    let before = state(&db, "local-plan-a").unwrap();
    let unavailable = projection(&db, &p, &changed, "runtime-a", &reading(61)).unwrap();
    assert_unavailable(&unavailable);
    assert_eq!(state(&db, "local-plan-a").unwrap(), before);
    scan_at(&db, &p, &changed, "runtime-a", &reading(61), true);
    assert_eq!(capture(&db).points.len(), 1);
    assert_unavailable(&projection(&db, &p, &changed, "runtime-a", &reading(61)).unwrap());
}

#[test]
fn verified_179ms_fresh_append_keeps_complete_earlier_points_and_queries_never_repair_or_renew() {
    let db = database();
    let p = plan();
    warm(&db, &p);
    let (old_view, old_reading) = fractional_view(&p, 62_100, 120.0);
    scan_at(&db, &p, &old_view, "runtime-a", &old_reading, true);
    let original = capture(&db);
    let (mut v, r) = fractional_view(&p, 62_256, 140.0);
    v["forecastLedgerFingerprint"] = json!("synthetic-new-full-head");
    v["forecastLedgerHeadFingerprint"] = json!("synthetic-old-head-plus-new-live-append");
    let cutoff = at(0) + Duration::milliseconds(62_077);
    assert_eq!(r.utc - cutoff, Duration::milliseconds(179));
    let proof = fresh_proof(&p, &db, &v, &r, cutoff);
    let before = state(&db, "local-plan-a").unwrap();
    assert_unavailable(&projection(&db, &p, &v, "runtime-a", &r).unwrap());
    assert_eq!(
        state(&db, "local-plan-a").unwrap(),
        before,
        "reads cannot consume even a valid writer proof"
    );
    scan_with_fresh_append(&db, &p, &v, &r, true, Some(&proof));
    let c = capture(&db);
    assert_eq!(
        serde_json::to_value(&c.points).unwrap(),
        serde_json::to_value(&original.points).unwrap(),
        "all complete earlier counts and fingerprints must remain byte-equivalent"
    );
    assert_eq!(c.head.as_ref().unwrap().utc, r.utc);
    assert_eq!(c.head.as_ref().unwrap().used, 140.0);
    assert_eq!(c.last_change, Some(r.utc));
    assert!(c.points.iter().all(|point| point.utc < cutoff));
    v["forecastLedgerHeadFingerprint"] = v["forecastLedgerFingerprint"].clone();
    let result = projection(&db, &p, &v, "runtime-a", &r).unwrap();
    assert_eq!(result["localPlanEta"]["state"], "conditional");
    assert_eq!(result["localPlanEta"]["asOf"], timestamp(r.utc));
    assert_eq!(result["localPlanEta"]["lastChangeAt"], timestamp(r.utc));
    assert_eq!(result["localPlanEta"]["notifyEligible"], false);
    let persisted = state(&db, "local-plan-a").unwrap();
    let later = Reading {
        utc: r.utc + Duration::seconds(1),
        mono_ms: r.mono_ms + 1000,
    };
    assert_eq!(
        projection(&db, &p, &v, "runtime-a", &later).unwrap(),
        result
    );
    assert_eq!(state(&db, "local-plan-a").unwrap(), persisted);
    let expired = Reading {
        utc: r.utc + Duration::milliseconds(15_001),
        mono_ms: r.mono_ms + 15_001,
    };
    assert_unavailable(&projection(&db, &p, &v, "runtime-a", &expired).unwrap());
    assert_eq!(state(&db, "local-plan-a").unwrap(), persisted);
}

#[test]
fn fresh_append_uses_original_dense_head_clock_before_pruning_a_more_than_fifteen_second_tail() {
    let db = database();
    let p = plan();
    warm(&db, &p);
    scan(&db, &p, 70, 140.0);
    scan(&db, &p, 73, 140.0);
    let (mut v, r) = fractional_view(&p, 76_000, 160.0);
    v["forecastLedgerFingerprint"] = json!("synthetic-new-full-head");
    v["forecastLedgerHeadFingerprint"] = json!("synthetic-old-head-with-live-append");
    let cutoff = at(0) + Duration::milliseconds(61_001);
    let proof = fresh_proof(&p, &db, &v, &r, cutoff);
    let old = capture(&db);
    assert_eq!(old.points.last().unwrap().utc, at(70));
    assert_eq!(old.head.unwrap().utc, at(73));
    assert!((r.utc - at(60)) > Duration::milliseconds(MAX_GAP_MS as i64));
    scan_with_fresh_append(&db, &p, &v, &r, true, Some(&proof));
    let c = capture(&db);
    assert_eq!(c.points.len(), 8, "the valid original 73s head witnesses collection continuity; the affected 70s trend point is removed and 0..60s counts remain intact");
    assert_eq!(c.points[6].utc, at(60));
    assert_eq!(c.points[7].utc, at(76));
    assert_eq!(c.last_change, Some(at(76)));
    assert!(c.break_reason.is_none());
    v["forecastLedgerHeadFingerprint"] = v["forecastLedgerFingerprint"].clone();
    assert_eq!(
        projection(&db, &p, &v, "runtime-a", &r).unwrap()["localPlanEta"]["state"],
        "conditional"
    );
}

#[test]
fn mismatched_forged_or_replayed_fresh_append_proofs_cannot_bypass_ledger_invalidation() {
    for mode in [
        "old-time",
        "old-fingerprint",
        "new-time",
        "new-fingerprint",
        "runtime",
        "source",
        "plan",
        "revision",
        "ledger-revision",
        "reading",
        "cutoff-too-old",
        "none",
    ] {
        let db = database();
        let p = plan();
        warm(&db, &p);
        scan(&db, &p, 69, 120.0);
        let (mut v, r) = fractional_view(&p, 76_000, 140.0);
        v["forecastLedgerFingerprint"] = json!("synthetic-new-full-head");
        v["forecastLedgerHeadFingerprint"] = json!("synthetic-old-head-with-live-append");
        let mut proof = fresh_proof(&p, &db, &v, &r, at(0) + Duration::milliseconds(61_001));
        match mode {
            "old-time" => proof.old_head_utc -= Duration::milliseconds(1),
            "old-fingerprint" => proof.old_head_fingerprint = "wrong".into(),
            "new-time" => proof.new_head_utc += Duration::milliseconds(1),
            "new-fingerprint" => proof.new_head_fingerprint = "wrong".into(),
            "runtime" => proof.runtime_id = "other-runtime".into(),
            "source" => proof.key.source_id = "other-source".into(),
            "plan" => proof.key.plan_id = "other-plan".into(),
            "revision" => proof.plan_revision += 1,
            "ledger-revision" => proof.new_head_ledger_revision += 1,
            "reading" => proof.verified_at.mono_ms += 1,
            "cutoff-too-old" => proof.truncate_from = at(60),
            _ => {}
        }
        scan_with_fresh_append(
            &db,
            &p,
            &v,
            &r,
            true,
            if mode == "none" { None } else { Some(&proof) },
        );
        let c = capture(&db);
        assert_eq!(c.points.len(), 1, "{mode}");
        assert_eq!(
            c.break_reason.as_deref(),
            Some("local_forecast_ledger_changed"),
            "{mode}"
        );
        assert!(c.last_change.is_none(), "{mode}");
    }
}

#[test]
fn fresh_append_constructor_enforces_the_exact_fifteen_second_limit_and_cannot_override_clock_or_scan_gates(
) {
    let db = database();
    let p = plan();
    warm(&db, &p);
    scan(&db, &p, 69, 120.0);
    let (mut v, r) = fractional_view(&p, 76_000, 140.0);
    v["forecastLedgerFingerprint"] = json!("synthetic-new-full-head");
    v["forecastLedgerHeadFingerprint"] = json!("synthetic-old-head-with-live-append");
    assert!(FreshAppendProof::verified(
        &p,
        "runtime-a",
        at(69),
        "synthetic-unchanged-ledger",
        r.utc,
        "synthetic-new-full-head",
        at(61),
        42,
        &r
    )
    .is_some());
    assert!(FreshAppendProof::verified(
        &p,
        "runtime-a",
        at(69),
        "synthetic-unchanged-ledger",
        r.utc,
        "synthetic-new-full-head",
        at(61) - Duration::nanoseconds(1),
        42,
        &r
    )
    .is_none());
    assert!(FreshAppendProof::verified(
        &p,
        "runtime-a",
        at(69),
        "synthetic-unchanged-ledger",
        r.utc,
        "synthetic-new-full-head",
        at(69),
        42,
        &r
    )
    .is_none());
    let mut bad_clock = r;
    bad_clock.mono_ms += 10_000;
    let proof = fresh_proof(&p, &db, &v, &bad_clock, at(61));
    scan_with_fresh_append(&db, &p, &v, &bad_clock, true, Some(&proof));
    assert_eq!(
        capture(&db).points.len(),
        1,
        "a proof cannot override the original head's paired-clock discontinuity"
    );
    let db = database();
    warm(&db, &p);
    scan(&db, &p, 69, 120.0);
    let proof = fresh_proof(&p, &db, &v, &r, at(61));
    scan_with_fresh_append(&db, &p, &v, &r, false, Some(&proof));
    assert!(capture(&db).points.is_empty());
    assert_eq!(
        capture(&db).break_reason.as_deref(),
        Some("local_forecast_scan_unreliable")
    );
}

#[test]
fn a_valid_fresh_append_does_not_lower_the_sixty_second_sampling_gate() {
    let db = database();
    let p = plan();
    for seconds in (0..=40).step_by(10) {
        scan(&db, &p, seconds, seconds as f64 * 2.0);
    }
    scan(&db, &p, 49, 80.0);
    let (mut v, r) = fractional_view(&p, 55_000, 100.0);
    v["forecastLedgerFingerprint"] = json!("synthetic-new-full-head");
    v["forecastLedgerHeadFingerprint"] = json!("synthetic-old-head-with-live-append");
    let proof = fresh_proof(&p, &db, &v, &r, at(41));
    scan_with_fresh_append(&db, &p, &v, &r, true, Some(&proof));
    assert_eq!(capture(&db).points.len(), 6);
    assert_eq!(capture(&db).last_change, Some(at(55)));
    v["forecastLedgerHeadFingerprint"] = v["forecastLedgerFingerprint"].clone();
    assert_unavailable(&projection(&db, &p, &v, "runtime-a", &r).unwrap());
}
