use super::*;
use crate::core::adapter::{self, CodexAdapter, Observation, SourceAdapter, StandardEvent};

struct Fixture {
    _tmp: tempfile::TempDir,
    core: Core,
}
impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&tmp.path().join("data"), &source).unwrap();
        Self { _tmp: tmp, core }
    }
    fn response(
        &self,
        id: &str,
        time: &str,
        thread: &str,
        model: &str,
        provider: Option<&str>,
    ) -> ResponseUsage {
        let observation = Observation {
            source_id: self.core.source_id.clone(),
            channel_id: format!("{}:jsonl", self.core.source_id),
            event: json!({"timestamp":time,"type":"token_usage_record","payload":{
                "thread_id":thread,"response_id":id,"turn_id":format!("turn-{id}"),
                "requested_model":model,"provider_id":provider,
                "usage":{"input_tokens":1000,"output_tokens":200,"total_tokens":1200,
                    "cached_input_tokens":600,"cache_write_input_tokens":0,"reasoning_output_tokens":50}
            }}),
        };
        let StandardEvent::ResponseUsage(response) =
            CodexAdapter.normalize(&observation).unwrap().remove(0)
        else {
            panic!("must be native usage")
        };
        response
    }
    fn store(&self, response: &ResponseUsage) {
        adapter::store_response(&self.core.db, response, false).unwrap();
        let mut session = json!({"id":response.thread_id,"sourceId":response.source_id,
            "cwd":format!("D:/private-project/{}",response.thread_id),"title":"Private title"});
        agent_kind::merge(
            &mut session,
            &json!({"thread_source":"user"}),
            "jsonl-session-meta",
            Some("0.147.0"),
        );
        self.core
            .db
            .execute(
                "INSERT OR IGNORE INTO thread_metadata(id,data) VALUES(?1,?2)",
                params![
                    format!("{}|{}", response.source_id, response.thread_id),
                    session.to_string()
                ],
            )
            .unwrap();
    }
    fn dashboard(&self, query: Value, request: Value) -> Value {
        self.core.usage_dashboard_snapshot(query, request).unwrap()["usageDashboard"].clone()
    }
    fn price(&mut self, model: &str) {
        pricing::save(
            &self.core.db,
            &json!({"model":model,"providerId":null,
            "effectiveFrom":"2020-01-01T00:00:00Z","inputPerMillion":"1",
            "outputPerMillion":"2","cacheReadPerMillion":"0.1","cacheWritePerMillion":"1.25"}),
        )
        .unwrap();
    }
    fn turn(&self, source: &str, thread: &str, id: &str) {
        let turn = json!({"id":id,"sourceId":source,"threadId":thread,"status":"completed",
            "lastEventAt":"2026-10-16T01:00:00Z","completedAt":"2026-10-16T01:00:00Z",
            "resultEvidence":"explicit-terminal","lifecycleEventAt":"2026-10-16T01:00:00Z",
            "durationMs":2000,"timeToFirstTokenMs":500});
        self.core
            .db
            .execute(
                "INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5)",
                params![
                    format!("{source}|{thread}|{id}"),
                    thread,
                    id,
                    "2026-10-16T01:00:00Z",
                    turn.to_string()
                ],
            )
            .unwrap();
    }
}

fn day() -> Value {
    json!({"from":"2026-10-16","to":"2026-10-16"})
}

#[test]
fn single_day_is_24_shanghai_hours_and_places_utc_cross_day_correctly() {
    let fixture = Fixture::new();
    fixture.store(&fixture.response("before", "2026-10-15T15:59:59Z", "t", "model", None));
    fixture.store(&fixture.response("midnight", "2026-10-15T16:00:00Z", "t", "model", None));
    fixture.store(&fixture.response("late", "2026-10-16T15:59:59Z", "t", "model", None));
    let dashboard = fixture.dashboard(day(), json!({}));
    let trend = dashboard["trend"].as_array().unwrap();
    assert_eq!(dashboard["granularity"], "hour");
    assert_eq!(trend.len(), 24);
    assert_eq!(trend[0]["bucket"], "2026-10-16T00:00:00+08:00");
    assert_eq!(trend[0]["responseCount"], 1);
    assert_eq!(trend[23]["responseCount"], 1);
    assert_eq!(dashboard["summary"]["totalTokens"], 2400);
    assert_eq!(dashboard["summary"]["normalInputTokens"], 800);
    assert_eq!(dashboard["summary"]["cacheHitRate"], 0.6);
    assert!(trend[1]["estimatedCostUsd"].is_null());
    assert!(dashboard["summary"]["estimatedCostUsd"].is_null());
}

#[test]
fn multi_day_trend_fills_empty_days_without_inventing_costs() {
    let fixture = Fixture::new();
    fixture.store(&fixture.response("one", "2026-10-16T01:00:00Z", "t", "model", None));
    let dashboard = fixture.dashboard(json!({"from":"2026-10-15","to":"2026-10-17"}), json!({}));
    assert_eq!(dashboard["granularity"], "day");
    let trend = dashboard["trend"].as_array().unwrap();
    assert_eq!(trend.len(), 3);
    assert_eq!(trend[0]["bucket"], "2026-10-15");
    assert_eq!(trend[0]["totalTokens"], 0);
    assert_eq!(trend[1]["totalTokens"], 1200);
    assert!(trend[2]["estimatedCostUsd"].is_null());
}

