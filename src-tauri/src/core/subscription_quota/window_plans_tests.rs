use super::*;
use chrono::Duration;

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}
fn start() -> DateTime<Utc> {
    at("2026-10-22T09:30:00Z")
}
fn end() -> DateTime<Utc> {
    at("2026-10-22T10:00:00Z")
}
fn later() -> DateTime<Utc> {
    at("2026-10-22T11:00:00Z")
}
fn plan() -> Value {
    json!({"id":"plan-a","revision":1,"sourceId":"source-a","title":"窗口计划", "type":"tokens",
        "effectiveFrom":start().to_rfc3339(),"endsAt":end().to_rfc3339(),"endPolicy":"window",
        "windowId":"shared:primary","periodInstanceId":"window-a","lifecycle":"active"})
}
fn db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE usage_plans(plan_id TEXT NOT NULL,revision INTEGER NOT NULL,source_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(plan_id,revision)); CREATE TABLE plan_period_state(owner_kind TEXT NOT NULL,owner_id TEXT NOT NULL,period_instance_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(owner_kind,owner_id,period_instance_id));").unwrap();
    let p = plan();
    db.execute(
        "INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES('plan-a',1,'source-a',?1)",
        [p.to_string()],
    )
    .unwrap();
    db
}
fn snapshot(sequence: u64, time: DateTime<Utc>, id: &str) -> Value {
    json!({
        "runtimeId":"runtime-a", "sourceId":"source-a", "bindingGeneration":"1", "identityEpoch":"1",
        "clearGeneration":null, "accountScope":null, "observationSequence":sequence.to_string(),
        "identityConsistency":"proven", "identityEvidence":{"kind":"synthetic_fixture_only"},
        "observationOutcome":"success", "status":"connected",
        "quotaRead":{"outcome":"success", "freshnessAnchor":time.to_rfc3339()},
        "buckets":[{"id":"shared", "primary":{"windowInstanceId":id,"usedPercent":3.0,
            "recovery":"fixed_reset", "resetsAt":end().timestamp()}}]
    })
}
fn proof(
    value: &mut Value,
    new: bool,
    same: bool,
    id: &str,
    next: Option<Interval>,
    boundary: Option<Interval>,
) {
    let evidence = ResetEvidence {
        confirmed_new_window: new,
        confirmed_same_window: same,
        window_instance_id: Some(id.into()),
        next_reset: next,
        actual_boundary: boundary,
    };
    value["buckets"][0]["primary"]["resetEvidence"] = serde_json::to_value(evidence).unwrap();
    value["buckets"][0]["primary"]["resetEvidence"]["previousWindowInstanceId"] = json!("window-a");
}
fn initialize(db: &Connection) {
    advance(db, &snapshot(1, start(), "window-a"), start()).unwrap();
}

#[test]
fn reaching_reset_or_changed_reset_timestamp_never_confirms_a_new_or_delayed_window() {
    let db = db();
    initialize(&db);
    assert!(projection(&db, &plan(), start()).unwrap().same_window);
    let p = projection(&db, &plan(), end()).unwrap();
    assert!(!p.same_window && !p.archived);
    let time = end() + Duration::seconds(10);
    let mut read = snapshot(2, time, "window-a");
    read["buckets"][0]["primary"]["resetsAt"] = json!(later().timestamp());
    advance(&db, &read, time).unwrap();
    let p = projection(&db, &plan(), time).unwrap();
    assert!(!p.same_window && !p.archived);
    assert_eq!(p.resolved_end, Some(end()));
    assert!(p.final_percent.is_none());
}

#[test]
fn positive_same_window_postponement_resumes_without_rewriting_configuration() {
    let db = db();
    initialize(&db);
    let time = end() + Duration::seconds(10);
    advance(&db, &snapshot(2, time, "window-a"), time).unwrap();
    let mut read = snapshot(3, time + Duration::seconds(1), "window-a");
    proof(
        &mut read,
        false,
        true,
        "window-a",
        Interval::exact(seconds(later())),
        None,
    );
    advance(&db, &read, time + Duration::seconds(1)).unwrap();
    let p = projection(&db, &plan(), time + Duration::seconds(1)).unwrap();
    assert!(p.same_window && !p.archived && !p.partial);
    assert_eq!(p.resolved_end, Some(later()));
    assert_eq!(latest_plans(&db).unwrap()[0]["endsAt"], end().to_rfc3339());
    assert_eq!(latest_plans(&db).unwrap()[0]["revision"], 1);
}

