use super::*;
use crate::core::adapter::{self, CodexAdapter, Observation, SourceAdapter, StandardEvent};

struct Fixture {
    _tmp: tempfile::TempDir,
    core: Core,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("source");
        std::fs::create_dir(&root).unwrap();
        let core = Core::open(&tmp.path().join("data"), &root).unwrap();
        Self { _tmp: tmp, core }
    }

    fn response(&self, id: &str, thread: &str, usage: Value) -> ResponseUsage {
        let observation = Observation {
            source_id: self.core.source_id.clone(),
            channel_id: format!("{}:jsonl", self.core.source_id),
            event: json!({"timestamp":now(),"type":"token_usage_record","payload":{
                "thread_id":thread,"response_id":id,"turn_id":format!("turn-{id}"),
                "requested_model":"aggregation-test-model","usage":usage
            }}),
        };
        let StandardEvent::ResponseUsage(response) =
            CodexAdapter.normalize(&observation).unwrap().remove(0)
        else {
            panic!("fixture must normalize into a native usage fact");
        };
        response
    }

    fn store(&self, response: &ResponseUsage) {
        adapter::store_response(&self.core.db, response, false).unwrap();
        let data = json!({"id":response.thread_id,"sourceId":response.source_id,
            "cwd":"D:/aggregation/project","title":"aggregation fixture"});
        self.core
            .db
            .execute(
                "INSERT OR IGNORE INTO thread_metadata(id,data) VALUES(?1,?2)",
                params![
                    format!("{}|{}", response.source_id, response.thread_id),
                    data.to_string()
                ],
            )
            .unwrap();
    }

    fn stored_facts(&self) -> Vec<String> {
        self.core
            .db
            .prepare("SELECT data FROM usage_responses ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }
}

#[test]
fn native_overflow_is_a_query_error_and_keeps_facts_queryable_individually() {
    for (first_input, first_output, second_input, second_output, metric) in [
        (i64::MAX, 0, 1, 0, "输入 token"),
        (0, i64::MAX, 0, 1, "输出 token"),
        (i64::MAX, 0, 0, 1, "总 token"),
    ] {
        let fixture = Fixture::new();
        for (id, input, output) in [
            ("first", first_input, first_output),
            ("second", second_input, second_output),
        ] {
            let response = fixture.response(
                id,
                id,
                json!({
                    "input_tokens":input,"output_tokens":output,"total_tokens":input+output,
                    "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0
                }),
            );
            fixture.store(&response);
        }
        let original = fixture.stored_facts();
        let error = fixture.core.snapshot(json!({})).unwrap_err();
        assert!(error.contains(metric), "{error}");
        assert!(error.contains("超出整数范围"), "{error}");
        assert_eq!(fixture.stored_facts(), original);
        for (thread, expected) in [("first", i64::MAX), ("second", 1)] {
            let selected = fixture.core.snapshot(json!({"threadId":thread})).unwrap();
            assert_eq!(selected["overview"]["totalTokens"].as_i64(), Some(expected));
            assert_eq!(
                selected["sessions"][0]["totalTokens"].as_i64(),
                Some(expected)
            );
        }
        // Export retains the exact recorded integers even when a sum cannot fit.
        let csv = fixture.core.export_csv(json!({})).unwrap();
        assert!(csv.contains(&i64::MAX.to_string()));
    }
}

#[test]
fn ordinary_aggregation_preserves_cache_subset_coverage_and_group_totals() {
    let fixture = Fixture::new();
    let first = fixture.response(
        "first",
        "shared",
        json!({
            "input_tokens":100,"output_tokens":20,"total_tokens":120,
            "cached_input_tokens":50,"cache_write_input_tokens":10,"reasoning_output_tokens":5
        }),
    );
    let second = fixture.response(
        "second",
        "shared",
        json!({
            "input_tokens":300,"output_tokens":30,"total_tokens":330
        }),
    );
    fixture.store(&first);
    fixture.store(&second);
    let snapshot = fixture.core.snapshot(json!({})).unwrap();
    let overview = &snapshot["overview"];
    for (field, expected) in [
        ("inputTokens", 400),
        ("outputTokens", 50),
        ("totalTokens", 450),
        ("cachedInputTokens", 50),
        ("cacheWriteInputTokens", 10),
        ("normalInputTokens", 40),
        ("reasoningOutputTokens", 5),
        ("responseCount", 2),
    ] {
        assert_eq!(overview[field], expected, "{field}");
    }
    for (field, expected) in [
        ("cacheHitRate", 0.5),
        ("cacheCoverage", 0.25),
        ("cacheWriteCoverage", 0.5),
        ("normalInputCoverage", 0.5),
    ] {
        assert_eq!(overview[field], expected, "{field}");
    }
    for group in ["sessions", "daily", "models", "projects"] {
        assert_eq!(snapshot[group][0]["totalTokens"], 450, "{group}");
        assert_eq!(snapshot[group][0]["responseCount"], 2, "{group}");
    }
}

#[test]
fn legacy_overflow_errors_without_promoting_or_deleting_legacy_facts() {
    let fixture = Fixture::new();
    for (id, total) in [("first", i64::MAX), ("second", 1)] {
        let response = fixture.response(
            id,
            "legacy-thread",
            json!({
                "input_tokens":total,"output_tokens":0,"total_tokens":total
            }),
        );
        let data = json!({"id":id,"sourceId":response.source_id,"threadId":response.thread_id,
            "turnId":response.turn_id,"eventTime":response.event_time,"from":response.event_time,
            "counts":response.counts});
        fixture.core.db.execute(
            "INSERT INTO legacy_usage_segments(id,source_id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id,response.source_id,response.thread_id,response.turn_id,response.event_time,data.to_string()],
        ).unwrap();
    }
    let error = fixture.core.snapshot(json!({})).unwrap_err();
    assert!(error.contains("旧语义 token汇总超出整数范围"), "{error}");
    let count: i64 = fixture
        .core
        .db
        .query_row("SELECT COUNT(*) FROM legacy_usage_segments", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
    assert!(fixture.stored_facts().is_empty());
}

#[test]
fn source_counter_overflow_is_reported_instead_of_wrapping_health_totals() {
    let mut fixture = Fixture::new();
    let second_root = fixture._tmp.path().join("second-source");
    std::fs::create_dir(&second_root).unwrap();
    let second = fixture.core.add_source(&second_root).unwrap();
    let sources = [fixture.core.source_id.clone(), second];
    // Use real completed discovery records, retaining all evidence when changing counters.
    for _ in 0..sources.len() {
        fixture.core.scan().unwrap();
    }
    for (source, count) in sources.iter().zip([u64::MAX, 1]) {
        let key = format!("lastSourceCheck:{source}");
        let mut check = get_setting(&fixture.core.db, &key).unwrap().unwrap();
        check["unsupportedFiles"] = json!(count);
        put_setting(&fixture.core.db, &key, &check).unwrap();
    }
    let error = fixture.core.snapshot(json!({})).unwrap_err();
    assert!(
        error.contains("不支持的文件数量汇总超出整数范围"),
        "{error}"
    );
    for (source, count) in sources.iter().zip([u64::MAX, 1]) {
        let key = format!("lastSourceCheck:{source}");
        let mut check = get_setting(&fixture.core.db, &key).unwrap().unwrap();
        check["unsupportedFiles"] = json!(0);
        put_setting(&fixture.core.db, &key, &check).unwrap();
        put_setting(
            &fixture.core.db,
            &format!("historyPending:{source}"),
            &json!(count),
        )
        .unwrap();
    }
    let error = fixture.core.snapshot(json!({})).unwrap_err();
    assert!(
        error.contains("历史待处理文件数量汇总超出整数范围"),
        "{error}"
    );
}
