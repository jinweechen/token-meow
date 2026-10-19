use super::*;
use crate::core::adapter::{CodexAdapter, Observation, SourceAdapter, StandardEvent};

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
    fn add_response(&self, id: &str) {
        let observation = Observation {
            source_id: self.core.source_id.clone(),
            channel_id: format!("{}:jsonl", self.core.source_id),
            event: json!({"timestamp":"2026-10-16T01:00:00Z","type":"token_usage_record",
                "payload":{"thread_id":"panel-thread","response_id":id,"turn_id":"panel-turn",
                "requested_model":"panel-model","usage":{"input_tokens":100,"output_tokens":20,
                "total_tokens":120,"cached_input_tokens":40,"cache_write_input_tokens":0,
                "reasoning_output_tokens":5}}}),
        };
        let StandardEvent::ResponseUsage(response) =
            CodexAdapter.normalize(&observation).unwrap().remove(0)
        else { panic!("the fixture must normalize to native usage") };
        adapter::store_response(&self.core.db, &response, false).unwrap();
    }
    fn add_panel_facts(&self) {
        self.add_response("panel-response");
        let source = &self.core.source_id;
        let metadata = json!({"id":"panel-thread","sourceId":source,"cwd":"D:/private-project",
            "title":"Private title","threadSource":"user","agentKind":"primary"});
        self.core.db.execute("INSERT INTO thread_metadata(id,data) VALUES(?1,?2)",
            params![format!("{source}|panel-thread"), metadata.to_string()]).unwrap();
        let turn = json!({"id":"panel-turn","sourceId":source,"threadId":"panel-thread",
            "status":"completed","lastEventAt":"2026-10-16T01:00:02Z",
            "completedAt":"2026-10-16T01:00:02Z","resultEvidence":"explicit-terminal",
            "durationMs":2000,"timeToFirstTokenMs":125});
        self.core.db.execute("INSERT INTO turns(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5)",
            params![format!("{source}|panel-turn"),"panel-thread","panel-turn","2026-10-16T01:00:02Z",turn.to_string()]).unwrap();
        let item = json!({"id":"panel-tool","sourceId":source,"threadId":"panel-thread",
            "turnId":"panel-turn","type":"function_call","name":"test-tool","status":"completed",
            "exitCode":0,"durationMs":50,"createdAt":"2026-10-16T01:00:01Z"});
        self.core.db.execute("INSERT INTO items(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5)",
            params!["panel-tool","panel-thread","panel-turn","2026-10-16T01:00:01Z",item.to_string()]).unwrap();
        let inbox = json!({"id":"panel-inbox","sourceId":source,"threadId":"panel-thread",
            "turnId":"panel-turn","kind":"completed","createdAt":"2026-10-16T01:00:02Z",
            "notifyEligible":true,"animationAcknowledged":false,"eligibleRuntime":self.core.runtime_id});
        self.core.db.execute("INSERT INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,?2,?3,?4,?5)",
            params!["panel-inbox","panel-thread","panel-turn","2026-10-16T01:00:02Z",inbox.to_string()]).unwrap();
        let budget = json!({"id":"panel-budget","scope":"daily","threshold":1000,"enabled":true});
        self.core.db.execute("INSERT INTO budgets(id,data) VALUES(?1,?2)",
            params!["panel-budget",budget.to_string()]).unwrap();
        pricing::save(&self.core.db, &json!({"model":"panel-model","providerId":null,
            "effectiveFrom":"2020-01-01T00:00:00Z","inputPerMillion":"1","outputPerMillion":"2",
            "cacheReadPerMillion":"0.1","cacheWritePerMillion":"1.25"})).unwrap();
    }
}

