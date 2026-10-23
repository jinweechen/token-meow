use super::*;

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T09:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(seconds)
}
fn reading(seconds: i64) -> Reading {
    Reading {
        utc: at(seconds),
        mono_ms: (1_000_000 + seconds * 1_000) as u64,
    }
}
fn schema(db: &Connection) {
    db.execute_batch("CREATE TABLE usage_plans(plan_id TEXT NOT NULL,revision INTEGER NOT NULL,source_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(plan_id,revision)); CREATE TABLE plan_period_state(owner_kind TEXT NOT NULL,owner_id TEXT NOT NULL,period_instance_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(owner_kind,owner_id,period_instance_id)); CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE usage_responses(id TEXT PRIMARY KEY,source_id TEXT NOT NULL,event_time TEXT NOT NULL,data TEXT NOT NULL); CREATE TABLE excluded_facts(fact_id TEXT PRIMARY KEY,reason TEXT NOT NULL); CREATE TABLE fact_aliases(alias TEXT PRIMARY KEY,fact_id TEXT NOT NULL,evidence TEXT NOT NULL);").unwrap();
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    schema(&db);
    db
}
fn snapshot(seconds: i64) -> Value {
    json!({"runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1",
        "clearGeneration":null,"accountScope":"account-a","identityConsistency":"proven",
        "identityEvidence":{"kind":"synthetic_fixture_only"},"observationOutcome":"success","status":"connected","persisted":true,
        "accountRead":{"freshnessAnchor":timestamp(at(seconds))},"entitlementSegment":"rights-a",
        "entitlementEvidence":{"verified":true,"source":"official","segmentId":"rights-a","ruleVersion":"rules-a","reference":"synthetic_fixture_only"}})
}
fn plan(id: &str, start: i64, duration: i64) -> Value {
    json!({"id":id,"revision":1,"type":"tokens","title":"test","sourceId":"source-a","modelDimension":"response","model":null,
        "target":1000,"effectiveFrom":timestamp(at(start)),"endsAt":timestamp(at(start+duration)),"endPolicy":"time",
        "remindersEnabled":true,"lifecycle":"active","endedAt":null})
}
fn view(plan: &Value, until: i64, used: f64, ended: bool) -> Value {
    let mut v = plan.clone();
    v["used"] = json!(used);
    v["evaluationState"] = json!("ready");
    v["lifecycle"] = json!(if ended { "ended" } else { "active" });
    v["measuredUntil"] = json!(timestamp(at(until)));
    v["reasons"] = json!([]);
    v["demandEvidence"] = json!({"meteringVersion":adapter::SEMANTICS,"ledgerComplete":true,
        "accountScope":"account-a","accountCoverageVerified":true,"modelCoverageComplete":true});
    v
}
fn write_plan(db: &Connection, plan: &Value) {
    db.execute(
        "INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES(?1,?2,?3,?4)",
        params![
            plan["id"].as_str(),
            plan["revision"].as_i64(),
            plan["sourceId"].as_str(),
            plan.to_string()
        ],
    )
    .unwrap();
}
fn begin(db: &Connection, plan: &Value, seconds: i64) {
    db.execute_batch("BEGIN").unwrap();
    write_plan(db, plan);
    initialize(
        db,
        plan,
        Some(&snapshot(seconds)),
        "runtime-a",
        &reading(seconds),
    )
    .unwrap();
    db.execute_batch("COMMIT").unwrap();
}
fn scan(db: &Connection, plan: &Value, view: &Value, seconds: i64, reliable: bool) {
    db.execute_batch("BEGIN").unwrap();
    observe(
        db,
        plan,
        view,
        Some(&snapshot(seconds)),
        "runtime-a",
        &reading(seconds),
        reliable,
    )
    .unwrap();
    db.execute_batch("COMMIT").unwrap();
}
fn fact(db: &Connection, id: &str, event: DateTime<Utc>, total: f64) {
    db.execute(
        "INSERT INTO usage_responses(id,source_id,event_time,data) VALUES(?1,'source-a',?2,?3)",
        params![
            id,
            timestamp(event),
            json!({"fixtureOnly":true,"total":total}).to_string()
        ],
    )
    .unwrap();
}
fn complete(db: &Connection, id: &str, start: i64, duration: i64, total: f64) -> Value {
    capture(db, id, start, duration, total, true)
}
fn capture(
    db: &Connection,
    id: &str,
    start: i64,
    duration: i64,
    total: f64,
    add_fact: bool,
) -> Value {
    let p = plan(id, start, duration);
    begin(db, &p, start);
    if add_fact && total > 0.0 {
        fact(db, &format!("{id}-response"), at(start + 1), total);
    }
    let mut offset = 10;
    while offset < duration {
        scan(
            db,
            &p,
            &view(&p, start + offset, total, false),
            start + offset,
            true,
        );
        offset += 10;
    }
    scan(
        db,
        &p,
        &view(&p, start + duration, total, true),
        start + duration,
        true,
    );
    assert_eq!(
        state(db, id).unwrap()["demandHistory"]["status"],
        "complete"
    );
    p
}
fn five(db: &Connection, amounts: [f64; 5]) -> Vec<Value> {
    amounts
        .iter()
        .enumerate()
        .map(|(i, total)| complete(db, &format!("p{i}"), i as i64 * 60, 30, *total))
        .collect()
}
fn input() -> Value {
    json!({"sourceId":"source-a","modelDimension":"response","model":null,"durationSeconds":30})
}
fn result(db: &Connection) -> Value {
    draft(db, &input(), Some(&snapshot(300)), "runtime-a", at(300))
}
fn has_reason(value: &Value, reason: &str) -> bool {
    value["reasons"]
        .as_array()
        .is_some_and(|items| items.iter().any(|v| v == reason))
}

