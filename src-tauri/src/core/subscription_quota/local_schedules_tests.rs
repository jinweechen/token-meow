use super::*;
use crate::budget_clock::test_support::TestClock;
use crate::core::adapter::{Counts, Metric, StandardEvent};

struct Rig {
    tmp: tempfile::TempDir,
    core: Core,
    clock: TestClock,
    at: Reading,
}
impl Rig {
    fn new() -> Self {
        let at = Reading {
            utc: time("2026-10-23T04:00:00Z").unwrap(),
            mono_ms: 100_000,
        };
        let clock = TestClock::new(at);
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&tmp.path().join("data"), &source).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        put_setting(&core.db,&format!("lastSourceCheck:{}",core.source_id),&json!({"checkedAt":timestamp(at.utc),"discoveryComplete":true,"cancelled":false,"issues":0,"discoveryErrors":0,"pendingFiles":0,"files":0})).unwrap();
        Self {
            tmp,
            core,
            clock,
            at,
        }
    }
    fn advance(&mut self, seconds: i64) {
        self.at.utc += Duration::seconds(seconds);
        self.at.mono_ms += seconds.unsigned_abs() * 1000;
        self.clock.set(self.at);
    }
    fn overview(&self) -> Value {
        self.core
            .read_local_schedules(&self.core.source_id)
            .unwrap()
    }
    fn input(&self, unit: &str) -> Value {
        json!({"expectedRevision":null,"expectedScope":self.overview()["scopeToken"],"sourceId":self.core.source_id,
            "title":format!("合成{unit}日程"),"unit":unit,"model":null,"modelDimension":"response",
            "rule":{"kind":"daily","anchorAt":"2026-10-22T16:00:00Z","endsAt":null,"timezone":"+08:00"},
            "startPolicy":"now","target":if unit=="credits"{"10.000000"}else{"100"},"autoRenew":true,"allocation":[],"remindersEnabled":unit=="tokens"})
    }
    fn create(&mut self, unit: &str) -> Value {
        self.core.save_local_schedule(&self.input(unit)).unwrap()
    }
    fn status(&mut self, template: &Value, state: &str) -> Result<Value> {
        self.core.set_local_schedule_status(&json!({"sourceId":self.core.source_id,"expectedScope":self.overview()["scopeToken"],"id":template["id"],"expectedRevision":template["revision"],"state":state}))
    }
    fn record_input(&self, instance: &Value, amount: &str) -> Value {
        json!({"sourceId":self.core.source_id,"expectedScope":self.overview()["scopeToken"],"instanceId":instance["id"],"idempotencyKey":uuid::Uuid::new_v4().to_string(),"occurredAt":timestamp(self.at.utc),"amount":amount,"note":"合成手动记录"})
    }
    fn usage(&self, id: &str, total: Metric) {
        let response = ResponseUsage {
            source_id: self.core.source_id.clone(),
            channel_id: format!("{}:jsonl", self.core.source_id),
            namespace: "test-local".into(),
            thread_id: "synthetic-thread".into(),
            response_id: id.into(),
            turn_id: None,
            root_turn_id: None,
            event_time: timestamp(self.at.utc - Duration::milliseconds(1)),
            collected_at: timestamp(self.at.utc),
            requested_model: None,
            observed_model: None,
            provider_id: None,
            account_scope_id: None,
            native_counts: BTreeMap::new(),
            counts: Counts {
                input: Metric::Valid(25),
                output: Metric::Valid(0),
                total,
                cache_read: Metric::Missing,
                cache_write: Metric::Missing,
                reasoning: Metric::Missing,
            },
            semantics_version: adapter::SEMANTICS.into(),
            adapter_version: "synthetic-test".into(),
            completeness: "complete".into(),
            phase: "response".into(),
            revision: None,
            live: false,
        };
        adapter::store_standard_event(
            &self.core.db,
            &StandardEvent::ResponseUsage(response),
            false,
        )
        .unwrap();
    }
}
fn by_unit<'a>(view: &'a Value, unit: &str) -> &'a Value {
    view["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["unit"] == unit)
        .unwrap()
}

