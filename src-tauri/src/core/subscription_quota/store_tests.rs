use super::super::super::adapter::{Counts, Metric, StandardEvent};
use super::*;
use crate::budget_clock::test_support::TestClock;

struct Rig {
    _tmp: tempfile::TempDir,
    core: Core,
    clock: TestClock,
    at: Reading,
}
impl Rig {
    fn new() -> Self {
        let at = Reading {
            utc: time("2026-10-22T08:00:00Z").unwrap(),
            mono_ms: 100000,
        };
        let clock = TestClock::new(at);
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&tmp.path().join("data"), &source).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        put_setting(&core.db,&format!("lastSourceCheck:{}",core.source_id),&json!({"checkedAt":timestamp(at.utc),"discoveryComplete":true,"cancelled":false,"issues":0,"discoveryErrors":0,"pendingFiles":0,"files":0})).unwrap();
        Self {
            _tmp: tmp,
            core,
            clock,
            at,
        }
    }
    fn advance(&mut self, ms: u64) {
        self.at.utc += Duration::milliseconds(ms as i64);
        self.at.mono_ms += ms;
        self.clock.set(self.at);
    }
    fn input(&self) -> Value {
        json!({"expectedRevision":null,"title":"两小时计划","type":"tokens","sourceId":self.core.source_id,"model":null,"modelDimension":"response","target":100,"endPolicy":"time","endsAt":timestamp(self.at.utc+Duration::hours(2)),"windowId":null,"remindersEnabled":true})
    }
    fn create(&mut self) -> Value {
        self.core.save_usage_plan(&self.input()).unwrap()["plans"][0].clone()
    }
    fn usage(&self, id: &str, n: i64, live: bool, observed: Option<&str>, requested: Option<&str>) {
        // A completed response precedes the scan's exclusive as-of boundary.
        self.usage_at(
            id,
            n,
            live,
            observed,
            requested,
            timestamp(self.at.utc - Duration::milliseconds(1)),
        );
    }
    fn usage_at(
        &self,
        id: &str,
        n: i64,
        live: bool,
        observed: Option<&str>,
        requested: Option<&str>,
        event_time: String,
    ) {
        let r = ResponseUsage {
            source_id: self.core.source_id.clone(),
            channel_id: format!("{}:jsonl", self.core.source_id),
            namespace: "test-native".into(),
            thread_id: "thread".into(),
            response_id: id.into(),
            turn_id: None,
            root_turn_id: None,
            event_time,
            collected_at: timestamp(self.at.utc),
            requested_model: requested.map(str::to_owned),
            observed_model: observed.map(str::to_owned),
            provider_id: None,
            account_scope_id: None,
            native_counts: BTreeMap::new(),
            counts: Counts {
                input: Metric::Valid(n),
                output: Metric::Valid(0),
                total: Metric::Valid(n),
                cache_read: Metric::Missing,
                cache_write: Metric::Missing,
                reasoning: Metric::Missing,
            },
            semantics_version: adapter::SEMANTICS.into(),
            adapter_version: "test".into(),
            completeness: "complete".into(),
            phase: "response".into(),
            revision: None,
            live,
        };
        adapter::store_standard_event(&self.core.db, &StandardEvent::ResponseUsage(r), false)
            .unwrap();
    }
    fn finish(&mut self, s: Option<Scan>) {
        let committed = BTreeSet::from([self.core.source_id.clone()]);
        self.core
            .finish_subscription_scan(s, &committed, false)
            .unwrap();
    }
    fn inbox_count(&self) -> i64 {
        self.core.db.query_row("SELECT COUNT(*) FROM inbox_events WHERE json_extract(data,'$.kind')='subscription_plan'",[],|r|r.get(0)).unwrap()
    }
    fn observation(&self, seq: &str, outcome: &str) -> Value {
        json!({"status":"connected","observationSequence":seq,"observationOutcome":outcome,"planType":"pro","identityEpoch":"1","identityConsistency":"unproven","identityConsistencyReason":"fixture_unproven","readStartedAt":timestamp(self.at.utc),"readFinishedAt":timestamp(self.at.utc),"collectedAt":timestamp(self.at.utc),"quotaRead":{"freshnessAnchor":timestamp(self.at.utc),"factTimeLower":null,"factTimeUpper":null},"buckets":[{"id":"codex","name":"共享额度","primary":{"usedPercent":60,"resetsAt":(self.at.utc+Duration::hours(1)).timestamp()}}]})
    }
}

