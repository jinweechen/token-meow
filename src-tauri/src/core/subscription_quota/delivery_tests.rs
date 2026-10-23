use super::*;
use crate::budget_clock::test_support::TestClock;
use crate::core::adapter::{Counts, Metric};
use serde_json::json;

fn at() -> Reading {
    Reading {
        utc: DateTime::parse_from_rfc3339("2026-10-22T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        mono_ms: 100_000,
    }
}
fn later(ms: i64) -> Reading {
    Reading {
        utc: at().utc + Duration::milliseconds(ms),
        mono_ms: at().mono_ms + ms as u64,
    }
}
fn response(id: &str, count: i64, at: DateTime<Utc>, live: bool) -> ResponseUsage {
    ResponseUsage {
        source_id: "source-a".into(),
        channel_id: "channel-a".into(),
        namespace: "native".into(),
        thread_id: "thread-a".into(),
        response_id: id.into(),
        turn_id: None,
        root_turn_id: None,
        event_time: at.to_rfc3339(),
        collected_at: at.to_rfc3339(),
        requested_model: Some("model-a".into()),
        observed_model: Some("model-a".into()),
        provider_id: None,
        account_scope_id: None,
        native_counts: BTreeMap::new(),
        counts: Counts {
            input: Metric::Valid(count),
            output: Metric::Valid(0),
            cache_read: Metric::Valid(0),
            cache_write: Metric::Missing,
            reasoning: Metric::Missing,
            total: Metric::Valid(count),
        },
        semantics_version: "codex-native-0.147-v1".into(),
        adapter_version: "fixture".into(),
        completeness: "complete".into(),
        phase: "response".into(),
        revision: None,
        live,
    }
}
fn insert_response(db: &Connection, r: &ResponseUsage) -> String {
    let id = crate::core::adapter::response_key(r);
    db.execute(
        "INSERT INTO usage_responses(id,source_id,event_time,data) VALUES(?1,?2,?3,?4)",
        params![
            id,
            r.source_id,
            r.event_time,
            serde_json::to_string(r).unwrap()
        ],
    )
    .unwrap();
    id
}
fn write_state(db: &Connection, used: i64, latched: Value) {
    let s =
        json!({"used":used,"latched":latched,"runtimeId":"runtime-a","evaluationState":"ready"});
    db.execute("INSERT OR REPLACE INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan','plan-a','plan-a',?1)",[s.to_string()]).unwrap();
}
fn plan() -> Value {
    json!({"id":"plan-a","revision":1,"sourceId":"source-a","type":"tokens","model":null,"modelDimension":"response","target":100,"effectiveFrom":(at().utc-Duration::hours(1)).to_rfc3339(),"endsAt":(at().utc+Duration::hours(1)).to_rfc3339(),"endPolicy":"time","lifecycle":"active","endedAt":null,"remindersEnabled":true})
}
fn put_plan(db: &Connection, p: &Value) {
    db.execute("INSERT OR REPLACE INTO usage_plans(plan_id,revision,source_id,data) VALUES('plan-a',?1,'source-a',?2)",params![p["revision"].as_i64().unwrap(),p.to_string()]).unwrap();
}
fn event_value() -> Value {
    json!({"id":"event-a","kind":"subscription_plan","sourceId":"source-a","planId":"plan-a","periodInstanceId":"plan-a","planRevision":1,"threshold":80,"eligibleRuntime":"runtime-a","eligibleContext":"context-a","eventTime":later(1000).utc.to_rfc3339(),"notifyEligible":true,"notificationState":"pending","notified":false})
}
fn put_event(db: &Connection, v: &Value) {
    db.execute(
        "INSERT OR REPLACE INTO inbox_events(id,data) VALUES('event-a',?1)",
        [v.to_string()],
    )
    .unwrap();
}
struct Harness {
    db: Connection,
    gate: Arc<Gate>,
    delivery: Delivery,
    sample: Sample,
    crossing: TokenCrossing,
    clock: TestClock,
}
impl Harness {
    fn new() -> Self {
        Self::with_db(Connection::open_in_memory().unwrap())
    }
    fn with_db(db: Connection) -> Self {
        let clock = TestClock::new(at());
        db.execute_batch("CREATE TABLE usage_plans(plan_id TEXT,revision INTEGER,source_id TEXT,data TEXT,PRIMARY KEY(plan_id,revision)); CREATE TABLE plan_period_state(owner_kind TEXT,owner_id TEXT,period_instance_id TEXT,data TEXT,PRIMARY KEY(owner_kind,owner_id,period_instance_id)); CREATE TABLE usage_responses(id TEXT PRIMARY KEY,source_id TEXT,event_time TEXT,data TEXT); CREATE TABLE inbox_events(id TEXT PRIMARY KEY,data TEXT); CREATE TABLE source_instances(id TEXT PRIMARY KEY,enabled INTEGER); CREATE TABLE excluded_facts(fact_id TEXT PRIMARY KEY); CREATE TABLE fact_aliases(alias TEXT PRIMARY KEY); CREATE TABLE turns(id TEXT PRIMARY KEY,data TEXT); INSERT INTO source_instances VALUES('source-a',1);").unwrap();
        put_plan(&db, &plan());
        write_state(&db, 70, json!([]));
        let old = insert_response(
            &db,
            &response("old", 70, at().utc - Duration::minutes(1), false),
        );
        let gate = Arc::new(Gate::new());
        gate.publish(true, false);
        let mut delivery = Delivery::default();
        let sample = delivery
            .begin(&db, Arc::clone(&gate), "runtime-a", "context-a")
            .unwrap()
            .unwrap();
        let new = crate::core::adapter::response_key(&response("new", 20, at().utc, true));
        Self {
            db,
            gate,
            delivery,
            sample,
            crossing: TokenCrossing {
                event_id: "event-a".into(),
                plan_id: "plan-a".into(),
                source_id: "source-a".into(),
                period_instance_id: "plan-a".into(),
                plan_revision: 1,
                threshold: 80,
                before_used: 70,
                after_used: 90,
                fact_ids: vec![old, new.clone()],
                fresh_fact_ids: vec![new],
            },
            clock,
        }
    }
    fn commit(&mut self) {
        self.commit_fresh(at().utc);
    }
    fn commit_fresh(&mut self, time: DateTime<Utc>) {
        self.clock.set(later(1000));
        let tx = self.db.unchecked_transaction().unwrap();
        insert_response(&tx, &response("new", 20, time, true));
        write_state(&tx, 90, json!([80]));
        put_event(&tx, &event_value());
        tx.commit().unwrap();
    }
    fn grant(&mut self) -> bool {
        self.delivery
            .grant(&self.db, &self.sample, self.crossing.clone(), true)
            .unwrap()
    }
    fn permit(&self) -> Option<NotificationPermit> {
        self.delivery
            .permit(&self.db, "event-a", "runtime-a", "context-a", true)
    }
    fn prompt_permit(&self) -> Option<NotificationPermit> {
        self.delivery
            .prompt_permit(&self.db, "event-a", "runtime-a", "context-a", true)
    }
    fn issued() -> Self {
        let mut h = Self::new();
        h.commit();
        assert!(h.grant());
        h
    }
}

#[test]
fn committed_new_token_crossing_gets_one_shared_fifteen_second_permit() {
    let h = Harness::issued();
    let p = h.permit().unwrap();
    assert_eq!(p.expires_utc(), later(15_000).utc);
    h.clock.set(later(14_999));
    assert!(h.permit().is_some());
    assert!(p.is_current());
    h.clock.set(later(15_000));
    assert!(h.permit().is_none());
    assert!(!p.is_current());
}

#[test]
fn unqualified_is_closed_even_after_a_qualified_test_grant() {
    let mut h = Harness::new();
    h.commit();
    assert!(!h
        .delivery
        .grant(&h.db, &h.sample, h.crossing.clone(), false)
        .unwrap());
    assert!(h.permit().is_none());
    // Qualification/revocation cannot revive an in-flight old sample.
    assert!(!h.grant());
    let h = Harness::issued();
    let queued = h.permit().unwrap();
    assert!(h
        .delivery
        .permit(&h.db, "event-a", "runtime-a", "context-a", false)
        .is_none());
    assert!(!queued.is_current());
}

#[test]
fn open_transaction_or_rollback_cannot_issue() {
    let mut h = Harness::new();
    h.clock.set(later(1000));
    h.db.execute_batch("BEGIN").unwrap();
    insert_response(&h.db, &response("new", 20, at().utc, true));
    write_state(&h.db, 90, json!([80]));
    put_event(&h.db, &event_value());
    assert!(!h.grant());
    h.db.execute_batch("ROLLBACK").unwrap();
    assert!(!h.grant());
    assert!(h.permit().is_none());
}

#[test]
fn default_or_restarted_delivery_never_reconstructs_from_persisted_history() {
    let mut h = Harness::issued();
    let queued = h.permit().unwrap();
    h.delivery = Delivery::default();
    assert!(!queued.is_current());
    assert!(h.permit().is_none());
    assert!(
        !h.grant(),
        "a retained Sample cannot cross Runtime reconstruction"
    );
    let after = h
        .delivery
        .begin(&h.db, Arc::clone(&h.gate), "runtime-a", "context-a")
        .unwrap()
        .unwrap();
    assert!(!h
        .delivery
        .grant(&h.db, &after, h.crossing.clone(), true)
        .unwrap());
}

#[test]
fn finish_and_owner_revoke_and_drop_revoke_queue_clones_without_reissuance() {
    for action in 0..3 {
        let mut h = Harness::issued();
        let p = h.permit().unwrap();
        match action {
            0 => h.delivery.finish("event-a"),
            1 => h.delivery.revoke_owner("plan-a"),
            _ => h.delivery.revoke_all(),
        };
        assert!(!p.is_current());
        assert!(h.permit().is_none());
        assert!(!h.grant());
    }
    let p = {
        let h = Harness::issued();
        h.permit().unwrap()
    };
    assert!(!p.is_current());
}

#[test]
fn late_claim_cannot_renew_original_fact_deadline() {
    let mut h = Harness::new();
    h.commit_fresh(at().utc - Duration::seconds(10));
    assert!(h.grant());
    let p = h.permit().unwrap();
    assert_eq!(p.expires_utc(), later(5000).utc);
    h.clock.set(later(4999));
    assert!(h.permit().is_some());
    h.clock.set(later(5000));
    assert!(h.permit().is_none());
    assert!(!p.is_current());
}

#[test]
fn queue_clone_expires_at_fixed_period_end_without_another_core_query() {
    let mut h = Harness::new();
    let mut p = plan();
    p["endsAt"] = json!(later(5000).utc.to_rfc3339());
    put_plan(&h.db, &p);
    h.sample = h
        .delivery
        .begin(&h.db, Arc::clone(&h.gate), "runtime-a", "context-a")
        .unwrap()
        .unwrap();
    h.commit();
    assert!(h.grant());
    let queued = h.permit().unwrap();
    assert_eq!(queued.expires_utc(), later(5000).utc);
    h.clock.set(later(5000));
    assert!(!queued.is_current());
}

#[test]
fn expired_or_future_facts_are_ineligible() {
    for delta in [-16_000, 2000] {
        let mut h = Harness::new();
        h.commit_fresh(at().utc + Duration::milliseconds(delta));
        assert!(!h.grant());
    }
}

#[test]
fn sleep_on_either_clock_expires_queued_permit() {
    for monotonic in [false, true] {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        h.clock.set(if monotonic {
            Reading {
                utc: later(2000).utc,
                mono_ms: later(16_000).mono_ms,
            }
        } else {
            Reading {
                utc: later(16_000).utc,
                mono_ms: later(2000).mono_ms,
            }
        });
        assert!(!p.is_current());
        assert!(h.permit().is_none());
    }
}

#[test]
fn paired_clock_rollback_or_divergence_revokes_all_clones_permanently() {
    for reading in [
        Reading {
            utc: at().utc,
            mono_ms: later(1001).mono_ms,
        },
        Reading {
            utc: later(1001).utc,
            mono_ms: at().mono_ms,
        },
        Reading {
            utc: later(5000).utc,
            mono_ms: later(1100).mono_ms,
        },
    ] {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        h.clock.set(reading);
        assert!(h.permit().is_none());
        assert!(!p.is_current());
        h.clock.set(later(6000));
        assert!(h.permit().is_none());
    }
}

#[test]
fn lock_disconnect_sleep_and_same_flags_epoch_aba_invalidate_queued_clones() {
    for boundary in 0..4 {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        match boundary {
            0 => h.gate.publish(true, true),
            1 => h.gate.publish(false, false),
            2 => h.gate.invalidate(),
            _ => h.gate.publish(true, false),
        };
        h.gate.publish(true, false);
        assert!(!p.is_current());
        assert!(h.permit().is_none());
    }
}

#[test]
fn unknown_or_blocked_host_cannot_sample_or_grant() {
    let mut h = Harness::new();
    h.gate.publish(false, false);
    assert!(h
        .delivery
        .begin(&h.db, Arc::clone(&h.gate), "runtime-a", "context-a")
        .unwrap()
        .is_none());
    h.commit();
    h.gate.publish(true, false);
    assert!(!h.grant());
}

#[test]
fn changing_plan_revision_target_pause_end_or_source_invalidates_persistent_authority() {
    for field in [
        "revision",
        "target",
        "remindersEnabled",
        "lifecycle",
        "sourceId",
        "type",
        "endsAt",
    ] {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        let mut config = plan();
        match field {
            "revision" => config[field] = json!(2),
            "target" => config[field] = json!(200),
            "remindersEnabled" => config[field] = json!(false),
            "lifecycle" => config[field] = json!("ended"),
            "sourceId" => config[field] = json!("source-b"),
            "type" => config[field] = json!("percent"),
            _ => config[field] = json!(at().utc.to_rfc3339()),
        };
        put_plan(&h.db, &config);
        assert!(h.permit().is_none(), "{field}");
        assert!(!p.is_current(), "{field}");
    }
}

#[test]
fn same_connection_fact_deletion_edit_alias_or_exclusion_revokes_queue_clone() {
    for change in 0..5 {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        let id = &h.crossing.fact_ids[if change == 0 { 0 } else { 1 }];
        match change {
            0 | 1 => {
                h.db.execute("DELETE FROM usage_responses WHERE id=?1", [id])
                    .unwrap();
            }
            2 => {
                h.db.execute("UPDATE usage_responses SET data=json_set(data,'$.counts.total.value',1) WHERE id=?1",[id]).unwrap();
            }
            3 => {
                h.db.execute("INSERT INTO fact_aliases VALUES(?1)", [id])
                    .unwrap();
            }
            _ => {
                h.db.execute("INSERT INTO excluded_facts VALUES(?1)", [id])
                    .unwrap();
            }
        };
        assert!(h.permit().is_none());
        assert!(!p.is_current());
    }
}

#[test]
fn durable_event_kind_identity_or_terminal_state_is_rechecked_instead_of_trusting_kind() {
    for (field, value) in [
        ("kind", json!("quota_eta")),
        ("kind", json!("unknown")),
        ("threshold", json!(100)),
        ("planRevision", json!(2)),
        ("eligibleContext", json!("changed")),
        ("notificationState", json!("sent")),
        ("notifyEligible", json!(false)),
    ] {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        let mut event = event_value();
        event[field] = value;
        put_event(&h.db, &event);
        assert!(h.permit().is_none(), "{field}");
        assert!(!p.is_current());
    }
    let h = Harness::issued();
    let p = h.permit().unwrap();
    h.db.execute("DELETE FROM inbox_events", []).unwrap();
    assert!(h.permit().is_none());
    assert!(!p.is_current());
}

#[test]
fn unknown_eta_and_percent_events_or_plans_never_get_registered_as_token_delivery() {
    for kind in ["quota_eta", "subscription_percent", "unknown"] {
        let mut h = Harness::new();
        h.commit();
        let mut e = event_value();
        e["kind"] = json!(kind);
        put_event(&h.db, &e);
        assert!(!h.grant());
    }
    let mut h = Harness::new();
    let mut p = plan();
    p["type"] = json!("percent");
    put_plan(&h.db, &p);
    h.commit();
    assert!(!h.grant());
}

#[test]
fn current_runtime_context_source_and_state_must_still_be_eligible() {
    for change in 0..5 {
        let h = Harness::issued();
        let p = h.permit().unwrap();
        match change {
            0 => {
                assert!(h
                    .delivery
                    .permit(&h.db, "event-a", "runtime-b", "context-a", true)
                    .is_none());
            }
            1 => {
                assert!(h
                    .delivery
                    .permit(&h.db, "event-a", "runtime-a", "context-b", true)
                    .is_none());
            }
            2 => {
                h.db.execute("UPDATE source_instances SET enabled=0", [])
                    .unwrap();
                assert!(h.permit().is_none());
            }
            3 => {
                write_state(&h.db, 70, json!([80]));
                assert!(h.permit().is_none());
            }
            _ => {
                write_state(&h.db, 90, json!([]));
                assert!(h.permit().is_none());
            }
        };
        assert!(!p.is_current());
    }
}

#[test]
fn claim_metadata_changes_preserve_permit_but_finish_closes_it() {
    let mut h = Harness::issued();
    let p = h.permit().unwrap();
    let mut e = event_value();
    e["notificationState"] = json!("claimed");
    e["notified"] = json!(true);
    e["claimedAt"] = json!(later(1000).utc.to_rfc3339());
    put_event(&h.db, &e);
    assert!(h.permit().is_some());
    h.delivery.finish("event-a");
    assert!(!p.is_current());
}

#[test]
fn uncommitted_restoration_cannot_authorize_a_rolled_back_plan() {
    let h = Harness::issued();
    let queued = h.permit().unwrap();
    let mut paused = plan();
    paused["remindersEnabled"] = json!(false);
    put_plan(&h.db, &paused);
    h.db.execute_batch("BEGIN").unwrap();
    put_plan(&h.db, &plan());
    assert!(h.permit().is_none());
    h.db.execute_batch("ROLLBACK").unwrap();
    assert!(!queued.is_current());
    assert!(h.permit().is_none());
}

#[test]
fn explicit_revocation_cancels_already_captured_inflight_samples() {
    for owner_only in [false, true] {
        let mut h = Harness::new();
        h.commit();
        if owner_only {
            h.delivery.revoke_owner("plan-a");
        } else {
            h.delivery.revoke_all();
        }
        assert!(!h.grant());
    }
}

#[test]
fn external_sqlite_commit_changes_data_version_and_revokes_permit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("delivery.sqlite");
    let mut h = Harness::with_db(Connection::open(&path).unwrap());
    h.commit();
    assert!(h.grant());
    let p = h.permit().unwrap();
    let external = Connection::open(path).unwrap();
    external
        .execute("INSERT INTO source_instances VALUES('unrelated',1)", [])
        .unwrap();
    assert!(h.permit().is_none());
    assert!(!p.is_current());
}