#[test]
fn actual_core_overview_covers_three_units_and_manual_void_contract() {
    let mut r = Rig::new();
    r.create("tokens");
    r.create("requests");
    let initial = r.create("credits");
    r.advance(1);
    r.usage("response-one", Metric::Valid(25));
    let credits = by_unit(&initial, "credits");
    let payload = r.record_input(credits, "1.200001");
    let recorded = r.core.record_local_consumption(&payload).unwrap();
    assert_eq!(by_unit(&recorded, "credits")["used"], "1.200001");
    let void = json!({"sourceId":r.core.source_id,"expectedScope":recorded["scopeToken"],"entryId":payload["idempotencyKey"],"idempotencyKey":uuid::Uuid::new_v4().to_string(),"expectedRevision":1});
    let view = r.core.void_local_consumption(&void).unwrap();
    assert_eq!(by_unit(&view, "credits")["used"], "0.000000");
    assert_eq!(by_unit(&view, "credits")["entries"][0]["voided"], true);
    assert_eq!(by_unit(&view, "credits")["actualUsed"], Value::Null);
    assert_eq!(by_unit(&view, "requests")["actualUsed"], Value::Null);
    assert_eq!(by_unit(&view, "tokens")["used"], "25");
    assert_eq!(by_unit(&view, "tokens")["actualUsed"], "25");
    println!("LOCAL_SCHEDULE_CONTRACT_FIXTURE={view}");
}

#[test]
fn first_partial_uniform_and_custom_curves_start_at_actual_save_time() {
    let mut r = Rig::new();
    let view = r.create("requests");
    assert_eq!(view["instances"][0]["plannedByNow"], "0");
    r.advance(6 * 3600);
    assert_eq!(r.overview()["instances"][0]["plannedByNow"], "50");
    let mut p = r.input("credits");
    p["allocation"] = json!([{"startMinute":0,"endMinute":720,"weightBps":4000},{"startMinute":720,"endMinute":1440,"weightBps":6000}]);
    let view = r.core.save_local_schedule(&p).unwrap();
    assert_eq!(by_unit(&view, "credits")["plannedByNow"], "0.000000");
    r.advance(3 * 3600);
    assert_eq!(
        by_unit(&r.overview(), "credits")["plannedByNow"],
        "5.000000"
    );
    let mut p = r.input("requests");
    p["allocation"] = json!([{"startMinute":0,"endMinute":60,"weightBps":10000}]);
    let view = r.core.save_local_schedule(&p).unwrap();
    let row = view["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!("no_available_allocation"))
        })
        .unwrap();
    assert_eq!(row["plannedByNow"], "0");
}

#[test]
fn monthly_boundaries_use_original_anchor_and_reject_overlong_curves() {
    let rule = Rule {
        kind: "monthly".into(),
        anchor_at: "2027-01-30T16:00:00Z".into(),
        ends_at: None,
        timezone: "+08:00".into(),
    };
    let feb = period_at(&rule, time("2027-02-28T04:00:00Z").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(timestamp(feb.start), "2027-02-27T16:00:00.000Z");
    assert_eq!(timestamp(feb.end), "2027-03-30T16:00:00.000Z");
    let mut r = Rig::new();
    let mut p = r.input("requests");
    p["rule"]["kind"] = json!("monthly");
    p["allocation"] = json!([{"startMinute":0,"endMinute":40321,"weightBps":10000}]);
    assert!(r.core.save_local_schedule(&p).is_err());
}

#[test]
fn next_period_future_anchor_and_arbitrary_once_window_are_respected() {
    let mut r = Rig::new();
    let mut p = r.input("requests");
    p["startPolicy"] = json!("next_period");
    let v = r.core.save_local_schedule(&p).unwrap();
    assert_eq!(v["instances"].as_array().unwrap().len(), 0);
    r.advance(12 * 3600);
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 1);
    let mut p = r.input("credits");
    p["rule"] = json!({"kind":"once","anchorAt":timestamp(r.at.utc+Duration::seconds(10)),"endsAt":timestamp(r.at.utc+Duration::seconds(85)),"timezone":"+08:00"});
    p["autoRenew"] = json!(false);
    let v = r.core.save_local_schedule(&p).unwrap();
    assert!(v["instances"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["unit"] != "credits"));
    r.advance(10);
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(by_unit(&v, "credits")["effectiveFrom"], timestamp(r.at.utc));
    r.advance(37);
    assert_eq!(
        by_unit(&r.overview(), "credits")["plannedByNow"],
        "4.933333"
    );
}

