use super::*;

const PARENT: &str = "00000000-0000-4000-8000-000000000011";
const CHILD: &str = "00000000-0000-4000-8000-000000000012";

struct Fixture {
    _temporary: tempfile::TempDir,
    core: Core,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir_all(&source).unwrap();
        let core = Core::open(&temporary.path().join("pet"), &source).unwrap();
        Self { _temporary: temporary, core }
    }

    fn send(&self, source: &str, context: &mut Context, second: u8, kind: &str, payload: Value) {
        self.send_ordinal(source, context, second, kind, payload, None);
    }

    fn send_ordinal(
        &self,
        source: &str,
        context: &mut Context,
        second: u8,
        kind: &str,
        payload: Value,
        ordinal: Option<u64>,
    ) {
        let event = json!({"timestamp":format!("2026-10-17T00:00:{second:02}.000Z"),"type":kind,"payload":payload,"ordinal":ordinal});
        ingest(&self.core.db, source, "tool-state-fixture", &event, context, None).unwrap();
    }

    fn context(&self, source: &str, thread: &str) -> Context {
        let mut context = Context::default();
        self.send(source, &mut context, 0, "session_meta", json!({"id":thread}));
        context
    }

    fn item(&self, source: &str, thread: &str, call: &str) -> Value {
        load_fact(&self.core.db, "items", &format!("{source}|{thread}|{call}"))
            .unwrap().unwrap()
    }

    fn item_count(&self) -> i64 {
        self.core.db.query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0)).unwrap()
    }
}

#[test]
fn tool_state_plain_output_and_later_begin_preserve_command_failure() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut context = fixture.context(source, PARENT);
    fixture.send(source, &mut context, 1, "response_item", json!({"type":"function_call","call_id":"call","name":"functions.exec_command","arguments":"SECRET_ARGUMENT"}));
    fixture.send(source, &mut context, 2, "event_msg", json!({"type":"exec_command_end","call_id":"call","exit_code":7,"duration_ms":15}));
    fixture.send(source, &mut context, 3, "response_item", json!({"type":"function_call_output","call_id":"call","output":"SECRET_RESULT_BODY"}));
    let after_output = fixture.item(source, PARENT, "call");
    assert_eq!(after_output["status"], "failed");
    assert_eq!(after_output["exitCode"], 7);
    fixture.send(source, &mut context, 4, "event_msg", json!({"type":"exec_command_begin","call_id":"call"}));
    fixture.send(source, &mut context, 5, "response_item", json!({"type":"function_call","call_id":"call","name":"functions.exec_command"}));
    let final_item = fixture.item(source, PARENT, "call");
    assert_eq!(final_item["status"], "failed");
    assert_eq!(final_item["exitCode"], 7);
    assert_eq!(final_item["durationMs"], 15);
    assert_eq!(final_item["name"], "functions.exec_command");
    assert_eq!(final_item["startedAt"], "2026-10-17T00:00:01.000Z");
    assert!(!final_item.to_string().contains("SECRET"));
    assert_eq!(fixture.item_count(), 1);
}

#[test]
fn tool_state_plain_output_preserves_mcp_error() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut context = fixture.context(source, PARENT);
    fixture.send(source, &mut context, 1, "event_msg", json!({"type":"mcp_tool_call_end","call_id":"mcp","tool_name":"mcp__sample__read","is_error":true,"duration_ms":23}));
    fixture.send(source, &mut context, 2, "response_item", json!({"type":"function_call_output","call_id":"mcp","output":"ordinary text"}));
    fixture.send(source, &mut context, 3, "response_item", json!({"type":"function_call_output","call_id":"mcp","output":{"isError":false}}));
    let item = fixture.item(source, PARENT, "mcp");
    assert_eq!(item["status"], "failed");
    assert_eq!(item["isError"], true);
    assert_eq!(item["durationMs"], 23);
    assert_eq!(item["name"], "mcp__sample__read");
    assert_eq!(item["kind"], "mcp");
}