#[test]
fn only_new_live_fact_delta_can_cross_and_duplicate_or_preexisting_events_do_not_issue() {
    for change in 0..5 {
        let mut h = Harness::new();
        h.commit();
        match change {
            0 => h.crossing.before_used = 80,
            1 => {
                h.crossing.fresh_fact_ids.clear();
            }
            2 => {
                h.crossing.fact_ids.push(h.crossing.fact_ids[0].clone());
            }
            3 => {
                h.db.execute("UPDATE usage_responses SET data=json_set(data,'$.live',json('false')) WHERE id=?1",[&h.crossing.fresh_fact_ids[0]]).unwrap();
            }
            _ => {
                h.crossing.after_used = 95;
            }
        };
        assert!(!h.grant());
    }
    let mut h = Harness::issued();
    assert!(!h.grant());
}

#[test]
fn configuration_mutation_during_collection_and_already_latched_baseline_fail_closed() {
    let mut h = Harness::new();
    let mut p = plan();
    p["revision"] = json!(2);
    put_plan(&h.db, &p);
    h.commit();
    h.crossing.plan_revision = 2;
    let mut e = event_value();
    e["planRevision"] = json!(2);
    put_event(&h.db, &e);
    assert!(!h.grant());
    let mut h = Harness::new();
    write_state(&h.db, 70, json!([80]));
    h.sample = h
        .delivery
        .begin(&h.db, Arc::clone(&h.gate), "runtime-a", "context-a")
        .unwrap()
        .unwrap();
    h.commit();
    assert!(!h.grant());
}

