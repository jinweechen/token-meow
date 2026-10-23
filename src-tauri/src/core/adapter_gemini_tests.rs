use super::*;

const TIME: &str = "2026-10-23T00:00:00Z";
fn tokens() -> Value {
    json!({"input":100,"tool":7,"output":20,"thoughts":5,"cached":40,"total":132})
}
fn header() -> Value {
    json!({"sessionId":"synthetic-session","projectHash":"a".repeat(64),"kind":"main"})
}
fn message(id: &str, usage: Value) -> Value {
    json!({"type":"gemini","id":id,"timestamp":TIME,"model":"synthetic-gemini-model","tokens":usage,
        "content":[{"text":"PRIVATE_MESSAGE_BODY"}],"thoughts":[{"text":"PRIVATE_THOUGHTS"}],
        "toolCalls":[{"args":{"secret":"PRIVATE_TOOL_ARGS"},"result":"PRIVATE_TOOL_RESULT"}]})
}
fn standalone(usage: Value) -> Value {
    let mut record = message("native-message", usage);
    record["sessionId"] = header()["sessionId"].clone();
    record
}
fn observation(event: Value) -> Observation {
    Observation {
        source_id: "synthetic-gemini-source".into(),
        channel_id: "synthetic-gemini-source:jsonl".into(),
        event,
    }
}
fn project(event: &Value) -> Value {
    GeminiAdapter
        .parse_line(&serde_json::to_vec(event).unwrap())
        .unwrap()
}
fn normalize(event: &Value) -> ResponseUsage {
    let mut events = GeminiAdapter
        .normalize(&observation(project(event)))
        .unwrap();
    assert_eq!(events.len(), 1);
    match events.pop().unwrap() {
        StandardEvent::ResponseUsage(row) => row,
        _ => panic!("不允许伪造其它事实"),
    }
}
fn database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(include_str!("../../migrations/001_initial.sql"))
        .unwrap();
    db.execute_batch(include_str!("../../migrations/002_multi_source.sql"))
        .unwrap();
    db
}
fn ingest(db: &Connection, state: &mut Context, event: &Value) -> usize {
    GeminiAdapter
        .ingest_local(
            db,
            &observation(project(event)),
            "synthetic-chats-file",
            state,
            None,
        )
        .unwrap()
}
fn rows(db: &Connection, table: &str) -> usize {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap() as usize
}
fn only_usage(db: &Connection) -> ResponseUsage {
    let data: String = db
        .query_row("SELECT data FROM usage_responses", [], |row| row.get(0))
        .unwrap();
    serde_json::from_str(&data).unwrap()
}
fn source(root: &Path) -> SourceConfig {
    SourceConfig {
        id: "synthetic-gemini-source".into(),
        root: root.to_path_buf(),
        environment: "windows-local".into(),
    }
}

#[test]
fn prompt_tool_candidate_thought_are_exclusive_components_cache_is_subset() {
    let row = normalize(&standalone(tokens()));
    assert_eq!(row.counts.input, Metric::Valid(107));
    assert_eq!(row.counts.output, Metric::Valid(25));
    assert_eq!(row.counts.cache_read, Metric::Valid(40));
    assert_eq!(row.counts.reasoning, Metric::Valid(5));
    assert_eq!(row.counts.total, Metric::Valid(132));
    assert_eq!(row.counts.cache_write, Metric::Missing);
    assert_eq!(row.native_counts["input"], Metric::Valid(100));
    assert_eq!(row.native_counts["output"], Metric::Valid(20));
    assert_eq!(row.namespace, NAMESPACE);
    assert_eq!(row.event_time, "2026-10-23T00:00:00.000Z");
    assert_eq!(row.semantics_version, SEMANTICS_VERSION);
    assert!(!GeminiAdapter.allows_revisions());
    assert!(
        row.revision.is_none() && row.turn_id.is_none() && row.root_turn_id.is_none() && !row.live
    );
    assert!(
        row.requested_model.is_none()
            && row.provider_id.is_none()
            && row.account_scope_id.is_none()
    );
}