#[test]
fn strict_requests_reject_invalid_pages_qualities_and_unbounded_dates() {
    let fixture = Fixture::new();
    for request in [
        json!(null),
        json!({"page":-1}),
        json!({"page":1.5}),
        json!({"page":1_000_001}),
        json!({"pageSize":0}),
        json!({"pageSize":101}),
        json!({"quality":"success"}),
        json!({"quality":null}),
        json!({"unexpected":1}),
    ] {
        assert!(
            fixture
                .core
                .usage_dashboard_snapshot(day(), request.clone())
                .is_err(),
            "{request}"
        );
    }
    for query in [
        json!(null),
        json!({"from":"2026-2-01"}),
        json!({"from":"2026-02-30"}),
        json!({"from":"2026-10-17","to":"2026-10-16"}),
        json!({"from":"2025-01-01","to":"2026-01-02"}),
    ] {
        assert!(
            fixture
                .core
                .usage_dashboard_snapshot(query.clone(), json!({}))
                .is_err(),
            "{query}"
        );
    }
    let accepted = fixture.dashboard(json!({"from":"2024-01-01","to":"2024-12-31"}), json!({}));
    assert_eq!(accepted["trend"].as_array().unwrap().len(), 366);
}

#[test]
fn existing_overview_and_full_filter_contract_are_preserved() {
    let fixture = Fixture::new();
    fixture.store(&fixture.response(
        "a",
        "2026-10-16T01:00:00Z",
        "thread-a",
        "model-a",
        Some("provider-a"),
    ));
    fixture.store(&fixture.response(
        "b",
        "2026-10-16T02:00:00Z",
        "thread-b",
        "model-b",
        Some("provider-b"),
    ));
    put_setting(
        &fixture.core.db,
        "projectMappings",
        &json!({"D:/private-project/thread-a":"A"}),
    )
    .unwrap();
    let mut q = day();
    for (key, value) in [
        ("sourceId", fixture.core.source_id.as_str()),
        ("threadId", "thread-a"),
        ("model", "model-a"),
        ("providerId", "provider-a"),
        ("clientKind", "codex"),
        ("project", "mapped:A"),
        ("agentKind", "primary"),
        ("rootTurnId", "turn-a"),
    ] {
        q[key] = json!(value);
    }
    let old = fixture.core.snapshot(q.clone()).unwrap();
    let new = fixture.core.usage_dashboard_snapshot(q, json!({})).unwrap();
    assert_eq!(new["overview"], old["overview"]);
    for (key, value) in old["overview"].as_object().unwrap() {
        assert_eq!(&new["usageDashboard"]["summary"][key], value, "{key}");
    }
    assert_eq!(new["usageDashboard"]["totalRows"], 1);
    assert_eq!(new["usageDashboard"]["models"][0]["key"], "model-a");
    assert_eq!(new["usageDashboard"]["providers"][0]["key"], "provider-a");
}

