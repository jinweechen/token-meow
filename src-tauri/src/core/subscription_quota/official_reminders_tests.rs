use super::*;
use chrono::Duration;

fn at(minutes: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T09:30:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::minutes(minutes)
}
fn config() -> Value {
    json!({"id":"plan-a","revision":1,"sourceId":"source-a","title":"官方目标",
        "type":"percent","target":80.0,"effectiveFrom":timestamp(at(0)),"endsAt":timestamp(at(30)),
        "endPolicy":"window","windowId":"shared:primary","periodInstanceId":"window-a",
        "lifecycle":"active","remindersEnabled":true})
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE usage_plans(plan_id TEXT NOT NULL,revision INTEGER NOT NULL,source_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(plan_id,revision)); CREATE TABLE plan_period_state(owner_kind TEXT NOT NULL,owner_id TEXT NOT NULL,period_instance_id TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(owner_kind,owner_id,period_instance_id)); CREATE TABLE account_snapshots(id INTEGER PRIMARY KEY AUTOINCREMENT,collected_at TEXT NOT NULL,data TEXT NOT NULL); CREATE TABLE inbox_events(id TEXT PRIMARY KEY,thread_id TEXT,turn_id TEXT,event_time TEXT,data TEXT NOT NULL);").unwrap();
    write_config(&db, &config());
    save(
        &db,
        "plan-a",
        &json!({"officialBaselinePending":true,"latched":[80],"preserved":{"value":7}}),
    )
    .unwrap();
    db
}
fn write_config(db: &Connection, value: &Value) {
    db.execute(
        "INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES(?1,?2,?3,?4)",
        params![
            value["id"].as_str(),
            value["revision"].as_i64(),
            value["sourceId"].as_str(),
            value.to_string()
        ],
    )
    .unwrap();
}
fn snapshot(sequence: u64, minutes: i64, used: f64) -> Value {
    json!({"runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1",
        "clearGeneration":null,"accountScope":null,"observationSequence":sequence.to_string(),
        "identityConsistency":"proven","identityEvidence":{"kind":"synthetic_fixture_only"},
        "observationOutcome":"success","status":"connected","persisted":false,
        "quotaRead":{"outcome":"success","freshnessAnchor":timestamp(at(minutes))},
        "buckets":[{"id":"shared","primary":{"windowInstanceId":"window-a","usedPercent":used,
            "recovery":"fixed_reset","resetsAt":at(30).timestamp()}}]})
}
fn committed(value: &Value) -> Value {
    let mut value = value.clone();
    value["persisted"] = json!(true);
    value
}
fn receipt(db: &Connection, value: &Value) {
    receipt_with_encoding(db, value, false);
}
fn receipt_with_encoding(db: &Connection, value: &Value, encoded: bool) {
    let record = json!({"documentKind":"subscription_observation","schemaVersion":1,"outcome":"success",
        "runtimeId":value["runtimeId"],"sourceId":value["sourceId"],"bindingGeneration":value["bindingGeneration"],
        "identityEpoch":value["identityEpoch"],"clearGeneration":value["clearGeneration"],
        "observationSequence":value["observationSequence"],"lastPersistedSequence":value["observationSequence"],
        "observation":value});
    let record = if encoded {
        super::super::observation_storage::encode(
            db,
            &record,
            instant(&value["quotaRead"]["freshnessAnchor"]).unwrap(),
        )
        .unwrap()
    } else {
        record
    };
    db.execute(
        "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
        params![
            value["quotaRead"]["freshnessAnchor"].as_str(),
            record.to_string()
        ],
    )
    .unwrap();
}
fn apply(
    db: &mut Connection,
    previous: Option<&Value>,
    current: &Value,
    now: DateTime<Utc>,
) -> Result<()> {
    let tx = db.transaction().map_err(err)?;
    receipt(&tx, current);
    window_plans::advance(&tx, current, now)?;
    advance(&tx, previous, current, "runtime-a", now)?;
    tx.commit().map_err(err)
}
fn events(db: &Connection) -> Vec<Value> {
    let mut query = db
        .prepare("SELECT data FROM inbox_events ORDER BY id")
        .unwrap();
    query
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect()
}
fn baseline(db: &mut Connection, used: f64) -> Value {
    let first = snapshot(1, 0, used);
    apply(db, None, &first, at(0)).unwrap();
    committed(&first)
}