fn day() -> Value { json!({"from":"2026-10-16","to":"2026-10-16"}) }
fn add_tool_only_events(fixture: &Fixture) {
    let time = "2026-10-16T01:00:03Z";
    let event = |payload: Value| json!({"timestamp":time,"type":"event_msg","payload":payload});
    let events = [
        json!({"timestamp":time,"type":"session_meta","payload":{"id":"tool-only-thread",
            "cwd":"D:/tool-only-project","thread_source":"user","cli_version":"0.147.0"}}),
        event(json!({"type":"exec_command_begin","call_id":"panel-error-command"})),
        event(json!({"type":"exec_command_end","call_id":"panel-error-command","exit_code":7,"duration_ms":20})),
        event(json!({"type":"mcp_tool_call_begin","call_id":"panel-mcp","tool_name":"mcp__fixture__lookup"})),
        event(json!({"type":"mcp_tool_call_end","call_id":"panel-mcp","tool_name":"mcp__fixture__lookup","is_error":true,"duration_ms":30})),
    ];
    let mut context = reader::Context::default();
    for value in events {
        adapter::ingest(&fixture.core.db, &fixture.core.source_id, "panel-tools", &value, &mut context, None).unwrap();
    }
}
fn without_dashboard_as_of(mut value: Value) -> Value {
    // Only the dashboard's response-time field is outside the pinned SQL view.
    if let Some(dashboard) = value.get_mut("usageDashboard").and_then(Value::as_object_mut) {
        dashboard.remove("asOf");
    }
    value
}
fn checkpoint(core: &Core) -> (i64, i64, i64) {
    core.db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
}

#[test]
fn session_pins_before_return_writer_commits_and_next_session_reads_new_values() {
    let mut fixture = Fixture::new();
    fixture.add_response("before");
    let session = fixture.core.begin_panel_read().unwrap();
    // No session query has run yet. Only begin_panel_read's real first read
    // can establish the historical view observed below.
    let source = fixture.core.source_id.clone();
    let root = fixture.core.source_root.to_string_lossy().into_owned();
    let tx = fixture.core.db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    put_setting(&tx, "preferences", &json!({"muted":false})).unwrap();
    tx.execute("UPDATE source_instances SET environment='reader-test' WHERE id=?1", [&source]).unwrap();
    tx.commit().unwrap();
    fixture.add_response("after");
    assert_eq!(fixture.core.snapshot(day()).unwrap()["overview"]["responseCount"], 2);
    let old = session.snapshot(day()).unwrap();
    assert_eq!(old["settings"]["muted"], true);
    assert_eq!(old["overview"]["responseCount"], 1);
    assert_eq!(old["health"]["sourceRoot"], root);
    assert_ne!(session.primary_source().unwrap()["environment"], "reader-test");
    session.finish().unwrap();
    let next = fixture.core.begin_panel_read().unwrap();
    assert_eq!(next.snapshot(day()).unwrap()["settings"]["muted"], false);
    assert_eq!(next.snapshot(day()).unwrap()["overview"]["responseCount"], 2);
    assert_eq!(next.primary_source().unwrap()["environment"], "reader-test");
    next.finish().unwrap();
}

#[test]
fn reader_is_readonly_and_cannot_create_a_missing_database() {
    let mut fixture = Fixture::new();
    let session = fixture.core.begin_panel_read().unwrap();
    assert!(session.connection().is_readonly(rusqlite::MAIN_DB).unwrap());
    let error = session.connection().execute("DELETE FROM settings", []).unwrap_err();
    assert_eq!(error.sqlite_error_code(), Some(rusqlite::ErrorCode::ReadOnly));
    session.finish().unwrap();
    let missing = fixture._tmp.path().join("empty-data");
    fs::create_dir(&missing).unwrap();
    fixture.core.data_dir = missing.clone();
    assert!(fixture.core.begin_panel_read().is_err());
    assert!(!missing.join("ledger.sqlite3").exists());
}

#[test]
fn snapshot_and_dashboard_share_all_static_facts_with_core() {
    let _clock = crate::budget_clock::test_support::TestClock::new(crate::budget_clock::Reading {
        utc: chrono::DateTime::parse_from_rfc3339("2026-10-16T12:00:00Z").unwrap().with_timezone(&Utc),
        mono_ms: 100,
    });
    let mut fixture = Fixture::new();
    fixture.add_panel_facts();
    for privacy in [false, true] {
        fixture.core.action("update_settings", &json!({"privacyMode":privacy})).unwrap();
        let changes = fixture.core.db.total_changes();
        let session = fixture.core.begin_panel_read().unwrap();
        assert_eq!(session.snapshot(day()).unwrap(), fixture.core.snapshot(day()).unwrap());
        let request = json!({"page":0,"pageSize":20,"quality":"all"});
        let mut reader = session.usage_dashboard_snapshot(day(), request.clone()).unwrap();
        let mut writer = fixture.core.usage_dashboard_snapshot(day(), request).unwrap();
        // asOf is intentionally wall time, not a database fact. Historical
        // fixture facts do not cross its cutoff; normalize this field only.
        reader["usageDashboard"].as_object_mut().unwrap().remove("asOf");
        writer["usageDashboard"].as_object_mut().unwrap().remove("asOf");
        assert_eq!(serde_json::to_string(&reader).unwrap(), serde_json::to_string(&writer).unwrap());
        assert_eq!(session.primary_source().unwrap(), fixture.core.primary_source().unwrap());
        assert_eq!(session.usage_price_sync().unwrap(), fixture.core.usage_price_sync().unwrap());
        assert_eq!(session.snapshot(day()).unwrap()["inbox"][0]["animationEligible"], true);
        assert_eq!(fixture.core.db.total_changes(), changes, "readers must not write settings or register a new Core");
        session.finish().unwrap();
    }
}

