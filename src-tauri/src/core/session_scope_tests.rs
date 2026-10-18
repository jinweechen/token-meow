//! Session navigation follows scoped facts, independently of token coverage.
use super::*;

const THREAD: &str = "00000000-0000-4000-8000-000000000011";

struct Fixture {
    temp: tempfile::TempDir,
    core: Core,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("pet"), &source).unwrap();
        Self { temp, core }
    }

    fn ingest(&self, source: &str, values: &[Value]) {
        let mut context = reader::Context::default();
        for value in values {
            adapter::ingest(&self.core.db, source, "session-fixture", value, &mut context, None)
                .unwrap();
        }
    }

    fn timed_turn(&self, source: &str, turn: &str, model: &str) {
        self.ingest(source, &[
            event("2025-12-31T01:00:00Z", "session_meta", json!({"id":THREAD,"cwd":"D:/fixture/project"})),
            event("2026-01-01T16:00:01Z", "event_msg", json!({"type":"task_started","turn_id":turn,"root_turn_id":turn})),
            event("2026-01-01T16:00:01Z", "turn_context", json!({"turn_id":turn,"root_turn_id":turn,"model":model})),
            event("2026-01-01T16:00:05Z", "event_msg", json!({"type":"task_complete","turn_id":turn,"root_turn_id":turn,"status":"completed","duration_ms":4000,"time_to_first_token_ms":125})),
        ]);
    }
}

fn event(time: &str, kind: &str, payload: Value) -> Value {
    json!({"timestamp":time,"type":kind,"payload":payload})
}

fn dated(extra: Value) -> Value {
    let mut query = json!({"from":"2026-01-02","to":"2026-01-02"});
    query.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    query
}

fn assert_no_usage(snapshot: &Value) {
    assert!(snapshot["overview"]["totalTokens"].is_null());
    assert_eq!(snapshot["overview"]["responseCount"], 0);
    assert_eq!(snapshot["overview"]["trustedMeteredResponseCount"], 0);
    for session in snapshot["sessions"].as_array().unwrap() {
        assert!(session["totalTokens"].is_null(), "no usage evidence must not display zero tokens");
        assert_eq!(session["responseCount"], 0);
    }
}