#[test]
fn offline_jump_restart_and_clock_rollback_never_backfill_or_reopen() {
    let mut r = Rig::new();
    r.create("requests");
    r.advance(1000 * 86400);
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 2);
    r.core.advance_local_schedules().unwrap();
    let source = r.tmp.path().join("source");
    let reopened = Core::open(&r.tmp.path().join("data"), &source).unwrap();
    r.core = reopened;
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 2);
    r.advance(-2 * 86400);
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(v["instances"].as_array().unwrap().len(), 2);
    assert!(v["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("clock_rollback")));
}

#[test]
fn empty_schedule_tick_has_no_database_write_or_clock_marker() {
    let mut r = Rig::new();
    let before = r.core.db.total_changes();
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.core.db.total_changes(), before);
    assert!(last_clock(&r.core.db).unwrap().is_none());
}

#[test]
fn edits_only_change_next_instance_and_pause_does_not_end_current() {
    let mut r = Rig::new();
    let v = r.create("requests");
    let old = v["instances"][0].clone();
    let t = v["templates"][0].clone();
    let mut p = r.input("requests");
    p["id"] = t["id"].clone();
    p["expectedRevision"] = t["revision"].clone();
    p["target"] = json!("200");
    let v = r.core.save_local_schedule(&p).unwrap();
    assert_eq!(v["instances"][0], old);
    let t = v["templates"][0].clone();
    let paused = r.status(&t, "paused").unwrap();
    assert_eq!(paused["instances"][0]["lifecycle"], "active");
    r.advance(12 * 3600 + 1);
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 1);
    let resumed = r.status(&paused["templates"][0], "active").unwrap();
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(v["instances"][0]["target"], "200");
    let disabled = r.status(&resumed["templates"][0], "disabled").unwrap();
    assert_eq!(disabled["instances"][0]["lifecycle"], "ended");
    assert!(r.status(&disabled["templates"][0], "active").is_err());
}