#[test]
fn exact_confirmed_boundary_archives_and_never_uses_the_successor_percentage() {
    let db = db();
    initialize(&db);
    let time = end() + Duration::seconds(10);
    let mut read = snapshot(2, time, "window-b");
    proof(
        &mut read,
        true,
        false,
        "window-b",
        Interval::exact(seconds(later())),
        Interval::exact(seconds(end())),
    );
    read["buckets"][0]["primary"]["resetEvidence"]["finalUsedPercent"] = json!(88.0);
    advance(&db, &read, time).unwrap();
    let p = projection(&db, &plan(), time).unwrap();
    assert!(p.archived && !p.partial && !p.same_window);
    assert_eq!(p.resolved_end, Some(end()));
    assert_eq!(p.final_percent, Some(88.0));
    assert_ne!(
        p.final_percent,
        read["buckets"][0]["primary"]["usedPercent"].as_f64()
    );
    assert_eq!(
        latest_plans(&db).unwrap()[0]["periodInstanceId"],
        "window-a"
    );
}

#[test]
fn exact_boundary_does_not_invent_a_final_percentage_when_none_was_reported() {
    let db = db();
    initialize(&db);
    let time = end() + Duration::seconds(10);
    let mut read = snapshot(2, time, "window-b");
    proof(
        &mut read,
        true,
        false,
        "window-b",
        Interval::exact(seconds(later())),
        Interval::exact(seconds(end())),
    );
    advance(&db, &read, time).unwrap();
    let p = projection(&db, &plan(), time).unwrap();
    assert!(p.archived && !p.partial);
    assert!(p.final_percent.is_none());
}

#[test]
fn unknown_or_nonexact_actual_boundary_archives_as_partial_not_a_guessed_end() {
    for boundary in [
        None,
        Interval::new(seconds(end()) - 5.0, seconds(end()) + 5.0),
    ] {
        let db = db();
        initialize(&db);
        let time = end() + Duration::seconds(10);
        let mut read = snapshot(2, time, "window-b");
        proof(
            &mut read,
            true,
            false,
            "window-b",
            Interval::exact(seconds(later())),
            boundary,
        );
        read["buckets"][0]["primary"]["resetEvidence"]["finalUsedPercent"] = json!(88.0);
        advance(&db, &read, time).unwrap();
        let p = projection(&db, &plan(), time).unwrap();
        assert!(p.archived && p.partial && !p.same_window);
        assert!(p.resolved_end.is_none() && p.final_percent.is_none());
    }
}

#[test]
fn earlier_manual_reset_uses_its_actual_boundary_and_pre_start_boundary_stays_partial() {
    for (boundary, partial) in [
        (start() + Duration::minutes(10), false),
        (start() - Duration::seconds(1), true),
    ] {
        let db = db();
        initialize(&db);
        let time = start() + Duration::minutes(20);
        let mut read = snapshot(2, time, "window-b");
        proof(
            &mut read,
            true,
            false,
            "window-b",
            Interval::exact(seconds(later())),
            Interval::exact(seconds(boundary)),
        );
        advance(&db, &read, time).unwrap();
        let p = projection(&db, &plan(), time).unwrap();
        assert!(p.archived);
        assert_eq!(p.partial, partial);
        assert_eq!(p.resolved_end, if partial { None } else { Some(boundary) });
    }
}

#[test]
fn no_identity_evidence_failure_stale_or_noncanonical_sequence_cannot_initialize() {
    for kind in 0..6 {
        let db = db();
        let mut read = snapshot(1, start(), "window-a");
        match kind {
            0 => read["identityConsistency"] = json!("unproven"),
            1 => read["identityEvidence"] = Value::Null,
            2 => read["observationOutcome"] = json!("failure"),
            3 => read["quotaRead"]["outcome"] = json!("failure"),
            4 => {
                read["quotaRead"]["freshnessAnchor"] =
                    json!((start() - Duration::seconds(240)).to_rfc3339())
            }
            _ => read["observationSequence"] = json!("01"),
        }
        advance(&db, &read, start()).unwrap();
        let p = projection(&db, &plan(), start()).unwrap();
        assert!(!p.same_window && !p.archived && p.proof_scope.is_none());
    }
}

#[test]
fn runtime_identity_binding_clear_and_source_changes_do_not_rebind_a_plan() {
    for field in [
        "runtimeId",
        "identityEpoch",
        "bindingGeneration",
        "clearGeneration",
        "sourceId",
        "accountScope",
    ] {
        let db = db();
        initialize(&db);
        let time = end() + Duration::seconds(10);
        let mut read = snapshot(2, time, "window-b");
        proof(
            &mut read,
            true,
            false,
            "window-b",
            Interval::exact(seconds(later())),
            Interval::exact(seconds(end())),
        );
        read[field] = match field {
            "identityEpoch" | "bindingGeneration" => json!("2"),
            _ => json!("different-domain"),
        };
        advance(&db, &read, time).unwrap();
        let p = projection(&db, &plan(), time).unwrap();
        assert!(!p.archived && !p.same_window, "{field}");
        assert_eq!(
            p.proof_scope.unwrap()[field],
            snapshot(1, start(), "window-a")[field]
        );
    }
}