#[test]
fn missing_tool_or_thought_does_not_reconcile_total_by_filling_zero() {
    for key in ["input", "tool", "output", "thoughts", "total"] {
        let mut usage = tokens();
        usage.as_object_mut().unwrap().remove(key);
        let row = normalize(&standalone(usage));
        assert_eq!(row.native_counts[key], Metric::Missing, "{key}");
        assert_eq!(row.counts.total, Metric::Missing, "{key}");
        assert!(row.completeness.starts_with("partial-source-recorded"));
    }
    let row = normalize(&standalone(json!({"input":100,"output":20,"total":120})));
    assert_eq!(row.native_counts["tool"], Metric::Missing);
    assert_eq!(row.native_counts["thoughts"], Metric::Missing);
    assert_eq!(row.native_counts["total"], Metric::Valid(120));
    assert_eq!(row.counts.total, Metric::Missing);
}

#[test]
fn null_missing_and_source_zero_are_distinct_without_upstream_zero_claim() {
    let row = normalize(&standalone(
        json!({"input":0,"tool":0,"output":0,"thoughts":0,"cached":null,"total":0}),
    ));
    assert_eq!(row.counts.total, Metric::Valid(0));
    assert_eq!(row.native_counts["cached"], Metric::Unknown);
    assert_eq!(row.counts.cache_read, Metric::Unknown);
    assert_eq!(row.counts.cache_write, Metric::Missing);
    assert!(row.completeness.contains("upstreamdefaults-unproven"));
    let mut usage = tokens();
    usage["tool"] = Value::Null;
    let row = normalize(&standalone(usage));
    assert_eq!(row.native_counts["tool"], Metric::Unknown);
    assert_eq!(row.counts.total, Metric::Unknown);
}

#[test]
fn wrong_total_is_invalid_and_native_total_is_never_rewritten() {
    let mut usage = tokens();
    usage["total"] = json!(131);
    let row = normalize(&standalone(usage));
    assert_eq!(row.counts.total, Metric::Invalid);
    assert_eq!(row.native_counts["total"], Metric::Valid(131));
}

#[test]
fn non_integer_counts_and_js_precision_overflow_are_not_coerced() {
    for invalid in [
        json!(-1),
        json!(1.0),
        json!("7"),
        json!(true),
        json!([]),
        json!({"body":"PRIVATE_BAD_COUNT"}),
        json!(9_007_199_254_740_992_u64),
        json!(u64::MAX),
    ] {
        let mut usage = tokens();
        usage["tool"] = invalid;
        let projected = project(&standalone(usage));
        assert!(!projected.to_string().contains("PRIVATE_BAD_COUNT"));
        let row = normalize(&projected);
        assert_eq!(row.native_counts["tool"], Metric::Invalid);
        assert_eq!(row.counts.input, Metric::Invalid);
        assert_eq!(row.counts.total, Metric::Invalid);
    }
    let row = normalize(&standalone(
        json!({"input":MAX_EXACT_INTEGER,"tool":1,"output":0,"thoughts":0,"total":MAX_EXACT_INTEGER}),
    ));
    assert_eq!(row.counts.input, Metric::Invalid);
    assert_eq!(row.counts.total, Metric::Invalid);
}

#[test]
fn cache_validation_does_not_double_add_or_invent_cache_write() {
    let mut usage = tokens();
    usage["cached"] = json!(101);
    let row = normalize(&standalone(usage));
    assert_eq!(row.native_counts["cached"], Metric::Valid(101));
    assert_eq!(row.counts.cache_read, Metric::Invalid);
    assert_eq!(row.counts.total, Metric::Valid(132));
    let mut usage = tokens();
    usage.as_object_mut().unwrap().remove("cached");
    let row = normalize(&standalone(usage));
    assert_eq!(row.counts.cache_read, Metric::Missing);
    assert_eq!(row.counts.total, Metric::Valid(132));
    let row = normalize(&standalone(
        json!({"tool":7,"output":20,"thoughts":5,"cached":40,"total":132}),
    ));
    assert_eq!(row.counts.cache_read, Metric::Unknown);
    assert_eq!(row.native_counts["cached"], Metric::Valid(40));
}