#[test]
fn subscription_plan_counts_from_save_and_never_falls_back_response_model() {
    let mut r = Rig::new();
    r.usage("before", 90, true, Some("A"), Some("B"));
    r.advance(1000);
    let mut input = r.input();
    input["model"] = json!("A");
    r.core.save_usage_plan(&input).unwrap();
    r.core.save_usage_plan(&r.input()).unwrap();
    r.advance(1000);
    r.usage("request-only", 40, true, None, Some("A"));
    r.usage("actual", 20, true, Some("A"), Some("B"));
    let overview = r.core.read_subscription_overview().unwrap();
    let model_plan = overview["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["model"] == "A")
        .unwrap();
    assert!(model_plan["used"].is_null());
    assert!(model_plan["remaining"].is_null());
    assert!(model_plan["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("model_coverage_incomplete")));
    let source_plan = overview["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["model"].is_null())
        .unwrap();
    assert_eq!(source_plan["used"], 60.0);
    assert_eq!(r.inbox_count(), 0);
    assert!(overview["modelUsage"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["model"] == "unknown" && v["modelDimension"] == "response"));
}
#[test]
fn subscription_revision_conflict_preserves_start_and_lower_budget_does_not_notify() {
    let mut r = Rig::new();
    let plan = r.create();
    r.advance(1000);
    r.usage("a", 90, true, None, None);
    let mut edit = r.input();
    edit["id"] = plan["id"].clone();
    edit["expectedRevision"] = json!(1);
    edit["endsAt"] = plan["endsAt"].clone();
    edit["target"] = json!(50);
    let updated = r.core.save_usage_plan(&edit).unwrap();
    assert_eq!(updated["plans"][0]["effectiveFrom"], plan["effectiveFrom"]);
    assert_eq!(updated["plans"][0]["used"], 90.0);
    assert_eq!(r.inbox_count(), 0);
    assert!(r.core.save_usage_plan(&edit).unwrap_err().contains("修改"));
    assert_eq!(latest_plans(&r.core.db).unwrap()[0]["revision"], 2);
}
#[test]
fn subscription_live_crossing_latches_once_and_history_cannot_notify() {
    let mut r = Rig::new();
    r.create();
    let s = r.core.begin_subscription_scan().unwrap();
    r.finish(s);
    r.advance(1000);
    let s = r.core.begin_subscription_scan().unwrap();
    r.usage("live", 85, true, None, None);
    r.finish(s);
    assert_eq!(r.inbox_count(), 1);
    r.advance(1000);
    let s = r.core.begin_subscription_scan().unwrap();
    r.usage("historical", 50, false, None, None);
    r.finish(s);
    assert_eq!(r.inbox_count(), 1);
    assert!(r.core.claim_notifications().unwrap().is_empty());
    let v: Value = serde_json::from_str(
        &r.core
            .db
            .query_row("SELECT data FROM inbox_events", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["notificationState"], "panel_only");
    assert_eq!(v["animationAcknowledged"], true);
}
#[test]
fn subscription_pause_continues_counting_and_resume_does_not_replay() {
    let mut r = Rig::new();
    let p = r.create();
    let id = p["id"].as_str().unwrap();
    r.core
        .set_usage_plan_status(&json!({"id":id,"expectedRevision":1,"remindersEnabled":false}))
        .unwrap();
    let s = r.core.begin_subscription_scan().unwrap();
    r.finish(s);
    r.advance(1000);
    let s = r.core.begin_subscription_scan().unwrap();
    r.usage("a", 85, true, None, None);
    r.finish(s);
    assert_eq!(
        r.core.read_subscription_overview().unwrap()["plans"][0]["used"],
        85.0
    );
    assert_eq!(r.inbox_count(), 0);
    r.core
        .set_usage_plan_status(&json!({"id":id,"expectedRevision":2,"remindersEnabled":true}))
        .unwrap();
    r.advance(1000);
    let s = r.core.begin_subscription_scan().unwrap();
    r.usage("b", 1, true, None, None);
    r.finish(s);
    assert_eq!(r.inbox_count(), 0);
}
#[test]
fn subscription_retained_facts_missing_make_progress_unknown() {
    let mut r = Rig::new();
    r.create();
    r.advance(1000);
    r.usage("a", 85, true, None, None);
    put_setting(
        &r.core.db,
        "retentionWatermark",
        &json!(timestamp(r.at.utc)),
    )
    .unwrap();
    let overview = r.core.read_subscription_overview().unwrap();
    assert!(overview["plans"][0]["used"].is_null());
    assert!(overview["plans"][0]["remaining"].is_null());
    assert_eq!(overview["plans"][0]["evaluationState"], "unknown");
}
#[test]
fn subscription_old_success_cannot_replace_newer_failure_or_renew_freshness() {
    let mut r = Rig::new();
    let newer = r.observation("2", "failure");
    r.core.settle_subscription_observation(0, newer).unwrap();
    let older = r.observation("1", "success");
    let current = r.core.settle_subscription_observation(0, older).unwrap();
    assert_eq!(current["observationSequence"], "2");
    assert_eq!(current["observationOutcome"], "failure");
    assert_eq!(
        r.core.read_subscription_overview().unwrap()["lastPersistedSequence"],
        "2"
    );
    let count: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM account_snapshots", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn subscription_failed_persistence_keeps_ram_watermark_and_cannot_be_retried_as_new() {
    let mut r = Rig::new();
    r.core.db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON account_snapshots BEGIN SELECT RAISE(ABORT,'fixture persistence failure'); END;").unwrap();
    let v = r
        .core
        .settle_subscription_observation(0, r.observation("2", "success"))
        .unwrap();
    assert_eq!(v["persisted"], false);
    assert!(v["asOfRevision"].is_null());
    r.core
        .db
        .execute_batch("DROP TRIGGER fail_receipt;")
        .unwrap();
    let old = r
        .core
        .settle_subscription_observation(0, r.observation("1", "success"))
        .unwrap();
    assert_eq!(old["observationSequence"], "2");
    assert_eq!(old["persisted"], false);
    let overview = r.core.read_subscription_overview().unwrap();
    assert_eq!(overview["lastSettledSequence"], "2");
    assert_eq!(overview["lastPersistedSequence"], "0");
    let fresh = r
        .core
        .settle_subscription_observation(0, r.observation("3", "success"))
        .unwrap();
    assert_eq!(fresh["persisted"], true);
}
#[test]
fn subscription_unproven_identity_does_not_allow_window_plan_or_persist_quota_facts() {
    let mut r = Rig::new();
    r.core
        .settle_subscription_observation(0, r.observation("1", "success"))
        .unwrap();
    let overview = r.core.read_subscription_overview().unwrap();
    assert_eq!(overview["windows"][0]["usedPercent"], 60.0);
    assert_eq!(overview["windows"][0]["canBindPlan"], false);
    assert_eq!(overview["calibrations"][0]["quality"], "learning");
    let mut input = r.input();
    input["type"] = json!("percent");
    input["endPolicy"] = json!("window");
    input["windowId"] = json!("codex:primary");
    assert!(r.core.save_usage_plan(&input).is_err());
    let text: String = r
        .core
        .db
        .query_row("SELECT data FROM account_snapshots", [], |row| row.get(0))
        .unwrap();
    let receipt: Value = serde_json::from_str(&text).unwrap();
    assert!(receipt.get("observation").is_none());
    assert!(r.core.save_usage_plan(&r.input()).is_ok());
}
#[test]
fn subscription_capacity_assumption_is_separate_and_never_creates_calibration() {
    let mut r = Rig::new();
    let plan = r.create();
    let v=r.core.save_quota_assumption(&json!({"expectedRevision":null,"sourceId":r.core.source_id,"windowId":"codex:primary","model":null,"capacityTokens":2000000,"validUntil":timestamp(r.at.utc+Duration::days(1)),"note":"手动线索"})).unwrap();
    assert_eq!(v["assumptions"][0]["capacityTokens"], 2000000.0);
    assert_eq!(v["plans"][0]["target"], plan["target"]);
    assert!(v["calibrations"].as_array().unwrap().is_empty());
}
#[test]
fn subscription_read_is_side_effect_free_and_clear_removes_all_own_data() {
    let mut r = Rig::new();
    r.create();
    put_setting(
        &r.core.db,
        "subscriptionEvidenceRetention:v1",
        &json!({"deletedTotal":17}),
    )
    .unwrap();
    let before: i64 = r
        .core
        .db
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    r.core.read_subscription_overview().unwrap();
    r.core.read_subscription_overview().unwrap();
    let after: i64 = r
        .core
        .db
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    assert_eq!(before, after);
    r.core.action("clear_data", &json!({})).unwrap();
    assert!(get_setting(&r.core.db, "subscriptionEvidenceRetention:v1")
        .unwrap()
        .is_none());
    assert!(latest_plans(&r.core.db).unwrap().is_empty());
    assert!(r.core.read_subscription_overview().unwrap()["plans"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(r.core.source_root.is_dir());
}

#[test]
fn subscription_no_rpc_failure_and_ttl_both_stale_the_overview_without_new_sequence() {
    let mut r = Rig::new();
    r.core
        .settle_subscription_observation(0, r.observation("1", "success"))
        .unwrap();
    assert_eq!(
        r.core.read_subscription_overview().unwrap()["membership"]["freshness"],
        "fresh"
    );
    r.advance(241000);
    assert_eq!(
        r.core.read_subscription_overview().unwrap()["membership"]["freshness"],
        "stale"
    );
    let mut failure = r.observation("1", "failure");
    failure["observationSequence"] = Value::Null;
    failure["status"] = json!("stale");
    r.core.settle_subscription_observation(0, failure).unwrap();
    let v = r.core.read_subscription_overview().unwrap();
    assert_eq!(v["lastSettledSequence"], "1");
    assert_eq!(v["lastPersistedSequence"], "1");
    assert_eq!(v["windows"][0]["freshness"], "stale");
    assert!(v["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("not_persisted")));
}
#[test]
fn subscription_shorter_retention_evicts_receipts_without_a_new_account_rpc() {
    let mut r = Rig::new();
    r.core
        .settle_subscription_observation(0, r.observation("1", "success"))
        .unwrap();
    r.advance(2 * 86400000);
    r.core
        .action("update_settings", &json!({"retentionDays":1}))
        .unwrap();
    r.core.action("apply_retention", &json!({})).unwrap();
    assert_eq!(
        r.core
            .db
            .query_row("SELECT COUNT(*) FROM account_snapshots", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn subscription_private_overview_hides_authored_text_while_background_plan_remains() {
    let mut r = Rig::new();
    r.create();
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    let v = r.core.read_subscription_overview().unwrap();
    assert_eq!(v["plans"][0]["title"], "使用计划");
    assert_eq!(
        current_plan(&r.core.db, v["plans"][0]["id"].as_str().unwrap()).unwrap()["title"],
        "两小时计划"
    );
}

#[test]
fn subscription_ipc_requires_captured_scope_and_rejects_clear_aba() {
    let mut r = Rig::new();
    let mut input = r.input();
    assert!(r
        .core
        .subscription_command("save", &input)
        .unwrap_err()
        .contains("范围"));
    let scope = r.core.subscription_scope_token().unwrap();
    input["expectedScope"] = json!(scope);
    r.core.subscription_command("save", &input).unwrap();
    r.core.action("clear_data", &json!({})).unwrap();
    assert_ne!(r.core.subscription_scope_token().unwrap(), scope);
    assert!(r
        .core
        .subscription_command("save", &input)
        .unwrap_err()
        .contains("范围"));
    assert!(latest_plans(&r.core.db).unwrap().is_empty());
}
#[test]
fn subscription_ipc_rejects_other_source_and_privacy_aba() {
    let mut r = Rig::new();
    let other = r._tmp.path().join("other");
    fs::create_dir(&other).unwrap();
    let other_id = r.core.add_source(&other).unwrap();
    let mut input = r.input();
    input["expectedScope"] = json!(r.core.subscription_scope_token().unwrap());
    input["sourceId"] = json!(other_id);
    assert!(r
        .core
        .subscription_command("save", &input)
        .unwrap_err()
        .contains("主来源"));
    input["sourceId"] = json!(r.core.source_id);
    let scope = r.core.subscription_scope_token().unwrap();
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    input["expectedScope"] = json!(r.core.subscription_scope_token().unwrap());
    assert!(r
        .core
        .subscription_command("save", &input)
        .unwrap_err()
        .contains("隐私"));
    r.core
        .action("update_settings", &json!({"privacyMode":false}))
        .unwrap();
    assert_ne!(r.core.subscription_scope_token().unwrap(), scope);
    r.core
        .action("set_primary_source", &json!({"id":other_id}))
        .unwrap();
    let original = r.core.source_id.clone();
    // source_id follows the current primary; the input still belongs to the old view.
    r.core
        .action("set_primary_source", &json!({"id":input["sourceId"]}))
        .unwrap();
    assert_ne!(r.core.subscription_scope_token().unwrap(), scope);
    assert_ne!(original, strv(&input, "sourceId"));
}
#[test]
fn subscription_natural_end_history_does_not_use_active_plan_slots() {
    let mut r = Rig::new();
    let p = r.create();
    let tx = r.core.db.transaction().unwrap();
    for n in 0..100 {
        let mut old = p.clone();
        old["id"] = json!(format!("old-{n}"));
        old["endsAt"] = json!(timestamp(r.at.utc));
        save_config(&tx, &old).unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(
        r.core.save_usage_plan(&r.input()).unwrap()["plans"]
            .as_array()
            .unwrap()
            .len(),
        102
    );
}
#[test]
fn subscription_outside_interval_conflict_does_not_poison_progress() {
    let mut r = Rig::new();
    r.create();
    r.advance(1000);
    r.usage("actual", 20, true, None, None);
    r.advance(100000);
    r.usage("future-conflict", 100, true, None, None);
    let id:String=r.core.db.query_row("SELECT id FROM usage_responses WHERE json_extract(data,'$.responseId')='future-conflict'",[],|row|row.get(0)).unwrap();
    r.core
        .db
        .execute(
            "INSERT INTO excluded_facts(fact_id,reason) VALUES(?1,'fixture')",
            [id],
        )
        .unwrap();
    r.at.utc -= Duration::milliseconds(100000);
    r.at.mono_ms -= 100000;
    r.clock.set(r.at);
    assert_eq!(
        r.core.read_subscription_overview().unwrap()["plans"][0]["used"],
        20.0
    );
}
#[test]
fn subscription_core_delivery_requires_fresh_commit_and_revokes_queue_on_plan_loss() {
    let mut r = Rig::new();
    r.core.subscription.native_test_qualified = true;
    r.core.notification_gate.publish(true, false);
    r.create();
    let scan = r.core.begin_subscription_scan().unwrap();
    r.finish(scan);
    r.advance(1000);
    let scan = r.core.begin_subscription_scan().unwrap();
    r.usage("fresh", 85, true, None, None);
    r.finish(scan);
    let claimed = r.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    let permit = r
        .core
        .notification_permit(&claimed[0])
        .expect("newly committed crossing receives a domain permit");
    assert!(permit.is_current());
    r.core.db.execute("DELETE FROM usage_plans", []).unwrap();
    assert!(r.core.notification_permit(&claimed[0]).is_none());
    assert!(
        !permit.is_current(),
        "Core rejection must revoke queued clones too"
    );
}
#[test]
fn subscription_delivery_owner_change_revokes_already_queued_clone() {
    let mut r = Rig::new();
    r.core.subscription.native_test_qualified = true;
    r.core.notification_gate.publish(true, false);
    let p = r.create();
    let scan = r.core.begin_subscription_scan().unwrap();
    r.finish(scan);
    r.advance(1000);
    let scan = r.core.begin_subscription_scan().unwrap();
    r.usage("fresh", 85, true, None, None);
    r.finish(scan);
    let event = r.core.claim_notifications().unwrap().pop().unwrap();
    let permit = r.core.notification_permit(&event).unwrap();
    r.core
        .set_usage_plan_status(&json!({"id":p["id"],"expectedRevision":1,"remindersEnabled":false}))
        .unwrap();
    assert!(!permit.is_current());
    assert!(r.core.notification_permit(&event).is_none());
}

#[test]
fn subscription_old_duplicate_observation_preserves_a_new_crossing_permit() {
    let mut r = Rig::new();
    r.core
        .settle_subscription_observation(0, r.observation("2", "success"))
        .unwrap();
    r.core.subscription.native_test_qualified = true;
    r.core.notification_gate.publish(true, false);
    r.create();
    let scan = r.core.begin_subscription_scan().unwrap();
    r.finish(scan);
    r.advance(1000);
    let scan = r.core.begin_subscription_scan().unwrap();
    r.usage("fresh", 85, true, None, None);
    r.finish(scan);
    let event = r.core.claim_notifications().unwrap().pop().unwrap();
    let permit = r.core.notification_permit(&event).unwrap();
    r.core
        .settle_subscription_observation(0, r.observation("1", "failure"))
        .unwrap();
    r.core
        .settle_subscription_observation(0, r.observation("2", "success"))
        .unwrap();
    assert!(permit.is_current());
    assert!(r.core.notification_permit(&event).is_some());
    r.core
        .settle_subscription_observation(0, r.observation("3", "failure"))
        .unwrap();
    assert!(
        permit.is_current(),
        "independent time plan is not owned by account RPC availability"
    );
    assert!(r.core.notification_permit(&event).is_some());
}
#[test]
fn subscription_normal_new_turn_metadata_does_not_suppress_crossing() {
    let mut r = Rig::new();
    r.create();
    let scan = r.core.begin_subscription_scan().unwrap();
    r.finish(scan);
    r.advance(1000);
    let scan = r.core.begin_subscription_scan().unwrap();
    r.usage("fresh", 85, true, None, None);
    r.core.db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES('new-turn','thread','turn',?1,?2)",params![timestamp(r.at.utc),json!({"id":"new-turn","sourceId":r.core.source_id,"status":"completed"}).to_string()]).unwrap();
    r.finish(scan);
    assert_eq!(r.inbox_count(), 1);
}

#[test]
fn subscription_no_rpc_or_unpersisted_failure_breaks_the_next_real_observation_segment() {
    let mut r = Rig::new();
    r.core
        .settle_subscription_observation(0, r.observation("1", "success"))
        .unwrap();
    let mut failure = r.observation("1", "failure");
    failure["observationSequence"] = Value::Null;
    r.core.settle_subscription_observation(0, failure).unwrap();
    let recovery = r
        .core
        .settle_subscription_observation(0, r.observation("2", "success"))
        .unwrap();
    assert_eq!(recovery["discontinuity"], true);
    let following = r
        .core
        .settle_subscription_observation(0, r.observation("3", "success"))
        .unwrap();
    assert_ne!(following["discontinuity"], true);
    r.core.db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON account_snapshots BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(
        r.core
            .settle_subscription_observation(0, r.observation("4", "success"))
            .unwrap()["persisted"],
        false
    );
    r.core
        .db
        .execute_batch("DROP TRIGGER fail_receipt;")
        .unwrap();
    assert_eq!(
        r.core
            .settle_subscription_observation(0, r.observation("5", "success"))
            .unwrap()["discontinuity"],
        true
    );
}

#[test]
fn subscription_core_local_forecast_is_auditable_without_provider_and_rejects_fact_correction() {
    let mut r = Rig::new();
    r.create();
    for step in 1..=8 {
        r.advance(10_000);
        let scan = r.core.begin_subscription_scan().unwrap();
        r.usage(&format!("forecast-{step}"), 10, true, Some("A"), Some("B"));
        r.finish(scan);
    }
    let before = r.core.db.total_changes();
    let overview = r.core.read_subscription_overview().unwrap();
    assert_eq!(overview["learningStatus"]["status"], "unavailable");
    let plan = &overview["plans"][0];
    let forecast = &plan["forecasts"]["localPlanEta"];
    assert_eq!(forecast["state"], "conditional", "{forecast}");
    assert_eq!(forecast["kind"], "local_plan_eta");
    assert!(forecast["inputLedgerFingerprint"].as_str().is_some());
    assert_eq!(forecast["planConfigurationRevision"], 1);
    assert_eq!(forecast["notifyEligible"], false);
    assert_eq!(overview["forecastResults"].as_array().unwrap().len(), 2);
    assert_eq!(
        before,
        r.core.db.total_changes(),
        "forecast query must remain read only"
    );
    // A fact correction that keeps the aggregate unchanged still invalidates
    // past rate evidence; comparing only the plan's current total is unsafe.
    r.core.db.execute("UPDATE usage_responses SET data=json_set(data,'$.providerId','corrected-provider') WHERE rowid=(SELECT MIN(rowid) FROM usage_responses)", []).unwrap();
    let corrected = r.core.read_subscription_overview().unwrap();
    assert_eq!(corrected["plans"][0]["used"], plan["used"]);
    assert_eq!(
        corrected["plans"][0]["forecasts"]["localPlanEta"]["state"],
        "unavailable"
    );
    assert!(corrected["plans"][0]["forecasts"]["localPlanEta"]["empiricalScenarioRange"].is_null());
}

#[test]
fn subscription_nanosecond_clock_forecast_uses_the_persisted_half_open_measurement_boundary() {
    let mut r = Rig::new();
    r.at.utc += Duration::nanoseconds(500_123);
    r.clock.set(r.at);
    let mut input = r.input();
    input["target"] = json!(1000);
    let configured = r.core.save_usage_plan(&input).unwrap()["plans"][0].clone();
    let start = time(configured["effectiveFrom"].as_str().unwrap()).unwrap();
    let id = configured["id"].as_str().unwrap();
    for step in 1..=8 {
        r.advance(10_000);
        let scan = r.core.begin_subscription_scan().unwrap();
        r.usage(
            &format!("nano-included-{step}"),
            10,
            true,
            Some("A"),
            Some("B"),
        );
        let measured_end = time(&timestamp(r.at.utc)).unwrap();
        let next_head_fact = measured_end + Duration::nanoseconds(1);
        assert!(
            next_head_fact < r.at.utc,
            "this fact is before the live clock but after its persisted measurement boundary"
        );
        r.usage_at(
            &format!("nano-next-head-{step}"),
            7,
            true,
            Some("A"),
            Some("B"),
            next_head_fact.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        );
        let expected_used = 10 * step + 7 * (step - 1);
        let expected = super::super::demand_history::ledger_fingerprint(
            &r.core.db,
            &r.core.source_id,
            start,
            measured_end,
        )
        .unwrap();
        let incompatible = super::super::demand_history::ledger_fingerprint(
            &r.core.db,
            &r.core.source_id,
            start,
            r.at.utc,
        )
        .unwrap();
        assert_ne!(
            expected, incompatible,
            "the native-clock interval includes a different set of facts"
        );
        let before = r.core.db.total_changes();
        let read = r.core.read_subscription_overview().unwrap();
        let view = &read["plans"][0];
        assert_eq!(view["measuredUntil"], timestamp(measured_end));
        assert_eq!(
            view["used"], expected_used as f64,
            "the +1ns fact must wait for the next measurement head"
        );
        assert_eq!(view["forecastLedgerFingerprint"], expected);
        assert_ne!(view["forecastLedgerFingerprint"], incompatible);
        assert_eq!(
            view["endsAt"], configured["endsAt"],
            "measurement normalization must not move the configured end"
        );
        assert_eq!(r.core.db.total_changes(), before);
        r.finish(scan);
        let persisted = state(&r.core.db, id).unwrap();
        let head = &persisted["localForecast"]["head"];
        assert_eq!(time(head["utc"].as_str().unwrap()).unwrap(), measured_end);
        assert_eq!(head["used"], expected_used as f64);
        assert_eq!(head["ledgerFingerprint"], expected);
        let observed = r.core.read_subscription_overview().unwrap();
        let observed = &observed["plans"][0];
        assert_eq!(
            observed["forecastLedgerHeadFingerprint"],
            head["ledgerFingerprint"]
        );
        for kind in ["localPlanEta", "periodEndUsage"] {
            assert!(
                !observed["forecasts"][kind]["reasons"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("local_forecast_ledger_changed")),
                "identical persisted half-open intervals cannot disagree: {observed}"
            );
        }
    }
    let current = r.core.read_subscription_overview().unwrap();
    let view = &current["plans"][0];
    assert_eq!(
        view["used"], 129.0,
        "seven previous +1ns facts enter later heads, the eighth still waits"
    );
    assert_eq!(
        view["forecasts"]["localPlanEta"]["state"], "conditional",
        "80 seconds of real Core scans must produce a rate: {view}"
    );
    assert_eq!(view["forecasts"]["periodEndUsage"]["state"], "conditional");
    assert!(view["forecasts"]["localPlanEta"]["empiricalScenarioRange"].is_object());
}

#[test]
fn subscription_time_plan_keeps_inflight_scan_and_permission_across_account_failure_and_rebinding()
{
    let mut r = Rig::new();
    r.core.subscription.native_test_qualified = true;
    r.core.notification_gate.publish(true, false);
    r.create();
    let baseline = r.core.begin_subscription_scan().unwrap();
    r.finish(baseline);
    r.advance(1000);
    let scan = r.core.begin_subscription_scan().unwrap();
    let mut failed = r.observation("1", "failure");
    failed["observationSequence"] = Value::Null;
    r.core.settle_subscription_observation(7, failed).unwrap();
    r.core.subscription_unavailable();
    // Initial quota domain creation and a bridge-only rebinding must preserve
    // this independent collector's already acquired local scan authority.
    r.core
        .settle_subscription_observation(7, r.observation("1", "failure"))
        .unwrap();
    r.usage("fresh-despite-account-failure", 85, true, None, None);
    r.finish(scan);
    let event = r.core.claim_notifications().unwrap().pop().unwrap();
    let permit = r.core.notification_permit(&event).unwrap();
    r.core
        .settle_subscription_observation(8, r.observation("1", "failure"))
        .unwrap();
    r.core.subscription_unavailable();
    assert!(permit.is_current());
    assert!(r.core.notification_permit(&event).is_some());
    r.core.notification_gate.publish(false, false);
    assert!(
        !permit.is_current(),
        "actual host permission still revokes independent plans"
    );
}

// Exercise real begin/finish writer integration rather than hand-authoring a
// trusted forecast proof. Read-only calls must remain unavailable until commit.
fn qualified_local_forecast_rig() -> (Rig, String) {
    let mut rig = Rig::new();
    let mut input = rig.input();
    input["target"] = json!(10000);
    let configured = rig.core.save_usage_plan(&input).unwrap()["plans"][0].clone();
    for step in 1..=8 {
        rig.advance(10_000);
        let scan = rig.core.begin_subscription_scan().unwrap();
        rig.usage(&format!("baseline-{step}"), 10, true, Some("A"), Some("B"));
        rig.finish(scan);
    }
    assert_eq!(
        rig.core.read_subscription_overview().unwrap()["plans"][0]["forecasts"]["localPlanEta"]
            ["state"],
        "conditional"
    );
    (rig, configured["id"].as_str().unwrap().to_owned())
}

#[test]
fn subscription_fresh_late_append_preserves_complete_baselines_only_after_writer_commit() {
    let (mut rig, id) = qualified_local_forecast_rig();
    let before = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
    let initial_head = time(before["head"]["utc"].as_str().unwrap()).unwrap();
    for step in 1..=4 {
        let old = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
        let old_head = time(old["head"]["utc"].as_str().unwrap()).unwrap();
        rig.advance(179);
        let scan = rig.core.begin_subscription_scan().unwrap();
        let event = old_head - Duration::milliseconds(1);
        rig.usage_at(
            &format!("late-{step}"),
            10,
            true,
            Some("A"),
            Some("B"),
            timestamp(event),
        );
        let changes = rig.core.db.total_changes();
        let pending = rig.core.read_subscription_overview().unwrap();
        assert_eq!(
            pending["plans"][0]["forecasts"]["localPlanEta"]["state"],
            "unavailable"
        );
        assert_eq!(
            rig.core.db.total_changes(),
            changes,
            "query cannot repair a writer proof"
        );
        rig.finish(scan);
        let after = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
        let retained: Vec<_> = before["points"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| time(p["utc"].as_str().unwrap()).unwrap() < initial_head)
            .cloned()
            .collect();
        for point in retained {
            assert!(
                after["points"].as_array().unwrap().contains(&point),
                "complete early baseline changed: {after}"
            );
        }
        assert!(
            !after["points"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| time(p["utc"].as_str().unwrap()).unwrap() == old_head),
            "affected old head must not remain a rate sample"
        );
        let overview = rig.core.read_subscription_overview().unwrap();
        assert_eq!(overview["plans"][0]["used"], (80 + step * 10) as f64);
        for kind in ["localPlanEta", "periodEndUsage"] {
            assert_eq!(
                overview["plans"][0]["forecasts"][kind]["state"], "conditional",
                "{overview}"
            );
            assert_eq!(
                overview["plans"][0]["forecasts"][kind]["notifyEligible"],
                false
            );
        }
        assert!(after["lastChange"].is_string());
        assert_eq!(rig.inbox_count(), 0);
    }
}

#[test]
fn subscription_late_append_does_not_hide_prior_fact_metadata_alias_or_exclusion_changes() {
    for modification in ["metadata", "alias", "exclusion"] {
        let (mut rig, id) = qualified_local_forecast_rig();
        let old = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
        let old_head = time(old["head"]["utc"].as_str().unwrap()).unwrap();
        rig.advance(179);
        let scan = rig.core.begin_subscription_scan().unwrap();
        rig.usage_at(
            "late-reject",
            10,
            true,
            Some("A"),
            Some("B"),
            timestamp(old_head - Duration::milliseconds(1)),
        );
        let existing: String = rig
            .core
            .db
            .query_row(
                "SELECT id FROM usage_responses ORDER BY rowid LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        match modification {
            "metadata" => {
                rig.core.db.execute("UPDATE usage_responses SET data=json_set(data,'$.providerId','changed-with-same-total') WHERE id=?1", [&existing]).unwrap();
            }
            "alias" => {
                rig.core.db.execute("INSERT INTO fact_aliases(alias,fact_id,evidence) VALUES('new-metadata-alias',?1,'changed')", [&existing]).unwrap();
            }
            "exclusion" => {
                rig.core
                    .db
                    .execute(
                        "INSERT INTO excluded_facts(fact_id,reason) VALUES(?1,'conflict')",
                        [&existing],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        rig.finish(scan);
        let after = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
        assert!(
            after["points"].as_array().unwrap().len() <= 1,
            "{modification}: {after}"
        );
        assert_ne!(
            rig.core.read_subscription_overview().unwrap()["plans"][0]["forecasts"]["localPlanEta"]
                ["state"],
            "conditional",
            "{modification}"
        );
        assert_eq!(rig.inbox_count(), 0);
    }
}

#[test]
fn subscription_late_append_rejects_replay_excess_lateness_and_fresh_alias_metadata() {
    for invalid in ["replay", "too-late", "fresh-alias"] {
        let (mut rig, id) = qualified_local_forecast_rig();
        let old = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
        let old_head = time(old["head"]["utc"].as_str().unwrap()).unwrap();
        rig.advance(179);
        let scan = rig.core.begin_subscription_scan().unwrap();
        let event = if invalid == "too-late" {
            rig.at.utc - Duration::milliseconds(15_001)
        } else {
            old_head - Duration::milliseconds(1)
        };
        rig.usage_at(
            "late-invalid",
            10,
            invalid != "replay",
            Some("A"),
            Some("B"),
            timestamp(event),
        );
        if invalid == "fresh-alias" {
            let fresh: String = rig
                .core
                .db
                .query_row(
                    "SELECT id FROM usage_responses ORDER BY rowid DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            rig.core.db.execute("INSERT INTO fact_aliases(alias,fact_id,evidence) VALUES('alias-on-fresh',?1,'associated')", [&fresh]).unwrap();
        }
        rig.finish(scan);
        let after = state(&rig.core.db, &id).unwrap()["localForecast"].clone();
        assert!(
            after["points"].as_array().unwrap().len() <= 1,
            "{invalid}: {after}"
        );
        assert_ne!(
            rig.core.read_subscription_overview().unwrap()["plans"][0]["forecasts"]["localPlanEta"]
                ["state"],
            "conditional",
            "{invalid}"
        );
        assert_eq!(rig.inbox_count(), 0);
    }
}

#[test]
fn subscription_request_model_fallback_swap_invalidates_same_total_and_cannot_be_hidden_by_fresh_append(
) {
    let mut rig = Rig::new();
    let mut input = rig.input();
    input["target"] = json!(10000);
    input["modelDimension"] = json!("request");
    input["model"] = json!("A");
    let plan = rig.core.save_usage_plan(&input).unwrap()["plans"][0].clone();
    let id = plan["id"].as_str().unwrap();
    for step in 1..=8 {
        rig.advance(10_000);
        let scan = rig.core.begin_subscription_scan().unwrap();
        rig.usage(&format!("fallback-{step}"), 10, true, Some("actual"), None);
        let turn = format!("fallback-turn-{step}");
        rig.core.db.execute("UPDATE usage_responses SET data=json_set(data,'$.turnId',?1) WHERE rowid=(SELECT MAX(rowid) FROM usage_responses)", [&turn]).unwrap();
        let model = if step % 2 == 1 { "A" } else { "B" };
        let data =
            json!({"id":turn,"sourceId":rig.core.source_id,"threadId":"thread","model":model});
        rig.core.db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread',?1,?2,?3)", params![turn,timestamp(rig.at.utc),data.to_string()]).unwrap();
        rig.finish(scan);
    }
    let original = rig.core.read_subscription_overview().unwrap();
    assert_eq!(original["plans"][0]["used"], 40.0);
    assert_eq!(
        original["plans"][0]["forecasts"]["localPlanEta"]["state"], "conditional",
        "{original}"
    );
    let old_head = time(
        state(&rig.core.db, id).unwrap()["localForecast"]["head"]["utc"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    rig.core
        .db
        .execute(
            "UPDATE turns SET data=json_set(data,'$.model','B') WHERE id='fallback-turn-1'",
            [],
        )
        .unwrap();
    rig.core
        .db
        .execute(
            "UPDATE turns SET data=json_set(data,'$.model','A') WHERE id='fallback-turn-8'",
            [],
        )
        .unwrap();
    let corrected = rig.core.read_subscription_overview().unwrap();
    assert_eq!(
        corrected["plans"][0]["used"], 40.0,
        "swap preserves the current model aggregate"
    );
    assert_ne!(
        corrected["plans"][0]["forecastLedgerHeadFingerprint"],
        original["plans"][0]["forecastLedgerHeadFingerprint"]
    );
    assert_eq!(
        corrected["plans"][0]["forecasts"]["localPlanEta"]["state"],
        "unavailable"
    );
    rig.advance(179);
    let scan = rig.core.begin_subscription_scan().unwrap();
    rig.usage_at(
        "late-after-model-swap",
        10,
        true,
        Some("actual"),
        Some("A"),
        timestamp(old_head - Duration::milliseconds(1)),
    );
    rig.finish(scan);
    let after = state(&rig.core.db, id).unwrap()["localForecast"].clone();
    assert_eq!(
        after["points"].as_array().unwrap().len(),
        1,
        "old per-model points were already changed: {after}"
    );
    let final_view = rig.core.read_subscription_overview().unwrap();
    assert_eq!(final_view["plans"][0]["used"], 50.0);
    assert_ne!(
        final_view["plans"][0]["forecasts"]["localPlanEta"]["state"],
        "conditional"
    );
    assert_eq!(rig.inbox_count(), 0);
}

fn subscription_crossing_with_pet_baseline() -> (Rig, Value, Value) {
    let mut rig = Rig::new();
    rig.core.subscription.native_test_qualified = true;
    rig.core.notification_gate.publish(true, false);
    rig.create();
    let scan = rig.core.begin_subscription_scan().unwrap();
    rig.finish(scan);
    let baseline = rig.core.pet_pulse(None).unwrap();
    rig.advance(1000);
    let scan = rig.core.begin_subscription_scan().unwrap();
    rig.usage("dual-channel", 85, true, None, None);
    rig.finish(scan);
    let event = rig.core.claim_notifications().unwrap().pop().unwrap();
    (rig, event, baseline["promptFeed"]["cursor"].clone())
}

#[test]
fn subscription_toast_completion_does_not_cancel_a_later_pet_pulse() {
    for status in ["sent", "shown", "failed"] {
        let (mut rig, event, cursor) = subscription_crossing_with_pet_baseline();
        let toast = rig.core.notification_permit(&event).unwrap();
        let id = event["id"].as_str().unwrap();
        rig.core
            .finish_notification(
                &[id.to_owned()],
                status,
                if status == "failed" {
                    Some("synthetic COM error")
                } else {
                    None
                },
            )
            .unwrap();
        assert!(
            !toast.is_current(),
            "toast completion must close its queued clones"
        );
        assert!(rig.core.notification_permit(&event).is_none());
        rig.advance(3000);
        let changes = rig.core.db.total_changes();
        let pulse = rig.core.pet_pulse(Some(cursor)).unwrap();
        assert_eq!(rig.core.db.total_changes(), changes);
        assert_eq!(
            pulse["prompts"].as_array().unwrap().len(),
            1,
            "{status}: {pulse}"
        );
        assert_eq!(pulse["prompts"][0]["id"], event["id"]);
        assert_eq!(pulse["prompts"][0]["kind"], "subscription_plan");
        assert_eq!(pulse["prompts"][0]["animationEligible"], true);
        let again = rig
            .core
            .pet_pulse(Some(pulse["promptFeed"]["cursor"].clone()))
            .unwrap();
        assert_eq!(
            again["prompts"],
            json!([]),
            "arrival cursor must not replay after {status}"
        );
        rig.core
            .action("acknowledge_delivery", &json!({"id":id}))
            .unwrap();
        assert!(!rig.core.domain_prompt_is_current("subscription_plan", id));
        assert!(!toast.is_current());
    }
}

#[test]
fn subscription_pet_acknowledgement_does_not_cancel_a_queued_toast() {
    let (mut rig, event, cursor) = subscription_crossing_with_pet_baseline();
    let toast = rig.core.notification_permit(&event).unwrap();
    let pulse = rig.core.pet_pulse(Some(cursor)).unwrap();
    assert_eq!(pulse["prompts"][0]["id"], event["id"]);
    let id = event["id"].as_str().unwrap();
    rig.core
        .action("acknowledge_delivery", &json!({"id":id}))
        .unwrap();
    assert!(!rig.core.domain_prompt_is_current("subscription_plan", id));
    assert!(toast.is_current());
    assert!(
        rig.core.notification_permit(&event).is_some(),
        "pet channel completion cannot consume toast authority"
    );
    rig.core
        .finish_notification(&[id.to_owned()], "sent", None)
        .unwrap();
    assert!(!toast.is_current());
    assert!(!rig.core.domain_prompt_is_current("subscription_plan", id));
}

#[test]
fn subscription_dual_channels_still_honor_suppression_expiry_and_host_boundaries() {
    for failure in ["suppressed", "pet-ttl", "host", "fact-correction"] {
        let (mut rig, event, cursor) = subscription_crossing_with_pet_baseline();
        let id = event["id"].as_str().unwrap();
        let toast = rig.core.notification_permit(&event).unwrap();
        if failure == "suppressed" {
            rig.core
                .finish_notification(&[id.to_owned()], "suppressed", None)
                .unwrap();
        } else {
            rig.core
                .finish_notification(&[id.to_owned()], "failed", Some("synthetic COM error"))
                .unwrap();
            match failure {
                "pet-ttl" => rig.advance(10_001),
                "host" => rig.core.notification_gate.publish(false, true),
                "fact-correction" => {
                    rig.core.db.execute("UPDATE usage_responses SET data=json_set(data,'$.providerId','same-total-correction')",[]).unwrap();
                }
                _ => unreachable!(),
            }
        }
        let pulse = rig.core.pet_pulse(Some(cursor)).unwrap();
        assert_eq!(pulse["prompts"], json!([]), "{failure}: {pulse}");
        assert!(!toast.is_current());
        let again = rig
            .core
            .pet_pulse(Some(pulse["promptFeed"]["cursor"].clone()))
            .unwrap();
        assert_eq!(again["prompts"], json!([]));
    }
}