#[test]
fn quota_bound_revocation_cancels_only_window_entries_and_their_already_queued_clones() {
    let mut h = Harness::issued();
    let time_permit = h.permit().unwrap();
    let instance = Arc::clone(&h.delivery.instance);
    let gate_epoch = h.gate.snapshot();
    let mut window_fixture = Harness::issued();
    let window_permit = window_fixture.permit().unwrap();
    let mut window_entry = window_fixture.delivery.permits.remove("event-a").unwrap();
    // This narrow revocation fixture starts from a genuinely granted Entry,
    // then classifies its captured plan as window-bound. It does not grant a
    // real window plan, mock configuration_valid, or establish native support.
    window_entry.plan["endPolicy"] = json!("window");
    window_entry.crossing.event_id = "window-fixture-event".into();
    h.delivery.issued.insert("window-fixture-event".into());
    h.delivery
        .permits
        .insert("window-fixture-event".into(), window_entry);
    let mut queue = vec![
        crate::notification_gate::QueuedNotification {
            event: json!({"id":"event-a","kind":"subscription_plan"}),
            permit: Some(time_permit.clone()),
        },
        crate::notification_gate::QueuedNotification {
            event: json!({"id":"window-fixture-event","kind":"subscription_plan"}),
            permit: Some(window_permit.clone()),
        },
    ];
    assert!(time_permit.is_current() && window_permit.is_current());
    h.delivery.revoke_quota_bound();
    assert!(
        !window_permit.is_current(),
        "revocation must reach the host's queued clone immediately"
    );
    assert!(!h.delivery.permits.contains_key("window-fixture-event"));
    assert!(
        h.delivery.issued.contains("window-fixture-event"),
        "revocation must retain the one-shot event marker"
    );
    assert!(time_permit.is_current());
    assert!(
        h.permit().is_some(),
        "the independent time plan must still pass full SQLite configuration/fact revalidation"
    );
    assert!(Arc::ptr_eq(&instance, &h.delivery.instance));
    assert_eq!(h.gate.snapshot(), gate_epoch);
    assert_eq!(
        crate::notification_gate::discard_invalid(&mut queue, &later(1000)),
        vec!["window-fixture-event"]
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].event["id"], "event-a");
    h.delivery.revoke_quota_bound();
    let _next = h
        .delivery
        .begin(&h.db, Arc::clone(&h.gate), "runtime-a", "context-a")
        .unwrap()
        .unwrap();
    assert!(
        !window_permit.is_current(),
        "a new collection round cannot revive an old removed window lease"
    );
    assert!(h.permit().is_some());
    assert!(h
        .delivery
        .permit(
            &h.db,
            "window-fixture-event",
            "runtime-a",
            "context-a",
            true
        )
        .is_none());
}