#[test]
fn whitelist_discards_all_bodies_and_unknown_token_fields_at_parse_boundary() {
    let mut event = standalone(tokens());
    event["summary"] = json!("PRIVATE_SUMMARY");
    event["directories"] = json!(["PRIVATE_DIRECTORY"]);
    event["tokens"]["providerBlob"] = json!({"secret":"PRIVATE_UNKNOWN_USAGE"});
    let value = project(&event);
    let encoded = value.to_string();
    assert!(!encoded.contains("PRIVATE_"));
    assert!(
        !encoded.contains("content")
            && !encoded.contains("toolCalls")
            && !encoded.contains("thoughts\":[")
    );
    assert_eq!(value["tokens"]["thoughts"], 5);
    assert_eq!(normalize(&value).counts.total, Metric::Valid(132));
    let db = database();
    let mut state = Context::default();
    assert_eq!(ingest(&db, &mut state, &event), 1);
    let stored = serde_json::to_string(&only_usage(&db)).unwrap();
    assert!(!stored.contains("PRIVATE_"));
    assert!(!serde_json::to_string(&state).unwrap().contains("PRIVATE_"));
}

#[test]
fn pending_null_missing_usage_never_creates_a_zero_fact() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    let mut pending = message("native-message", Value::Null);
    assert_eq!(ingest(&db, &mut state, &pending), 0);
    pending.as_object_mut().unwrap().remove("tokens");
    assert_eq!(ingest(&db, &mut state, &pending), 0);
    assert_eq!(rows(&db, "usage_responses"), 0);
    assert_eq!(rows(&db, "ingestion_issues"), 0);
    assert_eq!(
        ingest(&db, &mut state, &message("native-message", tokens())),
        1
    );
    assert_eq!(rows(&db, "usage_responses"), 1);
    assert_eq!(only_usage(&db).counts.total, Metric::Valid(132));
}

#[test]
fn duplicate_complete_is_idempotent_and_complete_conflict_is_quarantined() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    let event = message("native-message", tokens());
    assert_eq!(ingest(&db, &mut state, &event), 1);
    assert_eq!(ingest(&db, &mut state, &event), 0);
    let mut changed = event.clone();
    changed["tokens"]["output"] = json!(21);
    changed["tokens"]["total"] = json!(133);
    assert_eq!(ingest(&db, &mut state, &changed), 0);
    assert_eq!(rows(&db, "usage_responses"), 1);
    assert_eq!(rows(&db, "identity_conflicts"), 1);
    assert_eq!(rows(&db, "excluded_facts"), 1);
    assert_eq!(only_usage(&db).counts.total, Metric::Valid(132));
    assert_eq!(ingest(&db, &mut state, &changed), 0);
    assert_eq!(rows(&db, "identity_conflicts"), 1);
}

#[test]
fn null_after_complete_and_context_edits_do_not_delete_consumption() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    ingest(&db, &mut state, &message("native-message", tokens()));
    for edit in [
        message("native-message", Value::Null),
        json!({"$rewindTo":"native-message"}),
        json!({"$patch":{"removeIds":["native-message"],"orderIds":[]}}),
        json!({"$patch":{"id":"native-message","tokens":{"input":999},"content":"PRIVATE_PATCH"}}),
        json!({"$set":{"messages":[]}}),
    ] {
        assert_eq!(ingest(&db, &mut state, &edit), 0);
    }
    assert_eq!(rows(&db, "usage_responses"), 1);
    assert_eq!(rows(&db, "excluded_facts"), 0);
    assert_eq!(only_usage(&db).counts.total, Metric::Valid(132));
}

