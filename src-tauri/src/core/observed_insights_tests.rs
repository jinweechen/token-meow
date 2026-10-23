use super::*;
use crate::budget_clock::{test_support::TestClock, Reading};
use std::io::Write;

struct Rig {
    _tmp: tempfile::TempDir,
    core: Core,
    path: PathBuf,
}
impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&tmp.path().join("data"), &source).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        let path = source.join("rollout-insights.jsonl");
        Self {
            _tmp: tmp,
            core,
            path,
        }
    }
    fn append(&self, v: Value) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .unwrap();
        writeln!(file, "{v}").unwrap();
    }
    fn event(&self, kind: &str, payload: Value) {
        self.append(json!({"timestamp":"2026-10-23T04:00:00Z","type":kind,"payload":payload}));
    }
    fn input(&self) {
        self.event(
            "session_meta",
            json!({"id":"insights-thread","cwd":"D:/fixture/project","thread_source":"user"}),
        );
        self.event("event_msg",json!({"type":"thread_settings_applied","thread_settings":{"model":"requested-a","model_provider_id":"provider","reasoning_effort":"medium","service_tier":"fast","cwd":"DO_NOT_KEEP_CONFIG_PATH","permission_profile":{"secret":"DO_NOT_KEEP_PERMISSIONS"}}}));
        self.event("turn_context",json!({"turn_id":"turn-1","model":"requested-a","effort":"medium","service_tier":"fast"}));
        self.event("compacted",json!({"window_id":"window-1","window_number":1,"message":"DO_NOT_KEEP_COMPACTION_BODY","replacement_history":[{"secret":"DO_NOT_KEEP_HISTORY"}]}));
        self.event("event_msg", json!({"type":"context_compacted"}));
        self.event(
            "response_item",
            json!({"type":"context_compaction","id":"projected-item"}),
        );
        self.event(
            "compacted",
            json!({"window_id":"window-1","message":"SECOND_BODY"}),
        );
        self.event("event_msg",json!({"type":"thread_settings_applied","thread_settings":{"model":"requested-a","model_provider_id":"provider","reasoning_effort":"low","service_tier":"fast"}}));
        self.event("token_usage_record",json!({"thread_id":"insights-thread","turn_id":"turn-1","response_id":"response-1","requested_model":"requested-a","observed_model":"actual-b","provider_id":"provider","usage":{"input_tokens":1000,"output_tokens":100,"total_tokens":1100,"cached_input_tokens":200,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}));
    }
    fn dashboard(&self, q: Value) -> Value {
        self.core
            .usage_dashboard_snapshot(q, json!({"page":0,"pageSize":20}))
            .unwrap()["usageDashboard"]["observedInsights"]
            .clone()
    }
}
fn day() -> Value {
    json!({"from":"2026-10-23","to":"2026-10-23"})
}