#[test]
fn sent_shown_and_failed_toasts_keep_the_original_prompt_permit_current() {
    for status in ["sent", "shown", "failed"] {
        let mut h = Harness::issued();
        let toast = h.permit().unwrap();
        let prompt = h.prompt_permit().unwrap();
        let deadline = prompt.expires_utc();
        h.clock.set(later(3000));
        let mut e = event_value();
        e["notificationState"] = json!(status);
        e["animationAcknowledged"] = json!(false);
        e["notified"] = json!(true);
        put_event(&h.db, &e);
        h.delivery.finish_toast("event-a");
        assert!(!toast.is_current(), "{status}");
        assert!(
            h.permit().is_none(),
            "toast cannot be reissued after {status}"
        );
        let remaining = h.prompt_permit().unwrap();
        assert!(prompt.is_current(), "{status}");
        assert_eq!(remaining.expires_utc(), deadline);
        assert_eq!(
            deadline,
            later(15_000).utc,
            "finishing toast must not renew prompt TTL"
        );
    }
}

#[test]
fn toast_first_then_prompt_consumes_only_each_channel_and_cannot_replay_either() {
    let mut h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    h.delivery.finish_toast("event-a");
    assert!(!toast.is_current());
    assert!(prompt.is_current());
    assert!(h.prompt_permit().is_some());
    // Even a durable state rollback cannot revive the consumed toast channel.
    put_event(&h.db, &event_value());
    assert!(h.permit().is_none());
    h.delivery.finish_prompt("event-a");
    assert!(!prompt.is_current());
    assert!(h.delivery.permits.is_empty());
    assert!(h.delivery.issued.contains("event-a"));
    assert!(h.prompt_permit().is_none());
    assert!(!h.grant());
}