#[test]
fn checkpoint_and_old_json_migration_observe_native_ids_without_recharging() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    let first = message("first", tokens());
    let second = message("second", tokens());
    assert_eq!(ingest(&db, &mut state, &first), 1);
    assert_eq!(
        ingest(
            &db,
            &mut state,
            &json!({"$set":{"messages":[first,second]}})
        ),
        1
    );
    let mut snapshot = header();
    snapshot["messages"] = json!([message("first",tokens()),message("second",tokens()),
        {"type":"user","id":"user-id","timestamp":TIME,"content":"PRIVATE_USER"}]);
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    let records = GeminiAdapter
        .snapshot_records(&mut bytes.as_slice())
        .unwrap();
    assert_eq!(records.len(), 3);
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("PRIVATE_"));
    let mut restarted = Context::default();
    for record in records {
        assert_eq!(ingest(&db, &mut restarted, &record), 0);
    }
    assert_eq!(rows(&db, "usage_responses"), 2);
    assert_eq!(rows(&db, "identity_conflicts"), 0);
    assert!(GeminiAdapter.is_json_snapshot(Path::new("native.json")));
    assert!(!GeminiAdapter.is_json_snapshot(Path::new("native.jsonl")));
}

#[test]
fn root_header_messages_and_set_metadata_apply_without_retaining_text() {
    let db = database();
    let mut state = Context::default();
    let mut record = header();
    record["messages"] = json!([message("first", tokens())]);
    assert_eq!(ingest(&db, &mut state, &record), 1);
    assert_eq!(
        ingest(
            &db,
            &mut state,
            &json!({"$set":{"kind":"subagent","summary":"PRIVATE_SUMMARY","messages":[message("second",tokens())]}})
        ),
        1
    );
    assert_eq!(state.legacy.len(), 1);
    let context = &state.legacy[CONTEXT_KEY];
    assert_eq!(context["sessionId"], "synthetic-session");
    assert_eq!(context["kind"], "subagent");
    assert!(!context.to_string().contains("PRIVATE_") && !context.to_string().contains("messages"));
    assert!(serde_json::to_string(&state).unwrap().len() < 2048);
    assert_eq!(rows(&db, "turns"), 0);
    assert_eq!(rows(&db, "items"), 0);
    assert_eq!(rows(&db, "account_snapshots"), 0);
}

#[test]
fn model_or_native_time_conflict_cannot_hide_behind_identical_counts() {
    for field in ["model", "timestamp"] {
        let db = database();
        let mut state = Context::default();
        ingest(&db, &mut state, &header());
        let original = message("native-message", tokens());
        ingest(&db, &mut state, &original);
        let mut changed = original.clone();
        changed[field] = if field == "model" {
            json!("other-model")
        } else {
            json!("2026-10-23T01:00:00Z")
        };
        assert_eq!(ingest(&db, &mut state, &changed), 0);
        assert_eq!(rows(&db, "identity_conflicts"), 1);
        assert_eq!(rows(&db, "excluded_facts"), 1);
        let old = only_usage(&db);
        assert_eq!(
            old.observed_model.as_deref(),
            Some("synthetic-gemini-model")
        );
        assert_eq!(old.event_time, "2026-10-23T00:00:00.000Z");
    }
}

#[test]
fn equivalent_timezones_and_missing_to_known_model_are_safe_metadata_refinement() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    let mut unknown = message("native-message", tokens());
    unknown.as_object_mut().unwrap().remove("model");
    assert_eq!(ingest(&db, &mut state, &unknown), 1);
    let mut known = message("native-message", tokens());
    known["timestamp"] = json!("2026-10-23T08:00:00+08:00");
    assert_eq!(ingest(&db, &mut state, &known), 0);
    assert_eq!(rows(&db, "identity_conflicts"), 0);
    assert_eq!(
        only_usage(&db).observed_model.as_deref(),
        Some("synthetic-gemini-model")
    );
}

