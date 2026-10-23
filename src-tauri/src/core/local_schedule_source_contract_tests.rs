//! Stage5 日程与旧计划、来源配置和清除入口的真实 Core 集成合同。
use super::*;
use crate::budget_clock::{test_support::TestClock, Reading};

struct ScheduleRig {
    core: Core,
    source: PathBuf,
    temp: tempfile::TempDir,
    clock: TestClock,
    at: Reading,
}
impl ScheduleRig {
    fn new() -> Self {
        let at = Reading {
            utc: chrono::DateTime::parse_from_rfc3339("2026-10-23T04:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            mono_ms: 100_000,
        };
        let clock = TestClock::new(at);
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("data"), &source).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        let mut rig = Self {
            core,
            source,
            temp,
            clock,
            at,
        };
        // 真实扫描自有空目录建立本地完整性；不写官方账户、身份或额度证明。
        rig.scan();
        rig
    }
    fn scan(&mut self) {
        self.core.scan().unwrap();
    }
    fn set_time(&mut self, at: chrono::DateTime<Utc>) {
        let elapsed = (at - self.at.utc).num_milliseconds();
        assert!(elapsed >= 0);
        self.at = Reading {
            utc: at,
            mono_ms: self.at.mono_ms.checked_add(elapsed as u64).unwrap(),
        };
        self.clock.set(self.at);
    }
    fn advance(&mut self, seconds: i64) {
        self.set_time(self.at.utc + Duration::seconds(seconds));
    }
    fn read(&self) -> Value {
        self.core
            .read_local_schedules(&self.core.source_id)
            .unwrap()
    }
    fn input(&self, unit: &str) -> Value {
        json!({"expectedRevision":null,"expectedScope":self.read()["scopeToken"],"sourceId":self.core.source_id,
            "title":format!("合成{unit}周期"),"unit":unit,"model":null,"modelDimension":"response",
            "rule":{"kind":"daily","anchorAt":"2026-10-22T16:00:00Z","endsAt":null,"timezone":"+08:00"},
            "startPolicy":"now","target":"100","autoRenew":true,"allocation":[],"remindersEnabled":false})
    }
    fn create(&mut self, unit: &str) -> Value {
        self.core.save_local_schedule(&self.input(unit)).unwrap()
    }
    fn plan(&self, id: &str) -> String {
        self.core
            .db
            .query_row(
                "SELECT data FROM usage_plans WHERE plan_id=?1 ORDER BY revision DESC LIMIT 1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
    }
}
fn instance(view: &Value, unit: &str) -> Value {
    view["instances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["unit"] == unit)
        .unwrap()
        .clone()
}
fn has_reason(value: &Value, reason: &str) -> bool {
    value["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some(reason))
}
type Rows = Vec<(String, Vec<Vec<rusqlite::types::Value>>)>;
fn rows(core: &Core, tables: &[&str]) -> Rows {
    tables
        .iter()
        .map(|table| {
            // 表名均为本测试固定列表；SQL 值逐一比较，避免只比较计数漏掉覆写。
            let mut query = core
                .db
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let columns = query.column_count();
            let values = query
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| row.get(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (table.to_string(), values)
        })
        .collect()
}

#[test]
fn generated_token_plan_is_canonical_hidden_from_legacy_overview_and_cannot_be_edited_there() {
    let mut rig = ScheduleRig::new();
    let ordinary=rig.core.save_usage_plan(&json!({"expectedRevision":null,"title":"原有独立计划","type":"tokens",
        "sourceId":rig.core.source_id,"model":null,"modelDimension":"response","target":50,
        "endPolicy":"time","endsAt":(rig.at.utc+Duration::hours(3)).to_rfc3339(),"windowId":null,"remindersEnabled":false})).unwrap();
    let ordinary_id = ordinary["plans"][0]["id"].as_str().unwrap().to_owned();
    let local = rig.create("tokens");
    let generated = instance(&local, "tokens");
    let plan_id = generated["tokenPlanId"].as_str().unwrap();
    let canonical: Value = serde_json::from_str(&rig.plan(plan_id)).unwrap();
    assert_eq!(canonical["origin"]["kind"], "local_schedule");
    assert_eq!(canonical["origin"]["instanceId"], generated["id"]);
    assert_eq!(canonical["origin"]["sourceConfigurationRevision"], "0");
    assert_eq!(canonical["target"], 100);
    assert_eq!(canonical["effectiveFrom"], generated["effectiveFrom"]);
    assert_eq!(canonical["endsAt"], generated["periodEndsAt"]);
    let overview = rig.core.read_subscription_overview().unwrap();
    assert_eq!(overview["plans"].as_array().unwrap().len(), 1);
    assert_eq!(overview["plans"][0]["id"], ordinary_id);
    let tables = [
        "usage_plans",
        "plan_period_state",
        "local_schedule_templates",
        "local_schedule_instances",
    ];
    let original = rows(&rig.core, &tables);
    let edit=rig.core.save_usage_plan(&json!({"id":plan_id,"expectedRevision":canonical["revision"],"title":"不能在旧入口改写",
        "type":"tokens","sourceId":rig.core.source_id,"model":null,"modelDimension":"response","target":999,
        "endPolicy":"time","endsAt":canonical["endsAt"],"windowId":null,"remindersEnabled":false})).unwrap_err();
    assert!(edit.contains("周期模板"), "必须由 origin 保护拒绝：{edit}");
    let end = rig
        .core
        .set_usage_plan_status(
            &json!({"id":plan_id,"expectedRevision":canonical["revision"],"end":true}),
        )
        .unwrap_err();
    assert!(end.contains("周期模板"), "必须由 origin 保护拒绝：{end}");
    assert_eq!(
        rows(&rig.core, &tables),
        original,
        "两个旧入口失败不得修改旧计划或生成实例的 SQL 值"
    );
    assert!(rig.core.read_subscription_overview().unwrap()["plans"]
        .as_array()
        .unwrap()
        .iter()
        .all(|plan| plan["id"] != plan_id));
}

#[test]
fn source_a_b_a_invalidates_old_generated_plan_and_explicit_confirmation_only_creates_future_period(
) {
    let mut rig = ScheduleRig::new();
    let source_id = rig.core.source_id.clone();
    let created = rig.create("tokens");
    let old = instance(&created, "tokens");
    let old_plan = old["tokenPlanId"].as_str().unwrap().to_owned();
    let original_plan = rig.plan(&old_plan);
    rig.advance(1);
    rig.scan();
    assert_eq!(
        rig.core.test_read_local_usage_plan(&old_plan).unwrap()["used"],
        0.0
    );
    let b = rig.temp.path().join("source-b");
    fs::create_dir(&b).unwrap();
    rig.core
        .action("update_source", &json!({"id":source_id,"path":b}))
        .unwrap();
    rig.core
        .action("update_source", &json!({"id":source_id,"path":rig.source}))
        .unwrap();
    assert_eq!(
        rig.core.primary_source().unwrap()["configurationRevision"],
        "2"
    );
    let pending = rig.read();
    assert_eq!(pending["templates"][0]["bindingState"], "pending_rebind");
    assert!(has_reason(&instance(&pending, "tokens"), "pending_rebind"));
    let plan = rig.core.test_read_local_usage_plan(&old_plan).unwrap();
    for field in ["used", "remaining", "progressPercent"] {
        assert!(
            plan[field].is_null(),
            "{field} 不能把旧绑定显示成零或当前事实"
        );
    }
    assert!(has_reason(&plan, "source_binding_changed"));
    let old_end = chrono::DateTime::parse_from_rfc3339(old["periodEndsAt"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    rig.set_time(old_end + Duration::seconds(1));
    rig.scan();
    rig.scan();
    assert_eq!(
        rig.read()["instances"].as_array().unwrap().len(),
        1,
        "旧绑定即使跨周期扫描也不能自动续建"
    );
    let template = rig.read()["templates"][0].clone();
    let mut edit = rig.input("tokens");
    edit["id"] = template["id"].clone();
    edit["expectedRevision"] = template["revision"].clone();
    edit["target"] = json!("250");
    edit["startPolicy"] = json!("next_period");
    let confirmed = rig.core.save_local_schedule(&edit).unwrap();
    assert_eq!(confirmed["templates"][0]["bindingState"], "ready");
    assert_eq!(confirmed["templates"][0]["configurationRevision"], "2");
    assert_eq!(
        confirmed["instances"].as_array().unwrap().len(),
        1,
        "确认不能追溯生成当前或过去实例"
    );
    assert_eq!(
        rig.plan(&old_plan),
        original_plan,
        "旧实例的目标、起止与来源修订保持原 SQL JSON"
    );
    let first = chrono::DateTime::parse_from_rfc3339(
        confirmed["templates"][0]["firstStartsAt"].as_str().unwrap(),
    )
    .unwrap()
    .with_timezone(&Utc);
    assert!(first > rig.at.utc);
    rig.set_time(first + Duration::seconds(1));
    rig.scan();
    let final_view = rig.read();
    let instances = final_view["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 2);
    let new = instances
        .iter()
        .find(|value| value["id"] != old["id"])
        .unwrap();
    assert_eq!(new["target"], "250");
    assert_eq!(
        new["periodStartsAt"],
        confirmed["templates"][0]["firstStartsAt"]
    );
    let new_plan: Value =
        serde_json::from_str(&rig.plan(new["tokenPlanId"].as_str().unwrap())).unwrap();
    assert_eq!(new_plan["origin"]["sourceConfigurationRevision"], "2");
    let old_view = instances
        .iter()
        .find(|value| value["id"] == old["id"])
        .unwrap();
    assert!(old_view["used"].is_null() && has_reason(old_view, "pending_rebind"));
    assert_eq!(rig.plan(&old_plan), original_plan);
}

#[test]
fn clear_data_rolls_back_schedules_and_canonical_plans_on_failure_then_removes_everything_without_revival(
) {
    let mut rig = ScheduleRig::new();
    rig.create("tokens");
    let manual = instance(&rig.create("requests"), "requests");
    rig.advance(1);
    let consume = uuid::Uuid::new_v4().to_string();
    rig.core.record_local_consumption(&json!({"sourceId":rig.core.source_id,"instanceId":manual["id"],
        "expectedScope":rig.read()["scopeToken"],"idempotencyKey":consume,"occurredAt":rig.at.utc.to_rfc3339(),"amount":"7","note":"合成清除回滚样本"})).unwrap();
    rig.core.void_local_consumption(&json!({"sourceId":rig.core.source_id,"entryId":consume,"expectedRevision":1,
        "expectedScope":rig.read()["scopeToken"],"idempotencyKey":uuid::Uuid::new_v4().to_string()})).unwrap();
    let tables = [
        "local_manual_consumption",
        "local_schedule_instances",
        "local_schedule_templates",
        "usage_plans",
        "plan_period_state",
        "settings",
    ];
    let original = rows(&rig.core, &tables);
    assert!(get_setting(&rig.core.db, "localSchedulesRevision")
        .unwrap()
        .is_some());
    assert!(get_setting(&rig.core.db, "localSchedulesLastUtc")
        .unwrap()
        .is_some());
    rig.core.db.execute_batch("CREATE TRIGGER controlled_local_template_clear_failure BEFORE DELETE ON local_schedule_templates BEGIN SELECT RAISE(ABORT,'controlled-local-template-clear-failure'); END;").unwrap();
    let error = rig.core.action("clear_data", &json!({})).unwrap_err();
    assert!(
        error.contains("controlled-local-template-clear-failure"),
        "{error}"
    );
    assert_eq!(
        rows(&rig.core, &tables),
        original,
        "手动consume/void、实例、模板、canonical计划与settings须同事务回滚"
    );
    rig.core
        .db
        .execute_batch("DROP TRIGGER controlled_local_template_clear_failure;")
        .unwrap();
    rig.core.action("clear_data", &json!({})).unwrap();
    let assert_empty = |core: &Core| {
        for table in tables.iter().copied().filter(|table| *table != "settings") {
            assert_eq!(
                core.db
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0,
                "{table}"
            );
        }
        assert!(get_setting(&core.db, "localSchedulesRevision")
            .unwrap()
            .is_none());
        assert!(get_setting(&core.db, "localSchedulesLastUtc")
            .unwrap()
            .is_none());
    };
    assert_empty(&rig.core);
    rig.scan();
    rig.advance(3 * 86_400);
    rig.scan();
    rig.scan();
    assert_empty(&rig.core);
    let view = rig.read();
    assert!(
        view["templates"].as_array().unwrap().is_empty()
            && view["instances"].as_array().unwrap().is_empty()
    );
    assert!(rig.core.read_subscription_overview().unwrap()["plans"]
        .as_array()
        .unwrap()
        .is_empty());
}