#[test]
fn panel_tool_projection_preserves_full_api_and_all_other_facts_in_private_mode() {
    let _clock = crate::budget_clock::test_support::TestClock::new(crate::budget_clock::Reading {
        utc: chrono::DateTime::parse_from_rfc3339("2026-10-16T12:00:00Z").unwrap().with_timezone(&Utc),
        mono_ms: 100,
    });
    let mut fixture = Fixture::new();
    fixture.add_panel_facts();
    add_tool_only_events(&fixture);
    for privacy in [false, true] {
        fixture.core.action("update_settings", &json!({"privacyMode":privacy})).unwrap();
        let session = fixture.core.begin_panel_read().unwrap();
        for dashboard in [false, true] {
            let request = json!({"page":0,"pageSize":20,"quality":"all"});
            let (legacy, complete, deferred, old_session) = if dashboard {
                (fixture.core.usage_dashboard_snapshot(day(), request.clone()).unwrap(),
                 session.usage_dashboard_for_panel(day(), request.clone(), true).unwrap(),
                 session.usage_dashboard_for_panel(day(), request.clone(), false).unwrap(),
                 session.usage_dashboard_snapshot(day(), request).unwrap())
            } else {
                (fixture.core.snapshot(day()).unwrap(), session.snapshot_for_panel(day(), true).unwrap(),
                 session.snapshot_for_panel(day(), false).unwrap(), session.snapshot(day()).unwrap())
            };
            let legacy = without_dashboard_as_of(legacy);
            assert!(legacy.get("toolsLoaded").is_none(), "legacy Core shape must remain unchanged");
            assert_eq!(without_dashboard_as_of(old_session), legacy, "legacy session must remain complete");
            let mut complete = without_dashboard_as_of(complete);
            assert_eq!(complete.as_object_mut().unwrap().remove("toolsLoaded"), Some(json!(true)));
            assert_eq!(complete, legacy);
            let tools = complete["tools"].as_array().unwrap();
            assert!(tools.iter().any(|tool| tool["exitCode"] == 7 && tool["status"] == "failed"));
            assert!(tools.iter().any(|tool| tool["kind"] == "mcp" && tool["name"] == "mcp__fixture__lookup" && tool["isError"] == true));
            let mut deferred = without_dashboard_as_of(deferred);
            assert_eq!(deferred.as_object_mut().unwrap().remove("toolsLoaded"), Some(json!(false)));
            let mut expected = legacy;
            expected["tools"] = json!([]);
            assert_eq!(deferred, expected, "all public facts except tool details must match");
        }
        session.finish().unwrap();
    }
}