#[test]
fn sqlite_completed_intervals_survive_reopen_and_produce_editable_p50_draft() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("demand.db");
    {
        let db = Connection::open(&path).unwrap();
        schema(&db);
        five(&db, [0.0, 10.0, 20.0, 30.0, 40.0]);
    }
    let db = Connection::open(&path).unwrap();
    let value = result(&db);
    assert_eq!(value["plan"]["target"].as_f64(), Some(20.0));
    assert_eq!(value["historicalDemand"]["p90"].as_f64(), Some(40.0));
    assert_eq!(value["historicalDemand"]["sampleCount"], 5);
    assert_eq!(
        value["historicalDemand"]["durationSeconds"].as_f64(),
        Some(30.0)
    );
    assert_eq!(instant(&value["plan"]["endsAt"]), Some(at(330)));
    assert_eq!(value["plan"]["type"], "tokens");
    assert_eq!(value["plan"]["expectedRevision"], Value::Null);
    assert_eq!(value["constraintsVerified"], false);
    assert!(has_reason(&value, "quota_sustainability_unverified"));
}

#[test]
fn missing_or_open_intervals_are_not_zero_samples() {
    let db = database();
    for i in 0..4 {
        complete(&db, &format!("p{i}"), i * 60, 30, 10.0);
    }
    let open = plan("open", 240, 30);
    begin(&db, &open, 240);
    scan(&db, &open, &view(&open, 250, 0.0, false), 250, true);
    let value = result(&db);
    assert!(value["plan"].is_null());
    assert!(has_reason(
        &value,
        "comparable_covered_intervals_insufficient"
    ));
}

#[test]
fn proven_zero_is_counted_but_zero_median_does_not_invent_positive_target() {
    let db = database();
    five(&db, [0.0, 0.0, 10.0, 20.0, 30.0]);
    assert_eq!(result(&db)["plan"]["target"].as_f64(), Some(10.0));
    let zero = database();
    five(&zero, [0.0; 5]);
    let value = result(&zero);
    assert!(value["plan"].is_null());
    assert!(has_reason(
        &value,
        "zero_historical_demand_choose_manual_target"
    ));
    assert_eq!(value["historicalDemand"]["sampleCount"], 5);
}

#[test]
fn missing_scan_clock_jump_or_runtime_restart_permanently_invalidates_capture() {
    for mode in ["gap", "clock_jump", "runtime", "unreliable", "backwards"] {
        let db = database();
        let p = plan("p", 0, 30);
        begin(&db, &p, 0);
        let (seconds, mut clock) = if mode == "gap" {
            (20, reading(20))
        } else {
            (10, reading(10))
        };
        if mode == "clock_jump" {
            clock.mono_ms = 1_001_000;
        }
        if mode == "backwards" {
            clock.mono_ms = 999_999;
        }
        let mut s = snapshot(seconds);
        let runtime = if mode == "runtime" {
            s["runtimeId"] = json!("runtime-b");
            "runtime-b"
        } else {
            "runtime-a"
        };
        db.execute_batch("BEGIN").unwrap();
        observe(
            &db,
            &p,
            &view(&p, seconds, 0.0, false),
            Some(&s),
            runtime,
            &clock,
            mode != "unreliable",
        )
        .unwrap();
        initialize(&db, &p, Some(&snapshot(0)), "runtime-a", &reading(0)).unwrap();
        db.execute_batch("COMMIT").unwrap();
        assert_eq!(
            state(&db, "p").unwrap()["demandHistory"]["status"],
            "invalidated",
            "{mode}"
        );
    }
}