#[test]
fn dated_timed_turn_without_usage_keeps_its_session_and_timing() {
    let fixture = Fixture::new();
    fixture.timed_turn(&fixture.core.source_id, "timed-turn", "timing-model");
    let snapshot = fixture.core.snapshot(dated(json!({}))).unwrap();
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 1,
        "a recorded turn remains reachable even without a token response");
    assert_eq!(snapshot["sessions"][0]["id"], THREAD);
    assert_eq!(snapshot["turns"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["turns"][0]["durationMs"], 4000);
    assert_eq!(snapshot["turns"][0]["timeToFirstTokenMs"], 125);
    assert_eq!(snapshot["inbox"].as_array().unwrap().len(), 1);
    assert_no_usage(&snapshot);
    // Selection uses Shanghai completion day, not session creation day or UTC.
    let previous = fixture.core.snapshot(json!({"from":"2026-01-01","to":"2026-01-01"})).unwrap();
    assert!(previous["sessions"].as_array().unwrap().is_empty());
    assert!(previous["turns"].as_array().unwrap().is_empty());
    let wrong_model = fixture.core.snapshot(dated(json!({"model":"other-model"}))).unwrap();
    assert!(wrong_model["sessions"].as_array().unwrap().is_empty());
    fixture.ingest(&fixture.core.source_id, &[
        event("2026-01-01T16:00:06Z", "token_usage_record", json!({"thread_id":THREAD,
            "turn_id":"timed-turn","root_turn_id":"timed-turn","response_id":"recorded-zero",
            "usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0,
                "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}})),
    ]);
    let zero = fixture.core.snapshot(dated(json!({}))).unwrap();
    assert_eq!(zero["sessions"][0]["totalTokens"], 0, "a real zero response stays zero");
    assert_eq!(zero["sessions"][0]["responseCount"], 1);
    assert_eq!(zero["overview"]["totalTokens"], 0);
    assert_eq!(zero["overview"]["trustedMeteredResponseCount"], 1);
}

#[test]
fn fact_selected_sessions_preserve_source_model_project_and_root_intersections() {
    let mut fixture = Fixture::new();
    let second_root = fixture.temp.path().join("second-source");
    fs::create_dir(&second_root).unwrap();
    let second = fixture.core.add_source(&second_root).unwrap();
    let first = fixture.core.source_id.clone();
    fixture.timed_turn(&first, "first-turn", "model-a");
    fixture.timed_turn(&second, "second-turn", "model-b");
    let all = fixture.core.snapshot(json!({})).unwrap();
    let project = all["sessions"][0]["project"].clone();
    let selected = fixture.core.snapshot(dated(json!({"sourceId":second,"threadId":THREAD,
        "model":"model-b","project":project,"rootTurnId":"second-turn","clientKind":"codex"}))).unwrap();
    assert_eq!(selected["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(selected["sessions"][0]["sourceId"], second);
    assert_eq!(selected["turns"].as_array().unwrap().len(), 1);
    assert_no_usage(&selected);
    for extra in [
        json!({"sourceId":second,"model":"model-a"}),
        json!({"sourceId":second,"rootTurnId":"first-turn"}),
        json!({"sourceId":second,"project":"other-project"}),
        json!({"sourceId":second,"providerId":"unrecorded-provider"}),
        json!({"sourceId":second,"clientKind":"claude-code"}),
        json!({"sourceId":second,"agentKind":"primary"}),
    ] {
        let excluded = fixture.core.snapshot(dated(extra)).unwrap();
        assert!(excluded["sessions"].as_array().unwrap().is_empty(), "unproven scope must stay excluded");
        assert!(excluded["turns"].as_array().unwrap().is_empty());
    }
}

#[test]
fn dated_tool_only_session_keeps_navigation_without_inferred_model_or_turn() {
    let fixture = Fixture::new();
    fixture.ingest(&fixture.core.source_id, &[
        event("2025-12-31T01:00:00Z", "session_meta", json!({"id":THREAD,"cwd":"D:/fixture/tools"})),
        event("2026-01-01T16:00:02Z", "event_msg", json!({"type":"exec_command_end","call_id":"tool-only","exit_code":7,"duration_ms":25})),
    ]);
    let snapshot = fixture.core.snapshot(dated(json!({}))).unwrap();
    assert_eq!(snapshot["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["tools"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["tools"][0]["exitCode"], 7);
    assert!(snapshot["turns"].as_array().unwrap().is_empty());
    assert!(snapshot["inbox"].as_array().unwrap().is_empty());
    assert_no_usage(&snapshot);
    let missing_model = fixture.core.snapshot(dated(json!({"model":"not-inferred"}))).unwrap();
    assert!(missing_model["sessions"].as_array().unwrap().is_empty());
    assert!(missing_model["tools"].as_array().unwrap().is_empty());
}

#[test]
fn invalid_date_filters_are_rejected_consistently_before_snapshot_or_export() {
    let fixture = Fixture::new();
    for query in [
        json!({"from":""}),
        json!({"to":""}),
        json!({"from":null}),
        json!({"to":20260102}),
        json!({"from":"2026-02-30"}),
        json!({"from":"2026-1-02"}),
        json!({"to":"2026-01-02T00:00:00Z"}),
        json!({"from":"2026-01-03","to":"2026-01-02"}),
    ] {
        let snapshot = fixture.core.snapshot(query.clone()).expect_err("snapshot accepted malformed date");
        let export = fixture.core.export_csv(query.clone()).expect_err("export accepted malformed date");
        let dashboard = fixture.core.usage_dashboard_snapshot(query, json!({})).expect_err("dashboard accepted malformed date");
        assert_eq!(snapshot, export);
        assert_eq!(snapshot, dashboard);
    }
    // Missing endpoints keep the established unbounded-side semantics.
    for query in [json!({}), json!({"from":"2026-01-02"}), json!({"to":"2026-01-02"})] {
        fixture.core.snapshot(query.clone()).unwrap();
        fixture.core.export_csv(query).unwrap();
    }
}