#[test]
fn partial_non_null_usage_is_not_silently_promoted_to_native_revision() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    assert_eq!(
        ingest(
            &db,
            &mut state,
            &message(
                "native-message",
                json!({"input":100,"output":20,"total":132})
            )
        ),
        1
    );
    assert_eq!(only_usage(&db).counts.total, Metric::Missing);
    assert_eq!(rows(&db, "ingestion_issues"), 1);
    assert_eq!(
        ingest(&db, &mut state, &message("native-message", tokens())),
        0
    );
    assert_eq!(rows(&db, "identity_conflicts"), 1);
    assert_eq!(rows(&db, "excluded_facts"), 1);
}

#[test]
fn invalid_native_identity_or_time_has_no_filename_mtime_or_header_time_fallback() {
    for field in ["id", "timestamp"] {
        let db = database();
        let mut state = Context::default();
        let mut start = header();
        start["startTime"] = json!(TIME);
        ingest(&db, &mut state, &start);
        let mut event = message("native-message", tokens());
        event.as_object_mut().unwrap().remove(field);
        assert_eq!(ingest(&db, &mut state, &event), 0);
        assert_eq!(rows(&db, "usage_responses"), 0);
        assert_eq!(rows(&db, "ingestion_issues"), 1);
    }
    for invalid in [
        json!("not-a-time"),
        json!("2026-10-23T00:00:00"),
        Value::Null,
    ] {
        let mut event = standalone(tokens());
        event["timestamp"] = invalid;
        assert!(GeminiAdapter
            .normalize(&observation(project(&event)))
            .is_err());
    }
    let db = database();
    let mut state = Context::default();
    assert_eq!(
        ingest(&db, &mut state, &message("native-message", tokens())),
        0
    );
    assert_eq!(rows(&db, "usage_responses"), 0);
}

#[test]
fn changed_session_or_project_poison_only_current_metadata_context() {
    for changed in [
        json!({"sessionId":"other-session"}),
        json!({"$set":{"projectHash":"b".repeat(64)}}),
    ] {
        let db = database();
        let mut state = Context::default();
        ingest(&db, &mut state, &header());
        ingest(&db, &mut state, &message("first", tokens()));
        ingest(&db, &mut state, &changed);
        assert_eq!(ingest(&db, &mut state, &message("second", tokens())), 0);
        // 后续再发原 header 不能抹去同游标的冲突。
        ingest(&db, &mut state, &header());
        assert_eq!(ingest(&db, &mut state, &message("third", tokens())), 0);
        assert_eq!(rows(&db, "usage_responses"), 1);
        assert_eq!(state.legacy[CONTEXT_KEY]["invalidSession"], true);
        let mut new_generation = Context::default();
        ingest(&db, &mut new_generation, &header());
        assert_eq!(
            ingest(&db, &mut new_generation, &message("second", tokens())),
            1
        );
    }
}

#[test]
fn source_domains_are_independent_and_replayed_rows_use_same_identity_policy() {
    let db = database();
    let mut state = Context::default();
    let event = observation(project(&standalone(tokens())));
    assert_eq!(
        GeminiAdapter
            .ingest_local(&db, &event, "synthetic", &mut state, None)
            .unwrap(),
        1
    );
    assert_eq!(
        GeminiAdapter
            .ingest_replayed_local(&db, &event, "synthetic", &mut state, None)
            .unwrap(),
        0
    );
    let mut other = event.clone();
    other.source_id = "other-source".into();
    other.channel_id = "other-source:jsonl".into();
    assert_eq!(
        GeminiAdapter
            .ingest_local(&db, &other, "synthetic", &mut Context::default(), None)
            .unwrap(),
        1
    );
    assert_eq!(rows(&db, "usage_responses"), 2);
    assert_eq!(rows(&db, "identity_conflicts"), 0);
    assert_eq!(
        GeminiAdapter.identity_policy().scope,
        "source+native-sessionId+message.id"
    );
}

