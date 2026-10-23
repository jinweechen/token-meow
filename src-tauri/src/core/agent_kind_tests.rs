//! Authored protocol fixtures, not claims about a live primary-thread sample.
use super::*;

const CHILD: &str = "00000000-0000-4000-8000-000000000002";
const UNKNOWN: &str = "00000000-0000-4000-8000-000000000003";

fn session<'a>(snapshot: &'a Value, id: &str) -> &'a Value {
    snapshot["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap()
}
fn lines(records: &[Value]) -> String {
    records.iter().map(|v| format!("{v}\n")).collect()
}
fn meta(id: &str, thread_source: &str) -> Value {
    event(
        "session_meta",
        json!({"id":id,"cli_version":"0.147.0","thread_source":thread_source,"cwd":format!("D:/fixture/{id}")}),
    )
}

#[test]
fn agent_kind_file_scopes_native_legacy_fact_views_and_csv_but_not_global_state() {
    let mut r = Rig::new();
    for (id, kind, input) in [
        (THREAD, "primary", 100),
        (CHILD, "subagent", 200),
        (UNKNOWN, "unknown", 300),
    ] {
        let turn = format!("turn-{kind}");
        let root = if kind == "subagent" {
            "turn-primary"
        } else {
            turn.as_str()
        };
        let metadata = match kind {
            "primary" => meta(id, "user"),
            "subagent" => event(
                "session_meta",
                json!({"id":id,"parent_thread_id":THREAD,"source":{"subagent":"review"}}),
            ),
            _ => event("session_meta", json!({"id":id,"source":"vscode"})),
        };
        let mut usage = native(id, &format!("response-{kind}"), input);
        usage["payload"]["turn_id"] = json!(turn);
        usage["payload"]["root_turn_id"] = json!(root);
        let legacy_turn = format!("legacy-{kind}");
        let count = |n| {
            event(
                "event_msg",
                json!({"type":"token_count","thread_id":id,"info":{"total_token_usage":{"input_tokens":n,"output_tokens":0,"total_tokens":n}}}),
            )
        };
        r.put(&format!("{kind}.jsonl"), &lines(&[
            metadata,
            event("event_msg", json!({"type":"task_started","turn_id":turn,"root_turn_id":root})),
            event("turn_context", json!({"turn_id":turn,"root_turn_id":root,"model":format!("model-{kind}")})),
            usage,
            event("event_msg", json!({"type":"exec_command_end","call_id":format!("tool-{kind}"),"exit_code":0})),
            event("event_msg", json!({"type":"task_complete","turn_id":turn,"root_turn_id":root,"status":"completed"})),
            event("turn_context", json!({"turn_id":legacy_turn,"model":format!("legacy-model-{kind}")})),
            count(100), count(160),
        ]));
    }
    r.scan();
    r.core
        .action(
            "save_budget",
            &json!({"id":"independent","scope":"daily","threshold":10000}),
        )
        .unwrap();
    let all = r.snapshot();
    assert_eq!(all["overview"]["totalTokens"], 660);
    assert_eq!(all["overview"]["legacyTokens"], 180);
    assert_eq!(session(&all, THREAD)["rootThreadId"], THREAD);
    assert_eq!(session(&all, CHILD)["rootThreadId"], THREAD);
    assert!(session(&all, UNKNOWN)["rootThreadId"].is_null());
    for (kind, id, total, inbox_count) in [
        ("primary", THREAD, 120, 1),
        ("subagent", CHILD, 220, 0),
        ("unknown", UNKNOWN, 320, 1),
    ] {
        let q = json!({"agentKind":kind});
        let scoped = r.core.snapshot(q.clone()).unwrap();
        assert_eq!(scoped["overview"]["totalTokens"], total);
        assert_eq!(scoped["overview"]["legacyTokens"], 60);
        assert_eq!(scoped["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(scoped["sessions"][0]["id"], id);
        assert_eq!(scoped["sessions"][0]["agentKind"], kind);
        assert!(scoped["sessions"][0].get("agentKindEvidence").is_none());
        assert_eq!(scoped["turns"].as_array().unwrap().len(), 2);
        assert_eq!(scoped["tools"].as_array().unwrap().len(), 1);
        assert_eq!(scoped["tools"][0]["threadId"], id);
        assert_eq!(scoped["inbox"].as_array().unwrap().len(), inbox_count);
        assert_eq!(scoped["daily"].as_array().unwrap().len(), 1);
        assert_eq!(scoped["models"].as_array().unwrap().len(), 1);
        assert_eq!(scoped["activity"], all["activity"]);
        assert_eq!(scoped["health"], all["health"]);
        assert_eq!(scoped["budgets"], all["budgets"]);
        let csv = r.core.export_csv(q).unwrap();
        assert_eq!(csv.lines().count(), 2);
        assert!(csv.contains(&format!("response-{kind}")));
        assert!(!csv.contains("legacy-model"));
    }
}

#[test]
fn agent_kind_late_file_metadata_and_reopen_keep_v3_facts_and_cursor_identity() {
    let mut r = Rig::new();
    let p = r.put(
        "usage-first.jsonl",
        &lines(&[native(THREAD, "retained", 100)]),
    );
    r.scan();
    assert_eq!(session(&r.snapshot(), THREAD)["agentKind"], "unknown");
    let facts: Vec<Value> = json_rows(&r.core.db, "usage_responses").unwrap();
    let file_id: i64 = r
        .core
        .db
        .query_row("SELECT id FROM source_files LIMIT 1", [], |row| row.get(0))
        .unwrap();
    append_fixture(&p, &[meta(THREAD, "user")]);
    r.scan();
    let snapshot = r.snapshot();
    assert_eq!(session(&snapshot, THREAD)["agentKind"], "primary");
    assert_eq!(snapshot["overview"]["totalTokens"], 120);
    assert_eq!(json_rows(&r.core.db, "usage_responses").unwrap(), facts);
    let old_offset: i64 = r
        .core
        .db
        .query_row(
            "SELECT offset FROM source_files WHERE id=?1",
            [file_id],
            |row| row.get(0),
        )
        .unwrap();
    r.core.shutdown().unwrap();
    r.core = Core::open(&r.data, &r.source).unwrap();
    let reopened = r.snapshot();
    assert_eq!(session(&reopened, THREAD)["agentKind"], "primary");
    assert_eq!(json_rows(&r.core.db, "usage_responses").unwrap(), facts);
    assert_eq!(
        r.core
            .db
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        9
    );
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
    assert_eq!(
        r.core
            .db
            .query_row(
                "SELECT offset FROM source_files WHERE id=?1",
                [file_id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        old_offset
    );
    assert_eq!(json_rows(&r.core.db, "usage_responses").unwrap(), facts);
}

#[test]
fn agent_kind_conflicting_parents_and_user_child_evidence_are_sticky_and_bounded() {
    for reverse in [false, true] {
        let mut r = Rig::new();
        let a = event(
            "session_meta",
            json!({"id":CHILD,"parent_thread_id":THREAD}),
        );
        let b = event(
            "session_meta",
            json!({"id":CHILD,"parent_thread_id":UNKNOWN}),
        );
        direct(&r.core, &[native(CHILD, "retained-child", 100)]);
        let facts = json_rows(&r.core.db, "usage_responses").unwrap();
        let ordered = if reverse { [b, a] } else { [a, b] };
        for _ in 0..20 {
            direct(&r.core, &ordered);
        }
        r.core.action("merge_session_directory", &json!({"sourceId":r.core.source_id,"sessions":[{"id":CHILD,"parentThreadId":null,"threadSource":"user","directoryProtocolVersion":null}]})).unwrap();
        let key = format!("{}|{CHILD}", r.core.source_id);
        let stored = adapter::load_fact(&r.core.db, "thread_metadata", &key)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored["agentKindEvidence"]["parents"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            stored["agentKindEvidence"]["observations"]
                .as_array()
                .unwrap()
                .len()
                <= 8
        );
        let s = r.snapshot();
        let child = session(&s, CHILD);
        assert_eq!(child["agentKind"], "unknown");
        assert_eq!(child["agentKindReason"], "relationship_conflict");
        assert!(child["parentThreadId"].is_null());
        assert!(child["rootThreadId"].is_null());
        assert_eq!(s["overview"]["totalTokens"], 120);
        assert_eq!(s["health"]["conflicts"], 0);
        assert_eq!(json_rows(&r.core.db, "usage_responses").unwrap(), facts);
        r.core.shutdown().unwrap();
        r.core = Core::open(&r.data, &r.source).unwrap();
        assert_eq!(
            session(&r.snapshot(), CHILD)["agentKindReason"],
            "relationship_conflict"
        );
    }
    let r = Rig::new();
    direct(
        &r.core,
        &[
            meta(THREAD, "user"),
            event(
                "session_meta",
                json!({"id":THREAD,"source":{"subagent":"review"}}),
            ),
        ],
    );
    assert_eq!(session(&r.snapshot(), THREAD)["agentKind"], "unknown");
    let mut without_root = native(THREAD, "no-inferred-conflict-root", 100);
    without_root["payload"]
        .as_object_mut()
        .unwrap()
        .remove("root_turn_id");
    direct(
        &r.core,
        &[event("session_meta", json!({"id":THREAD})), without_root],
    );
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
    // Clearing an ambiguous parent projection must not manufacture a root turn.
    assert!(
        r.core.snapshot(json!({"rootTurnId":"turn-a"})).unwrap()["overview"]["totalTokens"]
            .is_null()
    );
}

#[test]
fn agent_kind_version_whitelist_unknown_taxonomy_and_directory_channel_are_explicit() {
    let mut r = Rig::new();
    let cases = [
        (
            "00000000-0000-4000-8000-000000000101",
            json!({"thread_source":"user","cli_version":"0.147.0"}),
            "primary",
        ),
        (
            "00000000-0000-4000-8000-000000000102",
            json!({"thread_source":"user"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000103",
            json!({"thread_source":"user","cli_version":"0.147.0-alpha.1"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000104",
            json!({"thread_source":"user","cli_version":"0.999.0"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000105",
            json!({"thread_source":"subagent","cli_version":"0.147.0"}),
            "subagent",
        ),
        (
            "00000000-0000-4000-8000-000000000106",
            json!({"thread_source":"memory_consolidation","cli_version":"0.147.0"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000107",
            json!({"thread_source":"PRIVATE_FUTURE_FEATURE","source":"PRIVATE_CUSTOM_SOURCE","cli_version":"0.147.0"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000108",
            json!({"source":{"subagent":{"other":"PRIVATE_OTHER_SOURCE"}}}),
            "subagent",
        ),
        (
            "00000000-0000-4000-8000-000000000109",
            json!({"source":"vscode","parent_thread_id":null}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000110",
            json!({"source":{"subagent":"review","custom":"PRIVATE_MULTI_VARIANT"}}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000111",
            json!({"source":{"subagent":{"other":"PRIVATE_MULTI_CHILD","thread_spawn":{"parent_thread_id":THREAD,"depth":1}}}}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000112",
            json!({"source":{"internal":"memory_consolidation"},"thread_source":"user","cli_version":"0.147.0"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000113",
            json!({"threadSource":"user","cli_version":"0.147.0"}),
            "unknown",
        ),
        (
            "00000000-0000-4000-8000-000000000114",
            json!({"thread_source":"user","cli_version":"0.147.0","source":{"subAgent":"review"}}),
            "unknown",
        ),
    ];
    for (id, mut payload, kind) in cases {
        payload["id"] = json!(id);
        direct(&r.core, &[event("session_meta", payload)]);
        let s = r.snapshot();
        assert_eq!(session(&s, id)["agentKind"], kind, "{id}");
    }
    r.core.action("merge_session_directory", &json!({"sourceId":r.core.source_id,"sessions":[{"id":THREAD,"threadSource":"user","source":"vscode","cliVersion":"0.147.0","directoryProtocolVersion":null},{"id":CHILD,"source":{"subAgent":{"thread_spawn":{"parent_thread_id":THREAD,"depth":1}}},"directoryProtocolVersion":null}]})).unwrap();
    let s = r.snapshot();
    assert_eq!(session(&s, THREAD)["agentKind"], "unknown");
    assert_eq!(session(&s, THREAD)["agentKindReason"], "version_unverified");
    assert_eq!(session(&s, CHILD)["agentKind"], "subagent");
    assert!(session(&s, CHILD)["rootThreadId"].is_null());
    assert_eq!(
        session(&s, "00000000-0000-4000-8000-000000000110")["agentKindReason"],
        "relationship_conflict"
    );
    assert_eq!(
        session(&s, "00000000-0000-4000-8000-000000000111")["agentKindReason"],
        "relationship_conflict"
    );
    assert_eq!(
        session(&s, "00000000-0000-4000-8000-000000000112")["agentKindReason"],
        "relationship_conflict"
    );
    let persisted = json_rows(&r.core.db, "thread_metadata").unwrap();
    assert!(!serde_json::to_string(&persisted)
        .unwrap()
        .contains("PRIVATE_"));
}

#[test]
fn agent_kind_same_thread_id_is_source_scoped_and_illegal_queries_fail_for_csv() {
    let mut r = Rig::new();
    r.put(
        "primary.jsonl",
        &lines(&[meta(THREAD, "user"), native(THREAD, "first-source", 100)]),
    );
    let second = r._tmp.path().join("second-source");
    fs::create_dir(&second).unwrap();
    fs::write(
        second.join("unknown.jsonl"),
        lines(&[
            event("session_meta", json!({"id":THREAD})),
            native(THREAD, "second-source", 200),
        ]),
    )
    .unwrap();
    let second_id = r
        .core
        .action("add_source", &json!({"path":second}))
        .unwrap()["id"]
        .clone();
    r.scan();
    let all = r.snapshot();
    assert_eq!(all["overview"]["totalTokens"], 340);
    assert_eq!(all["sessions"].as_array().unwrap().len(), 2);
    let unknown = r.core.snapshot(json!({"agentKind":"unknown"})).unwrap();
    assert_eq!(unknown["overview"]["totalTokens"], 220);
    assert_eq!(unknown["sessions"][0]["sourceId"], second_id);
    assert_eq!(
        r.core.snapshot(json!({"agentKind":"primary"})).unwrap()["overview"]["totalTokens"],
        120
    );
    for invalid in [
        Value::Null,
        json!(""),
        json!("all"),
        json!("root"),
        json!(true),
        json!(1),
        json!({}),
    ] {
        let q = json!({"agentKind":invalid});
        assert!(r
            .core
            .snapshot(q.clone())
            .unwrap_err()
            .contains("agentKind"));
        assert!(r.core.export_csv(q).unwrap_err().contains("agentKind"));
    }
}

#[test]
fn agent_kind_existing_v3_parent_is_read_without_fake_migration_and_root_requires_positive_end() {
    let r = Rig::new();
    let raw = json!({"id":CHILD,"sourceId":r.core.source_id,"title":"retained","parentThreadId":THREAD,"rootThreadId":THREAD});
    r.core
        .db
        .execute(
            "INSERT INTO thread_metadata(id,data) VALUES(?1,?2)",
            params![format!("{}|{CHILD}", r.core.source_id), raw.to_string()],
        )
        .unwrap();
    direct(
        &r.core,
        &[
            event("session_meta", json!({"id":THREAD})),
            native(CHILD, "old-v3-fact", 100),
        ],
    );
    let before = adapter::load_fact(
        &r.core.db,
        "thread_metadata",
        &format!("{}|{CHILD}", r.core.source_id),
    )
    .unwrap();
    let s = r.snapshot();
    assert_eq!(session(&s, CHILD)["agentKind"], "subagent");
    assert!(session(&s, CHILD)["rootThreadId"].is_null());
    assert_eq!(s["overview"]["totalTokens"], 120);
    assert_eq!(
        adapter::load_fact(
            &r.core.db,
            "thread_metadata",
            &format!("{}|{CHILD}", r.core.source_id)
        )
        .unwrap(),
        before
    );
    direct(&r.core, &[meta(THREAD, "user")]);
    assert_eq!(session(&r.snapshot(), CHILD)["rootThreadId"], THREAD);
    direct(
        &r.core,
        &[event(
            "session_meta",
            json!({"id":THREAD,"parent_thread_id":CHILD}),
        )],
    );
    let s = r.snapshot();
    assert!(session(&s, CHILD)["rootThreadId"].is_null());
    assert_eq!(s["overview"]["totalTokens"], 120);
}