fn settings(model: &str) -> Value {
    json!({"type":"thread_settings_applied","thread_settings":{"model":model,"model_provider_id":"provider","reasoning_effort":"medium","service_tier":"fast"}})
}
fn settings_rows(view: &Value) -> Vec<Value> {
    view["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "settings_snapshot" || row["kind"] == "settings_change")
        .cloned()
        .collect()
}
fn utc_text(time: chrono::DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn range(from: chrono::DateTime<Utc>, to: chrono::DateTime<Utc>) -> Value {
    let zone = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    json!({"from":from.with_timezone(&zone).date_naive().to_string(),"to":to.with_timezone(&zone).date_naive().to_string()})
}
impl Rig {
    fn ordered_event(&self, ordinal: u64, kind: &str, payload: Value) {
        self.append(json!({"timestamp":"2026-10-23T04:00:00.000Z","ordinal":ordinal,"type":kind,"payload":payload}));
    }
    fn at_event(&self, at: chrono::DateTime<Utc>, ordinal: u64, kind: &str, payload: Value) {
        self.append(
            json!({"timestamp":utc_text(at),"ordinal":ordinal,"type":kind,"payload":payload}),
        );
    }
    fn drain(&mut self) {
        for _ in 0..40 {
            let report = self.core.scan().unwrap();
            if report.pending_files == 0 {
                return;
            }
        }
        panic!("自有合成JSONL未在有界轮数内读完");
    }
    fn stored(&self) -> Vec<Value> {
        json_rows(&self.core.db, "context_snapshots")
            .unwrap()
            .into_iter()
            .filter(|row| row["domain"] == DOMAIN)
            .collect()
    }
}

fn stored_response(
    r: &Rig,
    index: usize,
    turn: &str,
    requested: &str,
    observed: &str,
) -> ResponseUsage {
    let observation = adapter::Observation {
        source_id: r.core.source_id.clone(),
        channel_id: format!("{}:jsonl", r.core.source_id),
        event: json!({"timestamp":"2026-10-23T04:00:00.000Z","type":"token_usage_record","payload":{"thread_id":"thread","turn_id":turn,"response_id":format!("response-{index}"),
            "requested_model":requested,"observed_model":observed,"provider_id":"provider","usage":{"input_tokens":1000,"output_tokens":100,"total_tokens":1100,"cached_input_tokens":200,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}}),
    };
    let adapter::StandardEvent::ResponseUsage(mut response) = adapter::CodexAdapter
        .normalize(&observation)
        .unwrap()
        .remove(0)
    else {
        panic!()
    };
    // 仅本测试的明确合成条件，不经过IPC写入任何身份或厂商quota证明。
    response.account_scope_id = Some("synthetic-attested-account".into());
    adapter::store_response(&r.core.db, &response, false).unwrap();
    response
}

#[test]
fn real_file_scan_retains_whitelist_and_deduplicates_compaction_projections() {
    let mut r = Rig::new();
    r.input();
    r.core.scan().unwrap();
    let v = r.dashboard(day());
    let rows = v["timeline"].as_array().unwrap();
    assert_eq!(rows.iter().filter(|v| v["kind"] == "compaction").count(), 1);
    assert_eq!(
        rows.iter()
            .filter(|v| v["kind"] == "settings_snapshot")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|v| v["kind"] == "settings_change")
            .count(),
        1
    );
    assert_eq!(
        rows.iter().filter(|v| v["kind"] == "cache_usage").count(),
        1
    );
    assert_eq!(v["cache"]["inputTokens"], "1000");
    assert_eq!(v["cache"]["cachedInputTokens"], "200");
    assert_eq!(v["cache"]["cacheReadPercent"], 20.0);
    assert_eq!(v["daily"][0]["usage"]["responseCount"], 1);
    assert_eq!(v["comparisons"][0]["models"][0]["model"], "actual-b");
    assert_eq!(
        v["comparisons"][0]["comparable"], false,
        "日志未提供账户身份，不能补造相同账户条件"
    );
    let stored =
        serde_json::to_string(&json_rows(&r.core.db, "context_snapshots").unwrap()).unwrap();
    for secret in [
        "DO_NOT_KEEP_COMPACTION_BODY",
        "DO_NOT_KEEP_HISTORY",
        "DO_NOT_KEEP_CONFIG_PATH",
        "DO_NOT_KEEP_PERMISSIONS",
        "SECOND_BODY",
    ] {
        assert!(!stored.contains(secret));
    }
    r.core.scan().unwrap();
    assert_eq!(r.dashboard(day())["timeline"], v["timeline"]);
    println!("OBSERVED_INSIGHTS_CONTRACT_FIXTURE={v}");
}

#[test]
fn provider_filter_preserves_matching_standalone_settings_and_other_scope_guards() {
    let mut r = Rig::new();
    r.input();
    r.drain();
    let unfiltered = settings_rows(&r.dashboard(day()));
    assert_eq!(unfiltered.len(), 2);
    assert!(unfiltered.iter().all(|row| row["turnId"].is_null()));

    let mut query = day();
    query["providerId"] = json!("provider");
    assert_eq!(settings_rows(&r.dashboard(query.clone())), unfiltered,
        "独立配置事件应按自己的供应商匹配，不要求回合元数据");
    for (field, value) in [("providerId", "other-provider"), ("threadId", "other-thread"),
        ("sourceId", "other-source"), ("project", "other-project")] {
        let mut mismatch = query.clone();
        mismatch[field] = json!(value);
        assert!(settings_rows(&r.dashboard(mismatch)).is_empty(), "仍应遵守 {field} 范围");
    }
}

#[test]
fn privacy_scope_and_empty_day_do_not_leak_or_infer_zero_actual_use() {
    let mut r = Rig::new();
    r.input();
    r.core.scan().unwrap();
    let original_timeline = r.dashboard(day())["timeline"].clone();
    let mut q = day();
    q["threadId"] = json!("other-thread");
    assert!(r.dashboard(q)["timeline"].as_array().unwrap().is_empty());
    let empty = r.dashboard(json!({"from":"2026-10-24","to":"2026-10-24"}));
    assert_eq!(empty["cache"]["cacheReadPercent"], Value::Null);
    assert_eq!(empty["daily"][0]["usage"]["totalTokens"], Value::Null);
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    let private = r.dashboard(day());
    assert!(private["timeline"].as_array().unwrap().is_empty());
    assert!(private["comparisons"].as_array().unwrap().is_empty());
    assert_eq!(private["daily"][0]["usage"]["responseCount"], 1);
    let serialized = private.to_string();
    for identity in ["insights-thread", "actual-b", "requested-a", "window-1"] {
        assert!(
            !serialized.contains(identity),
            "隐私投影不能带事件或模型身份：{identity}"
        );
    }
    r.core
        .action("update_settings", &json!({"privacyMode":false}))
        .unwrap();
    assert_eq!(
        r.dashboard(day())["timeline"],
        original_timeline,
        "切回隐私不会改写历史事实"
    );
}

#[test]
fn incomplete_cache_denominator_stays_unknown_and_requested_model_is_not_actual() {
    let r = Rig::new();
    let o = adapter::Observation {
        source_id: r.core.source_id.clone(),
        channel_id: format!("{}:jsonl", r.core.source_id),
        event: json!({"timestamp":"2026-10-23T04:00:00Z","type":"token_usage_record","payload":{"thread_id":"thread","response_id":"response","requested_model":"only-requested","usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}}}),
    };
    let adapter::StandardEvent::ResponseUsage(response) =
        adapter::CodexAdapter.normalize(&o).unwrap().remove(0)
    else {
        panic!()
    };
    adapter::store_response(&r.core.db, &response, false).unwrap();
    let v = r.dashboard(day());
    assert_eq!(v["cache"]["knownResponses"], 0);
    assert_eq!(v["cache"]["unknownResponses"], 1);
    assert_eq!(v["cache"]["cachedInputTokens"], Value::Null);
    assert!(v["comparisons"].as_array().unwrap().is_empty());
}

#[test]
fn complete_conditions_require_five_samples_per_actual_model_and_preserve_turn_timing_scope() {
    let r = Rig::new();
    for index in 0..10 {
        let model = if index < 5 { "actual-a" } else { "actual-b" };
        let turn = format!("t-{index}");
        let o = adapter::Observation {
            source_id: r.core.source_id.clone(),
            channel_id: format!("{}:jsonl", r.core.source_id),
            event: json!({"timestamp":"2026-10-23T04:00:00Z","type":"token_usage_record","payload":{"thread_id":"thread","turn_id":turn,"response_id":format!("r-{index}"),"requested_model":"different-request","observed_model":model,"provider_id":"provider","usage":{"input_tokens":1000,"output_tokens":100,"total_tokens":1100,"cached_input_tokens":200,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}}),
        };
        let adapter::StandardEvent::ResponseUsage(mut response) =
            adapter::CodexAdapter.normalize(&o).unwrap().remove(0)
        else {
            panic!()
        };
        response.account_scope_id = Some("synthetic-attested-account".into());
        adapter::store_response(&r.core.db, &response, false).unwrap();
        let t = json!({"id":turn,"sourceId":r.core.source_id,"threadId":"thread","reasoningEffort":"medium","serviceTier":"fast","timeToFirstTokenMs":100,"durationMs":1000,"completedAt":"2026-10-23T04:00:00Z","status":"completed"});
        r.core.db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread',?2,'2026-10-23T04:00:00.000Z',?3)",params![format!("{}|{turn}",r.core.source_id),turn,t.to_string()]).unwrap();
    }
    let v = r.dashboard(day());
    let c = &v["comparisons"][0];
    assert_eq!(c["comparable"], true);
    assert_eq!(c["rank"], Value::Null);
    assert_eq!(c["models"].as_array().unwrap().len(), 2);
    for m in c["models"].as_array().unwrap() {
        assert_eq!(m["samples"], 5);
        assert_eq!(m["meanTurnTimeToFirstTokenMs"], 100.0);
    }
    let mut q = day();
    q["model"] = json!("different-request");
    let filtered = r.dashboard(q);
    assert_eq!(filtered["comparisons"], v["comparisons"]);
}

#[test]
fn official_full_snapshot_clears_omitted_options_but_missing_turn_effort_does_not_claim_a_global_change(
) {
    let mut r = Rig::new();
    r.ordered_event(0, "session_meta", json!({"id":"insights-thread"}));
    r.ordered_event(1, "event_msg", settings("a"));
    // 官方skip_serializing_if=None的真实形状：省略字段，不能用显式null代替验证。
    r.ordered_event(2,"event_msg",json!({"type":"thread_settings_applied","thread_settings":{"model":"a","model_provider_id":"provider"}}));
    r.ordered_event(
        3,
        "turn_context",
        json!({"turn_id":"turn-1","model":"a","effort":"high"}),
    );
    r.ordered_event(4, "turn_context", json!({"turn_id":"turn-2","model":"a"}));
    r.drain();
    let rows = settings_rows(&r.dashboard(day()));
    assert_eq!(rows.len(), 3);
    let clear = rows.iter().find(|row| row["ordinal"] == "2").unwrap();
    assert!(clear["condition"]["effort"].is_null() && clear["condition"]["speed"].is_null());
    for field in ["effort", "speed"] {
        assert!(clear["changedFields"]
            .as_array()
            .unwrap()
            .contains(&json!(field)));
    }
    assert!(
        rows.iter().all(|row| row["ordinal"] != "4"),
        "缺失回合字段不是全局配置变化证据"
    );
    assert_eq!(rows[0]["ordinal"], "3");
    assert_eq!(rows[0]["condition"]["effort"], "high");
    let previous_turn =
        adapter::load_fact(&r.core.db, "turns", &format!("{}|turn-1", r.core.source_id))
            .unwrap()
            .unwrap();
    let next_turn =
        adapter::load_fact(&r.core.db, "turns", &format!("{}|turn-2", r.core.source_id))
            .unwrap()
            .unwrap();
    assert_eq!(previous_turn["reasoningEffort"], "high");
    assert!(
        next_turn["reasoningEffort"].is_null(),
        "该回合条件未知，不能沿用上一回合的显式high"
    );
}

#[test]
fn explicit_null_or_invalid_turn_fields_are_unknown_for_that_turn_without_clearing_global_settings()
{
    let mut r = Rig::new();
    r.ordered_event(0, "session_meta", json!({"id":"insights-thread"}));
    r.ordered_event(1, "event_msg", settings("a"));
    r.ordered_event(2,"turn_context",json!({"turn_id":"null-turn","model":"a","effort":null,"reasoning_effort":null,"service_tier":null}));
    r.ordered_event(
        3,
        "turn_context",
        json!({"turn_id":"invalid-turn","model":"a","effort":42,"service_tier":[]}),
    );
    r.drain();
    let rows = settings_rows(&r.dashboard(day()));
    assert_eq!(
        rows.len(),
        1,
        "缺失、null、类型错误都不能冒充明确全局配置变化"
    );
    assert_eq!(rows[0]["condition"]["effort"], "medium");
    assert_eq!(rows[0]["condition"]["speed"], "fast");
    for id in ["null-turn", "invalid-turn"] {
        let turn = adapter::load_fact(&r.core.db, "turns", &format!("{}|{id}", r.core.source_id))
            .unwrap()
            .unwrap();
        assert!(
            turn["reasoningEffort"].is_null() && turn["serviceTier"].is_null(),
            "回合本身仍不得继承全局条件成为已知样本"
        );
    }
}

#[test]
fn same_millisecond_native_ordinals_preserve_a_b_a_and_sort_losslessly_descending() {
    let mut r = Rig::new();
    let base = 9_007_199_254_740_991_u64;
    r.ordered_event(base - 1, "session_meta", json!({"id":"insights-thread"}));
    for (delta, model) in [(0, "a"), (1, "b"), (2, "a")] {
        r.ordered_event(base + delta, "event_msg", settings(model));
    }
    r.ordered_event(u64::MAX,"compacted",json!({"window_id":"huge-window","window_number":u64::MAX,"message":"DO_NOT_STORE_ORDER_BODY"}));
    r.drain();
    let view = r.dashboard(day());
    let rows = settings_rows(&view);
    assert_eq!(
        rows.len(),
        3,
        "第三次A是新原生记录，不能被第一次A的配置hash吞掉"
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["condition"]["model"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a", "b", "a"]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["ordinal"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["9007199254740993", "9007199254740992", "9007199254740991"]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    let compact = view["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "compaction")
        .unwrap();
    assert_eq!(compact["windowNumber"], u64::MAX.to_string());
    assert_eq!(compact["ordinal"], u64::MAX.to_string());
    assert!(!serde_json::to_string(&r.stored())
        .unwrap()
        .contains("DO_NOT_STORE_ORDER_BODY"));
    let before = r.stored();
    r.drain();
    assert_eq!(r.stored(), before);
}

#[test]
fn absent_native_ordinal_uses_same_file_record_position_without_replay_duplicates() {
    let mut r = Rig::new();
    r.event("session_meta", json!({"id":"insights-thread"}));
    for model in ["a", "b", "a"] {
        r.event("event_msg", settings(model));
    }
    r.drain();
    let view = r.dashboard(day());
    let rows = settings_rows(&view);
    assert_eq!(rows.len(), 3);
    let stream = rows[0]["recordStream"].as_str().unwrap();
    assert_eq!(stream.len(), 64);
    assert!(rows
        .iter()
        .all(|row| row["ordinal"].is_null() && row["recordStream"] == stream));
    let positions: Vec<_> = rows
        .iter()
        .map(|row| {
            row["recordPosition"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] > pair[1]),
        "同文件降序只能使用原生记录位置"
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["condition"]["model"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a", "b", "a"]
    );
    for _ in 0..3 {
        r.drain();
        assert_eq!(r.dashboard(day())["timeline"], view["timeline"]);
    }
    for context in r
        .core
        .db
        .prepare("SELECT context FROM source_files")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
    {
        assert!(
            !context.unwrap().contains("record_position"),
            "当前记录位置不得被序列化为cursor事实"
        );
    }
}

#[test]
fn cache_timeline_mid_loop_crop_reports_truncated_for_exactly_513_responses() {
    let r = Rig::new();
    for index in 0..513 {
        stored_response(&r, index, "turn", "requested", "actual");
    }
    let view = r.dashboard(day());
    assert_eq!(view["timeline"].as_array().unwrap().len(), 256);
    assert_eq!(view["timelineTruncated"], true);
    assert_eq!(view["daily"][0]["usage"]["responseCount"], 513);
    assert_eq!(
        view["cache"]["knownResponses"], 513,
        "展示裁剪不许裁掉统计分母"
    );
    assert_eq!(view["cache"]["inputTokens"], "513000");
    assert_eq!(view["cache"]["cachedInputTokens"], "102600");
}

#[test]
fn filtered_model_cannot_turn_multiresponse_native_turn_into_a_single_timing_sample() {
    let r = Rig::new();
    for index in 0..5 {
        let turn = format!("multi-{index}");
        stored_response(&r, index * 2, &turn, "keep", "actual-a");
        stored_response(&r, index * 2 + 1, &turn, "other-request", "actual-b");
        let t = json!({"id":turn,"sourceId":r.core.source_id,"threadId":"thread","reasoningEffort":"medium","serviceTier":"fast","timeToFirstTokenMs":100,"status":"completed"});
        r.core.db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread',?2,'2026-10-23T04:00:00.000Z',?3)",params![format!("{}|{turn}",r.core.source_id),turn,t.to_string()]).unwrap();
    }
    let all = r.dashboard(day());
    assert_eq!(all["comparisons"][0]["comparable"], true);
    let mut query = day();
    query["model"] = json!("keep");
    let filtered = r.dashboard(query);
    assert_eq!(filtered["daily"][0]["usage"]["responseCount"], 5);
    let models = filtered["comparisons"][0]["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["model"], "actual-a");
    assert_eq!(models[0]["samples"], 5);
    assert_eq!(models[0]["timedSamples"], 0);
    assert!(
        models[0]["meanTurnTimeToFirstTokenMs"].is_null(),
        "筛选隐藏第二响应不能制造可归因回合计时"
    );
}

#[test]
fn clear_keeps_old_metadata_below_watermark_and_new_equal_settings_establish_a_fresh_baseline() {
    let mut r = Rig::new();
    let before = Utc::now() - Duration::hours(1);
    r.at_event(before, 0, "session_meta", json!({"id":"insights-thread"}));
    r.at_event(before, 1, "event_msg", settings("same"));
    r.at_event(
        before,
        2,
        "compacted",
        json!({"window_id":"old-window","window_number":1,"message":"DO_NOT_STORE_HISTORY"}),
    );
    r.drain();
    assert_eq!(r.stored().len(), 2);
    let cleared = r.core.action("clear_data", &json!({})).unwrap();
    assert!(r.stored().is_empty());
    fs::copy(
        &r.path,
        r.path.parent().unwrap().join("old-copy-after-clear.jsonl"),
    )
    .unwrap();
    r.drain();
    assert!(r.stored().is_empty(), "复制旧文件不许绕过clear水位");
    let after = chrono::DateTime::parse_from_rfc3339(cleared["watermark"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc)
        + Duration::milliseconds(1);
    r.at_event(after, 3, "event_msg", settings("same"));
    r.drain();
    let fresh = r.dashboard(range(before, after));
    let rows = settings_rows(&fresh);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "settings_snapshot");
    assert_eq!(rows[0]["ordinal"], "3");
    r.core
        .action(
            "import_history",
            &json!({"from":utc_text(before-Duration::seconds(1))}),
        )
        .unwrap();
    r.drain();
    assert_eq!(
        r.stored()
            .iter()
            .filter(|row| row["kind"] == "compaction" && row["windowId"] == "old-window")
            .count(),
        1,
        "显式历史补导可以恢复旧metadata但不能重复压缩"
    );
    let restored = r.stored();
    r.drain();
    assert_eq!(r.stored(), restored);
}

#[test]
fn retention_watermark_survives_wider_preference_and_only_explicit_history_restores_metadata() {
    let current = Utc::now();
    let _clock = TestClock::new(Reading {
        utc: current,
        mono_ms: 100_000,
    });
    let mut r = Rig::new();
    let old = current - Duration::days(10);
    let recent = current - Duration::days(1);
    r.at_event(old, 0, "session_meta", json!({"id":"insights-thread"}));
    r.at_event(old, 1, "event_msg", settings("old"));
    r.at_event(
        old,
        2,
        "compacted",
        json!({"window_id":"retained-old","window_number":1}),
    );
    r.at_event(recent, 3, "event_msg", settings("recent"));
    r.at_event(
        recent,
        4,
        "compacted",
        json!({"window_id":"retained-recent","window_number":2}),
    );
    r.drain();
    assert_eq!(r.stored().len(), 4);
    r.core
        .action("update_settings", &json!({"retentionDays":7}))
        .unwrap();
    r.core.action("apply_retention", &json!({})).unwrap();
    assert_eq!(r.stored().len(), 2);
    let boundary = get_setting(&r.core.db, "retentionWatermark").unwrap();
    r.core
        .action("update_settings", &json!({"retentionDays":365}))
        .unwrap();
    fs::copy(
        &r.path,
        r.path
            .parent()
            .unwrap()
            .join("old-copy-after-retention.jsonl"),
    )
    .unwrap();
    r.drain();
    assert_eq!(
        get_setting(&r.core.db, "retentionWatermark").unwrap(),
        boundary
    );
    assert_eq!(r.stored().len(), 2);
    r.core
        .action(
            "import_history",
            &json!({"from":utc_text(old-Duration::seconds(1))}),
        )
        .unwrap();
    r.drain();
    assert_eq!(
        r.stored().len(),
        4,
        "只有显式导入才能恢复已清理的两个旧metadata事件"
    );
    let view = r.dashboard(range(old, current));
    assert_eq!(
        view["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["windowId"] == "retained-old")
            .count(),
        1
    );
    let restored = r.stored();
    r.drain();
    assert_eq!(r.stored(), restored);
}