#[test]
fn tool_state_older_explicit_end_upgrades_unknown_output() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut context = fixture.context(source, PARENT);
    fixture.send(source, &mut context, 1, "response_item", json!({"type":"function_call","call_id":"call","name":"exec_command"}));
    fixture.send(source, &mut context, 5, "response_item", json!({"type":"function_call_output","call_id":"call","output":"returned without recorded result"}));
    assert_eq!(fixture.item(source, PARENT, "call")["status"], "returned_unknown");
    fixture.send(source, &mut context, 3, "event_msg", json!({"type":"exec_command_end","call_id":"call","exit_code":7,"duration_ms":15}));
    let item = fixture.item(source, PARENT, "call");
    assert_eq!(item["status"], "failed");
    assert_eq!(item["exitCode"], 7);
    assert_eq!(item["durationMs"], 15);
    assert_eq!(item["eventTime"], "2026-10-17T00:00:05.000Z");
}

#[test]
fn tool_state_output_before_call_retains_result_and_fills_name() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut context = fixture.context(source, PARENT);
    for (call_kind, output_kind) in [
        ("function_call", "function_call_output"),
        ("custom_tool_call", "custom_tool_call_output"),
    ] {
        fixture.send(source, &mut context, 3, "response_item", json!({"type":output_kind,"call_id":call_kind,"output":{"exit_code":7,"wall_time_seconds":0.5,"output":"SECRET_BODY"}}));
        fixture.send(source, &mut context, 1, "response_item", json!({"type":call_kind,"call_id":call_kind,"name":"functions.exec_command","arguments":"SECRET_ARGUMENT"}));
        let item = fixture.item(source, PARENT, call_kind);
        assert_eq!(item["status"], "failed");
        assert_eq!(item["exitCode"], 7);
        assert_eq!(item["durationMs"], 500);
        assert_eq!(item["name"], "functions.exec_command");
        assert_eq!(item["kind"], "function");
        assert!(!item.to_string().contains("SECRET"));
    }
    assert_eq!(fixture.item_count(), 2);
}

#[test]
fn tool_state_unmarked_or_false_error_output_remains_unknown() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut context = fixture.context(source, PARENT);
    for (call, output) in [
        ("unmarked", json!("plain text")),
        ("false-error", json!({"isError":false})),
    ] {
        fixture.send(source, &mut context, 1, "response_item", json!({"type":"function_call","call_id":call,"name":"mcp__sample__read"}));
        fixture.send(source, &mut context, 2, "response_item", json!({"type":"function_call_output","call_id":call,"output":output}));
        let item = fixture.item(source, PARENT, call);
        assert_eq!(item["status"], "returned_unknown");
        assert!(item["exitCode"].is_null());
        if call == "false-error" {
            assert_eq!(item["isError"], false);
        }
    }
}