#[test]
fn prompt_ack_first_preserves_an_already_queued_toast_and_its_original_deadline() {
    let mut h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    let queued = crate::notification_gate::QueuedNotification {
        event: event_value(),
        permit: Some(toast.clone()),
    };
    let mut e = event_value();
    e["animationAcknowledged"] = json!(true);
    put_event(&h.db, &e);
    h.delivery.finish_prompt("event-a");
    assert!(!prompt.is_current());
    assert!(h.prompt_permit().is_none());
    assert!(h.permit().is_some());
    assert!(toast.is_current());
    let delivered = crate::notification_gate::deliver_batch(
        std::slice::from_ref(&queued),
        |_| Ok(()),
        |_| Ok(()),
        || true,
        crate::budget_clock::now,
    );
    assert_eq!(delivered.submitted, vec!["event-a"]);
    assert!(delivered.suppressed.is_empty());
    assert_eq!(toast.expires_utc(), later(15_000).utc);
    h.delivery.finish_toast("event-a");
    assert!(!toast.is_current());
    assert!(h.delivery.permits.is_empty());
}

#[test]
fn querying_terminal_toast_state_closes_only_toast_even_before_explicit_finish() {
    let h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    let mut e = event_value();
    e["notificationState"] = json!("failed");
    put_event(&h.db, &e);
    assert!(h.permit().is_none());
    assert!(!toast.is_current());
    assert!(prompt.is_current());
    assert!(h.prompt_permit().is_some());
    e["notificationState"] = json!("pending");
    put_event(&h.db, &e);
    assert!(h.permit().is_none());
    assert!(h.prompt_permit().is_some());
}