#[test]
fn old_or_repeated_sequence_cannot_apply_a_reset_to_newer_state() {
    let db = db();
    initialize(&db);
    let time = start() + Duration::seconds(10);
    advance(&db, &snapshot(3, time, "window-a"), time).unwrap();
    let mut old = snapshot(2, time, "window-b");
    proof(
        &mut old,
        true,
        false,
        "window-b",
        Interval::exact(seconds(later())),
        Interval::exact(seconds(time)),
    );
    advance(&db, &old, time).unwrap();
    old["observationSequence"] = json!("3");
    advance(&db, &old, time).unwrap();
    assert!(!projection(&db, &plan(), time).unwrap().archived);
}

#[test]
fn missing_previous_reference_and_conflicting_or_future_evidence_are_not_reset_proof() {
    for kind in 0..3 {
        let db = db();
        initialize(&db);
        let time = end() + Duration::seconds(10);
        let mut read = snapshot(2, time, "window-b");
        proof(
            &mut read,
            true,
            kind == 1,
            "window-b",
            Interval::exact(seconds(later())),
            Interval::exact(seconds(if kind == 2 {
                time + Duration::seconds(1)
            } else {
                end()
            })),
        );
        if kind == 0 {
            read["buckets"][0]["primary"]["resetEvidence"]["previousWindowInstanceId"] =
                json!("other-old-window");
        }
        advance(&db, &read, time).unwrap();
        let p = projection(&db, &plan(), time).unwrap();
        assert!(!p.archived && !p.same_window);
        assert_eq!(p.resolved_end, Some(end()));
    }
}

#[test]
fn uncertain_postponement_cannot_be_undone_by_a_plain_read() {
    let db = db();
    initialize(&db);
    let time = start() + Duration::seconds(10);
    let mut read = snapshot(2, time, "window-a");
    proof(
        &mut read,
        false,
        true,
        "window-a",
        Interval::new(seconds(end()) - 5.0, seconds(end()) + 5.0),
        None,
    );
    advance(&db, &read, time).unwrap();
    assert!(!projection(&db, &plan(), time).unwrap().same_window);
    advance(&db, &snapshot(3, time, "window-a"), time).unwrap();
    assert!(!projection(&db, &plan(), time).unwrap().same_window);
    let mut confirmed = snapshot(4, time, "window-a");
    proof(
        &mut confirmed,
        false,
        true,
        "window-a",
        Interval::exact(seconds(later())),
        None,
    );
    advance(&db, &confirmed, time).unwrap();
    assert!(projection(&db, &plan(), time).unwrap().same_window);
}

#[test]
fn projection_is_read_only_and_transaction_rollback_keeps_state_and_latches() {
    let mut db = db();
    save(
        &db,
        "plan-a",
        &json!({"latched":[80],"runtimeId":"runtime-a","unrelated":"preserved"}),
    )
    .unwrap();
    initialize(&db);
    let original = state(&db, "plan-a").unwrap();
    assert_eq!(original["latched"], json!([80]));
    assert_eq!(original["unrelated"], "preserved");
    let time = end() + Duration::seconds(10);
    let mut read = snapshot(2, time, "window-b");
    proof(
        &mut read,
        true,
        false,
        "window-b",
        Interval::exact(seconds(later())),
        Interval::exact(seconds(end())),
    );
    {
        let tx = db.transaction().unwrap();
        advance(&tx, &read, time).unwrap();
        tx.rollback().unwrap();
    }
    assert_eq!(state(&db, "plan-a").unwrap(), original);
    let before: i64 = db
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    projection(&db, &plan(), time).unwrap();
    projection(&db, &plan(), time).unwrap();
    let after: i64 = db
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn terminal_archive_never_reopens_and_fixed_time_plans_are_untouched() {
    let db = db();
    initialize(&db);
    let time = end() + Duration::seconds(10);
    let mut read = snapshot(2, time, "window-b");
    proof(
        &mut read,
        true,
        false,
        "window-b",
        Interval::exact(seconds(later())),
        None,
    );
    advance(&db, &read, time).unwrap();
    let archived = state(&db, "plan-a").unwrap();
    let mut original_window = snapshot(3, time, "window-a");
    proof(
        &mut original_window,
        false,
        true,
        "window-a",
        Interval::exact(seconds(later())),
        None,
    );
    advance(&db, &original_window, time).unwrap();
    assert_eq!(state(&db, "plan-a").unwrap(), archived);
    let mut fixed = plan();
    fixed["id"] = json!("fixed");
    fixed["endPolicy"] = json!("time");
    db.execute(
        "INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES('fixed',1,'source-a',?1)",
        [fixed.to_string()],
    )
    .unwrap();
    advance(&db, &snapshot(4, time, "window-a"), time).unwrap();
    assert_eq!(state(&db, "fixed").unwrap(), json!({}));
}