#[test]
fn official_target_is_native_eighty_not_eighty_percent_of_eighty_and_is_panel_only() {
    let mut db = database();
    let mut previous = baseline(&mut db, 63.0);
    for (sequence, minutes, used) in [(2, 2, 64.0), (3, 4, 79.0), (4, 6, 80.0)] {
        let current = snapshot(sequence, minutes, used);
        apply(&mut db, Some(&previous), &current, at(minutes)).unwrap();
        previous = committed(&current);
        assert_eq!(events(&db).len(), usize::from(used >= 80.0));
    }
    let values = events(&db);
    let event = &values[0];
    assert_eq!(event["officialTargetPercent"], 80.0);
    assert_eq!(event["threshold"], 100);
    assert_eq!(event["periodInstanceId"], "window-a");
    assert_eq!(event["ownerKind"], "plan");
    assert_eq!(event["notifyEligible"], false);
    assert_eq!(event["animationAcknowledged"], true);
    assert_eq!(event["notificationState"], "panel_only");
    assert_eq!(
        event["id"],
        hash(b"plan|plan-a|window-a|official_target|100|0")
    );
    let stored = state(&db, "plan-a").unwrap();
    assert_eq!(stored["latched"], json!([80, 100]));
    assert_eq!(stored["preserved"]["value"], 7);
    assert!(stored["windowPlan"].is_object());
}

#[test]
fn first_observation_and_already_above_target_never_backfill_a_crossing() {
    let mut db = database();
    let previous = baseline(&mut db, 85.0);
    apply(&mut db, Some(&previous), &snapshot(2, 2, 90.0), at(2)).unwrap();
    assert!(events(&db).is_empty());
    assert_eq!(
        state(&db, "plan-a").unwrap()["officialBaselinePending"],
        false
    );
}

#[test]
fn duplicate_old_results_and_configuration_revision_cannot_rearm_the_latch() {
    let mut db = database();
    let first = baseline(&mut db, 79.0);
    let crossed = snapshot(2, 2, 81.0);
    apply(&mut db, Some(&first), &crossed, at(2)).unwrap();
    db.execute(
        "UPDATE inbox_events SET data=json_set(data,'$.read',json('true'))",
        [],
    )
    .unwrap();
    apply(&mut db, Some(&first), &crossed, at(2)).unwrap();
    apply(&mut db, Some(&committed(&crossed)), &first, at(2)).unwrap();
    let mut config = config();
    config["revision"] = json!(2);
    config["title"] = json!("修改标题");
    write_config(&db, &config);
    let mut s = state(&db, "plan-a").unwrap();
    s["officialBaselinePending"] = json!(true);
    save(&db, "plan-a", &s).unwrap();
    let below = snapshot(3, 4, 79.0);
    apply(&mut db, Some(&committed(&crossed)), &below, at(4)).unwrap();
    apply(
        &mut db,
        Some(&committed(&below)),
        &snapshot(4, 6, 82.0),
        at(6),
    )
    .unwrap();
    assert_eq!(events(&db).len(), 1);
    assert_eq!(events(&db)[0]["read"], true);
    assert_eq!(state(&db, "plan-a").unwrap()["latched"], json!([80, 100]));
}

#[test]
fn target_edit_and_pause_resume_establish_new_baselines_without_replaying() {
    let mut db = database();
    let first = baseline(&mut db, 69.0);
    let mut p = config();
    p["revision"] = json!(2);
    p["target"] = json!(70);
    write_config(&db, &p);
    // Target comparison also protects against a caller forgetting pending.
    let changed = snapshot(2, 2, 71.0);
    apply(&mut db, Some(&first), &changed, at(2)).unwrap();
    assert!(events(&db).is_empty());
    p["revision"] = json!(3);
    p["remindersEnabled"] = json!(false);
    write_config(&db, &p);
    let mut s = state(&db, "plan-a").unwrap();
    s["officialBaselinePending"] = json!(true);
    save(&db, "plan-a", &s).unwrap();
    let paused = snapshot(3, 4, 85.0);
    apply(&mut db, Some(&committed(&changed)), &paused, at(4)).unwrap();
    p["revision"] = json!(4);
    p["remindersEnabled"] = json!(true);
    write_config(&db, &p);
    let mut s = state(&db, "plan-a").unwrap();
    s["officialBaselinePending"] = json!(true);
    save(&db, "plan-a", &s).unwrap();
    apply(
        &mut db,
        Some(&committed(&paused)),
        &snapshot(4, 6, 90.0),
        at(6),
    )
    .unwrap();
    assert!(events(&db).is_empty());
}