#[test]
fn querying_acknowledged_prompt_closes_only_prompt_and_ack_rollback_cannot_revive_it() {
    let h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    let mut e = event_value();
    e["animationAcknowledged"] = json!(true);
    put_event(&h.db, &e);
    assert!(h.prompt_permit().is_none());
    assert!(!prompt.is_current());
    assert!(toast.is_current());
    assert!(h.permit().is_some());
    e["animationAcknowledged"] = json!(false);
    put_event(&h.db, &e);
    assert!(h.prompt_permit().is_none());
    assert!(h.permit().is_some());
}

#[test]
fn suppressed_unknown_or_unqualified_state_revokes_both_channels() {
    for status in ["suppressed", "unknown"] {
        for prompt_query in [false, true] {
            let h = Harness::issued();
            let toast = h.permit().unwrap();
            let prompt = h.prompt_permit().unwrap();
            let mut e = event_value();
            e["notificationState"] = json!(status);
            put_event(&h.db, &e);
            assert!(if prompt_query {
                h.prompt_permit()
            } else {
                h.permit()
            }
            .is_none());
            assert!(!toast.is_current());
            assert!(!prompt.is_current());
        }
    }
    let h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    assert!(h
        .delivery
        .prompt_permit(&h.db, "event-a", "runtime-a", "context-a", false)
        .is_none());
    assert!(!toast.is_current());
    assert!(!prompt.is_current());
}