#[test]
fn tool_state_conflicting_terminal_results_remain_unknown_in_either_order() {
    for reverse in [false, true] {
        let fixture = Fixture::new();
        let source = &fixture.core.source_id;
        let mut context = fixture.context(source, PARENT);
        let mut results = vec![(2, 0), (3, 7)];
        if reverse { results.reverse(); }
        for (second, exit) in results {
            fixture.send(source, &mut context, second, "event_msg", json!({"type":"exec_command_end","call_id":"conflict","exit_code":exit,"duration_ms":15}));
        }
        let conflicted = fixture.item(source, PARENT, "conflict");
        assert_eq!(conflicted["status"], "returned_unknown");
        assert_eq!(conflicted["resultConflict"], true);
        assert!(conflicted["exitCode"].is_null());
        assert!(conflicted["isError"].is_null());
        let evidence = conflicted["resultEvidence"].as_array().unwrap();
        assert_eq!(evidence.len(), 2);
        assert!(evidence.iter().any(|value| value["exitCode"] == 0 && value["status"] == "completed"));
        assert!(evidence.iter().any(|value| value["exitCode"] == 7 && value["status"] == "failed"));
        assert!(evidence.iter().all(|value| value.as_object().unwrap().len() == 3));
        fixture.send(source, &mut context, 4, "response_item", json!({"type":"function_call_output","call_id":"conflict","output":"SECRET_BODY"}));
        fixture.send(source, &mut context, 5, "event_msg", json!({"type":"exec_command_begin","call_id":"conflict"}));
        fixture.send(source, &mut context, 3, "event_msg", json!({"type":"exec_command_end","call_id":"conflict","exit_code":7,"duration_ms":15}));
        let final_item = fixture.item(source, PARENT, "conflict");
        assert_eq!(final_item["status"], "returned_unknown");
        assert_eq!(final_item["resultConflict"], true);
        assert_eq!(final_item["resultEvidence"], conflicted["resultEvidence"]);
        assert!(!final_item.to_string().contains("SECRET"));
        let issues: i64 = fixture.core.db.query_row("SELECT COUNT(*) FROM ingestion_issues WHERE kind='conflicting_tool_result'", [], |row| row.get(0)).unwrap();
        assert_eq!(issues, 1);
        assert_eq!(fixture.item_count(), 1);
    }
}

#[test]
fn tool_state_call_identity_stays_separate_across_sources_and_fork_ordinals() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut fork = Context::default();
    fixture.send(source, &mut fork, 0, "session_meta", json!({"id":CHILD,"parent_thread_id":PARENT,"subagent_history_start_ordinal":2}));
    fixture.send(source, &mut fork, 0, "session_meta", json!({"id":PARENT}));
    fixture.send_ordinal(source, &mut fork, 1, "event_msg", json!({"type":"exec_command_end","call_id":"same-call","exit_code":7}), Some(1));
    fixture.send_ordinal(source, &mut fork, 2, "event_msg", json!({"type":"exec_command_end","call_id":"same-call","exit_code":0}), Some(2));
    let mut second_source = fixture.context("second-source", PARENT);
    fixture.send("second-source", &mut second_source, 3, "event_msg", json!({"type":"exec_command_end","call_id":"same-call","exit_code":9}));
    let parent = fixture.item(source, PARENT, "same-call");
    let child = fixture.item(source, CHILD, "same-call");
    let other_source = fixture.item("second-source", PARENT, "same-call");
    assert_eq!(parent["status"], "failed");
    assert_eq!(parent["exitCode"], 7);
    assert_eq!(child["status"], "completed");
    assert_eq!(child["exitCode"], 0);
    assert_eq!(other_source["exitCode"], 9);
    assert!(parent["resultConflict"].is_null());
    assert!(child["resultConflict"].is_null());
    assert_eq!(fixture.item_count(), 3);
}

#[test]
fn tool_state_duplicate_and_equal_time_begin_preserve_one_terminal_fact() {
    let fixture = Fixture::new();
    let source = &fixture.core.source_id;
    let mut context = fixture.context(source, PARENT);
    let end = json!({"type":"exec_command_end","call_id":"call","exit_code":0,"duration_ms":15});
    fixture.send(source, &mut context, 2, "event_msg", end.clone());
    fixture.send(source, &mut context, 2, "event_msg", json!({"type":"exec_command_begin","call_id":"call"}));
    fixture.send(source, &mut context, 1, "event_msg", json!({"type":"exec_command_begin","call_id":"call"}));
    fixture.send(source, &mut context, 2, "event_msg", end);
    let item = fixture.item(source, PARENT, "call");
    assert_eq!(item["status"], "completed");
    assert_eq!(item["exitCode"], 0);
    assert_eq!(item["durationMs"], 15);
    assert!(item["resultConflict"].is_null());
    assert_eq!(fixture.item_count(), 1);
}