#[test]
fn paused_reminders_and_title_or_target_edits_do_not_interrupt_capture() {
    let db = database();
    let mut p = plan("p", 0, 30);
    p["remindersEnabled"] = json!(false);
    begin(&db, &p, 0);
    scan(&db, &p, &view(&p, 10, 0.0, false), 10, true);
    p["title"] = json!("new title");
    p["target"] = json!(50);
    p["revision"] = json!(2);
    scan(&db, &p, &view(&p, 20, 0.0, false), 20, true);
    scan(&db, &p, &view(&p, 30, 0.0, true), 30, true);
    assert_eq!(
        state(&db, "p").unwrap()["demandHistory"]["status"],
        "complete"
    );
}

#[test]
fn unknown_stable_identity_or_rights_cannot_be_filled_by_plan_type() {
    for missing in [
        "accountScope",
        "identityEvidence",
        "entitlementSegment",
        "entitlementEvidence",
    ] {
        let db = database();
        let p = plan("p", 0, 30);
        let mut s = snapshot(0);
        s[missing] = Value::Null;
        s["planType"] = json!("plus");
        db.execute_batch("BEGIN").unwrap();
        write_plan(&db, &p);
        initialize(&db, &p, Some(&s), "runtime-a", &reading(0)).unwrap();
        db.execute_batch("COMMIT").unwrap();
        assert_eq!(
            state(&db, "p").unwrap()["demandHistory"]["status"],
            "invalidated",
            "{missing}"
        );
        let value = draft(&db, &input(), Some(&s), "runtime-a", at(0));
        assert!(value["plan"].is_null());
    }
}

#[test]
fn account_model_and_metering_coverage_are_mandatory() {
    for missing in [
        "ledgerComplete",
        "accountCoverageVerified",
        "modelCoverageComplete",
        "meteringVersion",
        "accountScope",
    ] {
        let db = database();
        let mut p = plan("p", 0, 30);
        p["model"] = json!("model-a");
        begin(&db, &p, 0);
        let mut v = view(&p, 10, 0.0, false);
        v["demandEvidence"][missing] = Value::Null;
        scan(&db, &p, &v, 10, true);
        assert_eq!(
            state(&db, "p").unwrap()["demandHistory"]["status"],
            "invalidated",
            "{missing}"
        );
    }
}

#[test]
fn overlapping_plans_count_as_one_independent_interval() {
    let db = database();
    fact(&db, "shared-response", at(1), 10.0);
    for i in 0..5 {
        capture(&db, &format!("p{i}"), 0, 30, 10.0, false);
    }
    assert!(has_reason(
        &result(&db),
        "comparable_covered_intervals_insufficient"
    ));
}

#[test]
fn reverse_interval_scheduling_preserves_existing_five_independent_samples() {
    let db = database();
    for i in 0..5 {
        fact(
            &db,
            &format!("p{i}-response"),
            at(i * 29 + 1),
            10.0 + i as f64,
        );
    }
    for i in 0..5 {
        capture(&db, &format!("p{i}"), i * 29, 29, 10.0 + i as f64, false);
    }
    capture(&db, "overlap", 115, 31, 14.0, false);
    let value = result(&db);
    assert_eq!(value["historicalDemand"]["sampleCount"], 5);
    assert_eq!(value["plan"]["target"].as_f64(), Some(12.0));
}

#[test]
fn duration_and_scope_mismatch_are_excluded_without_token_rescaling() {
    let db = database();
    five(&db, [10.0, 20.0, 30.0, 40.0, 50.0]);
    for (field, value) in [
        ("durationSeconds", json!(60)),
        ("modelDimension", json!("request")),
        ("model", json!("different-model")),
        ("sourceId", json!("different-source")),
    ] {
        let mut scope = input();
        scope[field] = value;
        let value = draft(&db, &scope, Some(&snapshot(300)), "runtime-a", at(300));
        assert!(value["plan"].is_null(), "{field}");
    }
    let mut scope = input();
    scope["endsAt"] = json!(timestamp(at(331)));
    assert!(has_reason(
        &draft(&db, &scope, Some(&snapshot(300)), "runtime-a", at(300)),
        "duration_mismatch"
    ));
}