#[test]
fn either_channel_rechecks_common_configuration_facts_scope_and_host_for_both_clones() {
    for mutation in ["plan", "fact", "context", "host"] {
        let h = Harness::issued();
        let toast = h.permit().unwrap();
        let prompt = h.prompt_permit().unwrap();
        match mutation {
            "plan" => {
                let mut p = plan();
                p["target"] = json!(99);
                put_plan(&h.db, &p);
            }
            "fact" => {
                h.db.execute(
                    "DELETE FROM usage_responses WHERE id=?1",
                    [&h.crossing.fresh_fact_ids[0]],
                )
                .unwrap();
            }
            "host" => {
                h.gate.publish(false, true);
                h.gate.publish(true, false);
            }
            _ => {}
        }
        assert!(
            h.delivery
                .prompt_permit(
                    &h.db,
                    "event-a",
                    "runtime-a",
                    if mutation == "context" {
                        "changed"
                    } else {
                        "context-a"
                    },
                    true
                )
                .is_none(),
            "{mutation}"
        );
        assert!(!toast.is_current(), "{mutation}");
        assert!(!prompt.is_current(), "{mutation}");
    }
}

#[test]
fn expiration_and_pruning_keep_original_pair_clock_deadlines_without_reissuance() {
    let mut h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    h.clock.set(later(3000));
    h.delivery.finish_toast("event-a");
    h.clock.set(later(14_999));
    assert!(h.prompt_permit().is_some());
    h.clock.set(later(15_000));
    assert!(h.prompt_permit().is_none());
    assert!(!toast.is_current());
    assert!(!prompt.is_current());
    h.delivery
        .begin(&h.db, Arc::clone(&h.gate), "runtime-a", "context-a")
        .unwrap();
    assert!(h.delivery.permits.is_empty());
    assert!(h.delivery.issued.contains("event-a"));
    assert!(!h.grant());
}

