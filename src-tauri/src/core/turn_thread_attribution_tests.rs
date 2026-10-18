//! A real ingestion regression for index-independent lifecycle attribution.
use super::*;

const OTHER_THREAD: &str = "00000000-0000-4000-8000-000000000002";

fn response(thread: &str, id: &str, turn: &str) -> Value {
    let mut value = native(thread, id, 100);
    value["payload"]["turn_id"] = json!(turn);
    value["payload"]["root_turn_id"] = json!(turn);
    value
}

#[test]
fn turn_thread_attribution_is_unique_source_scoped_and_independent_of_composite_index() {
    let mut outcomes = Vec::new();
    for indexed in [false, true] {
        let mut r = Rig::new();
        // Each branch owns a fresh database. Change only this index to prove
        // that index order cannot become lifecycle attribution evidence.
        r.core
            .db
            .execute_batch("DROP INDEX IF EXISTS usage_source_turn_thread;")
            .unwrap();
        if indexed {
            r.core.db.execute_batch("CREATE INDEX usage_source_turn_thread ON usage_responses(source_id,turn_id,thread_id);").unwrap();
        }
        direct(
            &r.core,
            &[
                response(OTHER_THREAD, "ambiguous-first", "ambiguous-turn"),
                response(THREAD, "ambiguous-second", "ambiguous-turn"),
            ],
        );
        let before = json_rows(&r.core.db, "usage_responses").unwrap();
        let mut ambiguous_context = reader::Context {
            owner: THREAD.into(),
            active_thread: THREAD.into(),
            current_turn: "preceding-turn".into(),
            ..Default::default()
        };
        for _ in 0..2 {
            for value in [
                event(
                    "event_msg",
                    json!({"type":"task_started","thread_id":THREAD,"turn_id":"ambiguous-turn","root_turn_id":"ambiguous-turn"}),
                ),
                event(
                    "event_msg",
                    json!({"type":"task_complete","thread_id":OTHER_THREAD,"turn_id":"ambiguous-turn","root_turn_id":"ambiguous-turn","status":"completed"}),
                ),
            ] {
                adapter::ingest(
                    &r.core.db,
                    &r.core.source_id,
                    "fixture",
                    &value,
                    &mut ambiguous_context,
                    None,
                )
                .unwrap();
            }
        }
        assert!(ambiguous_context.current_turn.is_empty());
        assert_eq!(ambiguous_context.owner, THREAD);
        assert_eq!(ambiguous_context.active_thread, THREAD);
        adapter::ingest(&r.core.db, &r.core.source_id, "fixture", &event("event_msg", json!({"type":"exec_command_end","call_id":"ambiguous-following-tool","exit_code":0})), &mut ambiguous_context, None).unwrap();
        let ambiguous = r.snapshot();
        assert_eq!(ambiguous["overview"]["totalTokens"], 240);
        assert!(ambiguous["turns"].as_array().unwrap().is_empty());
        assert!(ambiguous["inbox"].as_array().unwrap().is_empty());
        assert_eq!(ambiguous["tools"].as_array().unwrap().len(), 1);
        assert!(ambiguous["tools"][0]["turnId"].is_null());
        assert!(r.core.claim_notifications().unwrap().is_empty());
        assert_eq!(json_rows(&r.core.db, "usage_responses").unwrap(), before);
        let issues = ambiguous["health"]["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0]["kind"], "ambiguous_turn_thread");
        assert_eq!(
            issues[0]["count"], 1,
            "duplicate lifecycle records share the stable issue identity"
        );

        // Multiple responses in one thread are not ambiguity; the same raw turn
        // in another source cannot change this source's unique ownership.
        let other_root = r._tmp.path().join("other-source");
        fs::create_dir(&other_root).unwrap();
        let other_source = r.core.add_source(&other_root).unwrap();
        let mut ctx = reader::Context::default();
        adapter::ingest(
            &r.core.db,
            &other_source,
            "other-source-fixture",
            &response(OTHER_THREAD, "other-source-response", "unique-turn"),
            &mut ctx,
            None,
        )
        .unwrap();
        direct(
            &r.core,
            &[
                response(THREAD, "unique-one", "unique-turn"),
                response(THREAD, "unique-two", "unique-turn"),
                event(
                    "event_msg",
                    json!({"type":"task_started","thread_id":OTHER_THREAD,"turn_id":"unique-turn","root_turn_id":"unique-turn"}),
                ),
                event(
                    "event_msg",
                    json!({"type":"task_complete","thread_id":OTHER_THREAD,"turn_id":"unique-turn","root_turn_id":"unique-turn","status":"completed"}),
                ),
            ],
        );
        let result = r.snapshot();
        assert_eq!(result["overview"]["totalTokens"], 600);
        assert_eq!(result["turns"].as_array().unwrap().len(), 1);
        assert_eq!(result["turns"][0]["threadId"], THREAD);
        assert_eq!(result["turns"][0]["sourceId"], r.core.source_id);
        assert_eq!(result["inbox"].as_array().unwrap().len(), 1);
        assert_eq!(result["inbox"][0]["threadId"], THREAD);
        assert_eq!(result["inbox"][0]["status"], "completed");
        outcomes.push(json!({"total":result["overview"]["totalTokens"],"thread":result["turns"][0]["threadId"],"result":result["inbox"][0]["status"],"issue":result["health"]["issues"][0]["kind"]}));
    }
    assert_eq!(outcomes[0], outcomes[1]);

    // Independent arrival-order regression: lifecycle/context metadata can be
    // newer than the native response without being stronger ownership evidence.
    for sequence in [
        "chronological",
        "late-usage",
        "newer-context",
        "after-terminal",
    ] {
        let r = Rig::new();
        let base = Utc::now() - chrono::Duration::seconds(60);
        let stamp = |mut value: Value, seconds| {
            value["timestamp"] = json!((base + chrono::Duration::seconds(seconds))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
            value
        };
        let usage = stamp(response(THREAD, "arrival-response", "arrival-turn"), 3);
        let start = stamp(
            event(
                "event_msg",
                json!({"type":"task_started","thread_id":OTHER_THREAD,"turn_id":"arrival-turn","root_turn_id":"arrival-turn"}),
            ),
            5,
        );
        let completed = stamp(
            event(
                "event_msg",
                json!({"type":"task_complete","thread_id":OTHER_THREAD,"turn_id":"arrival-turn","root_turn_id":"arrival-turn","status":"completed"}),
            ),
            6,
        );
        let context = stamp(
            event(
                "turn_context",
                json!({"thread_id":OTHER_THREAD,"turn_id":"arrival-turn","root_turn_id":"arrival-turn","model":"recorded-model"}),
            ),
            8,
        );
        let mut retained_inbox = None;
        if sequence == "after-terminal" {
            direct(&r.core, &[start, completed]);
            let mut inbox = json_rows(&r.core.db, "inbox_events").unwrap().remove(0);
            // Authored delivery markers only: no OS notification is sent here.
            inbox["read"] = json!(true);
            inbox["notified"] = json!(true);
            inbox["animationAcknowledged"] = json!(true);
            inbox["notificationState"] = json!("sent");
            r.core
                .db
                .execute(
                    "UPDATE inbox_events SET data=?1 WHERE id=?2",
                    params![inbox.to_string(), strv(&inbox, "id")],
                )
                .unwrap();
            retained_inbox = Some(inbox);
            direct(&r.core, &[usage]);
        } else {
            let ordered = match sequence {
                "chronological" => vec![usage, start, completed, context],
                "late-usage" => vec![start, usage, completed],
                _ => vec![start, context, usage, completed],
            };
            direct(&r.core, &ordered);
        }
        let result = r.snapshot();
        assert_eq!(result["overview"]["totalTokens"], 120, "{sequence}");
        assert_eq!(
            result["turns"][0]["threadId"], THREAD,
            "unique native owner must survive {sequence}"
        );
        assert_eq!(
            result["inbox"][0]["threadId"], THREAD,
            "inbox must project the accepted native owner for {sequence}"
        );
        assert_eq!(result["inbox"][0]["status"], "completed");
        if let Some(mut retained) = retained_inbox {
            retained["threadId"] = json!(THREAD);
            let corrected = adapter::load_fact(&r.core.db, "inbox_events", &strv(&retained, "id"))
                .unwrap()
                .unwrap();
            assert_eq!(
                corrected, retained,
                "late ownership correction changes no inbox identity, result or delivery metadata"
            );
            direct(
                &r.core,
                &[stamp(
                    response(OTHER_THREAD, "late-conflicting-owner", "arrival-turn"),
                    4,
                )],
            );
            let ambiguous = r.snapshot();
            assert_eq!(ambiguous["turns"][0]["threadId"], "");
            assert_eq!(
                ambiguous["turns"][0]["threadAttribution"],
                "ambiguous-native-owner"
            );
            assert_eq!(ambiguous["inbox"][0]["threadId"], "");
            assert_eq!(ambiguous["inbox"][0]["notificationState"], "sent");
            assert_eq!(ambiguous["inbox"][0]["notified"], true);
            assert_eq!(ambiguous["inbox"][0]["read"], true);
            assert_eq!(ambiguous["inbox"][0]["notifyEligible"], false);
            assert_eq!(ambiguous["overview"]["totalTokens"], 240);
        }
    }
}
