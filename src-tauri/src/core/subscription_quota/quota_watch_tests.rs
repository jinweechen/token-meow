use super::super::model::{Interval, Reason};
use super::*;

fn at(minutes: i64) -> Reading {
    Reading {
        utc: DateTime::parse_from_rfc3339("2026-10-22T09:00:00.000Z")
            .unwrap()
            .with_timezone(&Utc)
            + Duration::minutes(minutes),
        mono_ms: 10_000 + (minutes as u64) * 60_000,
    }
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    // No usage_plans table: a quota observer must not fabricate a user plan.
    db.execute_batch("CREATE TABLE plan_period_state(owner_kind TEXT NOT NULL CHECK(owner_kind IN ('plan','quota_watch')),owner_id TEXT NOT NULL,period_instance_id TEXT NOT NULL,data TEXT NOT NULL CHECK(json_valid(data)),PRIMARY KEY(owner_kind,owner_id,period_instance_id)); CREATE TABLE account_snapshots(id INTEGER PRIMARY KEY,collected_at TEXT NOT NULL,data TEXT NOT NULL); CREATE TABLE inbox_events(id TEXT PRIMARY KEY,thread_id TEXT,turn_id TEXT,event_time TEXT,data TEXT NOT NULL);").unwrap();
    db
}
fn snapshot(sequence: u64, minutes: i64) -> Value {
    json!({"runtimeId":"runtime-a","sourceId":"source-a","bindingGeneration":"1","identityEpoch":"1","clearGeneration":null,
        "accountScope":null,"entitlementSegment":"fixture-entitlement-only","observationSequence":sequence.to_string(),
        "identityConsistency":"proven","identityEvidence":{"kind":"synthetic_fixture_only"},"status":"connected","observationOutcome":"success","persisted":false,
        "quotaRead":{"outcome":"success","freshnessAnchor":timestamp(at(minutes).utc),"factTimeLower":timestamp(at(minutes).utc),"factTimeUpper":timestamp(at(minutes).utc),"timeBasis":"verified_on_demand_bounds","clockEpoch":"fixture-clock"},
        "buckets":[{"id":"shared","primary":{"windowInstanceId":"window-a","usedPercent":70.0,"roundingStep":1.0,"ruleVersion":"fixture-rule-v1","recovery":"fixed_reset","resetsAt":at(360).utc.timestamp()}}]})
}
fn derived(current: &Value, condition: &str) -> Value {
    let seq = decimal(&current["observationSequence"]).unwrap();
    let now = seconds(&current["quotaRead"]["factTimeUpper"]).unwrap();
    let reset = at(360).utc.timestamp() as f64;
    let forecast = NativeForecast {
        algorithm_version: ALGORITHM_VERSION.into(),
        speed_percent_per_second: Interval::new(0.01, 0.03),
        eta_seconds: Interval::new(60.0, 120.0),
        exhaustion_time: if condition == "safe" {
            Interval::new(reset + 60.0, reset + 120.0)
        } else {
            Interval::new(now + 60.0, now + 120.0)
        },
        reset_time: Interval::exact(reset),
        will_exhaust_before_reset: if condition == "unknown" {
            None
        } else {
            Some(condition == "risk")
        },
        notify_candidate: condition == "risk",
        safe_candidate: condition == "safe",
        sample_count: 6,
        excluded_count: 0,
        longer_baseline: false,
        shared_endpoint: true,
        input_observation_ids: ((seq - 5)..=seq)
            .map(|n| {
                format!(
                    "{}:{n}:shared:primary",
                    current["runtimeId"].as_str().unwrap()
                )
            })
            .collect(),
        reasons: vec![],
    };
    json!({"algorithmVersion":ALGORITHM_VERSION,"risks":{"shared:primary":{"state":condition,"evidence":forecast}}})
}
fn receipt(db: &Connection, current: &Value) {
    receipt_with_encoding(db, current, false);
}
fn receipt_with_encoding(db: &Connection, current: &Value, encoded: bool) {
    let mut record = json!({"documentKind":"subscription_observation","schemaVersion":1,"runtimeId":current["runtimeId"],"sourceId":current["sourceId"],
        "bindingGeneration":current["bindingGeneration"],"identityEpoch":current["identityEpoch"],"clearGeneration":current["clearGeneration"],
        "observationSequence":current["observationSequence"],"lastSettledSequence":current["observationSequence"],"lastPersistedSequence":current["observationSequence"],"outcome":current["observationOutcome"]});
    if current["observationOutcome"] == "success" && current["identityConsistency"] == "proven" {
        record["observation"] = current.clone();
    }
    if encoded {
        record = super::super::observation_storage::encode(
            db,
            &record,
            instant(&current["quotaRead"]["freshnessAnchor"]).unwrap(),
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO account_snapshots(collected_at,data) VALUES(?1,?2)",
        params![timestamp(at(0).utc), record.to_string()],
    )
    .unwrap();
}
fn apply_raw(
    db: &mut Connection,
    current: &Value,
    derived: &Value,
    reading: Reading,
) -> Result<()> {
    let tx = db.transaction().map_err(err)?;
    receipt(&tx, current);
    advance(
        &tx,
        current,
        derived,
        current["runtimeId"].as_str().unwrap(),
        &reading,
    )?;
    tx.commit().map_err(err)
}
fn apply(db: &mut Connection, seq: u64, minutes: i64, condition: &str) {
    let current = snapshot(seq, minutes);
    apply_raw(db, &current, &derived(&current, condition), at(minutes)).unwrap();
}
fn events(db: &Connection) -> Vec<Value> {
    db.prepare("SELECT data FROM inbox_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}
fn states(db: &Connection) -> Vec<State> {
    db.prepare("SELECT data FROM plan_period_state WHERE owner_kind='quota_watch' ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}

#[test]
fn compressed_success_round_keeps_native_risk_confirmation_but_missing_base_cannot_renew_it() {
    let mut db = database();
    for (sequence, minutes) in [(6, 0), (7, 2), (8, 4)] {
        let current = snapshot(sequence, minutes);
        let tx = db.transaction().unwrap();
        receipt_with_encoding(&tx, &current, true);
        if sequence == 7 {
            let raw: String = tx
                .query_row(
                    "SELECT data FROM account_snapshots ORDER BY id DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let stored: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(stored["subscriptionStorage"]["encoding"], "reference");
            assert_eq!(
                stored["subscriptionStorage"]["independentTrainingSample"],
                false
            );
        }
        advance(
            &tx,
            &current,
            &derived(&current, if sequence == 6 { "unknown" } else { "risk" }),
            "runtime-a",
            &at(minutes),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        events(&db).len(),
        1,
        "a genuine new unchanged read still confirms the native risk episode"
    );
    let before = raw_states(&db);
    db.execute("DELETE FROM account_snapshots WHERE id=1", [])
        .unwrap();
    let current = snapshot(8, 4);
    let tx = db.transaction().unwrap();
    advance(
        &tx,
        &current,
        &derived(&current, "risk"),
        "runtime-a",
        &at(4),
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(raw_states(&db), before);
    assert_eq!(events(&db).len(), 1);
}
fn raw_states(db: &Connection) -> Vec<String> {
    db.prepare("SELECT data FROM plan_period_state ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn traced_snapshot(
    sequence: u64,
    elapsed_seconds: i64,
    used: f64,
    reset_seconds: i64,
) -> (Value, Reading) {
    let reading = Reading {
        utc: at(0).utc + Duration::seconds(elapsed_seconds),
        mono_ms: at(0).mono_ms + (elapsed_seconds as u64) * 1000,
    };
    let mut current = snapshot(sequence, 0);
    for key in ["freshnessAnchor", "factTimeLower", "factTimeUpper"] {
        current["quotaRead"][key] = json!(timestamp(reading.utc));
    }
    current["buckets"][0]["primary"]["usedPercent"] = json!(used);
    current["buckets"][0]["primary"]["resetsAt"] =
        json!((at(0).utc + Duration::seconds(reset_seconds)).timestamp());
    (current, reading)
}

fn project_and_apply(db: &mut Connection, current: &Value, reading: Reading) -> Value {
    let tx = db.transaction().unwrap();
    receipt(&tx, current);
    let history: Vec<Value> = tx
        .prepare("SELECT data FROM account_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    let mut persisted = current.clone();
    persisted["persisted"] = json!(true);
    let result = super::super::projector::projections(
        &persisted,
        &history,
        &[],
        "runtime-a",
        reading.utc.timestamp_millis() as f64 / 1000.0,
    );
    advance(&tx, &persisted, &result, "runtime-a", &reading).unwrap();
    tx.commit().unwrap();
    result
}

fn patch_project_and_apply(db: &mut Connection, current: &Value, reading: Reading) -> Value {
    use super::super::calibration_assembler::{
        apply, hydrated_observation, Assembly, ObservationPatch,
    };
    let mut native = current.clone();
    native.as_object_mut().unwrap().remove("entitlementSegment");
    for key in [
        "windowInstanceId",
        "roundingStep",
        "ruleVersion",
        "recovery",
    ] {
        native["buckets"][0]["primary"]
            .as_object_mut()
            .unwrap()
            .remove(key);
    }
    let tx = db.transaction().unwrap();
    receipt(&tx, &native);
    let receipt_id = tx.last_insert_rowid();
    let patch = ObservationPatch {
        structure_seal: None,
        receipt_id,
        observation_id: format!(
            "runtime-a:{}:shared:primary",
            current["observationSequence"].as_str().unwrap()
        ),
        window_id: "shared:primary".into(),
        snapshot_fields: json!({"identityEvidence":{"adapterId":"synthetic-fixture-only","reference":"fixture-identity-proof"},
            "entitlementSegment":"fixture-entitlement-only","entitlementEvidence":{"verified":true,"source":"official","segmentId":"fixture-entitlement-only","reference":"synthetic-fixture-only"},"clockEpoch":"fixture-clock"}),
        window_fields: json!({"roundingStep":1.0,"saturated":false,"saturationHeadroomBounded":false,"ruleVersion":"fixture-rule-v1","windowInstanceId":"window-a","recovery":"fixed_reset","windowInstanceEvidence":{"windowInstanceId":"window-a","reference":"synthetic-fixture-only"}}),
    };
    apply(
        &tx,
        &Assembly {
            patches: vec![patch],
            ..Default::default()
        },
    )
    .unwrap();
    let receipt: Value = serde_json::from_str(
        &tx.query_row(
            "SELECT data FROM account_snapshots WHERE id=?1",
            [receipt_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        receipt["observation"], native,
        "derived metadata must not edit native observation bytes"
    );
    let enriched = hydrated_observation(&receipt).unwrap();
    assert_ne!(enriched["buckets"], native["buckets"]);
    let history: Vec<Value> = tx
        .prepare("SELECT data FROM account_snapshots ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    let result = super::super::projector::projections(
        &enriched,
        &history,
        &[],
        "runtime-a",
        reading.utc.timestamp_millis() as f64 / 1000.0,
    );
    advance(&tx, &enriched, &result, "runtime-a", &reading).unwrap();
    tx.commit().unwrap();
    result
}

#[test]
fn production_patch_hydration_projector_and_watch_share_the_same_receipt_proof() {
    let mut db = database();
    for (i, used) in [20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 95.0, 97.0]
        .into_iter()
        .enumerate()
    {
        let (current, reading) = traced_snapshot(100 + i as u64, i as i64 * 120, used, 960);
        let result = patch_project_and_apply(&mut db, &current, reading);
        if i >= 6 {
            assert_eq!(
                result["risks"]["shared:primary"]["evidence"]["notifyCandidate"], true,
                "{result}"
            );
        }
    }
    assert_eq!(
        events(&db).len(),
        1,
        "patch-derived qualifications must be accepted against validated hydrated evidence"
    );
    assert!(states(&db)[0].risk_active);
    assert_eq!(
        events(&db)[0]["inputObservationSequences"],
        json!(["106", "107"])
    );
}

#[test]
fn real_projector_six_sample_ledger_enters_risk_then_two_safe_observations_resolve_it() {
    let mut db = database();
    // Actual NativeForecast values come from the production projector/evaluator,
    // not the typed module fixture used in the narrower state-machine tests.
    for (i, used) in [20.0, 21.0, 22.0, 23.0, 24.0, 25.0].into_iter().enumerate() {
        let (current, reading) = traced_snapshot(100 + i as u64, i as i64 * 120, used, 960);
        project_and_apply(&mut db, &current, reading);
    }
    for (seq, seconds, used) in [(106, 720, 95.0), (107, 840, 97.0)] {
        let (current, reading) = traced_snapshot(seq, seconds, used, 960);
        let result = project_and_apply(&mut db, &current, reading);
        assert_eq!(
            result["risks"]["shared:primary"]["evidence"]["notifyCandidate"], true,
            "{result}"
        );
    }
    assert_eq!(events(&db).len(), 1);
    assert!(states(&db)[0].risk_active);
    for (seq, seconds) in [(108, 945), (109, 955)] {
        let (current, reading) = traced_snapshot(seq, seconds, 97.0, 960);
        let result = project_and_apply(&mut db, &current, reading);
        assert_eq!(
            result["risks"]["shared:primary"]["evidence"]["willExhaustBeforeReset"], false,
            "{result}"
        );
        if seq == 108 {
            assert!(
                states(&db)[0].risk_active,
                "a single safe read cannot resolve the risk"
            );
        }
    }
    assert!(
        !states(&db)[0].risk_active,
        "two fresh all-scenario-safe reads must resolve the existing episode"
    );
    assert_eq!(
        events(&db).len(),
        1,
        "resolving risk must not fabricate another warning"
    );
}

#[test]
fn real_projector_primary_safe_with_a_faster_short_scenario_cannot_resolve_existing_risk() {
    let mut db = database();
    for (i, used) in [20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 95.0, 97.0]
        .into_iter()
        .enumerate()
    {
        let (current, reading) = traced_snapshot(100 + i as u64, i as i64 * 120, used, 960);
        project_and_apply(&mut db, &current, reading);
    }
    assert!(states(&db)[0].risk_active);
    assert_eq!(events(&db).len(), 1);
    // A same-window reset postponement and corrected historical readings can
    // rebuild the native baseline. Historical fixture insertion itself does
    // not advance the watch; only the two genuinely new tail reads below do.
    {
        let tx = db.transaction().unwrap();
        for (i, used) in [
            10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0,
        ]
        .into_iter()
        .enumerate()
        {
            let (current, _) = traced_snapshot(200 + i as u64, 1000 + i as i64 * 120, used, 2800);
            receipt(&tx, &current);
        }
        tx.commit().unwrap();
    }
    for (seq, seconds) in [(212, 2440), (213, 2450)] {
        let (current, reading) = traced_snapshot(seq, seconds, 80.0, 2800);
        let result = project_and_apply(&mut db, &current, reading);
        let f = &result["risks"]["shared:primary"]["evidence"];
        assert_eq!(
            f["willExhaustBeforeReset"], false,
            "long baseline should be safe: {result}"
        );
        assert_eq!(
            f["safeCandidate"], false,
            "the faster admitted suffix must block resolution: {result}"
        );
        assert!(f["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!(Reason::ResetUncertain)));
        assert!(states(&db)[0].risk_active);
        assert_eq!(states(&db)[0].safe_streak, 0);
    }
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn two_distinct_new_risk_observations_create_one_panel_event_without_any_plan() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    assert!(events(&db).is_empty());
    apply(&mut db, 102, 4, "risk");
    let e = events(&db);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0]["kind"], "quota_eta");
    assert_eq!(e[0]["ownerKind"], "quota_watch");
    assert!(e[0].get("planId").is_none());
    assert_eq!(e[0]["periodInstanceId"], "window-a");
    assert_eq!(e[0]["inputObservationSequences"], json!(["101", "102"]));
    assert_eq!(e[0]["notifyEligible"], false);
    assert_eq!(e[0]["animationAcknowledged"], true);
    assert_eq!(e[0]["notificationState"], "panel_only");
    let s = states(&db).remove(0);
    assert!(s.risk_active);
    assert!(s.candidate_eligible);
    assert_eq!(s.risk_episode, "0");
    assert_eq!(s.params, NativeParams::default());
    apply(&mut db, 103, 6, "risk");
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn first_loaded_risk_is_baseline_and_two_safe_reads_are_needed_before_reentry() {
    let mut db = database();
    apply(&mut db, 100, 0, "risk");
    apply(&mut db, 101, 2, "risk");
    apply(&mut db, 102, 4, "risk");
    assert!(events(&db).is_empty());
    apply(&mut db, 103, 6, "safe");
    assert!(states(&db)[0].risk_active);
    apply(&mut db, 104, 8, "safe");
    assert!(!states(&db)[0].risk_active);
    apply(&mut db, 105, 10, "risk");
    assert!(events(&db).is_empty());
    apply(&mut db, 106, 12, "risk");
    assert_eq!(events(&db).len(), 1);
    assert_eq!(events(&db)[0]["riskEpisode"], "1");
}

#[test]
fn failed_minimal_receipt_revokes_eligibility_without_claiming_risk_is_resolved() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    apply(&mut db, 102, 4, "risk");
    let mut failure = snapshot(103, 6);
    failure["observationOutcome"] = json!("failure");
    failure["identityConsistency"] = json!("unproven");
    failure.as_object_mut().unwrap().remove("buckets");
    failure.as_object_mut().unwrap().remove("quotaRead");
    apply_raw(&mut db, &failure, &json!({}), at(6)).unwrap();
    let s = states(&db).remove(0);
    assert!(s.risk_active);
    assert_eq!(s.risk_episode, "0");
    assert!(!s.candidate_eligible);
    assert_eq!(s.condition, "unknown");
    assert_eq!((s.risk_streak, s.safe_streak), (0, 0));
    apply(&mut db, 104, 8, "risk");
    apply(&mut db, 105, 10, "risk");
    assert_eq!(events(&db).len(), 1);
    apply(&mut db, 106, 12, "safe");
    assert!(states(&db)[0].risk_active);
    apply(&mut db, 107, 14, "unknown");
    apply(&mut db, 108, 16, "safe");
    assert!(states(&db)[0].risk_active);
    apply(&mut db, 109, 18, "safe");
    assert!(!states(&db)[0].risk_active);
}

#[test]
fn unknown_between_candidates_resets_confirmation_and_cannot_count_as_safe() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    apply(&mut db, 102, 4, "unknown");
    apply(&mut db, 103, 6, "risk");
    assert!(events(&db).is_empty());
    apply(&mut db, 104, 8, "risk");
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn legacy_forecast_without_all_scenario_safe_proof_cannot_resolve_a_latched_risk() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    apply(&mut db, 102, 4, "risk");
    for (seq, minutes) in [(103, 6), (104, 8)] {
        let current = snapshot(seq, minutes);
        let mut result = derived(&current, "safe");
        result["risks"]["shared:primary"]["evidence"]
            .as_object_mut()
            .unwrap()
            .remove("safeCandidate");
        apply_raw(&mut db, &current, &result, at(minutes)).unwrap();
        assert!(states(&db)[0].risk_active);
        assert_eq!(states(&db)[0].safe_streak, 0);
    }
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn suppressed_reentry_does_not_replay_after_cooldown_and_later_episode_can_notify() {
    let mut db = database();
    for (seq, min, kind) in [
        (100, 0, "safe"),
        (101, 2, "risk"),
        (102, 4, "risk"),
        (103, 6, "safe"),
        (104, 8, "safe"),
        (105, 10, "risk"),
        (106, 12, "risk"),
    ] {
        apply(&mut db, seq, min, kind);
    }
    assert_eq!(events(&db).len(), 1);
    assert_eq!(states(&db)[0].risk_episode, "1");
    apply(&mut db, 107, 36, "risk");
    apply(&mut db, 108, 38, "risk");
    assert_eq!(events(&db).len(), 1, "time passing is not a new transition");
    for (seq, min, kind) in [
        (109, 40, "safe"),
        (110, 42, "safe"),
        (111, 44, "risk"),
        (112, 46, "risk"),
    ] {
        apply(&mut db, seq, min, kind);
    }
    assert_eq!(events(&db).len(), 2);
    assert_eq!(events(&db)[1]["riskEpisode"], "2");
    assert_ne!(events(&db)[0]["id"], events(&db)[1]["id"]);
}

#[test]
fn repeated_or_older_receipt_has_no_latch_freshness_event_or_pruning_side_effect() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    let before = raw_states(&db);
    for current in [snapshot(101, 2), snapshot(100, 0)] {
        let tx = db.transaction().unwrap();
        advance(
            &tx,
            &current,
            &derived(&current, "risk"),
            "runtime-a",
            &at(3),
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(raw_states(&db), before);
        assert!(events(&db).is_empty());
    }
    apply(&mut db, 102, 4, "risk");
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn exact_success_receipt_proof_is_required_and_temporary_persisted_bit_is_irrelevant() {
    let mut db = database();
    let mut current = snapshot(100, 0);
    current["persisted"] = json!(true);
    {
        let tx = db.transaction().unwrap();
        advance(
            &tx,
            &current,
            &derived(&current, "risk"),
            "runtime-a",
            &at(0),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert!(states(&db).is_empty());
    {
        let tx = db.transaction().unwrap();
        receipt(&tx, &current);
        current["buckets"][0]["primary"]["usedPercent"] = json!(71);
        advance(
            &tx,
            &current,
            &derived(&current, "risk"),
            "runtime-a",
            &at(0),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert!(
        states(&db).is_empty(),
        "a different raw official value cannot borrow a receipt"
    );
    assert!(
        advance(
            &db,
            &current,
            &derived(&current, "risk"),
            "runtime-a",
            &at(0)
        )
        .is_err(),
        "caller must own the receipt transaction"
    );
}

#[test]
fn state_event_and_current_receipt_roll_back_as_one_transaction() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    let before = raw_states(&db);
    db.execute_batch("CREATE TRIGGER test_fail_watch_event BEFORE INSERT ON inbox_events BEGIN SELECT RAISE(ABORT,'test-owned-quota-watch-failure'); END;").unwrap();
    let current = snapshot(102, 4);
    assert!(apply_raw(&mut db, &current, &derived(&current, "risk"), at(4)).is_err());
    assert_eq!(raw_states(&db), before);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM account_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(events(&db).is_empty());
    db.execute_batch("DROP TRIGGER test_fail_watch_event;")
        .unwrap();
    apply(&mut db, 103, 6, "risk");
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn fact_gap_and_paired_clock_jump_break_the_two_read_confirmation() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    apply(&mut db, 102, 10, "risk");
    assert!(events(&db).is_empty());
    let current = snapshot(103, 12);
    let mut reading = at(12);
    reading.mono_ms -= 30_000;
    apply_raw(&mut db, &current, &derived(&current, "risk"), reading).unwrap();
    assert!(events(&db).is_empty());
    assert_eq!(states(&db)[0].risk_streak, 1);
    let current = snapshot(104, 14);
    let mut reading = at(14);
    reading.mono_ms -= 30_000;
    apply_raw(&mut db, &current, &derived(&current, "risk"), reading).unwrap();
    assert_eq!(events(&db).len(), 1);
}

#[test]
fn time_unknown_stale_activity_or_reset_overlap_cannot_confirm_risk_or_resolve_it() {
    for mode in [
        "basis",
        "old_activity",
        "reset_overlap",
        "missing_input",
        "missing_instance",
        "future_fact",
    ] {
        let mut db = database();
        apply(&mut db, 100, 0, "safe");
        apply(&mut db, 101, 2, "risk");
        let mut current = snapshot(102, 4);
        let mut result = derived(&current, "risk");
        match mode {
            "basis" => current["quotaRead"]["timeBasis"] = json!("unbounded-upstream"),
            "old_activity" => {
                result["risks"]["shared:primary"]["evidence"]["reasons"] =
                    json!([Reason::ActivityStale])
            }
            "reset_overlap" => {
                result["risks"]["shared:primary"]["evidence"]["exhaustionTime"] =
                    json!({"lower":at(359).utc.timestamp(),"upper":at(361).utc.timestamp()})
            }
            "missing_input" => {
                result["risks"]["shared:primary"]["evidence"]["inputObservationIds"] =
                    json!(["runtime-a:101:shared:primary"])
            }
            "missing_instance" => current["buckets"][0]["primary"]
                .as_object_mut()
                .unwrap()
                .remove("windowInstanceId")
                .map(|_| ())
                .unwrap(),
            _ => current["quotaRead"]["factTimeUpper"] = json!(timestamp(at(5).utc)),
        }
        apply_raw(&mut db, &current, &result, at(4)).unwrap();
        assert!(events(&db).is_empty(), "{mode}");
        assert!(!states(&db)[0].candidate_eligible, "{mode}");
        assert_eq!(states(&db)[0].risk_streak, 0, "{mode}");
    }
}

#[test]
fn identity_runtime_and_window_changes_start_fresh_baselines_without_crossing_old_scope() {
    let mut db = database();
    apply(&mut db, 100, 0, "safe");
    apply(&mut db, 101, 2, "risk");
    let mut current = snapshot(102, 4);
    current["identityEpoch"] = json!("2");
    apply_raw(&mut db, &current, &derived(&current, "risk"), at(4)).unwrap();
    assert!(events(&db).is_empty());
    current = snapshot(103, 6);
    current["runtimeId"] = json!("runtime-b");
    apply_raw(&mut db, &current, &derived(&current, "risk"), at(6)).unwrap();
    assert!(events(&db).is_empty());
    current = snapshot(104, 8);
    current["buckets"][0]["primary"]["windowInstanceId"] = json!("window-b");
    apply_raw(&mut db, &current, &derived(&current, "risk"), at(8)).unwrap();
    assert!(events(&db).is_empty());
    assert!(states(&db).iter().all(|s| !s.candidate_eligible));
    assert!(states(&db)
        .iter()
        .any(|s| s.period_instance_id == "window-b"));
}

#[test]
fn global_watch_state_count_is_bounded_and_does_not_delete_user_plan_state() {
    let mut db = database();
    let current = snapshot(100, 0);
    let r = Round {
        source: "source-a".into(),
        sequence: 100,
        successful: true,
    };
    let w = window(
        &current,
        &r,
        "shared:primary".into(),
        &current["buckets"][0]["primary"],
        &derived(&current, "safe"),
        &at(0),
    )
    .unwrap();
    let mut old = fresh_state(&w, &r, &at(0));
    old.scope["runtimeId"] = json!("another-runtime");
    {
        let tx = db.transaction().unwrap();
        for n in 0..MAX_STATES {
            old.owner_id = format!("old-{n}");
            save(&tx, &old).unwrap();
        }
        tx.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan','real-plan','real-period','{\"latched\":[80]}')",[]).unwrap();
        tx.commit().unwrap();
    }
    apply(&mut db, 101, 2, "safe");
    assert_eq!(states(&db).len(), MAX_STATES);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM plan_period_state WHERE owner_kind='plan'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(states(&db)
        .iter()
        .any(|s| s.scope["runtimeId"] == "runtime-a"));
    assert!(raw_states(&db).iter().all(|s| s.len() <= MAX_STATE_BYTES));
}