#[test]
fn manual_early_end_seals_actual_shorter_duration() {
    let db = database();
    let mut p = plan("p", 0, 60);
    begin(&db, &p, 0);
    scan(&db, &p, &view(&p, 10, 7.0, false), 10, true);
    p["endedAt"] = json!(timestamp(at(20)));
    p["lifecycle"] = json!("ended");
    scan(&db, &p, &view(&p, 20, 7.0, true), 20, true);
    let c = state(&db, "p").unwrap()["demandHistory"].clone();
    assert_eq!(c["status"], "complete");
    assert_eq!(instant(&c["actualEnd"]), Some(at(20)));
    assert_eq!(instant(&c["configuredEnd"]), Some(at(60)));
}

#[test]
fn completion_requires_exact_actual_measured_until() {
    for until in [29, 31] {
        let db = database();
        let p = plan("p", 0, 30);
        begin(&db, &p, 0);
        scan(&db, &p, &view(&p, 10, 0.0, false), 10, true);
        scan(&db, &p, &view(&p, 20, 0.0, false), 20, true);
        scan(&db, &p, &view(&p, until, 0.0, true), 30, true);
        let c = state(&db, "p").unwrap()["demandHistory"].clone();
        assert_eq!(c["status"], "invalidated");
        assert!(c["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "actual_period_coverage_unknown"));
    }
}

#[test]
fn window_pending_continues_capture_but_does_not_seal_without_confirmed_boundary() {
    let db = database();
    let mut p = plan("p", 0, 30);
    p["endPolicy"] = json!("window");
    begin(&db, &p, 0);
    let mut pending = view(&p, 10, 0.0, false);
    pending["evaluationState"] = json!("pending");
    pending["reasons"] = json!(["window_confirmation_pending"]);
    pending["used"] = Value::Null;
    scan(&db, &p, &pending, 10, true);
    assert_eq!(
        state(&db, "p").unwrap()["demandHistory"]["status"],
        "collecting"
    );
    scan(&db, &p, &view(&p, 20, 0.0, false), 20, true);
    scan(&db, &p, &view(&p, 30, 0.0, true), 30, true);
    assert_eq!(
        state(&db, "p").unwrap()["demandHistory"]["status"],
        "complete"
    );
    let unknown = database();
    begin(&unknown, &p, 0);
    pending["lifecycle"] = json!("ended");
    scan(&unknown, &p, &pending, 10, true);
    assert_eq!(
        state(&unknown, "p").unwrap()["demandHistory"]["status"],
        "invalidated"
    );
}

#[test]
fn changed_closed_facts_exclusions_and_aliases_are_detected_without_new_scan() {
    for mutation in ["delete", "revision", "late", "excluded", "alias"] {
        let db = database();
        five(&db, [10.0, 20.0, 30.0, 40.0, 50.0]);
        assert!(result(&db)["plan"].is_object());
        match mutation {
            "delete" => {
                db.execute("DELETE FROM usage_responses WHERE id='p0-response'", [])
                    .unwrap();
            }
            "revision" => {
                db.execute(
                    "UPDATE usage_responses SET data='{}' WHERE id='p0-response'",
                    [],
                )
                .unwrap();
            }
            "late" => fact(&db, "late", at(2), 1.0),
            "excluded" => {
                db.execute(
                    "INSERT INTO excluded_facts VALUES('p0-response','conflict')",
                    [],
                )
                .unwrap();
            }
            "alias" => {
                db.execute(
                    "INSERT INTO fact_aliases VALUES('other','p0-response','{}')",
                    [],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            has_reason(&result(&db), "comparable_covered_intervals_insufficient"),
            "{mutation}"
        );
    }
}

#[test]
fn fingerprint_includes_submillisecond_before_end_but_not_exact_end() {
    let db = database();
    let p = plan("p", 0, 30);
    begin(&db, &p, 0);
    fact(&db, "near-end", at(30) - Duration::microseconds(100), 10.0);
    scan(&db, &p, &view(&p, 10, 0.0, false), 10, true);
    scan(&db, &p, &view(&p, 20, 0.0, false), 20, true);
    scan(&db, &p, &view(&p, 30, 10.0, true), 30, true);
    let old = state(&db, "p").unwrap()["demandHistory"]["ledgerFingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    fact(&db, "exact-end", at(30), 1.0);
    assert_eq!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        old
    );
    db.execute("DELETE FROM usage_responses WHERE id='near-end'", [])
        .unwrap();
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        old
    );
}

#[test]
fn completed_interval_ignores_later_unrelated_collection_failure() {
    let db = database();
    let p = complete(&db, "p", 0, 30, 10.0);
    let mut v = view(&p, 30, 0.0, true);
    v["evaluationState"] = json!("unknown");
    v["used"] = Value::Null;
    v["reasons"] = json!(["collection_incomplete"]);
    fact(&db, "unrelated", at(60), 10.0);
    db.execute(
        "INSERT INTO excluded_facts VALUES('unrelated','conflict')",
        [],
    )
    .unwrap();
    scan(&db, &p, &v, 100, true);
    assert_eq!(
        state(&db, "p").unwrap()["demandHistory"]["status"],
        "complete"
    );
}

#[test]
fn same_verified_rights_can_read_closed_history_after_runtime_restart() {
    let db = database();
    five(&db, [10.0, 20.0, 30.0, 40.0, 50.0]);
    let mut s = snapshot(300);
    s["runtimeId"] = json!("runtime-b");
    s["bindingGeneration"] = json!("7");
    assert!(draft(&db, &input(), Some(&s), "runtime-b", at(300))["plan"].is_object());
    s["entitlementSegment"] = json!("rights-b");
    s["entitlementEvidence"]["segmentId"] = json!("rights-b");
    assert!(draft(&db, &input(), Some(&s), "runtime-b", at(300))["plan"].is_null());
    s["identityConsistency"] = json!("unproven");
    assert!(has_reason(
        &draft(&db, &input(), Some(&s), "runtime-b", at(300)),
        "identity_unproven"
    ));
}

#[test]
fn retention_or_clear_watermark_removes_old_sample_eligibility() {
    let db = database();
    five(&db, [10.0, 20.0, 30.0, 40.0, 50.0]);
    db.execute(
        "INSERT INTO settings(key,value) VALUES('clearWatermark',?1)",
        [json!(timestamp(at(1))).to_string()],
    )
    .unwrap();
    assert!(has_reason(
        &result(&db),
        "comparable_covered_intervals_insufficient"
    ));
    db.execute(
        "UPDATE settings SET value='\"invalid\"' WHERE key='clearWatermark'",
        [],
    )
    .unwrap();
    assert!(has_reason(&result(&db), "history_retention_unknown"));
}

#[test]
fn state_merge_and_rollback_preserve_existing_plan_children() {
    let db = database();
    let p = plan("p", 0, 30);
    db.execute(
        "INSERT INTO plan_period_state VALUES('plan','p','p',?1)",
        [json!({"latched":[80,100],"windowPlan":{"known":true}}).to_string()],
    )
    .unwrap();
    assert!(initialize(&db, &p, Some(&snapshot(0)), "runtime-a", &reading(0)).is_err());
    db.execute_batch("BEGIN").unwrap();
    initialize(&db, &p, Some(&snapshot(0)), "runtime-a", &reading(0)).unwrap();
    assert!(state(&db, "p").unwrap()["demandHistory"].is_object());
    db.execute_batch("ROLLBACK").unwrap();
    assert!(state(&db, "p").unwrap().get("demandHistory").is_none());
    begin(&db, &p, 0);
    let s = state(&db, "p").unwrap();
    assert_eq!(s["latched"], json!([80, 100]));
    assert_eq!(s["windowPlan"]["known"], true);
}

#[test]
fn declining_or_missing_usage_does_not_become_covered_zero() {
    for missing in [false, true] {
        let db = database();
        let p = plan("p", 0, 30);
        begin(&db, &p, 0);
        scan(&db, &p, &view(&p, 10, 10.0, false), 10, true);
        let mut v = view(&p, 20, 0.0, false);
        if missing {
            v["used"] = Value::Null;
        }
        scan(&db, &p, &v, 20, true);
        assert_eq!(
            state(&db, "p").unwrap()["demandHistory"]["status"],
            "invalidated"
        );
    }
}

fn fingerprint_fixture(db: &Connection) {
    for (id, seconds, data) in [
        ("a", 1, r#"{"total":10,"revision":1}"#),
        ("b", 2, r#"{"total":20,"revision":1}"#),
    ] {
        db.execute(
            "INSERT INTO usage_responses(id,source_id,event_time,data) VALUES(?1,'source-a',?2,?3)",
            params![id, timestamp(at(seconds)), data],
        )
        .unwrap();
    }
    db.execute("INSERT INTO excluded_facts VALUES('b','kept-old')", [])
        .unwrap();
    db.execute(
        "INSERT INTO fact_aliases VALUES('legacy-a','a',?1)",
        [r#"{"proof":"old"}"#],
    )
    .unwrap();
}
fn subtract_fresh(db: &Connection, ids: &[&str]) -> Option<String> {
    ledger_fingerprint_excluding_fresh(
        db,
        "source-a",
        at(0),
        at(30),
        &ids.iter().map(|id| (*id).to_owned()).collect(),
    )
    .unwrap()
}

#[test]
fn fingerprint_keeps_the_preexisting_raw_serialization_and_metadata_golden_digest() {
    let db = database();
    fingerprint_fixture(&db);
    // Fixed digest of the pre-refactor ordered raw rows, raw JSON text,
    // exclusions, aliases and exact RFC3339 interval. This is not derived by
    // the new subtraction path or by parsing/re-serializing fact data.
    let expected = "b46ce3b432d250ee22c5909a6985fc99ba99ac77bd539890e6eb132c6b7094f8";
    assert_eq!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        expected
    );
    assert_eq!(subtract_fresh(&db, &[]).as_deref(), Some(expected));
    assert_eq!(
        subtract_fresh(&db, &["not-present"]).as_deref(),
        Some(expected)
    );
}

#[test]
fn fresh_raw_facts_are_subtracted_while_all_old_metadata_remains_in_the_digest() {
    let db = database();
    fingerprint_fixture(&db);
    let before = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    fact(&db, "fresh-c", at(3), 40.0);
    fact(&db, "fresh-d", at(4), 50.0);
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        before
    );
    assert_eq!(
        subtract_fresh(&db, &["fresh-c", "fresh-d"]),
        Some(before.clone())
    );
    assert_ne!(subtract_fresh(&db, &["fresh-c"]), Some(before));
}

#[test]
fn same_total_old_fact_revisions_and_alias_or_exclusion_changes_are_never_hidden() {
    for change in [
        "raw-revision",
        "alias-evidence",
        "exclusion-reason",
        "new-old-alias",
        "new-old-exclusion",
    ] {
        let db = database();
        fingerprint_fixture(&db);
        let before = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
        fact(&db, "fresh-c", at(3), 40.0);
        match change {
            "raw-revision" => {
                db.execute(
                    "UPDATE usage_responses SET data=?1 WHERE id='a'",
                    [r#"{"total":10,"revision":2}"#],
                )
                .unwrap();
            }
            "alias-evidence" => {
                db.execute(
                    "UPDATE fact_aliases SET evidence=?1 WHERE alias='legacy-a'",
                    [r#"{"proof":"revised"}"#],
                )
                .unwrap();
            }
            "exclusion-reason" => {
                db.execute(
                    "UPDATE excluded_facts SET reason='revised' WHERE fact_id='b'",
                    [],
                )
                .unwrap();
            }
            "new-old-alias" => {
                db.execute("INSERT INTO fact_aliases VALUES('another-a','a','{}')", [])
                    .unwrap();
            }
            "new-old-exclusion" => {
                db.execute("INSERT INTO excluded_facts VALUES('a','new-conflict')", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let adjusted = subtract_fresh(&db, &["fresh-c"]);
        assert!(
            adjusted.is_some(),
            "old-only metadata is preserved, not removed: {change}"
        );
        assert_ne!(
            adjusted,
            Some(before),
            "same-total prior evidence changed: {change}"
        );
    }
}

#[test]
fn fresh_fact_with_either_alias_direction_or_exclusion_refuses_subtraction() {
    for relation in [
        "excluded",
        "alias-from-fresh",
        "alias-to-fresh",
        "fresh-to-outside",
    ] {
        let db = database();
        fact(&db, "old", at(1), 10.0);
        fact(&db, "fresh", at(2), 20.0);
        match relation {
            "excluded" => {
                db.execute("INSERT INTO excluded_facts VALUES('fresh','conflict')", [])
                    .unwrap();
            }
            "alias-from-fresh" => {
                db.execute("INSERT INTO fact_aliases VALUES('fresh','old','{}')", [])
                    .unwrap();
            }
            "alias-to-fresh" => {
                db.execute(
                    "INSERT INTO fact_aliases VALUES('old-alias','fresh','{}')",
                    [],
                )
                .unwrap();
            }
            "fresh-to-outside" => {
                db.execute(
                    "INSERT INTO fact_aliases VALUES('fresh','outside-interval','{}')",
                    [],
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(subtract_fresh(&db, &["fresh"]).is_none(), "{relation}");
        assert!(
            subtract_fresh(&db, &[]).is_some(),
            "ordinary metadata still hashes: {relation}"
        );
    }
}

#[test]
fn fresh_id_outside_raw_interval_still_refuses_when_its_alias_is_relevant_to_old_interval() {
    let db = database();
    fact(&db, "old", at(1), 10.0);
    fact(&db, "fresh-outside", at(60), 20.0);
    db.execute(
        "INSERT INTO fact_aliases VALUES('fresh-outside','old','{}')",
        [],
    )
    .unwrap();
    assert!(subtract_fresh(&db, &["fresh-outside"]).is_none());
}

#[test]
fn unrelated_fresh_ids_old_facts_and_metadata_do_not_change_interval_subtraction() {
    let db = database();
    fingerprint_fixture(&db);
    let before = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    fact(&db, "before-start", at(-1), 1.0);
    fact(&db, "after-end", at(60), 1.0);
    db.execute(
        "INSERT INTO usage_responses VALUES('other-source','source-b',?1,'{}')",
        [timestamp(at(1))],
    )
    .unwrap();
    for id in ["before-start", "after-end", "other-source"] {
        db.execute("INSERT INTO excluded_facts VALUES(?1,'unrelated')", [id])
            .unwrap();
        db.execute(
            "INSERT INTO fact_aliases VALUES(?1,?2,'{}')",
            params![format!("alias-{id}"), id],
        )
        .unwrap();
    }
    assert_eq!(
        subtract_fresh(&db, &["before-start", "after-end", "other-source"]),
        Some(before)
    );
}

#[test]
fn fresh_subtraction_preserves_exact_half_open_microsecond_boundaries() {
    let db = database();
    let start = at(0) + Duration::microseconds(100);
    let end = at(30) - Duration::microseconds(100);
    fact(&db, "old-at-start", start, 10.0);
    let before = ledger_fingerprint(&db, "source-a", start, end).unwrap();
    fact(
        &db,
        "fresh-before-end",
        end - Duration::microseconds(100),
        20.0,
    );
    fact(
        &db,
        "outside-before-start",
        start - Duration::microseconds(100),
        30.0,
    );
    fact(&db, "outside-exact-end", end, 40.0);
    db.execute(
        "INSERT INTO excluded_facts VALUES('outside-exact-end','unrelated')",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO fact_aliases VALUES('outside-alias','outside-before-start','{}')",
        [],
    )
    .unwrap();
    let fresh = [
        "fresh-before-end",
        "outside-before-start",
        "outside-exact-end",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        ledger_fingerprint_excluding_fresh(&db, "source-a", start, end, &fresh).unwrap(),
        Some(before.clone())
    );
    assert_ne!(
        ledger_fingerprint(&db, "source-a", start, end).unwrap(),
        before
    );
    assert!(
        ledger_fingerprint_excluding_fresh(&db, "source-a", start, end, &BTreeSet::new())
            .unwrap()
            .is_some()
    );
}

#[test]
fn empty_and_all_fresh_intervals_match_the_original_empty_digest() {
    let db = database();
    let empty = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    assert_eq!(subtract_fresh(&db, &[]), Some(empty.clone()));
    assert_eq!(subtract_fresh(&db, &["missing"]), Some(empty.clone()));
    fact(&db, "only-new", at(1), 10.0);
    assert_eq!(subtract_fresh(&db, &["only-new"]), Some(empty));
    let equal_boundary = ledger_fingerprint(&db, "source-a", at(1), at(1)).unwrap();
    assert_eq!(
        ledger_fingerprint_excluding_fresh(
            &db,
            "source-a",
            at(1),
            at(1),
            &["only-new".to_owned()].into_iter().collect()
        )
        .unwrap(),
        Some(equal_boundary)
    );
}

fn turn_table(db: &Connection) {
    db.execute_batch("CREATE TABLE turns(id TEXT PRIMARY KEY,data TEXT NOT NULL);")
        .unwrap();
}
fn fallback_fact(db: &Connection, id: &str, seconds: i64, turn: &str) {
    db.execute(
        "INSERT INTO usage_responses VALUES(?1,'source-a',?2,?3)",
        params![
            id,
            timestamp(at(seconds)),
            json!({"sourceId":"source-a","turnId":turn,"requestedModel":null,"total":10})
                .to_string()
        ],
    )
    .unwrap();
}
fn turn_fact(db: &Connection, row_id: &str, source: &str, turn: &str, model: Value) {
    db.execute(
        "INSERT INTO turns VALUES(?1,?2)",
        params![row_id,json!({"sourceId":source,"id":turn,"model":model,"eventTime":timestamp(at(-86_400)),"title":"ignored metadata"}).to_string()],
    ).unwrap();
}

#[test]
fn fallback_turn_model_swap_changes_prefix_and_full_digests_despite_equal_token_totals() {
    let db = database();
    turn_table(&db);
    fallback_fact(&db, "early-response", 2, "early-turn");
    fallback_fact(&db, "late-response", 20, "late-turn");
    turn_fact(&db, "row-early", "source-a", "early-turn", json!("model-a"));
    turn_fact(&db, "row-late", "source-a", "late-turn", json!("model-b"));
    let before_prefix = ledger_fingerprint(&db, "source-a", at(0), at(10)).unwrap();
    let before_all = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    db.execute("UPDATE turns SET data=json_set(data,'$.model',CASE id WHEN 'row-early' THEN 'model-b' ELSE 'model-a' END)",[]).unwrap();
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(10)).unwrap(),
        before_prefix
    );
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        before_all
    );
    assert_eq!(
        db.query_row(
            "SELECT SUM(json_extract(data,'$.total')) FROM usage_responses",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        20
    );
}

#[test]
fn fallback_missing_turn_appearance_and_later_known_model_each_change_the_digest() {
    let db = database();
    turn_table(&db);
    fallback_fact(&db, "response", 2, "missing-turn");
    let absent = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    turn_fact(&db, "late-row", "source-a", "missing-turn", Value::Null);
    let unknown = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    assert_ne!(
        absent, unknown,
        "existence is evidence even while the model is missing"
    );
    db.execute(
        "UPDATE turns SET data=json_set(data,'$.model','model-a')",
        [],
    )
    .unwrap();
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        unknown
    );
}

#[test]
fn fresh_only_turn_dependency_is_removed_before_turn_table_access_or_model_changes() {
    let db = database();
    fact(&db, "old-without-turn", at(1), 10.0);
    let baseline = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    fallback_fact(&db, "fresh-with-turn", 2, "fresh-turn");
    assert_eq!(
        subtract_fresh(&db, &["fresh-with-turn"]),
        Some(baseline.clone()),
        "no remaining lookup means no turns table is required"
    );
    turn_table(&db);
    turn_fact(
        &db,
        "new-turn-row",
        "source-a",
        "fresh-turn",
        json!("model-a"),
    );
    let whole = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    db.execute(
        "UPDATE turns SET data=json_set(data,'$.model','model-b')",
        [],
    )
    .unwrap();
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        whole
    );
    assert_eq!(subtract_fresh(&db, &["fresh-with-turn"]), Some(baseline));
}

#[test]
fn unrelated_turns_and_non_model_metadata_do_not_change_relevant_fallback_evidence() {
    let db = database();
    turn_table(&db);
    fallback_fact(&db, "response", 2, "used-turn");
    turn_fact(&db, "used-row", "source-a", "used-turn", json!("model-a"));
    let baseline = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    turn_fact(
        &db,
        "unused-row",
        "source-a",
        "unrelated-turn",
        json!("model-b"),
    );
    turn_fact(
        &db,
        "other-source-row",
        "source-b",
        "used-turn",
        json!("model-c"),
    );
    db.execute("UPDATE turns SET data=json_set(data,'$.title','changed','$.eventTime','2100-01-01T00:00:00Z')",[]).unwrap();
    assert_eq!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        baseline,
        "turn time and unrelated data never filter or contaminate lookup evidence"
    );
}

#[test]
fn fallback_duplicate_json_turn_keys_use_last_model_and_preserve_empty_string_semantics() {
    let db = database();
    turn_table(&db);
    fallback_fact(&db, "response", 2, "duplicate-turn");
    turn_fact(
        &db,
        "first-storage-row",
        "source-a",
        "duplicate-turn",
        json!("model-a"),
    );
    turn_fact(
        &db,
        "last-storage-row",
        "source-a",
        "duplicate-turn",
        json!("model-b"),
    );
    let last_wins = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    db.execute("UPDATE turns SET data=json_set(data,'$.model','ignored-first-change') WHERE id='first-storage-row'",[]).unwrap();
    assert_eq!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        last_wins
    );
    db.execute(
        "UPDATE turns SET data=json_set(data,'$.model','') WHERE id='last-storage-row'",
        [],
    )
    .unwrap();
    let empty_string = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
    assert_ne!(empty_string, last_wins);
    db.execute(
        "UPDATE turns SET data=json_set(data,'$.model',NULL) WHERE id='last-storage-row'",
        [],
    )
    .unwrap();
    assert_ne!(
        ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap(),
        empty_string,
        "queries accepts an empty model string and distinguishes it from no string"
    );
}

#[test]
fn explicit_requested_model_including_empty_string_never_reads_turns() {
    for model in ["model-a", ""] {
        let db = database();
        db.execute("INSERT INTO usage_responses VALUES('response','source-a',?1,?2)",params![timestamp(at(1)),json!({"sourceId":"source-a","turnId":"unused-turn","requestedModel":model,"total":10}).to_string()]).unwrap();
        let baseline = ledger_fingerprint(&db, "source-a", at(0), at(30)).unwrap();
        assert_eq!(subtract_fresh(&db, &[]), Some(baseline));
    }
}
