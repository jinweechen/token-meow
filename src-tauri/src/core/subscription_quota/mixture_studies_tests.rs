use super::*;
use crate::budget_clock::test_support::TestClock;

struct Rig {
    tmp: tempfile::TempDir,
    core: Core,
    clock: TestClock,
    at: Reading,
}
impl Rig {
    fn new() -> Self {
        let at = Reading {
            utc: time("2026-10-23T08:00:00Z").unwrap(),
            mono_ms: 200_000,
        };
        let clock = TestClock::new(at);
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&tmp.path().join("data"), &source).unwrap();
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
            .read_mixture_studies(&self.core.source_id)
            .unwrap()
    }
    fn input(&self) -> Value {
        let samples=(0..12).map(|i| {
            let a=(i%3+1)*100_000_u64;let b=((i*7)%5+1)*50_000_u64;let start=self.at.utc-Duration::hours(24)+Duration::minutes((i*2) as i64);
            json!({"id":uuid::Uuid::new_v4().to_string(),"fromAt":timestamp(start),"toAt":timestamp(start+Duration::minutes(1)),"deltaPoints":2.0*a as f64/1_000_000.0+5.0*b as f64/1_000_000.0,"tokens":[a.to_string(),b.to_string()]})
        }).collect::<Vec<_>>();
        json!({"sourceId":self.core.source_id,"expectedScope":self.overview()["scopeToken"],"expectedRevision":null,"config":{"title":"合成多模型条件实验",
            "condition":{"accountLabel":"合成账户","windowLabel":"合成窗口","effort":"固定","speed":"标准"},"models":["model-a","model-b"],"samples":samples,
            "rolling":{"asOf":timestamp(self.at.utc),"historyCompleteFrom":timestamp(self.at.utc-Duration::minutes(60)),"capacityPoints":10.0,"requiredPoints":2.0,"windowMinutes":60,
                "charges":[{"id":uuid::Uuid::new_v4().to_string(),"occurredAt":timestamp(self.at.utc-Duration::minutes(30)),"kind":"points","amountPoints":3.0},
                    {"id":uuid::Uuid::new_v4().to_string(),"occurredAt":timestamp(self.at.utc+Duration::minutes(15)),"kind":"models","tokens":["100000","100000"]}]}}})
    }
    fn save(&mut self) -> Value {
        self.core.save_mixture_study(&self.input()).unwrap()
    }
    fn edit(&self, study: &Value) -> Value {
        json!({"sourceId":self.core.source_id,"expectedScope":self.overview()["scopeToken"],"id":study["id"],"expectedRevision":study["revision"],"config":study["config"]})
    }
    fn delete(&self, study: &Value) -> Value {
        json!({"sourceId":self.core.source_id,"expectedScope":self.overview()["scopeToken"],"id":study["id"],"expectedRevision":study["revision"]})
    }
}
fn close(value: &Value, wanted: f64) {
    assert!(
        (value.as_f64().unwrap() - wanted).abs() < 1e-5,
        "{value} != {wanted}"
    );
}