#[test]
fn a_clock_rollback_seen_by_one_channel_revokes_the_other_channel_too() {
    let h = Harness::issued();
    let toast = h.permit().unwrap();
    let prompt = h.prompt_permit().unwrap();
    h.clock.set(later(5000));
    assert!(toast.is_current());
    h.clock.set(later(4000));
    assert!(h.prompt_permit().is_none());
    assert!(!toast.is_current());
    assert!(!prompt.is_current());
}

#[test]
fn shared_revocation_and_drop_revoke_both_channels_after_one_output_completed() {
    for mutation in ["owner", "all", "finish"] {
        let mut h = Harness::issued();
        let toast = h.permit().unwrap();
        let prompt = h.prompt_permit().unwrap();
        h.delivery.finish_toast("event-a");
        match mutation {
            "owner" => h.delivery.revoke_owner("plan-a"),
            "all" => h.delivery.revoke_all(),
            _ => h.delivery.finish("event-a"),
        };
        assert!(!toast.is_current());
        assert!(!prompt.is_current());
        assert!(h.delivery.permits.is_empty());
    }
    let _outer_clock = TestClock::new(later(1000));
    let prompt = {
        let mut h = Harness::issued();
        let prompt = h.prompt_permit().unwrap();
        h.delivery.finish_toast("event-a");
        prompt
    };
    assert!(!prompt.is_current());
}

#[test]
fn quota_bound_revocation_preserves_a_time_plan_sample_already_in_flight() {
    let mut h = Harness::new();
    let instance = Arc::clone(&h.delivery.instance);
    assert!(Arc::ptr_eq(&h.sample.instance, &instance));
    h.delivery.revoke_quota_bound();
    assert!(Arc::ptr_eq(&h.delivery.instance, &instance));
    h.commit();
    // The sample predates the official-quota boundary. The real production
    // grant still rechecks its unchanged time plan, committed live delta,
    // paired clocks, source, state, and newly inserted event.
    assert!(h.grant());
    let queued = h.permit().unwrap();
    h.delivery.revoke_quota_bound();
    assert!(queued.is_current());
    assert!(h.permit().is_some());
    assert!(
        !h.grant(),
        "the retained sample still cannot issue the same event twice"
    );
}