#[test]
fn fact_reference_uses_native_context_and_never_aliases_a_checkpoint_batch() {
    let db = database();
    let mut state = Context::default();
    ingest(&db, &mut state, &header());
    let event = observation(project(&message("native-message", tokens())));
    assert!(GeminiAdapter.fact_reference(&event).is_none());
    assert!(GeminiAdapter
        .fact_reference_with_context(&event, &Context::default())
        .is_none());
    GeminiAdapter
        .ingest_local(&db, &event, "synthetic", &mut state, None)
        .unwrap();
    assert_eq!(
        GeminiAdapter.fact_reference_with_context(&event, &state),
        Some(response_key(&only_usage(&db)))
    );
    let checkpoint = observation(project(
        &json!({"$set":{"messages":[message("native-message",tokens())]}}),
    ));
    assert!(GeminiAdapter
        .fact_reference_with_context(&checkpoint, &state)
        .is_none());
}

#[test]
fn cutoff_and_transaction_rollback_preserve_exact_once_facts() {
    let db = database();
    let event = observation(project(&standalone(tokens())));
    let cutoff = chrono::DateTime::parse_from_rfc3339("2026-10-23T00:00:01Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(
        GeminiAdapter
            .ingest_local(
                &db,
                &event,
                "synthetic",
                &mut Context::default(),
                Some(&cutoff)
            )
            .unwrap(),
        0
    );
    assert_eq!(rows(&db, "usage_responses"), 0);
    db.execute_batch("BEGIN").unwrap();
    assert_eq!(
        GeminiAdapter
            .ingest_local(&db, &event, "synthetic", &mut Context::default(), None)
            .unwrap(),
        1
    );
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(rows(&db, "usage_responses"), 0);
    assert_eq!(
        GeminiAdapter
            .ingest_local(&db, &event, "synthetic", &mut Context::default(), None)
            .unwrap(),
        1
    );
    assert_eq!(
        GeminiAdapter
            .ingest_local(&db, &event, "synthetic", &mut Context::default(), None)
            .unwrap(),
        0
    );
}

#[test]
fn parser_rejects_duplicate_or_unsafe_known_fields_without_leaking_error_values() {
    for bytes in [
        br#"{"sessionId":"first","sessionId":"PRIVATE_DUPLICATE"}"#.as_slice(),
        br#"{"sessionId":123,"content":"PRIVATE_BODY"}"#.as_slice(),
        br#"{"type":"gemini","tokens":{"input":1,"input":2}}"#.as_slice(),
    ] {
        let error = GeminiAdapter.parse_line(bytes).unwrap_err();
        assert!(!error.contains("PRIVATE_") && !error.contains("123"));
    }
    for id in [
        "bad|identity".to_owned(),
        "bad\nidentity".to_owned(),
        "bad\u{202e}identity".to_owned(),
        "x".repeat(MAX_TEXT_BYTES + 1),
    ] {
        let error = GeminiAdapter
            .parse_line(&serde_json::to_vec(&json!({"sessionId":id})).unwrap())
            .unwrap_err();
        assert!(!error.contains("bad"));
    }
    assert!(GeminiAdapter
        .parse_line(&vec![b' '; MAX_LINE_BYTES + 1])
        .is_err());
    assert!(GeminiAdapter
        .parse_line(&serde_json::to_vec(&json!({"projectHash":"PRIVATE_PATH"})).unwrap())
        .is_err());
}

#[test]
fn snapshot_budget_message_limit_and_contract_are_bounded() {
    let too_large = vec![b' '; MAX_SNAPSHOT_BYTES + 1];
    assert!(GeminiAdapter
        .snapshot_records(&mut too_large.as_slice())
        .unwrap_err()
        .contains("16 MiB"));
    for event in [
        json!({"messages":[]}),
        json!({"sessionId":"s"}),
        json!({"sessionId":"s","messages":[],"$set":{}}),
    ] {
        let bytes = serde_json::to_vec(&event).unwrap();
        assert!(GeminiAdapter
            .snapshot_records(&mut bytes.as_slice())
            .is_err());
    }
    let mut oversized = header();
    oversized["messages"] = json!((0..=MAX_MESSAGES)
        .map(|_| json!({"type":"info"}))
        .collect::<Vec<_>>());
    let bytes = serde_json::to_vec(&oversized).unwrap();
    assert!(GeminiAdapter
        .snapshot_records(&mut bytes.as_slice())
        .is_err());
    let combined = json!({"messages":(0..MAX_MESSAGES).map(|_|json!({"type":"info"})).collect::<Vec<_>>(),"$set":{"messages":[{"type":"info"}]}});
    assert!(GeminiAdapter
        .parse_line(&serde_json::to_vec(&combined).unwrap())
        .is_err());
}