#[test]
fn deferred_tools_keep_tool_only_navigation_and_identical_scope_and_date_filters() {
    let fixture = Fixture::new();
    add_tool_only_events(&fixture);
    let session = fixture.core.begin_panel_read().unwrap();
    let selected = session.snapshot_for_panel(day(), false).unwrap();
    assert_eq!(selected["overview"]["responseCount"], 0);
    assert_eq!(selected["turns"], json!([]));
    assert_eq!(selected["inbox"], json!([]));
    assert_eq!(selected["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(selected["sessions"][0]["id"], "tool-only-thread");
    assert!(selected["sessions"][0]["totalTokens"].is_null());
    assert_eq!(selected["sessions"][0]["responseCount"], 0);
    for (query, expected_sessions) in [
        (day(), 1),
        (json!({"from":"2026-10-16","to":"2026-10-16","sourceId":fixture.core.source_id,"threadId":"tool-only-thread","clientKind":"codex","model":"unknown"}), 1),
        (json!({"from":"2026-10-17","to":"2026-10-17"}), 0),
        (json!({"from":"2026-10-16","to":"2026-10-16","sourceId":"other-source"}), 0),
        (json!({"from":"2026-10-16","to":"2026-10-16","threadId":"other-thread"}), 0),
        (json!({"from":"2026-10-16","to":"2026-10-16","model":"other-model"}), 0),
    ] {
        let complete = session.snapshot_for_panel(query.clone(), true).unwrap();
        assert_eq!(complete["sessions"].as_array().unwrap().len(), expected_sessions);
        assert_eq!(complete["tools"].as_array().unwrap().len(), expected_sessions * 2);
        let mut expected = complete;
        expected["tools"] = json!([]);
        expected["toolsLoaded"] = json!(false);
        assert_eq!(session.snapshot_for_panel(query, false).unwrap(), expected);
    }
    session.finish().unwrap();
}

#[test]
fn session_keeps_the_captured_primary_and_runtime_after_writer_changes_domain() {
    let mut fixture = Fixture::new();
    fixture.add_panel_facts();
    let old_primary = fixture.core.primary_source().unwrap();
    let session = fixture.core.begin_panel_read().unwrap();
    let new_root = fixture._tmp.path().join("second-source");
    fs::create_dir(&new_root).unwrap();
    let second = fixture.core.add_source(&new_root).unwrap();
    fixture.core.action("set_primary_source", &json!({"id":second})).unwrap();
    assert_eq!(session.primary_source().unwrap(), old_primary);
    assert_eq!(session.snapshot(day()).unwrap()["primarySourceId"], old_primary["id"]);
    assert_eq!(session.snapshot(day()).unwrap()["inbox"][0]["animationEligible"], true);
    let next = fixture.core.begin_panel_read().unwrap();
    assert_eq!(next.primary_source().unwrap()["id"], second);
    next.finish().unwrap();
    session.finish().unwrap();
}

#[test]
fn errors_keep_original_messages_and_drop_releases_the_wal_view() {
    let fixture = Fixture::new();
    let invalid = json!({"from":"not-a-date"});
    let failed_read = || -> Result<Value> {
        let session = fixture.core.begin_panel_read()?;
        session.snapshot(invalid.clone())
    };
    assert_eq!(failed_read().unwrap_err(), fixture.core.snapshot(invalid).unwrap_err());
    assert_eq!(checkpoint(&fixture.core), (0, 0, 0));
    let session = fixture.core.begin_panel_read().unwrap();
    let invalid_request = json!({"quality":"invalid"});
    assert_eq!(session.usage_dashboard_snapshot(day(), invalid_request.clone()).unwrap_err(),
        fixture.core.usage_dashboard_snapshot(day(), invalid_request).unwrap_err());
    drop(session);
    assert_eq!(checkpoint(&fixture.core), (0, 0, 0));
}

#[test]
fn interrupt_from_another_thread_releases_active_sql_and_the_read_transaction() {
    let fixture = Fixture::new();
    let session = fixture.core.begin_panel_read().unwrap();
    let interrupt = session.interrupt_handle();
    let (started, observed) = std::sync::mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = session.connection().query_row::<i64, _, _>(
            "WITH RECURSIVE numbers(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM numbers WHERE x<100000000) SELECT sum(x) FROM numbers",
            [], |row| row.get(0),
        );
        let code = result.unwrap_err().sqlite_error().map(|error| error.extended_code);
        // SQLite may already have rolled back an interrupted transaction.
        // finish must still close it without inventing a second failure.
        (code, session.finish())
    });
    observed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    // Repeat until completion: a one-shot interrupt before SQLite begins
    // executing is a no-op and would make this test timing-dependent.
    while !worker.is_finished() && std::time::Instant::now() < deadline {
        interrupt.interrupt();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(worker.is_finished(), "the private test query did not respond to interruption");
    let (code, closed) = worker.join().unwrap();
    assert_eq!(code, Some(rusqlite::ffi::SQLITE_INTERRUPT));
    closed.unwrap();
    assert_eq!(checkpoint(&fixture.core), (0, 0, 0));
}