#[test]
fn actual_core_manual_fit_then_model_charge_then_complete_release_contract() {
    let mut r = Rig::new();
    let view = r.save();
    let study = &view["studies"][0];
    let fit = &study["projection"]["fit"];
    assert_eq!(fit["kind"], "separated");
    close(&fit["weights"][0]["pointsPerMillionTokens"], 2.0);
    close(&fit["weights"][1]["pointsPerMillionTokens"], 5.0);
    assert_eq!(fit["trainSamples"], 9);
    assert_eq!(fit["holdoutSamples"], 3);
    let rolling = &study["projection"]["rolling"];
    close(&rolling["points"][0]["usedPoints"], 3.0);
    let future = rolling["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["at"] == timestamp(r.at.utc + Duration::minutes(15)))
        .unwrap();
    close(&future["usedPoints"], 3.7);
    close(
        &rolling["points"].as_array().unwrap().last().unwrap()["usedPoints"],
        0.0,
    );
    assert_eq!(
        rolling["endsAt"],
        timestamp(r.at.utc + Duration::minutes(120))
    );
    for value in [&study["projection"], fit, rolling] {
        assert_eq!(value["quality"], "conditional-manual");
        assert_eq!(value["constraintsVerified"], false);
        assert_eq!(value["notifyEligible"], false);
        assert_eq!(value["uncertaintyBounds"], Value::Null);
    }
    assert!(latest_plans(&r.core.db).unwrap().is_empty());
    let calibrations: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM quota_calibrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(calibrations, 0);
    println!("MIXTURE_STUDY_CONTRACT_FIXTURE={view}");
}

#[test]
fn read_and_restart_do_not_move_frozen_simulation_or_write_data() {
    let mut r = Rig::new();
    let v = r.save();
    let projection = v["studies"][0]["projection"].clone();
    let changes = r.core.db.total_changes();
    r.advance(4 * 3600);
    assert_eq!(r.overview()["studies"][0]["projection"], projection);
    assert_eq!(r.core.db.total_changes(), changes);
    let source = r.tmp.path().join("source");
    r.core = Core::open(&r.tmp.path().join("data"), &source).unwrap();
    assert_eq!(r.overview()["studies"][0]["bindingState"], "ready");
    assert_eq!(r.overview()["studies"][0]["projection"], projection);
    let study = r.overview()["studies"][0].clone();
    let deleted = r.core.delete_mixture_study(&r.delete(&study)).unwrap();
    assert!(deleted["studies"].as_array().unwrap().is_empty());
    assert!(r.core.delete_mixture_study(&r.delete(&study)).is_err());
}

#[test]
fn collinear_models_never_price_arbitrary_model_charge_but_points_still_simulate() {
    let mut r = Rig::new();
    let mut p = r.input();
    for sample in p["config"]["samples"].as_array_mut().unwrap() {
        sample["tokens"] = json!(["100000", "100000"]);
        sample["deltaPoints"] = json!(0.7);
    }
    let v = r.core.save_mixture_study(&p).unwrap();
    let study = &v["studies"][0];
    assert_ne!(study["projection"]["fit"]["kind"], "separated");
    assert_eq!(study["projection"]["fit"]["weights"], Value::Null);
    let points = study["projection"]["rolling"]["points"].as_array().unwrap();
    assert!(points.iter().any(|p| p["usedPoints"].is_null()));
    close(&points[0]["usedPoints"], 3.0);
    close(&points.last().unwrap()["usedPoints"], 0.0);
    let mut edit = r.edit(study);
    edit["config"]["rolling"]["charges"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["kind"] == "points");
    let v = r.core.save_mixture_study(&edit).unwrap();
    assert!(v["studies"][0]["projection"]["rolling"]["points"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["usedPoints"].is_number()));
}

#[test]
fn unknown_history_is_not_zero_and_clears_only_after_assumed_window() {
    let mut r = Rig::new();
    let mut p = r.input();
    p["config"]["rolling"]["historyCompleteFrom"] = Value::Null;
    p["config"]["rolling"]["charges"] = json!([]);
    let v = r.core.save_mixture_study(&p).unwrap();
    let rolling = &v["studies"][0]["projection"]["rolling"];
    assert_eq!(rolling["points"][0]["usedPoints"], Value::Null);
    assert_eq!(rolling["points"][0]["remainingPoints"], Value::Null);
    assert_eq!(
        rolling["nextAvailableAt"],
        timestamp(r.at.utc + Duration::minutes(60))
    );
    let at = rolling["points"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["at"] == timestamp(r.at.utc + Duration::minutes(60)))
        .unwrap();
    close(&at["usedPoints"], 0.0);
}

#[test]
fn stale_scope_revision_source_aba_and_explicit_rebind_are_enforced() {
    let mut r = Rig::new();
    let v = r.save();
    let study = v["studies"][0].clone();
    let stale = r.edit(&study);
    let root: String = r
        .core
        .db
        .query_row(
            "SELECT root FROM source_instances WHERE id=?1",
            [&r.core.source_id],
            |row| row.get(0),
        )
        .unwrap();
    let other = r.tmp.path().join("source-b");
    fs::create_dir(&other).unwrap();
    r.core
        .action(
            "update_source",
            &json!({"id":r.core.source_id,"path":other,"enabled":true}),
        )
        .unwrap();
    r.core
        .action(
            "update_source",
            &json!({"id":r.core.source_id,"path":root,"enabled":true}),
        )
        .unwrap();
    assert!(r.core.save_mixture_study(&stale).is_err());
    let v = r.overview();
    assert_eq!(v["studies"][0]["bindingState"], "pending_rebind");
    assert_eq!(v["studies"][0]["projection"]["fit"]["kind"], "unavailable");
    let points = v["studies"][0]["projection"]["rolling"]["points"]
        .as_array()
        .unwrap();
    assert_eq!(points.len(), 2);
    assert!(points.iter().all(|point| point["usedPoints"].is_null()
        && point["remainingPoints"].is_null()
        && point["available"].is_null()));
    println!("MIXTURE_STUDY_PENDING_CONTRACT_FIXTURE={v}");
    let edit = r.edit(&study);
    let v = r.core.save_mixture_study(&edit).unwrap();
    assert_eq!(v["studies"][0]["bindingState"], "ready");
    assert_eq!(v["studies"][0]["configurationRevision"], "2");
    assert!(r.core.save_mixture_study(&edit).is_err());
    assert!(r.core.delete_mixture_study(&r.delete(&study)).is_err());
}

#[test]
fn privacy_hides_all_authored_data_and_rejects_even_fresh_scope_writes() {
    let mut r = Rig::new();
    let v = r.save();
    let study = &v["studies"][0];
    let stale = r.edit(study);
    let delete = r.delete(study);
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    let v = r.overview();
    assert!(v["studies"].as_array().unwrap().is_empty());
    assert!(v["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("privacy_mode")));
    assert!(!v.to_string().contains("model-a"));
    assert!(r.core.save_mixture_study(&stale).is_err());
    let mut fresh = stale;
    fresh["expectedScope"] = v["scopeToken"].clone();
    assert!(r.core.save_mixture_study(&fresh).is_err());
    let mut delete = delete;
    delete["expectedScope"] = v["scopeToken"].clone();
    assert!(r.core.delete_mixture_study(&delete).is_err());
}

#[test]
fn clear_and_failed_transactions_remove_or_preserve_exact_book_atomically() {
    let mut r = Rig::new();
    put_setting(&r.core.db, "testMixtureStudyFailBeforeCommit", &json!(true)).unwrap();
    assert!(r.core.save_mixture_study(&r.input()).is_err());
    assert!(get_setting(&r.core.db, KEY).unwrap().is_none());
    put_setting(
        &r.core.db,
        "testMixtureStudyFailBeforeCommit",
        &json!(false),
    )
    .unwrap();
    let v = r.save();
    let old = get_setting(&r.core.db, KEY).unwrap();
    let delete = r.delete(&v["studies"][0]);
    put_setting(&r.core.db, "testMixtureStudyFailBeforeCommit", &json!(true)).unwrap();
    assert!(r.core.delete_mixture_study(&delete).is_err());
    assert_eq!(get_setting(&r.core.db, KEY).unwrap(), old);
    put_setting(
        &r.core.db,
        "testMixtureStudyFailBeforeCommit",
        &json!(false),
    )
    .unwrap();
    let stale = r.edit(&v["studies"][0]);
    // 在 clear_data 晚段 settings 删除失败，已经清掉的研究键必须一并回滚。
    r.core.db.execute_batch("CREATE TRIGGER fail_mixture_clear BEFORE DELETE ON settings WHEN OLD.key='mixtureStudiesV1' BEGIN SELECT RAISE(ABORT,'injected-late-clear'); END;").unwrap();
    assert!(r.core.action("clear_data", &json!({})).is_err());
    assert_eq!(get_setting(&r.core.db, KEY).unwrap(), old);
    r.core
        .db
        .execute_batch("DROP TRIGGER fail_mixture_clear;")
        .unwrap();
    r.core.action("clear_data", &json!({})).unwrap();
    assert!(r.overview()["studies"].as_array().unwrap().is_empty());
    assert!(get_setting(&r.core.db, KEY).unwrap().is_none());
    assert!(r.core.save_mixture_study(&stale).is_err());
}

#[test]
fn typed_contract_rejects_forged_proof_missing_tokens_overlap_and_future_samples() {
    let mut r = Rig::new();
    let mut p = r.input();
    p["config"]["verified"] = json!(true);
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["samples"][0]["tokens"] = json!(["100000"]);
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["samples"][0]["tokens"] = json!(["00", "1"]);
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["samples"][1]["fromAt"] = p["config"]["samples"][0]["fromAt"].clone();
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["samples"][11]["toAt"] = json!(timestamp(r.at.utc + Duration::seconds(1)));
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["rolling"]["historyCompleteFrom"] =
        json!(timestamp(r.at.utc + Duration::seconds(1)));
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["rolling"]["charges"][1]["occurredAt"] =
        json!(timestamp(r.at.utc + Duration::minutes(61)));
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["rolling"]["charges"][1]["id"] = p["config"]["samples"][0]["id"].clone();
    assert!(r.core.save_mixture_study(&p).is_err());
}

#[test]
fn model_and_point_limits_and_small_sample_unavailable_are_explicit() {
    let mut r = Rig::new();
    let mut p = r.input();
    p["config"]["models"] = json!(["a", "b", "c", "d"]);
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["rolling"]["capacityPoints"] = json!(1_000_000_001.0);
    assert!(r.core.save_mixture_study(&p).is_err());
    let mut p = r.input();
    p["config"]["samples"] = json!([]);
    p["config"]["rolling"]["charges"] = json!([]);
    let v = r.core.save_mixture_study(&p).unwrap();
    assert_eq!(v["studies"][0]["projection"]["fit"]["kind"], "unavailable");
    assert_eq!(v["studies"][0]["projection"]["fit"]["weights"], Value::Null);
}

#[test]
fn clock_rollback_blocks_revisions_and_result_without_moving_frozen_asof() {
    let mut r = Rig::new();
    let v = r.save();
    let edit = r.edit(&v["studies"][0]);
    let as_of = v["studies"][0]["config"]["rolling"]["asOf"].clone();
    r.advance(-1);
    let v = r.overview();
    assert_eq!(v["studies"][0]["projection"]["fit"]["kind"], "unavailable");
    assert_eq!(v["studies"][0]["config"]["rolling"]["asOf"], as_of);
    assert!(r.core.save_mixture_study(&edit).is_err());
}

#[test]
fn too_large_predicted_model_charge_is_unknown_without_losing_valid_fit_or_points() {
    let mut r = Rig::new();
    let mut p = r.input();
    p["config"]["models"] = json!(["model-a"]);
    for (i, sample) in p["config"]["samples"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        let n = (i % 4 + 1) as u64;
        sample["tokens"] = json!([n.to_string()]);
        sample["deltaPoints"] = json!(n as f64 * 100_000_000.0);
    }
    p["config"]["rolling"]["charges"][1]["tokens"] = json!(["9007199254740991"]);
    let v = r.core.save_mixture_study(&p).unwrap();
    let study = &v["studies"][0];
    assert_eq!(study["projection"]["fit"]["kind"], "separated");
    let rolling = &study["projection"]["rolling"];
    assert!(rolling["reasons"]
        .as_array()
        .unwrap()
        .contains(&json!("prediction_out_of_range")));
    close(&rolling["points"][0]["usedPoints"], 3.0);
    assert!(rolling["points"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["usedPoints"].is_null()));
    close(
        &rolling["points"].as_array().unwrap().last().unwrap()["usedPoints"],
        0.0,
    );
}