#[test]
fn snapshot_read_failure_does_not_publish_a_partial_record_set() {
    struct BrokenRead;
    impl Read for BrokenRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("PRIVATE_IO_DETAIL"))
        }
    }
    let error = GeminiAdapter.snapshot_records(&mut BrokenRead).unwrap_err();
    assert!(!error.contains("PRIVATE_"));
}

#[test]
fn snapshot_streaming_checks_reader_during_body_and_rejects_trailing_json() {
    use std::io::Cursor;
    struct CancelDuringBody {
        input: Cursor<Vec<u8>>,
        remaining: usize,
    }
    impl Read for CancelDuringBody {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "PRIVATE_CANCEL_DETAIL",
                ));
            }
            let count = buffer.len().min(self.remaining).min(32);
            let read = self.input.read(&mut buffer[..count])?;
            self.remaining -= read;
            Ok(read)
        }
    }
    let mut snapshot = header();
    let mut record = message("native-message", tokens());
    record["content"] = json!("PRIVATE_BODY".repeat(1024));
    snapshot["messages"] = json!([record]);
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    let mut cancelled = CancelDuringBody {
        input: Cursor::new(bytes.clone()),
        remaining: 256,
    };
    // 不能因为已读到元数据前缀就发布部分 snapshot；中途取消必须整组失败。
    assert!(GeminiAdapter.snapshot_records(&mut cancelled).is_err());
    let mut trailing = bytes;
    trailing.extend_from_slice(b" {} ");
    assert!(GeminiAdapter
        .snapshot_records(&mut trailing.as_slice())
        .is_err());
}

#[test]
fn no_lifecycle_account_quota_or_fake_success_is_inferred() {
    for event in [
        json!({"type":"user","id":"u","timestamp":TIME,"tokens":tokens()}),
        json!({"type":"info","stop_reason":"end_turn","status":"completed"}),
        json!({"$patch":{"id":"native-message","stop_reason":"end_turn","tokens":tokens()}}),
    ] {
        assert!(GeminiAdapter
            .normalize(&observation(project(&event)))
            .unwrap()
            .is_empty());
    }
    for key in [
        "turnCompletion",
        "turnTiming",
        "requestTiming",
        "subagents",
        "context",
        "chatTarget",
        "quota",
        "accountUsage",
        "cacheWrite",
    ] {
        assert_eq!(
            GeminiAdapter.capabilities()[key]["support"],
            "不支持",
            "{key}"
        );
    }
    assert!(GeminiAdapter.build_chat_target("native-session").is_none());
}

#[test]
fn discovery_visits_only_registered_tmp_project_chats_json_or_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("registered-gemini");
    fs::create_dir_all(root.join("tmp/project-a/chats/nested")).unwrap();
    fs::create_dir_all(root.join("tmp/project-b/chats")).unwrap();
    fs::create_dir_all(root.join("other/chats")).unwrap();
    for relative in [
        "tmp/project-a/chats/one.json",
        "tmp/project-a/chats/nested/two.jsonl",
        "tmp/project-b/chats/three.jsonl",
        "settings.json",
        "oauth_creds.json",
        "tmp/ignored.json",
        "tmp/project-a/config.json",
        "other/chats/not-chat.json",
        "tmp/project-a/chats/bad.json.unreadable-1",
    ] {
        fs::write(root.join(relative), "{}").unwrap();
    }
    fs::write(
        root.join("tmp/project-a/chats/archive.jsonl.gz"),
        "synthetic",
    )
    .unwrap();
    let found = GeminiAdapter.discover(&source(&root)).unwrap();
    assert_eq!(found.files.len(), 3);
    assert_eq!(found.unsupported, 1);
    assert!(found.errors.is_empty() && !found.cancelled);
    let selected_tmp = GeminiAdapter.discover(&source(&root.join("tmp"))).unwrap();
    assert_eq!(selected_tmp.files, found.files);
    assert!(GeminiAdapter
        .discover(&source(Path::new("relative")))
        .is_err());
}