#[test]
fn decimal_idempotency_conflicts_void_and_sums_are_exact() {
    let mut r = Rig::new();
    let v = r.create("credits");
    let instance = by_unit(&v, "credits");
    let p = r.record_input(instance, "9007199254.740991");
    let v = r.core.record_local_consumption(&p).unwrap();
    let revision = v["revision"].clone();
    assert_eq!(
        r.core.record_local_consumption(&p).unwrap()["revision"],
        revision
    );
    let mut conflict = p.clone();
    conflict["amount"] = json!("1.000000");
    assert!(r.core.record_local_consumption(&conflict).is_err());
    let p2 = r.record_input(instance, "9007199254.740991");
    let v = r.core.record_local_consumption(&p2).unwrap();
    assert_eq!(by_unit(&v, "credits")["used"], "18014398509.481982");
    assert_eq!(by_unit(&v, "credits")["remaining"], "0.000000");
    let void = json!({"sourceId":r.core.source_id,"expectedScope":v["scopeToken"],"entryId":p["idempotencyKey"],"idempotencyKey":uuid::Uuid::new_v4().to_string(),"expectedRevision":1});
    let v = r.core.void_local_consumption(&void).unwrap();
    assert_eq!(by_unit(&v, "credits")["used"], "9007199254.740991");
    let rev = v["revision"].clone();
    assert_eq!(
        r.core.void_local_consumption(&void).unwrap()["revision"],
        rev
    );
    let mut second = void.clone();
    second["idempotencyKey"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(r.core.void_local_consumption(&second).is_err());
    assert!(r
        .core
        .record_local_consumption(&r.record_input(instance, "0.0000001"))
        .is_err());
    assert!(r
        .core
        .record_local_consumption(&r.record_input(instance, "0"))
        .is_err());
}

#[test]
fn manual_times_and_units_are_not_inferred_from_response_counts() {
    let mut r = Rig::new();
    let v = r.create("tokens");
    assert!(r
        .core
        .record_local_consumption(&r.record_input(&v["instances"][0], "1"))
        .is_err());
    let v = r.create("requests");
    let i = by_unit(&v, "requests");
    let mut p = r.record_input(i, "1");
    p["occurredAt"] = json!(timestamp(r.at.utc - Duration::milliseconds(1)));
    assert!(r.core.record_local_consumption(&p).is_err());
    p["occurredAt"] = json!(i["periodEndsAt"]);
    assert!(r.core.record_local_consumption(&p).is_err());
    p["occurredAt"] = json!(timestamp(r.at.utc + Duration::seconds(1)));
    assert!(r.core.record_local_consumption(&p).is_err());
    r.advance(1);
    r.usage("not-a-request", Metric::Valid(25));
    let v = r.overview();
    assert_eq!(by_unit(&v, "requests")["used"], "0");
    assert_eq!(by_unit(&v, "requests")["actualUsed"], Value::Null);
    let mut p = r.record_input(i, "1");
    p["verified"] = json!(true);
    assert!(r.core.record_local_consumption(&p).is_err());
}

#[test]
fn changed_binding_aba_and_stale_scope_cannot_reuse_old_budget() {
    let mut r = Rig::new();
    let v = r.create("tokens");
    let stale = r.input("requests");
    let root: String = r
        .core
        .db
        .query_row(
            "SELECT root FROM source_instances WHERE id=?1",
            [&r.core.source_id],
            |row| row.get(0),
        )
        .unwrap();
    r.core
        .action(
            "update_source",
            &json!({"id":r.core.source_id,"path":root,"enabled":true}),
        )
        .unwrap();
    assert!(r.core.save_local_schedule(&stale).is_err());
    let changed = r.overview();
    assert_eq!(changed["templates"][0]["bindingState"], "pending_rebind");
    assert_eq!(changed["instances"][0]["used"], Value::Null);
    assert!(changed["instances"][0]["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("source_binding_changed")));
    let canonical = current_plan(
        &r.core.db,
        v["instances"][0]["tokenPlanId"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(canonical["origin"]["sourceConfigurationRevision"], "0");
    let old = changed["scopeToken"].clone();
    r.core
        .db
        .execute(
            "UPDATE source_instances SET environment='isolated' WHERE id=?1",
            [&r.core.source_id],
        )
        .unwrap();
    assert_ne!(r.overview()["scopeToken"], old);
}

#[test]
fn privacy_clear_and_clock_rollback_revoke_writes_without_moving_token_bounds() {
    let mut r = Rig::new();
    let v = r.create("tokens");
    let t = v["templates"][0].clone();
    let plan_id = v["instances"][0]["tokenPlanId"].as_str().unwrap();
    let before = current_plan(&r.core.db, plan_id).unwrap();
    r.advance(-1);
    assert!(r.status(&t, "disabled").is_err());
    assert_eq!(current_plan(&r.core.db, plan_id).unwrap(), before);
    r.advance(1);
    let stale = r.input("requests");
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    let mut p = r.input("requests");
    assert!(r.core.save_local_schedule(&p).is_err());
    p["expectedScope"] = r.overview()["scopeToken"].clone();
    assert!(r.core.save_local_schedule(&p).is_err());
    assert_eq!(r.overview()["templates"][0]["model"], Value::Null);
    r.core
        .action("update_settings", &json!({"privacyMode":false}))
        .unwrap();
    r.core.action("clear_data", &json!({})).unwrap();
    assert!(r.core.save_local_schedule(&stale).is_err());
    assert!(r.overview()["instances"].as_array().unwrap().is_empty());
    assert!(last_clock(&r.core.db).unwrap().is_none());
}

#[test]
fn incomplete_native_tokens_keep_schedule_projection_unknown() {
    let mut r = Rig::new();
    r.create("tokens");
    r.advance(1);
    r.usage("unknown-response", Metric::Missing);
    let v = r.overview();
    assert_eq!(v["instances"][0]["used"], Value::Null);
    assert_eq!(v["instances"][0]["actualUsed"], Value::Null);
    assert!(v["instances"][0]["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("metering_incomplete")));
}

#[test]
fn failure_before_commit_rolls_back_template_plan_manual_entry_and_revision() {
    let mut r = Rig::new();
    put_setting(
        &r.core.db,
        "testLocalScheduleFailBeforeCommit",
        &json!(true),
    )
    .unwrap();
    assert!(r.core.save_local_schedule(&r.input("tokens")).is_err());
    assert!(r.overview()["templates"].as_array().unwrap().is_empty());
    assert!(latest_plans(&r.core.db).unwrap().is_empty());
    put_setting(
        &r.core.db,
        "testLocalScheduleFailBeforeCommit",
        &json!(false),
    )
    .unwrap();
    let v = r.create("credits");
    let before = r.overview();
    let p = r.record_input(by_unit(&v, "credits"), "1.000001");
    put_setting(
        &r.core.db,
        "testLocalScheduleFailBeforeCommit",
        &json!(true),
    )
    .unwrap();
    assert!(r.core.record_local_consumption(&p).is_err());
    assert_eq!(r.overview(), before);
}

#[test]
fn manual_units_cannot_create_reminders_or_native_delivery_evidence() {
    let mut r = Rig::new();
    let mut p = r.input("requests");
    p["remindersEnabled"] = json!(true);
    assert!(r.core.save_local_schedule(&p).is_err());
    let v = r.create("requests");
    let p = r.record_input(&v["instances"][0], "100");
    r.core.record_local_consumption(&p).unwrap();
    r.core.record_local_consumption(&p).unwrap();
    let count: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM inbox_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn editing_started_next_period_template_uses_immediately_following_boundary() {
    let mut r = Rig::new();
    let mut p = r.input("requests");
    p["startPolicy"] = json!("next_period");
    r.core.save_local_schedule(&p).unwrap();
    r.advance(12 * 3600);
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(v["instances"].as_array().unwrap().len(), 1);
    let end = v["instances"][0]["periodEndsAt"].clone();
    r.advance(3600);
    p["expectedScope"] = r.overview()["scopeToken"].clone();
    p["id"] = v["templates"][0]["id"].clone();
    p["expectedRevision"] = v["templates"][0]["revision"].clone();
    p["target"] = json!("200");
    let edited = r.core.save_local_schedule(&p).unwrap();
    assert_eq!(edited["templates"][0]["firstStartsAt"], end);
    assert_eq!(edited["instances"][0]["target"], "100");
    r.advance(23 * 3600);
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(v["instances"].as_array().unwrap().len(), 2);
    assert_eq!(v["instances"][0]["effectiveFrom"], end);
    assert_eq!(v["instances"][0]["target"], "200");
}

#[test]
fn privacy_suspends_renewal_then_resumes_current_period_with_unknown_baseline() {
    let mut r = Rig::new();
    r.create("tokens");
    let before = last_clock(&r.core.db).unwrap();
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    r.advance(10 * 86400 + 120);
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 1);
    assert_eq!(last_clock(&r.core.db).unwrap(), before);
    r.usage("incomplete-during-privacy", Metric::Missing);
    r.core
        .action("update_settings", &json!({"privacyMode":false}))
        .unwrap();
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(v["instances"].as_array().unwrap().len(), 2);
    let current = &v["instances"][0];
    assert_eq!(current["used"], Value::Null);
    assert!(current["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("metering_incomplete")));
    let id = current["tokenPlanId"].as_str().unwrap();
    let baseline = state(&r.core.db, id).unwrap();
    assert_eq!(baseline["baselineOnly"], true);
    assert_eq!(baseline["used"], Value::Null);
    let events: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM inbox_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(events, 0);
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 2);
}

#[test]
fn ended_rebound_template_respects_explicit_next_period_policy() {
    let mut r = Rig::new();
    let v = r.create("requests");
    r.advance(12 * 3600 + 1);
    let root: String = r
        .core
        .db
        .query_row(
            "SELECT root FROM source_instances WHERE id=?1",
            [&r.core.source_id],
            |row| row.get(0),
        )
        .unwrap();
    r.core
        .action(
            "update_source",
            &json!({"id":r.core.source_id,"path":root,"enabled":true}),
        )
        .unwrap();
    let mut p = r.input("requests");
    p["id"] = v["templates"][0]["id"].clone();
    p["expectedRevision"] = v["templates"][0]["revision"].clone();
    p["target"] = json!("200");
    p["startPolicy"] = json!("next_period");
    let edited = r.core.save_local_schedule(&p).unwrap();
    assert_eq!(edited["instances"].as_array().unwrap().len(), 1);
    let first = edited["templates"][0]["firstStartsAt"].clone();
    assert!(time(first.as_str().unwrap()).unwrap() > r.at.utc);
    r.core.advance_local_schedules().unwrap();
    assert_eq!(r.overview()["instances"].as_array().unwrap().len(), 1);
    r.advance(86400 - 1);
    r.core.advance_local_schedules().unwrap();
    let v = r.overview();
    assert_eq!(v["instances"].as_array().unwrap().len(), 2);
    assert_eq!(v["instances"][0]["effectiveFrom"], first);
    assert_eq!(v["instances"][0]["target"], "200");
}