#[test]
fn pending_baseline_absorbs_one_real_crossing_then_allows_a_later_new_crossing() {
    let mut db = database();
    let first = baseline(&mut db, 79.0);
    let mut s = state(&db, "plan-a").unwrap();
    s["officialBaselinePending"] = json!(true);
    save(&db, "plan-a", &s).unwrap();
    let next = snapshot(2, 2, 81.0);
    apply(&mut db, Some(&first), &next, at(2)).unwrap();
    assert!(events(&db).is_empty());
    let below = snapshot(3, 4, 79.0);
    apply(&mut db, Some(&committed(&next)), &below, at(4)).unwrap();
    apply(
        &mut db,
        Some(&committed(&below)),
        &snapshot(4, 6, 81.0),
        at(6),
    )
    .unwrap();
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn unproven_unpersisted_stale_and_historical_previous_cannot_authorize_a_crossing() {
    for kind in 0..5 {
        let mut db = database();
        let mut previous = baseline(&mut db, 79.0);
        let minutes = if kind == 4 { 4 } else { 2 };
        match kind {
            0 => previous["identityConsistency"] = json!("unproven"),
            1 => previous["persisted"] = json!(false),
            2 => previous["historyOnly"] = json!(true),
            3 => {
                db.execute("DELETE FROM account_snapshots", []).unwrap();
            }
            _ => {}
        }
        apply(
            &mut db,
            Some(&previous),
            &snapshot(2, minutes, 81.0),
            at(minutes),
        )
        .unwrap();
        assert!(events(&db).is_empty(), "kind {kind}");
    }
}

#[test]
fn changed_runtime_source_binding_identity_clear_account_or_window_cannot_cross() {
    for (key, value) in [
        ("runtimeId", json!("runtime-b")),
        ("sourceId", json!("source-b")),
        ("bindingGeneration", json!("2")),
        ("identityEpoch", json!("2")),
        ("clearGeneration", json!("new-clear")),
        ("accountScope", json!("new-account")),
    ] {
        let mut db = database();
        let previous = baseline(&mut db, 79.0);
        let mut current = snapshot(2, 2, 81.0);
        current[key] = value;
        apply(&mut db, Some(&previous), &current, at(2)).unwrap();
        assert!(events(&db).is_empty(), "{key}");
    }
    let mut db = database();
    let previous = baseline(&mut db, 79.0);
    let mut current = snapshot(2, 2, 81.0);
    current["buckets"][0]["primary"]["windowInstanceId"] = json!("window-b");
    apply(&mut db, Some(&previous), &current, at(2)).unwrap();
    assert!(events(&db).is_empty());
}

#[test]
fn current_receipt_is_required_even_when_the_window_projection_was_updated() {
    let mut db = database();
    let previous = baseline(&mut db, 79.0);
    let current = snapshot(2, 2, 81.0);
    let tx = db.transaction().unwrap();
    window_plans::advance(&tx, &current, at(2)).unwrap();
    advance(&tx, Some(&previous), &current, "runtime-a", at(2)).unwrap();
    tx.commit().unwrap();
    assert!(events(&db).is_empty());
    assert_eq!(
        state(&db, "plan-a").unwrap()["officialBaseline"]["observationSequence"],
        "1"
    );
}

#[test]
fn failed_event_insert_rolls_back_receipt_window_progress_and_latch_together() {
    let mut db = database();
    let previous = baseline(&mut db, 79.0);
    let before = state(&db, "plan-a").unwrap();
    db.execute_batch("CREATE TRIGGER fail_official BEFORE INSERT ON inbox_events BEGIN SELECT RAISE(ABORT,'fixture transaction failure'); END;").unwrap();
    let current = snapshot(2, 2, 81.0);
    assert!(apply(&mut db, Some(&previous), &current, at(2)).is_err());
    assert_eq!(state(&db, "plan-a").unwrap(), before);
    assert!(events(&db).is_empty());
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM account_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    db.execute_batch("DROP TRIGGER fail_official;").unwrap();
    apply(&mut db, Some(&previous), &current, at(2)).unwrap();
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn percent_decrease_cannot_rearm_an_existing_official_latch() {
    let mut db = database();
    let first = baseline(&mut db, 79.0);
    let high = snapshot(2, 2, 81.0);
    apply(&mut db, Some(&first), &high, at(2)).unwrap();
    let low = snapshot(3, 4, 60.0);
    apply(&mut db, Some(&committed(&high)), &low, at(4)).unwrap();
    apply(
        &mut db,
        Some(&committed(&low)),
        &snapshot(4, 6, 90.0),
        at(6),
    )
    .unwrap();
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn reset_wait_and_positive_postponement_cannot_replay_waiting_interval_growth() {
    let mut db = database();
    let before = snapshot(1, 29, 79.0);
    apply(&mut db, None, &before, at(29)).unwrap();
    let mut resumed = snapshot(2, 30, 81.0);
    resumed["quotaRead"]["freshnessAnchor"] = json!(timestamp(at(30) + Duration::seconds(10)));
    resumed["buckets"][0]["primary"]["resetsAt"] = json!(at(90).timestamp());
    resumed["buckets"][0]["primary"]["resetEvidence"] = json!({"confirmedNewWindow":false,"confirmedSameWindow":true,
        "windowInstanceId":"window-a","nextReset":{"lower":at(90).timestamp() as f64,"upper":at(90).timestamp() as f64},"actualBoundary":null});
    apply(
        &mut db,
        Some(&committed(&before)),
        &resumed,
        at(30) + Duration::seconds(10),
    )
    .unwrap();
    assert!(events(&db).is_empty()); // no intermediate poll was needed to preserve the old boundary
    let mut below = snapshot(3, 32, 79.0);
    below["buckets"][0]["primary"]["resetsAt"] = json!(at(90).timestamp());
    apply(&mut db, Some(&committed(&resumed)), &below, at(32)).unwrap();
    let mut later = snapshot(4, 34, 81.0);
    later["buckets"][0]["primary"]["resetsAt"] = json!(at(90).timestamp());
    apply(&mut db, Some(&committed(&below)), &later, at(34)).unwrap();
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn autocommit_is_rejected_so_a_latch_cannot_commit_separately_from_its_receipt() {
    let db = database();
    assert!(advance(&db, None, &snapshot(1, 0, 81.0), "runtime-a", at(0)).is_err());
    assert!(events(&db).is_empty());
}

#[test]
fn official_crossing_resolves_a_compressed_previous_round_and_rejects_a_removed_base() {
    for remove_base in [false, true] {
        let mut db = database();
        let mut previous = None;
        for (sequence, minutes, used) in [(1, 0, 79.0), (2, 2, 79.0), (3, 4, 80.0)] {
            if sequence == 3 && remove_base {
                db.execute("DELETE FROM account_snapshots WHERE id=1", [])
                    .unwrap();
            }
            let current = snapshot(sequence, minutes, used);
            let tx = db.transaction().unwrap();
            receipt_with_encoding(&tx, &current, true);
            if sequence == 2 {
                let encoded: String = tx.query_row("SELECT json_extract(data,'$.subscriptionStorage.encoding') FROM account_snapshots ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).unwrap();
                assert_eq!(encoded, "reference");
            }
            window_plans::advance(&tx, &current, at(minutes)).unwrap();
            advance(&tx, previous.as_ref(), &current, "runtime-a", at(minutes)).unwrap();
            tx.commit().unwrap();
            previous = Some(committed(&current));
        }
        assert_eq!(events(&db).len(), usize::from(!remove_base));
    }
}