#[test]
fn discovery_keeps_readable_siblings_and_marks_partial_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("registered-gemini");
    let good = root.join("tmp/good/chats");
    let bad = root.join("tmp/bad/chats");
    fs::create_dir_all(&good).unwrap();
    fs::create_dir_all(&bad).unwrap();
    fs::write(good.join("native.jsonl"), "{}").unwrap();
    let found = discover_with(&source(&root), &AtomicBool::new(false), &|path| {
        if path == bad {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "synthetic",
            ))
        } else {
            fs::read_dir(path)
        }
    })
    .unwrap();
    assert_eq!(found.files, vec![good.join("native.jsonl")]);
    assert_eq!(found.errors, vec![bad]);
    let cancelled = GeminiAdapter
        .discover_with_cancel(&source(&root), &AtomicBool::new(true))
        .unwrap();
    assert!(cancelled.cancelled && cancelled.files.is_empty());
    let cancel = AtomicBool::new(false);
    let mid = discover_with(&source(&root), &cancel, &|path| {
        let result = fs::read_dir(path);
        cancel.store(true, Ordering::Relaxed);
        result
    })
    .unwrap();
    assert!(mid.cancelled);
}

#[test]
fn discovery_depth_limit_marks_coverage_gap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("registered-gemini");
    let chats = root.join("tmp/project/chats");
    fs::create_dir_all(&chats).unwrap();
    let mut deepest = chats;
    for _ in 0..=MAX_DISCOVERY_DEPTH {
        deepest = deepest.join("d");
        fs::create_dir(&deepest).unwrap();
    }
    fs::write(deepest.join("too-deep.jsonl"), "{}").unwrap();
    let found = GeminiAdapter.discover(&source(&root)).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.errors, vec![deepest]);
}

#[cfg(unix)]
#[test]
fn discovery_rejects_symlink_root_ancestor_and_entries() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("registered-gemini");
    let chats = root.join("tmp/project/chats");
    fs::create_dir_all(&chats).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("private.jsonl"), "{}").unwrap();
    symlink(outside.join("private.jsonl"), chats.join("linked.jsonl")).unwrap();
    symlink(&outside, chats.join("linked-dir")).unwrap();
    let found = GeminiAdapter.discover(&source(&root)).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.unsupported, 2);
    let alias = dir.path().join("alias");
    symlink(&root, &alias).unwrap();
    let found = GeminiAdapter.discover(&source(&alias.join("tmp"))).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.unsupported, 1);
}

#[cfg(windows)]
#[test]
fn discovery_rejects_windows_junction_entries_and_ancestor() {
    // 测试只在 tempfile 自有目录内创建/移除 junction，不修改系统设置或用户来源。
    use std::os::windows::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("registered-gemini");
    let chats = root.join("tmp").join("project").join("chats");
    fs::create_dir_all(&chats).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("private.jsonl"), "{}").unwrap();
    let junction = chats.join("junction");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(status.status.success());
    let found = GeminiAdapter.discover(&source(&root)).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.unsupported, 1);
    let alias = dir.path().join("alias");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&alias)
        .arg(&root)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(status.status.success());
    let found = GeminiAdapter.discover(&source(&alias.join("tmp"))).unwrap();
    assert!(found.files.is_empty());
    assert_eq!(found.unsupported, 1);
    fs::remove_dir(&alias).unwrap();
    fs::remove_dir(&junction).unwrap();
}