#[test]
fn pagination_sorts_real_timestamps_stably_and_keeps_out_of_range_page_empty() {
    let fixture = Fixture::new();
    for (id, time) in [
        ("early", "2026-10-16T09:00:00+08:00"),
        ("late", "2026-10-16T02:00:00Z"),
        ("same-a", "2026-10-16T10:00:00+08:00"),
        ("same-b", "2026-10-16T02:00:00Z"),
    ] {
        fixture.store(&fixture.response(id, time, "thread", "model", None));
    }
    let full = fixture.dashboard(day(), json!({"pageSize":100}));
    let ids: Vec<_> = full["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].clone())
        .collect();
    assert_eq!(full["rows"][3]["responseId"], "early");
    for page in 0..4 {
        let result = fixture.dashboard(day(), json!({"page":page,"pageSize":1}));
        assert_eq!(result["rows"][0]["id"], ids[page]);
        assert_eq!(result["totalRows"], 4);
        assert_eq!(result["summary"]["totalTokens"], 4800);
    }
    let empty = fixture.dashboard(day(), json!({"page":99,"pageSize":20}));
    assert_eq!(empty["request"]["page"], 99);
    assert_eq!(empty["totalRows"], 4);
    assert!(empty["rows"].as_array().unwrap().is_empty());
}

#[test]
fn price_quality_filters_only_details_and_exact_costs_match_groups() {
    let mut fixture = Fixture::new();
    fixture.price("priced-model");
    fixture.store(&fixture.response("priced", "2026-10-16T01:00:00Z", "t", "priced-model", None));
    fixture.store(&fixture.response(
        "unpriced",
        "2026-10-16T02:00:00Z",
        "t",
        "unpriced-model",
        None,
    ));
    let all = fixture.dashboard(day(), json!({}));
    let priced = fixture.dashboard(day(), json!({"quality":"priced"}));
    let unpriced = fixture.dashboard(day(), json!({"quality":"unpriced"}));
    assert_eq!(all["summary"]["estimatedCostUsd"], "0.00086");
    assert_eq!(all["summary"]["pricedResponses"], 1);
    assert_eq!(all["summary"]["unpricedResponses"], 1);
    assert_eq!(priced["totalRows"], 1);
    assert_eq!(unpriced["totalRows"], 1);
    assert_eq!(priced["rows"][0]["responseId"], "priced");
    assert!(unpriced["rows"][0]["estimatedCostUsd"].is_null());
    assert!(priced["rows"][0]["pricingVersionId"].is_string());
    for key in ["summary", "trend", "models", "providers"] {
        assert_eq!(all[key], priced[key], "{key}");
        assert_eq!(all[key], unpriced[key], "{key}");
    }
    assert_eq!(all["providers"][0]["key"], "unknown");
    assert!(all["rows"][0]["providerId"].is_null());
    assert_eq!(
        all["providers"][0]["estimatedCostUsd"],
        all["summary"]["estimatedCostUsd"]
    );
}

#[test]
fn missing_optional_metrics_stay_null_and_are_not_priced_as_zero() {
    let mut fixture = Fixture::new();
    fixture.price("model");
    let mut response = fixture.response("missing", "2026-10-16T01:00:00Z", "t", "model", None);
    response.counts.cache_read = Metric::Missing;
    response.counts.cache_write = Metric::Unknown;
    response.counts.reasoning = Metric::Missing;
    fixture.store(&response);
    let result = fixture.dashboard(day(), json!({}));
    for key in [
        "cachedInputTokens",
        "cacheWriteInputTokens",
        "reasoningOutputTokens",
        "normalInputTokens",
        "estimatedCostUsd",
        "turnDurationMs",
        "turnTimeToFirstTokenMs",
    ] {
        assert!(result["rows"][0][key].is_null(), "{key}");
    }
    assert!(result["summary"]["cacheHitRate"].is_null());
    assert_eq!(result["summary"]["unpricedResponses"], 1);
}

#[test]
fn excluded_and_invalid_observations_are_counted_but_never_in_dashboard_facts() {
    let fixture = Fixture::new();
    let good = fixture.response("good", "2026-10-16T01:00:00Z", "t", "model", None);
    let excluded = fixture.response("excluded", "2026-10-16T02:00:00Z", "t", "model", None);
    let mut invalid = fixture.response("invalid", "2026-10-16T03:00:00Z", "t", "model", None);
    invalid.counts.total = Metric::Invalid;
    for response in [&good, &excluded, &invalid] {
        fixture.store(response);
    }
    fixture
        .core
        .db
        .execute(
            "INSERT INTO excluded_facts(fact_id,reason) VALUES(?1,'fixture')",
            params![response_key(&excluded)],
        )
        .unwrap();
    let result = fixture.dashboard(day(), json!({}));
    assert_eq!(result["totalRows"], 1);
    assert_eq!(result["summary"]["observedResponseCount"], 3);
    assert_eq!(result["summary"]["excludedResponses"], 2);
    assert_eq!(result["summary"]["trustedMeteredResponseCount"], 1);
    assert_eq!(result["summary"]["totalTokens"], 1200);
    assert_eq!(result["rows"][0]["responseId"], "good");
    for key in ["projects", "sessions"] {
        assert_eq!(result[key][0]["totalTokens"], 1200);
        assert_eq!(result[key][0]["responseCount"], 1);
    }
}

#[test]
fn turn_timing_requires_complete_source_thread_turn_identity() {
    let fixture = Fixture::new();
    let response = fixture.response("one", "2026-10-16T01:00:00Z", "thread-a", "model", None);
    fixture.store(&response);
    fixture.turn(&fixture.core.source_id, "thread-b", "turn-one");
    let mismatch = fixture.dashboard(day(), json!({}));
    assert!(mismatch["rows"][0]["turnTimeToFirstTokenMs"].is_null());
    assert!(mismatch["models"][0]["turnTimeToFirstTokenMs"].is_null());
    assert_eq!(mismatch["models"][0]["timedResponses"], 0);
    fixture.turn(&fixture.core.source_id, "thread-a", "turn-one");
    let exact = fixture.dashboard(day(), json!({}));
    assert_eq!(exact["rows"][0]["turnDurationMs"], 2000);
    assert_eq!(exact["rows"][0]["turnTimeToFirstTokenMs"], 500);
    assert_eq!(exact["models"][0]["turnTimeToFirstTokenMs"], 500.0);
    assert_eq!(exact["models"][0]["timedResponses"], 1);
    assert_eq!(exact["rows"][0]["timingScope"], "turn");
    assert!(exact["rows"][0]["speedTokensPerSecond"].is_null());
    assert!(exact["rows"][0].get("statusCode").is_none());
}

#[test]
fn filtering_cannot_make_multi_response_turn_timing_unambiguous() {
    let fixture = Fixture::new();
    let one = fixture.response(
        "one",
        "2026-10-16T01:00:00Z",
        "thread",
        "selected-model",
        None,
    );
    let mut two = fixture.response("two", "2026-10-16T02:00:00Z", "thread", "other-model", None);
    two.turn_id = one.turn_id.clone();
    fixture.store(&one);
    fixture.store(&two);
    fixture.turn(&fixture.core.source_id, "thread", "turn-one");
    let mut query = day();
    query["model"] = json!("selected-model");
    let result = fixture.dashboard(query, json!({}));
    assert_eq!(result["rows"].as_array().unwrap().len(), 1);
    assert!(result["rows"][0]["turnDurationMs"].is_null());
    assert!(result["rows"][0]["turnTimeToFirstTokenMs"].is_null());
    assert!(result["models"][0]["turnTimeToFirstTokenMs"].is_null());
    assert_eq!(result["models"][0]["timedResponses"], 0);
    assert!(result["providers"][0]["turnTimeToFirstTokenMs"].is_null());
    assert_eq!(result["providers"][0]["timedResponses"], 0);
}

#[test]
fn privacy_dashboard_hides_native_ids_and_never_returns_titles_or_paths() {
    let fixture = Fixture::new();
    fixture.store(&fixture.response(
        "private-response",
        "2026-10-16T01:00:00Z",
        "private-thread",
        "model",
        None,
    ));
    let mut preferences = fixture.core.settings().unwrap();
    preferences["privacyMode"] = json!(true);
    put_setting(&fixture.core.db, "preferences", &preferences).unwrap();
    let result = fixture.dashboard(day(), json!({}));
    assert!(result["rows"][0]["threadId"].is_null());
    assert!(result["rows"][0]["responseId"].is_null());
    assert_eq!(result["rows"][0]["id"].as_str().unwrap().len(), 64);
    let text = result.to_string();
    for private in [
        "private-thread",
        "private-response",
        "D:/private-project",
        "Private title",
    ] {
        assert!(!text.contains(private), "{private}");
    }
}

#[test]
fn model_filter_and_group_reuse_existing_turn_model_fallback() {
    let fixture = Fixture::new();
    let mut response = fixture.response(
        "missing-model",
        "2026-10-16T01:00:00Z",
        "thread",
        "ignored",
        None,
    );
    response.requested_model = None;
    fixture.store(&response);
    fixture.turn(&fixture.core.source_id, "thread", "turn-missing-model");
    fixture
        .core
        .db
        .execute(
            "UPDATE turns SET data=json_set(data,'$.model','turn-model')",
            [],
        )
        .unwrap();
    let mut query = day();
    query["model"] = json!("turn-model");
    let old = fixture.core.snapshot(query.clone()).unwrap();
    let result = fixture.dashboard(query, json!({}));
    assert_eq!(
        result["summary"]["totalTokens"],
        old["overview"]["totalTokens"]
    );
    assert_eq!(result["totalRows"], 1);
    assert_eq!(result["models"][0]["key"], "turn-model");
    assert_eq!(result["rows"][0]["requestedModel"], "turn-model");
}

#[test]
fn inferred_date_range_is_bounded_and_empty_day_still_has_24_hours() {
    let fixture = Fixture::new();
    let empty = fixture.dashboard(day(), json!({}));
    assert_eq!(empty["trend"].as_array().unwrap().len(), 24);
    assert_eq!(empty["summary"]["trustedMeteredResponseCount"], 0);
    assert!(empty["summary"]["totalTokens"].is_null());
    assert!(empty["summary"]["estimatedCostUsd"].is_null());
    fixture.store(&fixture.response("old", "2020-10-16T01:00:00Z", "t", "model", None));
    fixture.store(&fixture.response("new", "2026-10-16T01:00:00Z", "t", "model", None));
    let error = fixture
        .core
        .usage_dashboard_snapshot(json!({}), json!({}))
        .unwrap_err();
    assert!(error.contains("366"), "{error}");
}

#[test]
fn explicit_zero_prices_are_distinct_from_missing_prices() {
    let fixture = Fixture::new();
    pricing::save(
        &fixture.core.db,
        &json!({"model":"zero-model","providerId":null,
        "effectiveFrom":"2020-01-01T00:00:00Z","inputPerMillion":"0","outputPerMillion":"0",
        "cacheReadPerMillion":"0","cacheWritePerMillion":"0"}),
    )
    .unwrap();
    fixture.store(&fixture.response("free", "2026-10-16T01:00:00Z", "t", "zero-model", None));
    let result = fixture.dashboard(day(), json!({"quality":"priced"}));
    assert_eq!(result["summary"]["estimatedCostUsd"], "0");
    assert_eq!(result["summary"]["pricedResponses"], 1);
    assert_eq!(result["rows"][0]["estimatedCostUsd"], "0");
    assert_eq!(result["trend"][9]["estimatedCostUsd"], "0");
    assert!(result["trend"][8]["estimatedCostUsd"].is_null());
}

#[test]
fn group_turn_ttft_mean_uses_only_valid_attributable_samples_and_exposes_coverage() {
    let fixture = Fixture::new();
    for (id, timing) in [
        ("fast", json!(500)),
        ("slow", json!(1500)),
        ("zero", json!(0)),
        ("missing", Value::Null),
        ("negative", json!(-1)),
        ("string", json!("500")),
    ] {
        fixture.store(&fixture.response(
            id,
            "2026-10-16T01:00:00Z",
            "thread",
            "model",
            Some("provider"),
        ));
        let turn_id = format!("turn-{id}");
        fixture.turn(&fixture.core.source_id, "thread", &turn_id);
        fixture
            .core
            .db
            .execute(
                "UPDATE turns SET data=json_set(data,'$.timeToFirstTokenMs',json(?1)) WHERE id=?2",
                params![
                    timing.to_string(),
                    format!("{}|thread|{turn_id}", fixture.core.source_id)
                ],
            )
            .unwrap();
    }
    let all = fixture.dashboard(day(), json!({}));
    for key in ["models", "providers"] {
        let group = &all[key][0];
        assert_eq!(group["responseCount"], 6);
        assert_eq!(group["timedResponses"], 3);
        let mean = group["turnTimeToFirstTokenMs"].as_f64().unwrap();
        assert!((mean - 2000.0 / 3.0).abs() < 1e-9, "{mean}");
        assert!(group["successRate"].is_null());
        assert!(group["speedTokensPerSecond"].is_null());
        assert_eq!(group["timingScope"], "turn");
    }
    let filtered_details = fixture.dashboard(day(), json!({"quality":"priced","pageSize":1}));
    assert_eq!(filtered_details["totalRows"], 0);
    assert_eq!(filtered_details["models"], all["models"]);
    assert_eq!(filtered_details["providers"], all["providers"]);
}

#[test]
fn group_sorting_prefers_response_count_then_stable_key_over_token_total() {
    let fixture = Fixture::new();
    for (id, model, provider, input) in [
        ("z-one", "z-model", "z-provider", 1000),
        ("z-two", "z-model", "z-provider", 1000),
        ("b-one", "b-model", "b-provider", 1000),
        ("b-two", "b-model", "b-provider", 1000),
        ("a-large", "a-model", "a-provider", 100_000),
    ] {
        let mut response =
            fixture.response(id, "2026-10-16T01:00:00Z", "thread", model, Some(provider));
        response.counts.input = Metric::Valid(input);
        response.counts.total = Metric::Valid(input + 200);
        response
            .native_counts
            .insert("input_tokens".into(), Metric::Valid(input));
        response
            .native_counts
            .insert("total_tokens".into(), Metric::Valid(input + 200));
        fixture.store(&response);
    }
    let result = fixture.dashboard(day(), json!({}));
    for (key, expected) in [
        ("models", json!(["b-model", "z-model", "a-model"])),
        (
            "providers",
            json!(["b-provider", "z-provider", "a-provider"]),
        ),
    ] {
        let keys: Vec<_> = result[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| group["key"].clone())
            .collect();
        assert_eq!(json!(keys), expected);
        assert_eq!(result[key][0]["responseCount"], 2);
        assert_eq!(result[key][2]["responseCount"], 1);
    }
}

#[test]
fn scope_groups_partition_trusted_facts_with_unknown_projects_and_price_coverage() {
    let mut fixture = Fixture::new();
    fixture.price("priced");
    let one = fixture.response(
        "one",
        "2026-10-16T01:00:00Z",
        "shared-thread",
        "priced",
        None,
    );
    let mut other_source = fixture.response(
        "two",
        "2026-10-16T02:00:00Z",
        "shared-thread",
        "unpriced",
        None,
    );
    other_source.source_id = "second-source".into();
    let three = fixture.response(
        "three",
        "2026-10-16T03:00:00Z",
        "another-thread",
        "priced",
        None,
    );
    let unknown = fixture.response(
        "four",
        "2026-10-16T04:00:00Z",
        "no-metadata",
        "unpriced",
        None,
    );
    for response in [&one, &other_source, &three, &unknown] {
        fixture.store(response);
    }
    fixture
        .core
        .db
        .execute(
            "DELETE FROM thread_metadata WHERE json_extract(data,'$.id')='no-metadata'",
            [],
        )
        .unwrap();
    put_setting(
        &fixture.core.db,
        "projectMappings",
        &json!({"D:/private-project":"Known project"}),
    )
    .unwrap();
    let result = fixture.dashboard(day(), json!({}));
    assert_eq!(result["projects"].as_array().unwrap().len(), 2);
    assert_eq!(result["sessions"].as_array().unwrap().len(), 4);
    let known = &result["projects"][0];
    assert_eq!(known["projectId"], "mapped:Known project");
    assert_eq!(known["sessionCount"], 3);
    assert_eq!(known["totalTokens"], 3600);
    assert_eq!(known["pricedResponses"], 2);
    assert_eq!(known["unpricedResponses"], 1);
    assert_eq!(known["estimatedCostUsd"], "0.00172");
    assert!(known["sourceId"].is_null());
    assert!(known["threadId"].is_null());
    assert_eq!(known["firstEventTime"], "2026-10-16T09:00:00+08:00");
    assert_eq!(known["lastEventTime"], "2026-10-16T11:00:00+08:00");
    assert_eq!(result["projects"][1]["projectId"], "unknown");
    assert_eq!(result["projects"][1]["name"], "项目未知");
    for key in ["projects", "sessions"] {
        let groups = result[key].as_array().unwrap();
        for metric in [
            "totalTokens",
            "inputTokens",
            "outputTokens",
            "cachedInputTokens",
            "cacheWriteInputTokens",
            "reasoningOutputTokens",
            "normalInputTokens",
            "pricedResponses",
            "unpricedResponses",
        ] {
            let sum: i64 = groups
                .iter()
                .map(|group| group[metric].as_i64().unwrap())
                .sum();
            assert_eq!(json!(sum), result["summary"][metric], "{key}.{metric}");
        }
        assert!(groups
            .iter()
            .all(|group| group["key"].as_str().unwrap().len() == 64));
    }
    let shared: Vec<_> = result["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|group| group["threadId"] == "shared-thread")
        .collect();
    assert_eq!(shared.len(), 2);
    assert_ne!(shared[0]["key"], shared[1]["key"]);
    assert_ne!(shared[0]["sourceId"], shared[1]["sourceId"]);
    let details_only = fixture.dashboard(day(), json!({"quality":"unpriced","pageSize":1}));
    for key in ["projects", "sessions", "forecastBasis"] {
        assert_eq!(details_only[key], result[key]);
    }
}

#[test]
fn scope_groups_sort_by_tokens_then_stable_hash_and_preserve_missing_cache_coverage() {
    let fixture = Fixture::new();
    for (id, thread, input) in [
        ("one", "small", 1000),
        ("two", "small", 1000),
        ("large-b", "large-b", 10000),
        ("large-a", "large-a", 10000),
    ] {
        let mut response = fixture.response(id, "2026-10-16T01:00:00Z", thread, "model", None);
        response.counts.input = Metric::Valid(input);
        response.counts.total = Metric::Valid(input + 200);
        response.counts.cache_write = Metric::Missing;
        fixture.store(&response);
    }
    let result = fixture.dashboard(day(), json!({}));
    for key in ["projects", "sessions"] {
        let groups = result[key].as_array().unwrap();
        assert_eq!(groups[0]["totalTokens"], 10200);
        assert_eq!(groups[1]["totalTokens"], 10200);
        assert_eq!(groups[2]["totalTokens"], 2400);
        assert!(groups[0]["key"].as_str() < groups[1]["key"].as_str());
        assert!(groups
            .iter()
            .all(|group| group["cacheWriteInputTokens"].is_null()
                && group["cacheWriteCoverage"].as_f64() == Some(0.0)));
        assert!(groups.iter().all(
            |group| group["estimatedCostUsd"].is_null() && group["normalInputTokens"].is_null()
        ));
    }
}

#[test]
fn scope_privacy_hides_every_navigation_identity_and_keeps_hashes_and_totals_stable() {
    let fixture = Fixture::new();
    let response = fixture.response(
        "private-response",
        "2026-10-16T01:00:00Z",
        "private-thread",
        "model",
        None,
    );
    fixture.store(&response);
    fixture.turn(
        &fixture.core.source_id,
        "private-thread",
        "turn-private-response",
    );
    put_setting(
        &fixture.core.db,
        "projectMappings",
        &json!({"D:/private-project":"Private mapped project"}),
    )
    .unwrap();
    let before = fixture.dashboard(day(), json!({}));
    let mut preferences = fixture.core.settings().unwrap();
    preferences["privacyMode"] = json!(true);
    put_setting(&fixture.core.db, "preferences", &preferences).unwrap();
    let after = fixture.dashboard(day(), json!({}));
    for key in ["projects", "sessions"] {
        let group = &after[key][0];
        assert!(
            group["sourceId"].is_null()
                && group["threadId"].is_null()
                && group["projectId"].is_null()
        );
        assert_eq!(group["key"], before[key][0]["key"]);
        assert_eq!(group["totalTokens"], before[key][0]["totalTokens"]);
        assert!(group["name"].as_str().unwrap().starts_with("匿名"));
    }
    assert_eq!(after["forecastBasis"], before["forecastBasis"]);
    assert!(after["rows"][0]["sourceId"].is_null());
    let text = after.to_string();
    for private in [
        fixture.core.source_id.as_str(),
        "private-thread",
        "private-response",
        "Private title",
        "Private mapped project",
        "D:/private-project",
    ] {
        assert!(!text.contains(private), "{private}");
    }
}

#[test]
fn forecast_daily_matches_hourly_totals_and_explicit_empty_days() {
    let fixture = Fixture::new();
    fixture.store(&fixture.response("one", "2026-10-16T01:00:00Z", "thread", "model", None));
    fixture.store(&fixture.response("two", "2026-10-16T02:00:00Z", "thread", "model", None));
    let result = fixture.dashboard(day(), json!({}));
    assert_eq!(
        result["forecastBasis"]["daily"],
        json!([{"date":"2026-10-16","totalTokens":2400,"responseCount":2}])
    );
    let multi = fixture.dashboard(json!({"from":"2026-10-15","to":"2026-10-17"}), json!({}));
    for (day, bucket) in multi["forecastBasis"]["daily"]
        .as_array()
        .unwrap()
        .iter()
        .zip(multi["trend"].as_array().unwrap())
    {
        assert_eq!(day["date"], bucket["bucket"]);
        assert_eq!(day["totalTokens"], bucket["totalTokens"]);
        assert_eq!(day["responseCount"], bucket["responseCount"]);
    }
    assert_eq!(multi["forecastBasis"]["daily"][0]["totalTokens"], 0);
    assert_eq!(multi["forecastBasis"]["daily"][2]["responseCount"], 0);
}

#[test]
fn forecast_completed_turn_sums_native_response_totals_without_adding_cache_again() {
    let fixture = Fixture::new();
    let one = fixture.response("one", "2026-10-16T01:00:00Z", "thread", "model", None);
    let mut two = fixture.response("two", "2026-10-16T01:00:00Z", "thread", "model", None);
    two.turn_id = one.turn_id.clone();
    fixture.store(&one);
    fixture.store(&two);
    fixture.turn(&fixture.core.source_id, "thread", "turn-one");
    let result = fixture.dashboard(day(), json!({}));
    let basis = &result["forecastBasis"];
    assert_eq!(basis["completedTurns"].as_array().unwrap().len(), 1);
    assert_eq!(basis["completedTurns"][0]["totalTokens"], 2400);
    assert_eq!(basis["completedTurns"][0]["responseCount"], 2);
    assert_eq!(
        basis["completedTurns"][0]["key"].as_str().unwrap().len(),
        64
    );
    assert_eq!(basis["omittedIncompleteTurns"], 0);
}

#[test]
fn forecast_omits_partial_model_date_excluded_or_invalid_turns() {
    for reason in ["model", "date", "excluded", "invalid"] {
        let fixture = Fixture::new();
        let one = fixture.response("one", "2026-10-16T01:00:00Z", "thread", "model", None);
        let mut two =
            fixture.response("two", "2026-10-16T01:00:00Z", "thread", "other-model", None);
        two.turn_id = one.turn_id.clone();
        if reason == "date" {
            two.event_time = "2026-10-15T01:00:00Z".into();
        }
        if reason == "invalid" {
            two.counts.total = Metric::Invalid;
        }
        fixture.store(&one);
        fixture.store(&two);
        fixture.turn(&fixture.core.source_id, "thread", "turn-one");
        if reason == "excluded" {
            fixture
                .core
                .db
                .execute(
                    "INSERT INTO excluded_facts(fact_id,reason) VALUES(?1,'fixture')",
                    params![response_key(&two)],
                )
                .unwrap();
        }
        let mut query = day();
        if reason == "model" {
            query["model"] = json!("model");
        }
        let result = fixture.dashboard(query, json!({}));
        assert_eq!(
            result["forecastBasis"]["completedTurns"],
            json!([]),
            "{reason}"
        );
        assert_eq!(
            result["forecastBasis"]["omittedIncompleteTurns"], 1,
            "{reason}"
        );
    }
}

#[test]
fn forecast_requires_unique_source_thread_lifecycle_and_explicit_completion_evidence() {
    for reason in [
        "no-lifecycle",
        "wrong-thread",
        "wrong-source",
        "active",
        "failed",
        "missing-evidence",
        "bad-time",
        "outside-range",
        "duplicate-lifecycle",
    ] {
        let fixture = Fixture::new();
        fixture.store(&fixture.response("one", "2026-10-16T01:00:00Z", "thread", "model", None));
        if reason != "no-lifecycle" {
            fixture.turn(
                if reason == "wrong-source" {
                    "other-source"
                } else {
                    &fixture.core.source_id
                },
                if reason == "wrong-thread" {
                    "other-thread"
                } else {
                    "thread"
                },
                "turn-one",
            );
        }
        let change = match reason {
            "active" => Some(json!({"status":"active"})),
            "failed" => Some(json!({"status":"failed"})),
            "missing-evidence" => Some(json!({"resultEvidence":null})),
            "bad-time" => Some(json!({"completedAt":"invalid"})),
            "outside-range" => Some(json!({"completedAt":"2026-10-15T01:00:00Z"})),
            _ => None,
        };
        if let Some(change) = change {
            fixture
                .core
                .db
                .execute(
                    "UPDATE turns SET data=json_patch(data,?1)",
                    params![change.to_string()],
                )
                .unwrap();
        }
        if reason == "duplicate-lifecycle" {
            fixture.core.db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) SELECT 'duplicate',thread_id,turn_id,event_time,data FROM turns", []).unwrap();
        }
        let result = fixture.dashboard(day(), json!({}));
        assert_eq!(
            result["forecastBasis"]["completedTurns"],
            json!([]),
            "{reason}"
        );
        assert_eq!(
            result["forecastBasis"]["omittedIncompleteTurns"], 1,
            "{reason}"
        );
    }
    let fixture = Fixture::new();
    let mut response = fixture.response("no-turn", "2026-10-16T01:00:00Z", "thread", "model", None);
    response.turn_id = None;
    fixture.store(&response);
    let result = fixture.dashboard(day(), json!({}));
    assert_eq!(result["forecastBasis"]["completedTurns"], json!([]));
    assert_eq!(result["forecastBasis"]["omittedIncompleteTurns"], 0);
}

#[test]
fn forecast_keeps_latest_twenty_complete_turns_and_never_merges_sources() {
    let fixture = Fixture::new();
    for index in 0..22 {
        let id = format!("response-{index:02}");
        fixture.store(&fixture.response(&id, "2026-10-16T01:00:00Z", "thread", "model", None));
        fixture.turn(&fixture.core.source_id, "thread", &format!("turn-{id}"));
        fixture
            .core
            .db
            .execute(
                "UPDATE turns SET data=json_set(data,'$.completedAt',?1) WHERE turn_id=?2",
                params![
                    format!("2026-10-16T02:{index:02}:00Z"),
                    format!("turn-{id}")
                ],
            )
            .unwrap();
    }
    let result = fixture.dashboard(day(), json!({}));
    let completed = result["forecastBasis"]["completedTurns"]
        .as_array()
        .unwrap();
    assert_eq!(completed.len(), 20);
    assert_eq!(completed[0]["completedAt"], "2026-10-16T02:21:00+00:00");
    assert_eq!(completed[19]["completedAt"], "2026-10-16T02:02:00+00:00");
    assert_eq!(result["forecastBasis"]["omittedIncompleteTurns"], 0);

    let fixture = Fixture::new();
    let one = fixture.response("shared", "2026-10-16T01:00:00Z", "thread", "model", None);
    let mut two = one.clone();
    two.source_id = "second-source".into();
    fixture.store(&one);
    fixture.store(&two);
    fixture.turn(&fixture.core.source_id, "thread", "turn-shared");
    fixture.turn("second-source", "thread", "turn-shared");
    let result = fixture.dashboard(day(), json!({}));
    let completed = result["forecastBasis"]["completedTurns"]
        .as_array()
        .unwrap();
    assert_eq!(completed.len(), 2);
    assert_ne!(completed[0]["key"], completed[1]["key"]);
    assert!(completed
        .iter()
        .all(|sample| sample["totalTokens"] == 1200 && sample["responseCount"] == 1));
}

#[test]
fn forecast_fences_future_records_and_completions_before_limiting_samples() {
    let fixture = Fixture::new();
    let clock = Utc::now();
    let past = (clock - chrono::Duration::minutes(1)).to_rfc3339();
    let completed_at = (clock - chrono::Duration::seconds(1)).to_rfc3339();
    let future = (clock + chrono::Duration::minutes(10)).to_rfc3339();
    let selected_from = local_day(&past).unwrap();
    let selected_to = local_day(&future).unwrap();
    for index in 0..22 {
        let id = format!("candidate-{index:02}");
        fixture.store(&fixture.response(&id, &past, "thread", "model", None));
        fixture.turn(&fixture.core.source_id, "thread", &format!("turn-{id}"));
        fixture
            .core
            .db
            .execute(
                "UPDATE turns SET data=json_set(data,'$.completedAt',?1) WHERE turn_id=?2",
                params![
                    if index == 0 { &completed_at } else { &future },
                    format!("turn-{id}")
                ],
            )
            .unwrap();
    }
    let mut future_response = fixture.response("future-usage", &future, "thread", "model", None);
    future_response.turn_id = None;
    fixture.store(&future_response);
    let result = fixture.dashboard(json!({"from":selected_from,"to":selected_to}), json!({}));
    assert_eq!(result["summary"]["totalTokens"], 23 * 1200);
    let daily: i64 = result["forecastBasis"]["daily"]
        .as_array()
        .unwrap()
        .iter()
        .map(|day| day["totalTokens"].as_i64().unwrap())
        .sum();
    assert_eq!(daily, 22 * 1200);
    assert_eq!(
        result["forecastBasis"]["completedTurns"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        result["forecastBasis"]["completedTurns"][0]["completedAt"],
        completed_at
    );
    assert_eq!(result["forecastBasis"]["omittedIncompleteTurns"], 21);
    let as_of = DateTime::parse_from_rfc3339(result["asOf"].as_str().unwrap()).unwrap();
    assert!(DateTime::parse_from_rfc3339(&completed_at).unwrap() <= as_of);
    assert!(DateTime::parse_from_rfc3339(&future).unwrap() > as_of);
}
