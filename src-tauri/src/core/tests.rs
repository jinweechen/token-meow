use super::adapter::{
    response_key, CodexAdapter, Metric, Observation, ResponseUsage, SourceAdapter, StandardEvent,
};
use super::*;
use std::io::Write;
const THREAD: &str = "00000000-0000-4000-8000-000000000001";
struct Rig {
    _tmp: tempfile::TempDir,
    source: PathBuf,
    data: PathBuf,
    core: Core,
}
impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let data = tmp.path().join("pet");
        fs::create_dir_all(&source).unwrap();
        let core = Core::open(&data, &source).unwrap();
        core.notification_gate().publish(true,false);
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        Self {
            _tmp: tmp,
            source,
            data,
            core,
        }
    }
    fn fixture(&self) -> String {
        include_str!("../../../tests/fixtures/codex-native.jsonl")
            .replace("2026-10-15", &Utc::now().format("%Y-%m-%d").to_string())
    }
    fn put(&self, name: &str, text: &str) -> PathBuf {
        let p = self.source.join(name);
        fs::write(&p, text.as_bytes()).unwrap();
        p
    }
    fn scan(&mut self) {
        for _ in 0..5 {
            self.core.scan().unwrap();
        }
    }
    fn snapshot(&self) -> Value {
        self.core.snapshot(json!({})).unwrap()
    }
}
fn event(kind: &str, p: Value) -> Value {
    json!({"timestamp":now(),"type":kind,"payload":p})
}
fn native(thread: &str, id: &str, input: i64) -> Value {
    event(
        "token_usage_record",
        json!({"thread_id":thread,"response_id":id,"turn_id":"turn-a","root_turn_id":"turn-a","usage":{"input_tokens":input,"output_tokens":20,"cached_input_tokens":50,"cache_write_input_tokens":0,"reasoning_output_tokens":10,"total_tokens":input+20}}),
    )
}
fn normalized(v: Value, source: &str) -> ResponseUsage {
    let o = Observation {
        source_id: source.into(),
        channel_id: format!("{source}:jsonl"),
        event: v,
    };
    match CodexAdapter.normalize(&o).unwrap().remove(0) {
        StandardEvent::ResponseUsage(r) => r,
        _ => panic!(),
    }
}
fn direct(core: &Core, values: &[Value]) {
    let mut ctx = reader::Context::default();
    for v in values {
        adapter::ingest(&core.db, &core.source_id, "fixture", v, &mut ctx, None).unwrap();
    }
}

#[test]
fn fixture_native_and_cumulative_are_not_added_twice() {
    let mut r = Rig::new();
    r.put("one.jsonl", &r.fixture());
    r.scan();
    let s = r.snapshot();
    assert_eq!(s["overview"]["totalTokens"], 1200);
    assert_eq!(s["overview"]["responseCount"], 1);
    assert_eq!(s["overview"]["cacheHitRate"], 0.8);
    assert_eq!(s["overview"]["normalInputTokens"], 100);
    assert_eq!(s["overview"]["legacyTokens"], 0);
    assert_eq!(s["tools"][0]["exitCode"], 7);
    assert_eq!(s["tools"][0]["durationMs"], 1500);
    assert_eq!(s["inbox"][0]["status"], "unknown");
    assert_eq!(s["inbox"][0]["notifyEligible"], false);
    assert_eq!(s["turns"][0]["timeToFirstTokenMs"], 500);
    assert!(r.core.has_thread(THREAD).unwrap());
}
#[test]
fn repeat_copy_archive_and_restart_preserve_exact_totals() {
    let mut r = Rig::new();
    let p = r.put("one.jsonl", &r.fixture());
    r.scan();
    fs::copy(&p, r.source.join("copy.jsonl")).unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1200);
    let id: i64 = r
        .core
        .db
        .query_row(
            "SELECT id FROM source_files WHERE path=?1",
            [p.to_string_lossy().to_string()],
            |x| x.get(0),
        )
        .unwrap();
    let archive = r.source.join("archived_sessions");
    fs::create_dir(&archive).unwrap();
    fs::rename(&p, archive.join("one.jsonl")).unwrap();
    r.scan();
    let moved: i64 = r
        .core
        .db
        .query_row(
            "SELECT id FROM source_files WHERE path=?1",
            [archive.join("one.jsonl").to_string_lossy().to_string()],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(id, moved);
    r.core = Core::open(&r.data, &r.source).unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1200);
    assert_eq!(r.snapshot()["inbox"].as_array().unwrap().len(), 1);
}
#[test]
fn byte_cursor_waits_for_split_utf8_complete_line() {
    let mut r = Rig::new();
    let line = event("session_meta", json!({"id":THREAD,"cwd":"D:/猫娘"})).to_string() + "\n";
    let split = line.as_bytes().iter().position(|b| *b >= 128).unwrap() + 1;
    let p = r.put(
        "split.jsonl",
        std::str::from_utf8(&line.as_bytes()[..split - 1]).unwrap(),
    );
    let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(&line.as_bytes()[split - 1..split]).unwrap();
    r.scan();
    let cursor: i64 = r
        .core
        .db
        .query_row("SELECT offset FROM source_files LIMIT 1", [], |x| x.get(0))
        .unwrap();
    assert_eq!(cursor, 0);
    f.write_all(&line.as_bytes()[split..]).unwrap();
    f.write_all((native(THREAD, "split-response", 100).to_string() + "\n").as_bytes())
        .unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
    assert_eq!(r.snapshot()["sessions"][0]["cwd"], "D:/猫娘");
}
#[test]
fn truncate_and_replace_same_path_start_new_physical_identity_without_double_usage() {
    let mut r = Rig::new();
    let p = r.put("one.jsonl", &r.fixture());
    r.scan();
    fs::write(
        &p,
        (native(THREAD, "response-two", 100).to_string() + "\n").as_bytes(),
    )
    .unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1320);
    fs::remove_file(&p).unwrap();
    r.put("one.jsonl", &r.fixture());
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1320);
    let count: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM source_files", [], |x| x.get(0))
        .unwrap();
    assert!(count >= 2);
}
#[test]
fn before_commit_failure_rolls_back_facts_and_cursor_after_commit_restart_is_idempotent() {
    let mut r = Rig::new();
    r.put("one.jsonl", &r.fixture());
    put_setting(&r.core.db, "testFailBeforeCommit", &json!(true)).unwrap();
    r.core.scan().unwrap();
    assert_eq!(r.snapshot()["overview"]["responseCount"], 0);
    let n: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM ingested_records", [], |x| x.get(0))
        .unwrap();
    assert_eq!(n, 0);
    put_setting(&r.core.db, "testFailBeforeCommit", &json!(false)).unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1200);
    r.core.shutdown().unwrap();
    r.core = Core::open(&r.data, &r.source).unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1200);
}
#[test]
fn conflicting_identity_excludes_whole_fact_and_never_overwrites_original() {
    let r = Rig::new();
    let a = normalized(native(THREAD, "same", 100), &r.core.source_id);
    let b = normalized(native(THREAD, "same", 101), &r.core.source_id);
    adapter::store_response(&r.core.db, &a, false).unwrap();
    adapter::store_response(&r.core.db, &b, false).unwrap();
    let s = r.snapshot();
    assert!(s["overview"]["totalTokens"].is_null());
    assert_eq!(s["health"]["conflicts"], 1);
    let old = adapter::load_fact(&r.core.db, "usage_responses", &response_key(&a))
        .unwrap()
        .unwrap();
    assert_eq!(old["counts"]["input"]["value"], 100);
}
#[test]
fn metadata_enrichment_does_not_change_usage_identity() {
    let r = Rig::new();
    let a = normalized(native(THREAD, "same", 100), &r.core.source_id);
    let mut b = a.clone();
    b.requested_model = Some("late-model".into());
    adapter::store_response(&r.core.db, &a, false).unwrap();
    adapter::store_response(&r.core.db, &b, false).unwrap();
    let s = r.snapshot();
    assert_eq!(s["overview"]["totalTokens"], 120);
    assert_eq!(s["health"]["conflicts"], 0);
    assert_eq!(s["models"][0]["name"], "late-model");
}
#[test]
fn independent_namespaces_same_short_id_are_separate_legitimate_usage() {
    let r = Rig::new();
    let a = normalized(native(THREAD, "same-short-id", 100), &r.core.source_id);
    let mut b = a.clone();
    b.namespace = "synthetic-second-tool-session".into();
    b.source_id = "synthetic-independent".into();
    adapter::store_response(&r.core.db, &a, false).unwrap();
    adapter::store_response(&r.core.db, &b, false).unwrap();
    let s = r.snapshot();
    assert_eq!(s["overview"]["totalTokens"], 240);
    assert_eq!(s["health"]["conflicts"], 0);
}
#[test]
fn verified_stream_revision_never_accepts_late_partial_over_final() {
    let r = Rig::new();
    let mut a = normalized(native(THREAD, "rev", 100), &r.core.source_id);
    a.phase = "partial".into();
    a.revision = Some(1);
    adapter::store_response(&r.core.db, &a, true).unwrap();
    let mut final_r = a.clone();
    final_r.phase = "final".into();
    final_r.revision = Some(2);
    final_r.counts.input = Metric::Valid(200);
    final_r.counts.total = Metric::Valid(220);
    adapter::store_response(&r.core.db, &final_r, true).unwrap();
    adapter::store_response(&r.core.db, &a, true).unwrap();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 220);
    assert_eq!(r.snapshot()["health"]["conflicts"], 0);
}
#[test]
fn codex_native_does_not_allow_metering_revisions() {
    let r = Rig::new();
    let mut a = normalized(native(THREAD, "rev", 100), &r.core.source_id);
    a.revision = Some(1);
    let mut b = a.clone();
    b.revision = Some(2);
    b.counts.input = Metric::Valid(200);
    b.counts.total = Metric::Valid(220);
    adapter::store_response(&r.core.db, &a, false).unwrap();
    adapter::store_response(&r.core.db, &b, false).unwrap();
    assert_eq!(r.snapshot()["health"]["conflicts"], 1);
}
#[test]
fn missing_zero_invalid_and_unknown_are_distinct() {
    let mut v = native(THREAD, "missing", 100);
    v["payload"]["usage"]
        .as_object_mut()
        .unwrap()
        .remove("cache_write_input_tokens");
    let r = normalized(v, "s");
    assert_eq!(r.counts.cache_write, Metric::Missing);
    let z = normalized(native(THREAD, "zero", 100), "s");
    assert_eq!(z.counts.cache_write, Metric::Valid(0));
    let mut v = native(THREAD, "bad", 100);
    v["payload"]["usage"]["cached_input_tokens"] = json!(101);
    let bad = normalized(v, "s");
    assert_eq!(bad.counts.cache_read, Metric::Invalid);
    assert_ne!(Metric::Unknown, Metric::Missing);
}
#[test]
fn cache_ratio_uses_only_valid_subset_and_reports_input_coverage() {
    let r = Rig::new();
    let a = normalized(native(THREAD, "a", 100), &r.core.source_id);
    let mut v = native(THREAD, "b", 300);
    v["payload"]["usage"]
        .as_object_mut()
        .unwrap()
        .remove("cached_input_tokens");
    let b = normalized(v, &r.core.source_id);
    adapter::store_response(&r.core.db, &a, false).unwrap();
    adapter::store_response(&r.core.db, &b, false).unwrap();
    let s = r.snapshot();
    assert_eq!(s["overview"]["cacheHitRate"], 0.5);
    assert_eq!(s["overview"]["cacheCoverage"], 0.25);
}
#[test]
fn child_inherits_parent_history_without_reassigning_parent_usage_or_notifying_child() {
    let r = Rig::new();
    let mut ctx = reader::Context::default();
    let child = "00000000-0000-4000-8000-000000000002";
    let values = [
        json!({"ordinal":0,"timestamp":now(),"type":"session_meta","payload":{"id":child,"parent_thread_id":THREAD,"subagent_history_start_ordinal":5,"cwd":"D:/child"}}),
        json!({"ordinal":1,"timestamp":now(),"type":"session_meta","payload":{"id":THREAD,"cwd":"D:/root"}}),
        native(THREAD, "parent", 100),
        json!({"ordinal":6,"timestamp":now(),"type":"turn_context","payload":{"turn_id":"child-turn","root_turn_id":"root-turn","model":"child-model"}}),
        json!({"ordinal":7,"timestamp":now(),"type":"token_usage_record","payload":{"thread_id":child,"turn_id":"child-turn","root_turn_id":"root-turn","response_id":"child","usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120}}}),
        event(
            "event_msg",
            json!({"type":"task_complete","turn_id":"child-turn","root_turn_id":"root-turn"}),
        ),
    ];
    for v in values {
        adapter::ingest(&r.core.db, &r.core.source_id, "fixture", &v, &mut ctx, None).unwrap();
    }
    let s = r.snapshot();
    assert_eq!(s["overview"]["totalTokens"], 240);
    assert_eq!(s["inbox"].as_array().unwrap().len(), 0);
    let child = s["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == child)
        .unwrap();
    assert_eq!(child["parentThreadId"], THREAD);
    // The parent relationship is explicit, but that parent's own kind is unknown.
    assert_eq!(child["agentKind"], "subagent");
    assert!(child["rootThreadId"].is_null());
}

#[path = "agent_kind_tests.rs"]
mod agent_kind_contract;
#[path = "query_index_tests.rs"]
mod query_index_contract;
#[path = "retention_index_tests.rs"]
mod retention_index_contract;
#[path = "turn_thread_attribution_tests.rs"]
mod turn_thread_attribution_contract;
#[test]
fn completion_refinement_updates_one_record_and_preserves_read_and_delivery() {
    let mut r = Rig::new();
    direct(
        &r.core,
        &[
            event("session_meta", json!({"id":THREAD})),
            event(
                "event_msg",
                json!({"type":"task_complete","turn_id":"t","root_turn_id":"t"}),
            ),
        ],
    );
    let id = strv(&r.snapshot()["inbox"][0], "id");
    r.core.action("mark_read", &json!({"id":id})).unwrap();
    r.core
        .action("acknowledge_delivery", &json!({"id":id}))
        .unwrap();
    direct(
        &r.core,
        &[event(
            "event_msg",
            json!({"type":"task_complete","turn_id":"t","root_turn_id":"t","status":"completed"}),
        )],
    );
    let s = r.snapshot();
    assert_eq!(s["inbox"].as_array().unwrap().len(), 1);
    assert_eq!(s["inbox"][0]["status"], "completed");
    assert_eq!(s["inbox"][0]["read"], true);
    assert_eq!(s["inbox"][0]["notifyEligible"], false);
}
#[test]
fn legacy_resets_and_model_switches_create_gaps_not_fake_delta() {
    let r = Rig::new();
    let mut ctx = reader::Context {
        owner: THREAD.into(),
        active_thread: THREAD.into(),
        current_turn: "legacy-turn".into(),
        model: "old-model".into(),
        ..Default::default()
    };
    for n in [100, 150, 10, 40] {
        let p = json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":n,"output_tokens":n/10,"total_tokens":n+n/10}}});
        adapter::ingest(
            &r.core.db,
            &r.core.source_id,
            "legacy",
            &event("event_msg", p),
            &mut ctx,
            None,
        )
        .unwrap();
    }
    assert_eq!(r.snapshot()["overview"]["totalTokens"], Value::Null);
    assert_eq!(r.snapshot()["overview"]["legacyTokens"], 88);
    ctx.model = "new-model".into();
    let p = json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"output_tokens":5,"total_tokens":55}}});
    adapter::ingest(
        &r.core.db,
        &r.core.source_id,
        "legacy",
        &event("event_msg", p),
        &mut ctx,
        None,
    )
    .unwrap();
    assert_eq!(r.snapshot()["overview"]["legacyTokens"], 88);
}
#[test]
fn budget_identity_is_per_rule_version_scope_local_day_and_tier_not_usage_event() {
    let mut r = Rig::new();
    r.core
        .action(
            "save_budget",
            &json!({"id":"daily","scope":"daily","threshold":100}),
        )
        .unwrap();
    direct(&r.core, &[native(THREAD, "one", 100)]);
    queries::evaluate_budgets(&mut r.core).unwrap();
    direct(&r.core, &[native(THREAD, "two", 100)]);
    queries::evaluate_budgets(&mut r.core).unwrap();
    assert_eq!(r.snapshot()["inbox"].as_array().unwrap().len(), 1);
    assert_eq!(r.snapshot()["budgets"][0]["usedTokens"], 240);
    r.core
        .action(
            "save_budget",
            &json!({"id":"daily","scope":"daily","threshold":200}),
        )
        .unwrap();
    queries::evaluate_budgets(&mut r.core).unwrap();
    assert_eq!(r.snapshot()["inbox"].as_array().unwrap().len(), 2);
}
#[test]
fn clear_watermark_prevents_automatic_refill_but_explicit_history_import_is_possible() {
    let mut r = Rig::new();
    let p = r.put("one.jsonl", &r.fixture());
    r.scan();
    r.core.action("clear_data", &json!({})).unwrap();
    fs::copy(p, r.source.join("copied-after-clear.jsonl")).unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["responseCount"], 0);
    r.core
        .action("import_history", &json!({"from":"1970-01-01"}))
        .unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1200);
}
#[test]
fn csv_blocks_formulas_newlines_and_can_anonymize_project() {
    assert_eq!(
        queries::test_csv_cell(" \t=SUM(A1)\r\nx"),
        "\"' \t=SUM(A1)  x\""
    );
    let mut r = Rig::new();
    r.put("one.jsonl", &r.fixture());
    r.scan();
    let csv = r.core.export_csv(json!({"anonymize":true})).unwrap();
    assert!(csv.contains("项目-"));
    assert!(!csv.contains("D:/projects"));
    assert!(!csv.contains("demo"));
}
#[test]
fn settings_core_values_win_legacy_migration_but_defaults_do_not_block_missing_keys() {
    let mut r = Rig::new();
    assert_eq!(r.core.settings().unwrap()["muted"], true);
    r.core
        .action("save_settings", &json!({"muted":false}))
        .unwrap();
    let s = r
        .core
        .migrate_desktop_settings(Some(json!({"muted":true,"scale":1.4,"petVisible":false})))
        .unwrap();
    assert_eq!(s["muted"], false);
    assert_eq!(s["petScale"], 1.4);
    assert_eq!(s["petVisible"], false);
    r.core
        .migrate_desktop_settings(Some(json!({"petScale":2})))
        .unwrap();
    assert_eq!(r.core.settings().unwrap()["petScale"], 1.4);
}
#[test]
fn registered_source_roots_include_disabled_without_fact_or_health_tables() {
    let mut r = Rig::new();
    let second = r._tmp.path().join("disabled-source");
    fs::create_dir(&second).unwrap();
    let id = r.core.add_source(&second).unwrap();
    r.core
        .action("update_source", &json!({"id":id,"enabled":false}))
        .unwrap();
    let sources = r.core.sources().unwrap();
    assert_eq!(sources.len(), 2);
    assert_eq!(sources.iter().find(|source| source["id"] == id).unwrap()["enabled"], false);
    let primary = r.core.primary_source().unwrap();
    let counts = source_configuration_counts(&r.core);
    let mut expected = vec![r.source.clone(), second];
    expected.sort();
    let mut roots = r.core.registered_source_roots().unwrap();
    roots.sort();
    assert_eq!(roots, expected);

    r.core.db.execute_batch("DROP TABLE usage_responses;").unwrap();
    let error = r.core.sources().unwrap_err();
    assert!(error.contains("usage_responses"), "{error}");
    let mut roots = r.core.registered_source_roots().unwrap();
    roots.sort();
    assert_eq!(roots, expected);
    assert_eq!(r.core.primary_source().unwrap(), primary);
    assert_eq!(source_configuration_counts(&r.core), counts);
    let enabled: bool = r.core.db.query_row(
        "SELECT enabled FROM source_instances WHERE id=?1", [&id], |row| row.get(0),
    ).unwrap();
    assert!(!enabled);
}

#[test]
fn source_registration_selection_relocation_and_restart_keep_stable_identities() {
    let mut r = Rig::new();
    let second = r._tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    let id = r.core.add_source(&second).unwrap();
    assert_ne!(id, r.core.source_id);
    r.core
        .action("set_primary_source", &json!({"id":id}))
        .unwrap();
    let moved = r._tmp.path().join("moved");
    fs::rename(&second, &moved).unwrap();
    r.core
        .action("update_source", &json!({"id":id,"path":moved}))
        .unwrap();
    r.core = Core::open(&r.data, &r.source).unwrap();
    assert_eq!(r.core.primary_source().unwrap()["id"], id);
    assert_eq!(
        r.core.primary_source().unwrap()["root"],
        moved.to_string_lossy().to_string()
    );
    r.core
        .action("update_source", &json!({"id":id,"enabled":false}))
        .unwrap();
    assert_eq!(r.core.primary_source().unwrap()["enabled"], false);
}

#[test]
fn primary_source_write_failure_preserves_memory_persistence_and_restart_binding() {
    let mut r = Rig::new();
    let original = r.core.primary_source().unwrap();
    let original_root = r.core.source_root.clone();
    let second = r._tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    let id = r.core.add_source(&second).unwrap();
    r.core.db.execute_batch(
        "CREATE TEMP TRIGGER fail_primary_source BEFORE UPDATE OF value ON settings
         WHEN NEW.key='primarySourceId'
         BEGIN SELECT RAISE(ABORT,'injected primary source write failure'); END;",
    ).unwrap();

    let error = r.core.action("set_primary_source", &json!({"id":id})).unwrap_err();
    assert!(error.contains("injected primary source write failure"), "{error}");
    assert_eq!(r.core.source_id, original["id"].as_str().unwrap());
    assert_eq!(r.core.source_root, original_root);
    assert_eq!(r.core.primary_source().unwrap(), original);
    assert_eq!(get_setting(&r.core.db, "primarySourceId").unwrap(), Some(original["id"].clone()));

    r.core.db.execute_batch("DROP TRIGGER fail_primary_source").unwrap();
    r.core = Core::open(&r.data, &r.source).unwrap();
    assert_eq!(r.core.primary_source().unwrap(), original);
    let selected = r.core.action("set_primary_source", &json!({"id":id})).unwrap();
    assert_eq!(selected["id"], id);
    assert_eq!(r.core.source_root, second);
    r.core = Core::open(&r.data, &r.source).unwrap();
    assert_eq!(r.core.primary_source().unwrap(), selected);
}

fn source_configuration_counts(core: &Core) -> (i64, i64, i64) {
    core.db.query_row(
        "SELECT (SELECT COUNT(*) FROM source_instances),
                (SELECT COUNT(*) FROM collection_channels),
                (SELECT COUNT(*) FROM source_capabilities)",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap()
}

#[test]
fn source_registration_failure_rolls_back_source_channel_and_capabilities_before_retry() {
    for table in ["collection_channels", "source_capabilities"] {
        let mut r = Rig::new();
        let original = r.core.primary_source().unwrap();
        let before = source_configuration_counts(&r.core);
        let second = r._tmp.path().join("second");
        fs::create_dir(&second).unwrap();
        r.core.db.execute_batch(&format!(
            "CREATE TEMP TRIGGER fail_source_registration BEFORE INSERT ON {table}
             BEGIN SELECT RAISE(ABORT,'injected source registration failure'); END;",
        )).unwrap();

        let error = r.core.add_source(&second).unwrap_err();
        assert!(error.contains("injected source registration failure"), "{table}: {error}");
        assert_eq!(source_configuration_counts(&r.core), before, "{table}");
        assert_eq!(r.core.primary_source().unwrap(), original);
        assert_eq!(r.core.source_root, r.source);

        r.core.db.execute_batch("DROP TRIGGER fail_source_registration").unwrap();
        r.core = Core::open(&r.data, &r.source).unwrap();
        assert_eq!(source_configuration_counts(&r.core), before, "{table}");
        let id = r.core.add_source(&second).unwrap();
        assert_eq!(r.core.add_source(&second).unwrap(), id);
        assert_eq!(source_configuration_counts(&r.core), (before.0+1, before.1+1, before.2+1));
        r.core = Core::open(&r.data, &r.source).unwrap();
        assert_eq!(source_configuration_counts(&r.core), (before.0+1, before.1+1, before.2+1));
        assert_eq!(r.core.primary_source().unwrap(), original);
    }
}

#[test]
fn source_update_and_relocation_failure_roll_back_path_enabled_and_memory_before_retry() {
    for action in ["update_source", "relocate_source"] {
        let mut r = Rig::new();
        r.put("one.jsonl", &r.fixture());
        r.scan();
        let original = r.core.primary_source().unwrap();
        let original_root = r.core.source_root.clone();
        let source_counts = source_configuration_counts(&r.core);
        let response_count: i64 = r.core.db.query_row("SELECT COUNT(*) FROM usage_responses", [], |row| row.get(0)).unwrap();
        assert_eq!(response_count, 1);
        let moved = r._tmp.path().join("moved");
        fs::create_dir(&moved).unwrap();
        r.core.db.execute_batch(
            "CREATE TEMP TRIGGER fail_source_enable BEFORE UPDATE OF enabled ON source_instances
             BEGIN SELECT RAISE(ABORT,'injected source enable write failure'); END;",
        ).unwrap();
        let patch = json!({"id":original["id"], "path":moved, "enabled":false});

        let error = r.core.action(action, &patch).unwrap_err();
        assert!(error.contains("injected source enable write failure"), "{action}: {error}");
        assert_eq!(r.core.source_root, original_root, "{action}");
        assert_eq!(r.core.source_id, original["id"].as_str().unwrap());
        assert_eq!(r.core.primary_source().unwrap(), original, "{action}");
        assert_eq!(source_configuration_counts(&r.core), source_counts);
        assert_eq!(get_setting(&r.core.db, "primarySourceId").unwrap(), Some(original["id"].clone()));
        let retained: i64 = r.core.db.query_row("SELECT COUNT(*) FROM usage_responses", [], |row| row.get(0)).unwrap();
        assert_eq!(retained, response_count);

        r.core.db.execute_batch("DROP TRIGGER fail_source_enable").unwrap();
        r.core = Core::open(&r.data, &r.source).unwrap();
        assert_eq!(r.core.primary_source().unwrap(), original, "{action}");
        r.core.action(action, &patch).unwrap();
        assert_eq!(r.core.source_root, moved);
        let changed = r.core.primary_source().unwrap();
        assert_eq!(changed["id"], original["id"]);
        assert_eq!(changed["root"], moved.to_string_lossy().to_string());
        assert_eq!(changed["enabled"], false);
        r.core = Core::open(&r.data, &r.source).unwrap();
        assert_eq!(r.core.primary_source().unwrap(), changed, "{action}");
        let retained: i64 = r.core.db.query_row("SELECT COUNT(*) FROM usage_responses", [], |row| row.get(0)).unwrap();
        assert_eq!(retained, response_count);
    }
}

#[test]
fn rejects_data_source_path_overlap_before_writing_any_database() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("codex");
    fs::create_dir(&source).unwrap();
    assert!(Core::open(&source.join("pet"), &source).is_err());
    assert!(!source.join("pet").exists());
    assert!(Core::open(tmp.path(), &source).is_err());
    let mut r = Rig::new();
    assert!(r.core.add_source(&r.data.join("source")).is_err());
    assert!(r
        .core
        .action(
            "update_source",
            &json!({"id":r.core.source_id,"path":r.data})
        )
        .is_err());
}
#[test]
fn cancellation_does_not_commit_a_partial_record() {
    let mut r = Rig::new();
    r.put("one.jsonl", &r.fixture());
    let cancel = std::sync::atomic::AtomicBool::new(true);
    r.core.scan_with_cancel(&cancel).unwrap();
    assert_eq!(r.snapshot()["overview"]["responseCount"], 0);
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1200);
}
#[test]
fn oversized_half_line_makes_bounded_progress_then_recovers_following_valid_line() {
    let mut r = Rig::new();
    let p = r.source.join("large.jsonl");
    let mut f = fs::File::create(&p).unwrap();
    f.write_all(&vec![b'x'; 10 * 1024 * 1024]).unwrap();
    r.scan();
    let offset: i64 = r
        .core
        .db
        .query_row("SELECT offset FROM source_files LIMIT 1", [], |x| x.get(0))
        .unwrap();
    assert_eq!(offset, 0);
    let context: String = r
        .core
        .db
        .query_row("SELECT context FROM source_files LIMIT 1", [], |x| x.get(0))
        .unwrap();
    assert!(
        serde_json::from_str::<Value>(&context).unwrap()["discard_offset"]
            .as_u64()
            .unwrap()
            > 0
    );
    drop(f);
    let mut f = fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(b"\n").unwrap();
    f.write_all((native(THREAD, "after-large", 100).to_string() + "\n").as_bytes())
        .unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
}
#[test]
fn malformed_complete_record_does_not_stop_other_records_or_store_body() {
    let mut r = Rig::new();
    r.put(
        "bad.jsonl",
        &(String::from("{\"private-body\":nope}\n")
            + &native(THREAD, "ok", 100).to_string()
            + "\n"),
    );
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
    let messages: String = r
        .core
        .db
        .query_row(
            "SELECT group_concat(message) FROM ingestion_issues",
            [],
            |x| x.get(0),
        )
        .unwrap();
    assert!(!messages.contains("private-body"));
}

struct SyntheticSecondAdapter;
impl SourceAdapter for SyntheticSecondAdapter {
    fn client_kind(&self) -> &'static str {
        "synthetic_second_semantics"
    }
    fn capabilities(&self) -> Value {
        json!({"usage":{"support":"synthetic-only","granularity":"response"}})
    }
    fn normalize(&self, o: &Observation) -> Result<Vec<StandardEvent>> {
        let u = &o.event;
        let mut r = normalized(
            native("synthetic-session", "synthetic-request", 100),
            &o.source_id,
        );
        r.namespace = "synthetic-exclusive-input-v1".into();
        let read = u["cache_read_input_tokens"]
            .as_i64()
            .ok_or("missing cache read")?;
        let write = u["cache_creation_input_tokens"]
            .as_i64()
            .ok_or("missing cache creation")?;
        let ordinary = u["input_tokens"].as_i64().ok_or("missing input")?;
        let output = u["output_tokens"].as_i64().ok_or("missing output")?;
        r.native_counts = [
            ("input_tokens", ordinary),
            ("cache_read_input_tokens", read),
            ("cache_creation_input_tokens", write),
            ("output_tokens", output),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), Metric::Valid(v)))
        .collect();
        r.counts = adapter::Counts {
            input: Metric::Valid(ordinary + read + write),
            output: Metric::Valid(output),
            cache_read: Metric::Valid(read),
            cache_write: Metric::Valid(write),
            reasoning: Metric::Unknown,
            total: Metric::Valid(ordinary + read + write + output),
        };
        r.semantics_version = "synthetic-exclusive-input-v1".into();
        r.adapter_version = "synthetic-test-only".into();
        r.completeness = "synthetic-only".into();
        Ok(vec![StandardEvent::ResponseUsage(r)])
    }
}
#[test]
fn second_semantics_fixture_normalizes_exclusive_input_and_never_double_adds_cache_subtypes() {
    let r = Rig::new();
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/multi-semantics.json")).unwrap();
    let o = Observation {
        source_id: "synthetic".into(),
        channel_id: "synthetic:fixture".into(),
        event: fixture["second_semantics"].clone(),
    };
    let events = SyntheticSecondAdapter.normalize(&o).unwrap();
    for e in events {
        adapter::store_standard_event(&r.core.db, &e, false).unwrap();
    }
    let s = r.snapshot();
    assert_eq!(s["overview"]["inputTokens"], 1000);
    assert_eq!(s["overview"]["normalInputTokens"], 100);
    assert_eq!(s["overview"]["totalTokens"], 1200);
    assert_eq!(s["overview"]["cacheHitRate"], 0.8);
    assert!(s["overview"]["reasoningOutputTokens"].is_null());
    assert!(SyntheticSecondAdapter.build_chat_target(THREAD).is_none());
}
#[test]
fn aggregate_quota_and_context_snapshots_cannot_manufacture_response_rows_or_tokens() {
    let r = Rig::new();
    let a = adapter::AggregateObservation {
        id: "aggregate".into(),
        source_id: "s".into(),
        channel_id: "otel".into(),
        granularity: adapter::Granularity::Window,
        mode: adapter::MeasurementMode::Cumulative,
        interval_start: now(),
        interval_end: now(),
        epoch: "process-start-1".into(),
        dimensions: Default::default(),
        metrics: [("tokens".into(), Metric::Valid(999999))].into(),
        semantics_version: "synthetic-v1".into(),
        coverage: "independent-aggregate-only".into(),
    };
    adapter::store_standard_event(&r.core.db, &StandardEvent::UsageAggregate(a.clone()), false)
        .unwrap();
    adapter::store_standard_event(&r.core.db, &StandardEvent::UsageAggregate(a), false).unwrap();
    let snapshot = adapter::SnapshotObservation {
        id: "context".into(),
        source_id: "s".into(),
        channel_id: "statusline".into(),
        observed_at: now(),
        account_scope_id: None,
        metrics: [("context_tokens".into(), Metric::Valid(500))].into(),
        semantics_version: "synthetic-v1".into(),
        validity: "source-recorded".into(),
    };
    adapter::store_standard_event(
        &r.core.db,
        &StandardEvent::ContextSnapshot(snapshot.clone()),
        false,
    )
    .unwrap();
    adapter::store_standard_event(&r.core.db, &StandardEvent::QuotaSnapshot(snapshot), false)
        .unwrap();
    assert_eq!(r.snapshot()["overview"]["responseCount"], 0);
    assert!(r.snapshot()["overview"]["totalTokens"].is_null());
    assert_eq!(json_rows(&r.core.db, "usage_aggregates").unwrap().len(), 1);
}
#[test]
fn cross_source_suspicion_excludes_exact_range_until_copy_evidence_resolves_it() {
    let r = Rig::new();
    let a = normalized(native(THREAD, "copy", 100), &r.core.source_id);
    let mut b = a.clone();
    b.source_id = "source-copy".into();
    let first = response_key(&a);
    let second = response_key(&b);
    adapter::store_response(&r.core.db, &a, false).unwrap();
    adapter::store_response(&r.core.db, &b, false).unwrap();
    adapter::mark_suspected_overlap(
        &r.core.db,
        &[first.clone(), second.clone()],
        "fixture: known copied root, not matched automatically",
    )
    .unwrap();
    assert!(r.snapshot()["overview"]["totalTokens"].is_null());
    adapter::merge_verified_copy(&r.core.db, &first, &second, "").unwrap_err();
    adapter::merge_verified_copy(&r.core.db,&first,&second,"fixture manual proof: original thread+response, all metering, and copied-root relation verified").unwrap();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
    assert_eq!(r.snapshot()["overview"]["responseCount"], 1);
}
#[test]
fn global_activity_is_unfiltered_counts_root_tasks_and_rejects_future_activity() {
    let r = Rig::new();
    let values = [
        event("session_meta", json!({"id":THREAD})),
        event(
            "event_msg",
            json!({"type":"task_started","turn_id":"root-active","root_turn_id":"root-active"}),
        ),
        event(
            "event_msg",
            json!({"type":"task_started","turn_id":"child-active","root_turn_id":"root-active","thread_id":"child-thread","turn_attribution":{"parent_turn_id":"root-active"}}),
        ),
    ];
    direct(&r.core, &values);
    let s = r
        .core
        .snapshot(json!({"from":"2000-01-01","to":"2000-01-01","model":"different-model"}))
        .unwrap();
    assert_eq!(s["turns"].as_array().unwrap().len(), 0);
    assert_eq!(s["activity"]["activeTurns"], 1);
    let mut future = event(
        "event_msg",
        json!({"type":"task_started","turn_id":"future-root","root_turn_id":"future-root"}),
    );
    future["timestamp"] = json!((Utc::now() + Duration::days(1)).to_rfc3339());
    direct(&r.core, &[future]);
    assert_eq!(r.snapshot()["activity"]["activeTurns"], 1);
    assert_eq!(r.snapshot()["activity"]["uncertainTurns"], 1);
}
#[test]
fn a_valid_response_identity_with_missing_usage_still_counts_as_recorded_not_trusted_metered() {
    let r = Rig::new();
    let mut v = native(THREAD, "unmetered", 100);
    v["payload"]["usage"]
        .as_object_mut()
        .unwrap()
        .remove("output_tokens");
    adapter::store_response(&r.core.db, &normalized(v, &r.core.source_id), false).unwrap();
    let s = r.snapshot();
    assert_eq!(s["overview"]["responseCount"], 1);
    assert_eq!(s["overview"]["trustedMeteredResponseCount"], 0);
    assert!(s["overview"]["totalTokens"].is_null());
}
#[test]
fn static_head_files_do_not_starve_older_history_or_a_second_source() {
    let mut r = Rig::new();
    for n in 0..160 {
        let text = native(THREAD, &format!("many-{n}"), 100).to_string() + "\n";
        r.put(&format!("{n:04}.jsonl"), &text);
    }
    let second = r._tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    r.core.add_source(&second).unwrap();
    fs::write(
        second.join("later.jsonl"),
        native(THREAD, "second-source-unique", 100).to_string() + "\n",
    )
    .unwrap();
    for _ in 0..40 {
        r.core.scan().unwrap();
        if r.snapshot()["overview"]["responseCount"] == 161 {
            break;
        }
    }
    assert_eq!(r.snapshot()["overview"]["responseCount"], 161);
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 19320);
}
#[test]
fn file_growth_with_restored_mtime_is_still_collected() {
    let mut r = Rig::new();
    let p = r.put(
        "live.jsonl",
        &(native(THREAD, "first", 100).to_string() + "\n"),
    );
    r.scan();
    let previous = fs::metadata(&p).unwrap().modified().unwrap();
    let f = fs::OpenOptions::new().append(true).open(&p).unwrap();
    let mut writer = &f;
    writer
        .write_all((native(THREAD, "second", 100).to_string() + "\n").as_bytes())
        .unwrap();
    f.set_times(fs::FileTimes::new().set_modified(previous))
        .unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 240);
}
#[cfg(windows)]
#[test]
fn locked_source_recovers_after_release_while_another_source_progresses() {
    use std::os::windows::fs::OpenOptionsExt;
    let mut r = Rig::new();
    let p = r.put(
        "locked.jsonl",
        &(native(THREAD, "locked", 100).to_string() + "\n"),
    );
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&p)
        .unwrap();
    let second = r._tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    r.core.add_source(&second).unwrap();
    fs::write(
        second.join("ok.jsonl"),
        native(THREAD, "unlocked-source", 100).to_string() + "\n",
    )
    .unwrap();
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 120);
    drop(lock);
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 240);
}
#[test]
fn notification_claim_is_atomic_preserves_refinement_and_never_replays_on_restart() {
    let mut r = Rig::new();
    let mut ctx = reader::Context {
        owner: THREAD.into(),
        active_thread: THREAD.into(),
        notify_eligible: true,
        runtime_start: r.core.runtime_start.clone(),
        runtime_id: r.core.runtime_id.clone(),
        ..Default::default()
    };
    let complete = event(
        "event_msg",
        json!({"type":"task_complete","turn_id":"live","root_turn_id":"live"}),
    );
    adapter::ingest(
        &r.core.db,
        &r.core.source_id,
        "live",
        &complete,
        &mut ctx,
        None,
    )
    .unwrap();
    let claimed = r.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    let id = strv(&claimed[0], "id");
    assert!(r.core.claim_notifications().unwrap().is_empty());
    r.core
        .finish_notification(&[id.clone()], "shown", None)
        .unwrap();
    r.core
        .action("acknowledge_delivery", &json!({"id":id}))
        .unwrap();
    let refine = event(
        "event_msg",
        json!({"type":"task_complete","turn_id":"live","root_turn_id":"live","status":"completed"}),
    );
    adapter::ingest(
        &r.core.db,
        &r.core.source_id,
        "live",
        &refine,
        &mut ctx,
        None,
    )
    .unwrap();
    assert!(r.core.claim_notifications().unwrap().is_empty());
    assert_eq!(r.snapshot()["inbox"][0]["notificationState"], "shown");
    assert_eq!(r.snapshot()["inbox"][0]["animationEligible"], false);
    r.core = Core::open(&r.data, &r.source).unwrap();
    assert!(r.core.claim_notifications().unwrap().is_empty());
    assert_eq!(r.snapshot()["inbox"][0]["animationEligible"], false);
}
#[test]
fn do_not_disturb_suppresses_live_notification_persistently_instead_of_delaying_it() {
    let mut r = Rig::new();
    r.core
        .action("save_settings", &json!({"doNotDisturb":true}))
        .unwrap();
    let mut ctx = reader::Context {
        owner: THREAD.into(),
        notify_eligible: true,
        runtime_start: r.core.runtime_start.clone(),
        runtime_id: r.core.runtime_id.clone(),
        ..Default::default()
    };
    adapter::ingest(
        &r.core.db,
        &r.core.source_id,
        "live",
        &event(
            "event_msg",
            json!({"type":"task_complete","turn_id":"dnd","root_turn_id":"dnd"}),
        ),
        &mut ctx,
        None,
    )
    .unwrap();
    assert!(r.core.claim_notifications().unwrap().is_empty());
    r.core
        .action("save_settings", &json!({"doNotDisturb":false}))
        .unwrap();
    assert!(r.core.claim_notifications().unwrap().is_empty());
    assert_eq!(r.snapshot()["inbox"][0]["notificationState"], "suppressed");
}
#[test]
fn migration_takes_backup_and_keeps_existing_settings_and_source_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let data = tmp.path().join("pet");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&data).unwrap();
    {
        let db = Connection::open(data.join("ledger.sqlite3")).unwrap();
        db.execute_batch(include_str!("../../migrations/001_initial.sql"))
            .unwrap();
        db.execute(
            "INSERT INTO source_instances(id,root,created_at) VALUES('old-source',?1,?2)",
            params![source.to_string_lossy(), now()],
        )
        .unwrap();
        put_setting(&db, "preferences", &json!({"muted":false})).unwrap();
    }
    let core = Core::open(&data, &source).unwrap();
    assert_eq!(core.source_id, "old-source");
    assert_eq!(core.settings().unwrap()["muted"], false);
    let backups = fs::read_dir(&data)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("ledger-before-v6-")
        })
        .count();
    assert_eq!(backups, 1);
    assert_eq!(
        core.db
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        6
    );
}

#[test]
fn all_fact_views_honor_project_model_provider_and_root_filters_and_missing_root_usage_is_related()
{
    let mut r = Rig::new();
    for (thread, turn, cwd, model, provider) in [
        (THREAD, "one-root", "D:/a/demo", "model-a", "provider-a"),
        (
            "00000000-0000-4000-8000-000000000002",
            "two-root",
            "D:/b/demo",
            "model-b",
            "provider-b",
        ),
    ] {
        let mut usage = native(thread, &format!("response-{turn}"), 100);
        usage["payload"]["turn_id"] = json!(turn);
        usage["payload"]
            .as_object_mut()
            .unwrap()
            .remove("root_turn_id");
        usage["payload"]["provider_id"] = json!(provider);
        direct(
            &r.core,
            &[
                event("session_meta", json!({"id":thread,"cwd":cwd})),
                event(
                    "event_msg",
                    json!({"type":"task_started","turn_id":turn,"root_turn_id":turn}),
                ),
                event(
                    "turn_context",
                    json!({"turn_id":turn,"root_turn_id":turn,"model":model}),
                ),
                usage,
                event(
                    "event_msg",
                    json!({"type":"exec_command_end","call_id":format!("tool-{turn}"),"exit_code":0}),
                ),
                event(
                    "event_msg",
                    json!({"type":"task_complete","turn_id":turn,"root_turn_id":turn}),
                ),
            ],
        );
    }
    let all = r.snapshot();
    assert_eq!(all["projects"].as_array().unwrap().len(), 2);
    let project = all["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"] == "D:/a/demo")
        .unwrap()["id"]
        .clone();
    let q = json!({"project":project,"model":"model-a","providerId":"provider-a","rootTurnId":"one-root"});
    let s = r.core.snapshot(q).unwrap();
    assert_eq!(s["overview"]["totalTokens"], 120);
    for field in ["sessions", "turns", "tools", "inbox"] {
        assert_eq!(
            s[field].as_array().unwrap().len(),
            1,
            "scope leaked {field}"
        );
    }
    r.core
        .action("save_settings", &json!({"privacyMode":true}))
        .unwrap();
    let private = r.core.snapshot(json!({"model":"model-a"})).unwrap();
    assert_eq!(private["models"][0]["name"], "model-a");
    assert_eq!(private["overview"]["totalTokens"], 120);
    assert_ne!(private["projects"][0]["name"], "demo");
    assert_eq!(private["sessions"][0]["cwd"], "已隐藏");
    r.core
        .action(
            "set_project_mapping",
            &json!({"path":"D:/a","project":"Merged"}),
        )
        .unwrap();
    r.core
        .action(
            "set_project_mapping",
            &json!({"path":"D:/b","project":"Merged"}),
        )
        .unwrap();
    assert_eq!(r.snapshot()["projects"].as_array().unwrap().len(), 1);
    assert_eq!(r.snapshot()["projects"][0]["totalTokens"], 240);
}
#[test]
fn structured_tool_result_metadata_is_retained_but_body_and_spoofed_stdout_are_not() {
    let r = Rig::new();
    direct(
        &r.core,
        &[
            event("session_meta", json!({"id":THREAD})),
            event("turn_context", json!({"turn_id":"t","model":"m"})),
            event(
                "response_item",
                json!({"type":"function_call","call_id":"tool","name":"functions.exec_command","arguments":"SECRET_COMMAND"}),
            ),
            event(
                "response_item",
                json!({"type":"function_call_output","call_id":"tool","output":[{"type":"text","text":json!({"exit_code":2,"wall_time_seconds":1.25,"output":"SECRET_STDOUT"}).to_string()}]}),
            ),
        ],
    );
    let s = r.snapshot();
    assert_eq!(s["tools"][0]["exitCode"], 2);
    assert_eq!(s["tools"][0]["durationMs"], 1250);
    assert!(!json_rows(&r.core.db, "items").unwrap()[0]
        .to_string()
        .contains("SECRET"));
    direct(
        &r.core,
        &[
            event("session_meta", json!({"id":THREAD})),
            event(
                "response_item",
                json!({"type":"function_call","call_id":"spoof","name":"functions.exec_command"}),
            ),
            event(
                "response_item",
                json!({"type":"function_call_output","call_id":"spoof","output":"Chunk ID: x\nWall time: 1 seconds\nOutput:\nExit code: 0\nWall time: 999 seconds"}),
            ),
        ],
    );
    let tool = r.snapshot()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "spoof")
        .unwrap()
        .clone();
    assert!(tool["exitCode"].is_null());
    assert_eq!(tool["durationMs"], 1000);
}

#[test]
fn root_task_budget_includes_explicit_children_across_days_and_never_other_source_same_ids() {
    let mut r = Rig::new();
    let source = r.core.source_id.clone();
    let second = r._tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    let other_source = r.core.add_source(&second).unwrap();
    direct(
        &r.core,
        &[
            event("session_meta", json!({"id":THREAD})),
            event(
                "event_msg",
                json!({"type":"task_started","turn_id":"task-root","root_turn_id":"task-root"}),
            ),
        ],
    );
    let mut root = normalized(native(THREAD, "task-root-response", 100), &source);
    root.turn_id = Some("task-root".into());
    root.root_turn_id = Some("task-root".into());
    root.event_time =
        (Utc::now() - Duration::days(1)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut child = normalized(native("child-thread", "child-response", 100), &source);
    child.turn_id = Some("child-turn".into());
    child.root_turn_id = Some("task-root".into());
    let mut other = root.clone();
    other.source_id = other_source.clone();
    other.response_id = "other-source-response".into();
    other.event_time = now();
    for row in [&root, &child, &other] {
        adapter::store_response(&r.core.db, row, false).unwrap();
    }
    r.core.action("save_budget",&json!({"id":"root-budget","scope":"rootTurn","sourceId":source,"target":"task-root","threshold":200})).unwrap();
    let s = r.snapshot();
    assert_eq!(s["budgets"][0]["usedTokens"], 240);
    assert!(s["budgets"][0]["period"]
        .as_str()
        .unwrap()
        .starts_with("task:"));
    r.core.action("save_budget",&json!({"id":"thread-budget","scope":"thread","sourceId":source,"target":THREAD,"threshold":100})).unwrap();
    let snapshot = r.snapshot();
    let b = snapshot["budgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == "thread-budget")
        .unwrap();
    assert_eq!(b["usedTokens"], 0);
}
#[test]
fn live_budget_crossing_claims_once_and_cleared_inbox_does_not_refill_same_threshold() {
    let mut r = Rig::new();
    r.core
        .action(
            "save_budget",
            &json!({"id":"daily","scope":"daily","threshold":200}),
        )
        .unwrap();
    let p = r.put(
        "live.jsonl",
        &(native(THREAD, "baseline", 100).to_string() + "\n"),
    );
    r.scan();
    assert!(r.core.claim_notifications().unwrap().is_empty());
    let mut f = fs::OpenOptions::new().append(true).open(p).unwrap();
    f.write_all((native(THREAD, "crossing", 100).to_string() + "\n").as_bytes())
        .unwrap();
    r.scan();
    let claimed = r.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["kind"], "budget");
    assert!(r.core.claim_notifications().unwrap().is_empty());
    r.core.action("clear_inbox", &json!({})).unwrap();
    r.scan();
    assert!(r.snapshot()["inbox"].as_array().unwrap().is_empty());
}
#[test]
fn chat_target_requires_exact_source_membership_registered_adapter_and_plain_uuid() {
    let mut r = Rig::new();
    direct(&r.core, &[event("session_meta", json!({"id":THREAD}))]);
    assert_eq!(
        r.core
            .chat_target(&r.core.source_id, THREAD)
            .unwrap()
            .as_str(),
        format!("codex://threads/{THREAD}")
    );
    assert!(r.core.chat_target("unknown", THREAD).is_err());
    assert!(r
        .core
        .chat_target(&r.core.source_id, "https://example.invalid")
        .is_err());
    direct(
        &r.core,
        &[event(
            "session_meta",
            json!({"id":format!("{THREAD}?run=bad")}),
        )],
    );
    assert!(r
        .core
        .chat_target(&r.core.source_id, &format!("{THREAD}?run=bad"))
        .is_err());
    let second = r._tmp.path().join("second");
    fs::create_dir(&second).unwrap();
    let id = r.core.add_source(&second).unwrap();
    assert!(r.core.chat_target(&id, THREAD).is_err());
    r.core
        .db
        .execute(
            "INSERT INTO thread_metadata(id,data) VALUES(?1,?2)",
            params![
                format!("{id}|{THREAD}"),
                json!({"id":THREAD,"sourceId":id}).to_string()
            ],
        )
        .unwrap();
    assert!(r.core.chat_target(&id, THREAD).is_err());
    r.core
        .db
        .execute(
            "UPDATE source_instances SET client_kind='unreleased' WHERE id=?1",
            [&id],
        )
        .unwrap();
    assert!(r.core.chat_target(&id, THREAD).is_err());
}

fn history_fixture(r: &Rig, legacy: bool) -> (String, String) {
    let old = (Utc::now() - Duration::days(2)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let stamp = |mut value: Value| {
        value["timestamp"] = json!(old);
        value.to_string() + "\n"
    };
    let mut text = stamp(event(
        "session_meta",
        json!({"id":THREAD,"cwd":"D:\\history-project"}),
    ));
    text += &stamp(event(
        "turn_context",
        json!({"turn_id":"turn-a","model":"gpt-history"}),
    ));
    text += &stamp(event(
        "event_msg",
        json!({"type":"task_started","turn_id":"turn-a"}),
    ));
    text += &stamp(event(
        "event_msg",
        json!({"type":"exec_command_begin","call_id":"old-tool","tool_name":"exec_command"}),
    ));
    if legacy {
        text += &stamp(cumulative_fixture(100));
    } else {
        text += &stamp(native(THREAD, "history-first", 100));
    }
    let noise = stamp(event(
        "event_msg",
        json!({"type":"ignored_fixture","ignored":"z".repeat(1024)}),
    ));
    for _ in 0..6500 {
        text += &noise;
    }
    if !legacy {
        text += &stamp(native(THREAD, "history-last", 200));
    }
    let _ = r;
    (text, old)
}
fn cumulative_fixture(total: i64) -> Value {
    event(
        "event_msg",
        json!({"type":"token_count","thread_id":THREAD,"info":{"total_token_usage":{"input_tokens":total,"output_tokens":0,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":total}}}),
    )
}
fn append_fixture(path: &Path, records: &[Value]) {
    let mut writer = fs::OpenOptions::new().append(true).open(path).unwrap();
    for record in records {
        writeln!(writer, "{record}").unwrap();
    }
}
fn response_present(r: &Rig, id: &str) -> bool {
    r.core
        .db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_responses WHERE response_id=?1)",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}
fn drain_history(r: &mut Rig) {
    for _ in 0..100 {
        r.core.scan().unwrap();
        if r.snapshot()["health"]["history"]["status"] == "idle" {
            return;
        }
    }
    panic!("bounded fixture history did not drain");
}

#[test]
fn paused_history_keeps_old_cursor_but_collects_new_files_and_old_file_tail_without_recount() {
    let mut r = Rig::new();
    let (text, old) = history_fixture(&r, false);
    let path = r.put("long-history.jsonl", &text);
    r.core.runtime_start = now();
    r.core.scan().unwrap();
    let before: i64 = r
        .core
        .db
        .query_row(
            "SELECT offset FROM source_files WHERE path=?1",
            [path.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert!((before as u64) < fs::metadata(&path).unwrap().len());
    r.core.action("pause_history", &json!({})).unwrap();
    let live_line = native(THREAD, "live-old-file", 300).to_string() + "\n";
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&live_line.as_bytes()[..20])
        .unwrap();
    r.scan();
    assert_eq!(
        r.snapshot()["health"]["pendingFiles"],
        1,
        "one file with two pending cursors must be counted once"
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&live_line.as_bytes()[20..])
        .unwrap();
    append_fixture(
        &path,
        &[
            event(
                "event_msg",
                json!({"type":"exec_command_end","call_id":"old-tool","exit_code":7,"duration_ms":15}),
            ),
            event(
                "event_msg",
                json!({"type":"task_complete","turn_id":"turn-a","status":"completed","duration_ms":25}),
            ),
        ],
    );
    let other = "00000000-0000-4000-8000-000000000002";
    let mut new_live = native(other, "live-new-file", 400);
    new_live["payload"]["turn_id"] = json!("turn-new-live");
    new_live["payload"]["root_turn_id"] = json!("turn-new-live");
    r.put(
        "new-live-session.jsonl",
        &(event("session_meta", json!({"id":other,"cwd":"D:\\new-live"})).to_string()
            + "\n"
            + &new_live.to_string()
            + "\n"),
    );
    r.scan();
    assert!(response_present(&r, "live-old-file"));
    assert!(response_present(&r, "live-new-file"));
    assert!(!response_present(&r, "history-last"));
    let still: i64 = r
        .core
        .db
        .query_row(
            "SELECT offset FROM source_files WHERE path=?1",
            [path.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        before, still,
        "pausing must not fast-forward or erase history"
    );
    assert_eq!(r.snapshot()["health"]["history"]["status"], "paused");
    r.core.action("resume_history", &json!({})).unwrap();
    drain_history(&mut r);
    assert_eq!(r.snapshot()["overview"]["responseCount"], 4);
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1080);
    let tool = adapter::load_fact(
        &r.core.db,
        "items",
        &format!("{}|{THREAD}|old-tool", r.core.source_id),
    )
    .unwrap()
    .unwrap();
    assert_eq!(tool["status"], "failed");
    assert_eq!(tool["exitCode"], 7);
    let turn = adapter::load_fact(&r.core.db, "turns", &format!("{}|turn-a", r.core.source_id))
        .unwrap()
        .unwrap();
    assert_eq!(turn["status"], "completed");
    assert_eq!(turn["startedAt"], old);
    assert_eq!(turn["threadId"], THREAD);
    r.scan();
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1080);
}

#[test]
fn cancelled_history_survives_restart_and_requires_explicit_reimport_while_live_remains_active() {
    let mut r = Rig::new();
    let (text, _) = history_fixture(&r, false);
    let path = r.put("cancel-history.jsonl", &text);
    r.core.runtime_start = now();
    r.core.scan().unwrap();
    r.core.action("cancel_history", &json!({})).unwrap();
    append_fixture(&path, &[native(THREAD, "after-cancel", 300)]);
    r.scan();
    assert!(response_present(&r, "after-cancel"));
    assert!(!response_present(&r, "history-last"));
    r.core = Core::open(&r.data, &r.source).unwrap();
    assert_eq!(r.snapshot()["health"]["history"]["status"], "cancelled");
    assert!(r.core.action("resume_history", &json!({})).is_err());
    append_fixture(&path, &[native(THREAD, "after-restart", 400)]);
    r.scan();
    assert!(response_present(&r, "after-restart"));
    assert!(!response_present(&r, "history-last"));
    r.core
        .action("import_history", &json!({"from":"1970-01-01"}))
        .unwrap();
    drain_history(&mut r);
    assert_eq!(r.snapshot()["overview"]["responseCount"], 4);
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1080);
}

#[test]
fn independent_live_legacy_cursor_needs_its_own_baseline_then_full_history_fills_only_proven_gap() {
    let mut r = Rig::new();
    let (text, _) = history_fixture(&r, true);
    let path = r.put("legacy-history.jsonl", &text);
    r.core.runtime_start = now();
    r.core.scan().unwrap();
    r.core.action("pause_history", &json!({})).unwrap();
    append_fixture(
        &path,
        &[
            event(
                "turn_context",
                json!({"turn_id":"turn-a","model":"gpt-history"}),
            ),
            cumulative_fixture(150),
        ],
    );
    r.scan();
    assert_eq!(
        r.snapshot()["overview"]["legacyTokens"],
        0,
        "historical cursor baseline must not bridge unread bytes in the live cursor"
    );
    std::thread::sleep(std::time::Duration::from_millis(2));
    append_fixture(&path, &[cumulative_fixture(180)]);
    r.scan();
    assert_eq!(r.snapshot()["overview"]["legacyTokens"], 30);
    r.core.action("resume_history", &json!({})).unwrap();
    drain_history(&mut r);
    assert_eq!(r.snapshot()["overview"]["legacyTokens"], 80);
    r.scan();
    assert_eq!(r.snapshot()["overview"]["legacyTokens"], 80);
}

#[test]
fn v3_context_adds_live_cursor_without_resetting_history_and_both_survive_archive_and_truncate() {
    let mut r = Rig::new();
    let path = r.put("migration-tail.jsonl", &r.fixture());
    r.scan();
    let raw: String = r
        .core
        .db
        .query_row(
            "SELECT context FROM source_files WHERE path=?1",
            [path.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let mut context: Value = serde_json::from_str(&raw).unwrap();
    context.as_object_mut().unwrap().remove("live_cursor");
    r.core
        .db
        .execute(
            "UPDATE source_files SET context=?1 WHERE path=?2",
            params![context.to_string(), path.to_string_lossy()],
        )
        .unwrap();
    r.core.action("pause_history", &json!({})).unwrap();
    append_fixture(&path, &[native(THREAD, "v3-live", 200)]);
    r.scan();
    assert!(response_present(&r, "v3-live"));
    let archive = r.source.join("archived_sessions");
    fs::create_dir(&archive).unwrap();
    let moved = archive.join("moved.jsonl");
    fs::rename(&path, &moved).unwrap();
    append_fixture(&moved, &[native(THREAD, "archive-live", 300)]);
    r.scan();
    assert!(response_present(&r, "archive-live"));
    fs::write(
        &moved,
        format!(
            "{}\n{}\n",
            event("session_meta", json!({"id":THREAD})),
            native(THREAD, "truncated-live", 400)
        ),
    )
    .unwrap();
    r.scan();
    assert!(response_present(&r, "truncated-live"));
    assert_eq!(r.snapshot()["overview"]["responseCount"], 4);
}

#[test]
fn per_source_health_is_unfiltered_has_real_success_watermark_and_preserves_unsupported_capability()
{
    let mut r = Rig::new();
    r.put("health.jsonl", &r.fixture());
    r.put("not-yet-supported.gz", "unsupported fixture");
    r.scan();
    assert_eq!(r.snapshot()["health"]["unsupportedFiles"], 1);
    let extra = r._tmp.path().join("second-health");
    fs::create_dir(&extra).unwrap();
    let source = r.core.add_source(&extra).unwrap();
    let snapshot = r
        .core
        .snapshot(json!({"from":"1900-01-01","to":"1900-01-01","model":"absent"}))
        .unwrap();
    let sources = snapshot["sources"].as_array().unwrap();
    let primary = sources
        .iter()
        .find(|s| s["id"] == r.core.source_id)
        .unwrap();
    assert!(primary["health"]["lastSuccessfulCheck"].is_string());
    assert!(primary["health"]["observedFrom"].is_string());
    assert!(primary["health"]["lastEvent"].is_string());
    let unseen = sources.iter().find(|s| s["id"] == source).unwrap();
    assert!(unseen["health"]["lastSuccessfulCheck"].is_null());
    assert!(unseen["health"]["unknownModels"].is_null());
    assert!(
        snapshot["health"]["unsupportedFiles"].is_null(),
        "an enabled, unexamined source prevents claiming the complete unsupported-file count"
    );
    assert_eq!(snapshot["health"]["unsupportedKnownFiles"], 1);
    let capabilities = snapshot["sourceCapabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["sourceId"] == r.core.source_id)
        .unwrap();
    assert_eq!(capabilities["requestTiming"]["runtime"], "不可用");
    assert_eq!(capabilities["requestTiming"]["sourceRuntime"], "可用");
    r.scan();
    assert_eq!(
        r.snapshot()["health"]["unsupportedFiles"],
        1,
        "a later healthy source cannot overwrite the first source's unsupported files with zero"
    );
}

#[test]
fn oversized_directory_is_rejected_before_any_partial_metadata_write() {
    let mut r = Rig::new();
    r.core
        .action(
            "merge_session_directory",
            &json!({"sessions":[{"id":THREAD,"title":"retained title","cwd":"D:\\keep"}]}),
        )
        .unwrap();
    let mut rows = vec![json!({"id":THREAD,"title":"must not overwrite"})];
    for index in 100..10_100 {
        rows.push(
            json!({"id":format!("00000000-0000-4000-8000-{index:012x}"),"title":"must not insert"}),
        );
    }
    let error = r
        .core
        .action("merge_session_directory", &json!({"sessions":rows}))
        .unwrap_err();
    assert!(error.contains("安全上限") && error.contains("未声明完整覆盖"));
    let count: i64 = r
        .core
        .db
        .query_row("SELECT COUNT(*) FROM thread_metadata", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let saved = adapter::load_fact(
        &r.core.db,
        "thread_metadata",
        &format!("{}|{THREAD}", r.core.source_id),
    )
    .unwrap()
    .unwrap();
    assert_eq!(saved["title"], "retained title");
}

#[test]
fn work_care_settings_defaults_validation_atomicity_and_restart_round_trip() {
    let mut r = Rig::new();
    // A pre-feature preference record receives defaults without replacing an explicit preference.
    put_setting(&r.core.db, "preferences", &json!({"muted":false})).unwrap();
    let defaults = r.core.settings().unwrap();
    assert_eq!(defaults["workCareEnabled"], true);
    assert_eq!(defaults["careTone"], "mixed");
    assert_eq!(defaults["careIntervalMinutes"], 25);
    assert_eq!(defaults["muted"], false);
    r.core
        .action(
            "update_settings",
            &json!({"settings":{"workCareEnabled":false,"careTone":"playful","careIntervalMinutes":45}}),
        )
        .unwrap();
    r.core.shutdown().unwrap();
    drop(r.core);
    let mut reopened = Core::open(&r.data, &r.source).unwrap();
    let disabled = reopened.settings().unwrap();
    assert_eq!(disabled["workCareEnabled"], false);
    assert_eq!(disabled["careTone"], "playful");
    assert_eq!(disabled["careIntervalMinutes"], 45);
    assert_eq!(disabled["muted"], false);
    let saved_before = get_setting(&reopened.db, "preferences").unwrap();
    for (key, invalid) in [
        ("workCareEnabled", json!("false")),
        ("workCareEnabled", Value::Null),
        ("careTone", json!("formal")),
        ("careTone", Value::Null),
        ("careIntervalMinutes", json!(30)),
        ("careIntervalMinutes", json!(25.0)),
        ("careIntervalMinutes", json!("45")),
        ("careIntervalMinutes", Value::Null),
    ] {
        let mut payload = json!({"muted":true,"workCareEnabled":true,"careTone":"gentle","careIntervalMinutes":60});
        payload[key] = invalid;
        let error = reopened.action("save_settings", &payload).unwrap_err();
        assert!(error.contains(key), "invalid field must be named: {error}");
        assert_eq!(
            get_setting(&reopened.db, "preferences").unwrap(),
            saved_before,
            "an invalid care setting must not partially save this or other preferences"
        );
    }
    for (tone, interval) in [("gentle", 60), ("mixed", 25)] {
        let result = reopened
            .action(
                "save_settings",
                &json!({"careTone":tone,"careIntervalMinutes":interval}),
            )
            .unwrap();
        assert_eq!(result["careTone"], tone);
        assert_eq!(result["careIntervalMinutes"], interval);
        assert_eq!(result["workCareEnabled"], false);
    }
    reopened
        .action("update_settings", &json!({"workCareEnabled":true}))
        .unwrap();
    reopened.shutdown().unwrap();
    drop(reopened);
    let enabled = Core::open(&r.data, &r.source).unwrap().settings().unwrap();
    assert_eq!(enabled["workCareEnabled"], true);
    assert_eq!(enabled["careTone"], "mixed");
    assert_eq!(enabled["careIntervalMinutes"], 25);
    assert_eq!(enabled["muted"], false);
}

struct V1MigrationFixture {
    _tmp: tempfile::TempDir,
    source: PathBuf,
    data: PathBuf,
    log: PathBuf,
    facts: Vec<(String, String, String)>,
    records: Vec<(i64, String, String)>,
    cursor_context: String,
    fingerprint: String,
    last_start: i64,
    last_hash: String,
    log_bytes: Vec<u8>,
    preferences: Value,
}

fn v1_migration_fixture() -> V1MigrationFixture {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let data = tmp.path().join("pet");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&data).unwrap();
    let usages = [
        ("v1-native-a", 1000, 200, 800, 100, 100),
        ("v1-native-b", 400, 100, 200, 0, 20),
    ]
    .map(|(id, input, output, cache, write, reasoning)| {
        event(
            "token_usage_record",
            json!({"thread_id":THREAD,"response_id":id,"turn_id":"v1-turn","root_turn_id":"v1-turn","model":"v1-recorded-model","usage":{"input_tokens":input,"output_tokens":output,"cached_input_tokens":cache,"cache_write_input_tokens":write,"reasoning_output_tokens":reasoning,"total_tokens":input+output}}),
        )
    });
    let entries = [
        event("session_meta", json!({"id":THREAD,"cwd":"D:/v1/project"})),
        event(
            "turn_context",
            json!({"turn_id":"v1-turn","root_turn_id":"v1-turn","model":"v1-recorded-model"}),
        ),
        usages[0].clone(),
        usages[1].clone(),
        event(
            "event_msg",
            json!({"type":"task_complete","turn_id":"v1-turn","root_turn_id":"v1-turn","status":"completed","duration_ms":4000,"time_to_first_token_ms":500}),
        ),
    ];
    let lines: Vec<_> = entries.iter().map(|v| v.to_string() + "\n").collect();
    let log_bytes = lines.concat().into_bytes();
    let log = source.join("v1-owned.jsonl");
    fs::write(&log, &log_bytes).unwrap();
    let last_start = lines[..lines.len() - 1]
        .iter()
        .map(|s| s.len() as i64)
        .sum();
    let last_hash = hash(lines.last().unwrap().as_bytes());
    let fingerprint = hash(&log_bytes[..log_bytes.len().min(4096)]);
    // This is the old v1 JSON context: the new live cursor is deliberately absent.
    let cursor_context = json!({"owner":THREAD,"active_thread":THREAD,"current_turn":"v1-turn","own_start":null,"model":"v1-recorded-model","parent":"","legacy":{}}).to_string();
    let preferences = json!({"muted":false,"privacyMode":false,"petScale":1.25,"reducedMotion":true,"retentionDays":365});
    let db = Connection::open(data.join("ledger.sqlite3")).unwrap();
    db.execute_batch(include_str!("../../migrations/001_initial.sql"))
        .unwrap();
    db.execute(
        "INSERT INTO source_instances(id,root,created_at) VALUES('v1-stable-source',?1,?2)",
        params![source.to_string_lossy(), now()],
    )
    .unwrap();
    db.execute("INSERT INTO source_files(id,path,fingerprint,generation,offset,last_start,last_hash,context) VALUES(17,?1,?2,7,?3,?4,?5,?6)",params![log.to_string_lossy(),fingerprint,log_bytes.len() as i64,last_start,last_hash,cursor_context]).unwrap();
    let mut offset = 0i64;
    let mut records = Vec::new();
    for (entry, line) in entries.iter().zip(&lines) {
        let kind = strv(entry, "type");
        let digest = hash(line.as_bytes());
        db.execute("INSERT INTO ingested_records(file_id,generation,offset,kind,digest) VALUES(17,7,?1,?2,?3)",params![offset,kind,digest]).unwrap();
        records.push((offset, kind, digest));
        offset += line.len() as i64;
    }
    let mut facts = Vec::new();
    for value in usages {
        // Seed verified native metering in the actual v1 physical schema, not through a v3 Core.
        let fact = normalized(value, "v1-stable-source");
        let id = response_key(&fact);
        let digest = hash(json!({"native":fact.native_counts,"counts":fact.counts,"semantics":fact.semantics_version}).to_string().as_bytes());
        let data = serde_json::to_string(&fact).unwrap();
        db.execute("INSERT INTO usage_responses(id,source_id,thread_id,response_id,turn_id,event_time,digest,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,fact.source_id,fact.thread_id,fact.response_id,fact.turn_id,fact.event_time,digest,data]).unwrap();
        facts.push((id, digest, data));
    }
    db.execute("INSERT INTO thread_metadata(id,data) VALUES(?1,?2)",params![format!("v1-stable-source|{THREAD}"),json!({"id":THREAD,"sourceId":"v1-stable-source","title":"v1 retained title","cwd":"D:/v1/project","parentThreadId":null}).to_string()]).unwrap();
    put_setting(&db, "preferences", &preferences).unwrap();
    put_setting(&db, "primarySourceId", &json!("v1-stable-source")).unwrap();
    put_setting(&db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
    drop(db);
    V1MigrationFixture {
        _tmp: tmp,
        source,
        data,
        log,
        facts,
        records,
        cursor_context,
        fingerprint,
        last_start,
        last_hash,
        log_bytes,
        preferences,
    }
}

fn assert_v1_fixture_retained(db: &Connection, fixture: &V1MigrationFixture, exact_context: bool) {
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage_responses", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    for (id, digest, data) in &fixture.facts {
        let saved: (String, String) = db
            .query_row(
                "SELECT digest,data FROM usage_responses WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            &saved.0, digest,
            "migration must preserve the metering digest"
        );
        assert_eq!(
            &saved.1, data,
            "migration must not invent missing account/model evidence"
        );
    }
    let saved: (String, i64, i64, i64, String, String) = db.query_row("SELECT fingerprint,generation,offset,last_start,last_hash,context FROM source_files WHERE id=17",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).unwrap();
    assert_eq!(saved.0, fixture.fingerprint);
    assert_eq!(saved.1, 7);
    assert_eq!(saved.2, fixture.log_bytes.len() as i64);
    assert_eq!(saved.3, fixture.last_start);
    assert_eq!(saved.4, fixture.last_hash);
    if exact_context {
        assert_eq!(saved.5, fixture.cursor_context);
    } else {
        let old: Value = serde_json::from_str(&fixture.cursor_context).unwrap();
        let current: Value = serde_json::from_str(&saved.5).unwrap();
        for (key, value) in old.as_object().unwrap() {
            assert_eq!(current[key], *value, "old context field changed: {key}");
        }
    }
    let physical = db.prepare("SELECT offset,kind,digest FROM ingested_records WHERE file_id=17 AND generation=7 ORDER BY offset").unwrap().query_map([],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))).unwrap().collect::<std::result::Result<Vec<_>,_>>().unwrap();
    assert_eq!(physical, fixture.records);
    assert_eq!(
        get_setting(db, "preferences").unwrap(),
        Some(fixture.preferences.clone())
    );
    assert_eq!(
        get_setting(db, "primarySourceId").unwrap(),
        Some(json!("v1-stable-source"))
    );
}

fn assert_v1_migrated_snapshot(core: &Core) {
    let snapshot = core.snapshot(json!({})).unwrap();
    for (key, expected) in [
        ("inputTokens", 1400),
        ("outputTokens", 300),
        ("totalTokens", 1700),
        ("cachedInputTokens", 1000),
        ("cacheWriteInputTokens", 100),
        ("reasoningOutputTokens", 120),
        ("responseCount", 2),
    ] {
        assert_eq!(snapshot["overview"][key], expected, "wrong migrated {key}");
    }
    assert_eq!(core.primary_source().unwrap()["id"], "v1-stable-source");
    assert_eq!(snapshot["health"]["conflicts"], 0);
    assert_eq!(core.settings().unwrap()["muted"], false);
    assert_eq!(core.settings().unwrap()["petScale"], 1.25);
    assert_eq!(core.settings().unwrap()["reducedMotion"], true);
}

fn replay_v1_copy_through_production_scan(core: &mut Core, fixture: &V1MigrationFixture) {
    fs::copy(&fixture.log, fixture.source.join("v1-replayed-copy.jsonl")).unwrap();
    let mut copied_records = 0i64;
    for _ in 0..20 {
        core.scan().unwrap();
        copied_records = core
            .db
            .query_row(
                "SELECT COUNT(*) FROM ingested_records WHERE file_id<>17",
                [],
                |row| row.get(0),
            )
            .unwrap();
        if copied_records >= fixture.records.len() as i64 {
            break;
        }
    }
    assert_eq!(
        copied_records,
        fixture.records.len() as i64,
        "the copied source must actually have been read, not only leave old totals visible"
    );
    assert_v1_migrated_snapshot(core);
    assert_v1_fixture_retained(&core.db, fixture, false);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn v1_migration_preserves_existing_native_facts_physical_cursor_and_replay_after_restart() {
    let fixture = v1_migration_fixture();
    let mut core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_v1_fixture_retained(&core.db, &fixture, true);
    assert_v1_migrated_snapshot(&core);
    assert_eq!(
        core.db
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        6
    );
    let identity: (String, String) = core
        .db
        .query_row(
            "SELECT source_id,channel_id FROM source_files WHERE id=17",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        identity,
        ("v1-stable-source".into(), "v1-stable-source:jsonl".into())
    );
    assert_eq!(
        core.db
            .query_row(
                "SELECT COUNT(*) FROM ingested_records WHERE file_id=17 AND fact_id IS NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        fixture.records.len() as i64,
        "missing v1 fact-reference evidence stays unknown until replay"
    );
    replay_v1_copy_through_production_scan(&mut core, &fixture);
    core.shutdown().unwrap();
    drop(core);
    let mut reopened = Core::open(&fixture.data, &fixture.source).unwrap();
    reopened.scan().unwrap();
    assert_v1_migrated_snapshot(&reopened);
    assert_v1_fixture_retained(&reopened.db, &fixture, false);
}

#[test]
fn v1_migration_ddl_failure_rolls_back_and_readable_backup_recovers_original_metering() {
    let fixture = v1_migration_fixture();
    {
        let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
        // Fail a real CREATE after six ALTERs and the first CREATE in production migration 002.
        db.execute_batch("CREATE TABLE source_capabilities(fixture_marker TEXT NOT NULL); INSERT INTO source_capabilities VALUES('only-this-fixture-injects-a-ddl-collision');").unwrap();
    }
    let error = match Core::open(&fixture.data, &fixture.source) {
        Ok(_) => panic!("the injected production migration DDL must fail"),
        Err(error) => error,
    };
    assert!(
        error.contains("source_capabilities") && error.contains("already exists"),
        "unexpected migration failure: {error}"
    );
    let assert_rolled_back = |db: &Connection| {
        assert_v1_fixture_retained(db, &fixture, true);
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let source_columns = db
            .prepare("PRAGMA table_info(source_instances)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(source_columns, ["id", "root", "created_at"]);
        let file_columns = db
            .prepare("PRAGMA table_info(source_files)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            !file_columns.contains(&"source_id".into())
                && !file_columns.contains(&"channel_id".into())
                && !file_columns.contains(&"os_file_key".into())
        );
        assert_eq!(db.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name='collection_channels'",[],|row|row.get::<_,i64>(0)).unwrap(),0,"DDL before the injected failure must roll back too");
        assert_eq!(
            db.query_row(
                "SELECT fixture_marker FROM source_capabilities",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "only-this-fixture-injects-a-ddl-collision"
        );
    };
    {
        let original = Connection::open_with_flags(
            fixture.data.join("ledger.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_rolled_back(&original);
    }
    let backups: Vec<_> = fs::read_dir(&fixture.data)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("ledger-before-v6-")
        })
        .map(|entry| entry.path())
        .collect();
    assert_eq!(backups.len(), 1);
    {
        let backup =
            Connection::open_with_flags(&backups[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_rolled_back(&backup);
    }
    let recovered_data = fixture._tmp.path().join("recovered-pet");
    fs::create_dir(&recovered_data).unwrap();
    fs::copy(&backups[0], recovered_data.join("ledger.sqlite3")).unwrap();
    {
        let restored = Connection::open(recovered_data.join("ledger.sqlite3")).unwrap();
        // Explicitly remove only our injected obstruction in a copy; never auto-repair real user data.
        restored
            .execute_batch("DROP TABLE source_capabilities;")
            .unwrap();
        assert_v1_fixture_retained(&restored, &fixture, true);
    }
    let mut recovered = Core::open(&recovered_data, &fixture.source).unwrap();
    assert_v1_migrated_snapshot(&recovered);
    replay_v1_copy_through_production_scan(&mut recovered, &fixture);
    recovered.shutdown().unwrap();
    drop(recovered);
    let reopened = Core::open(&recovered_data, &fixture.source).unwrap();
    assert_v1_migrated_snapshot(&reopened);
    assert_v1_fixture_retained(&reopened.db, &fixture, false);
    let original = Connection::open_with_flags(
        fixture.data.join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_rolled_back(&original);
}

#[cfg(windows)]
#[test]
fn v1_migration_verified_file_keeps_physical_id_and_cursor_when_archived() {
    let fixture = v1_migration_fixture();
    let mut core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_eq!(
        core.scan().unwrap().lines,
        0,
        "already committed v1 bytes do not need replay"
    );
    let key_before_move: String = core
        .db
        .query_row(
            "SELECT os_file_key FROM source_files WHERE id=17",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let archive = fixture.source.join("archived_sessions");
    fs::create_dir(&archive).unwrap();
    let moved = archive.join("v1-owned.jsonl");
    fs::rename(&fixture.log, &moved).unwrap();
    let mut replayed_lines = 0;
    for _ in 0..5 {
        replayed_lines += core.scan().unwrap().lines;
    }
    let archived_id: i64 = core
        .db
        .query_row(
            "SELECT id FROM source_files WHERE path=?1",
            [moved.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archived_id,17,"migration must adopt a verified same-file physical key before archive; key before move was {key_before_move:?}");
    assert!(!key_before_move.is_empty());
    assert_eq!(
        replayed_lines, 0,
        "moving a fully committed migrated file must retain its cursor"
    );
    assert_eq!(
        core.db
            .query_row("SELECT COUNT(*) FROM source_files", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_v1_fixture_retained(&core.db, &fixture, false);
    assert_v1_migrated_snapshot(&core);
    assert_eq!(fs::read(moved).unwrap(), fixture.log_bytes);
}

#[cfg(windows)]
#[test]
fn v1_unverified_truncated_or_replaced_file_never_adopts_the_old_physical_id() {
    use std::os::windows::fs::OpenOptionsExt;
    for case in ["no-anchor", "truncated", "replaced"] {
        let fixture = v1_migration_fixture();
        if case == "no-anchor" {
            let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
            db.execute("UPDATE source_files SET last_hash='' WHERE id=17", [])
                .unwrap();
        }
        let mut core = Core::open(&fixture.data, &fixture.source).unwrap();
        let held_original = if case == "replaced" {
            Some(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1 | 2 | 4)
                    .open(&fixture.log)
                    .unwrap(),
            )
        } else {
            None
        };
        if case != "no-anchor" {
            if case == "replaced" {
                fs::remove_file(&fixture.log).unwrap();
            }
            let mut text = native(THREAD, &format!("guard-{case}"), 100).to_string() + "\n";
            if case == "replaced" {
                // Keep length above the committed offset: this case must fail anchor validation,
                // not merely share the truncation condition of the preceding case.
                text +=
                    &(json!({"type":"fixture_noop","padding":"x".repeat(fixture.log_bytes.len())})
                        .to_string()
                        + "\n");
                assert!(text.len() > fixture.log_bytes.len());
            } else {
                assert!(text.len() < fixture.log_bytes.len());
            }
            fs::write(&fixture.log, text).unwrap();
        }
        for _ in 0..5 {
            core.scan().unwrap();
        }
        let (old_path, old_key, old_offset): (String, String, i64) = core
            .db
            .query_row(
                "SELECT path,os_file_key,offset FROM source_files WHERE id=17",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(
            old_path.contains("#replaced-17"),
            "{case}: unverified legacy row must be retired"
        );
        assert!(
            old_key.is_empty(),
            "{case}: must not attach the current physical file to old evidence"
        );
        assert_eq!(old_offset, fixture.log_bytes.len() as i64);
        let (current_id, current_key): (i64, String) = core
            .db
            .query_row(
                "SELECT id,os_file_key FROM source_files WHERE path=?1",
                [fixture.log.to_string_lossy().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_ne!(current_id, 17, "{case}");
        assert!(
            !current_key.is_empty(),
            "{case}: current file receives its own observed identity"
        );
        for (id, digest, data) in &fixture.facts {
            let original: (String, String) = core
                .db
                .query_row(
                    "SELECT digest,data FROM usage_responses WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                original,
                (digest.clone(), data.clone()),
                "{case}: retained facts changed"
            );
        }
        let snapshot = core.snapshot(json!({})).unwrap();
        assert_eq!(
            snapshot["overview"]["totalTokens"],
            if case == "no-anchor" { 1700 } else { 1820 },
            "{case}"
        );
        assert_eq!(snapshot["health"]["conflicts"], 0);
        let archive = fixture.source.join("archived_sessions");
        fs::create_dir(&archive).unwrap();
        let moved = archive.join("current.jsonl");
        fs::rename(&fixture.log, &moved).unwrap();
        for _ in 0..5 {
            core.scan().unwrap();
        }
        let archived_id: i64 = core
            .db
            .query_row(
                "SELECT id FROM source_files WHERE path=?1",
                [moved.to_string_lossy().to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            archived_id, current_id,
            "{case}: only the newly observed physical file may follow its archive move"
        );
        assert_eq!(
            core.db
                .query_row(
                    "SELECT os_file_key FROM source_files WHERE id=17",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            ""
        );
        drop(held_original);
    }
}

#[cfg(windows)]
#[test]
fn nonempty_physical_key_is_never_overwritten_when_same_path_is_replaced() {
    use std::os::windows::fs::OpenOptionsExt;
    let mut r = Rig::new();
    let path = r.put("observed.jsonl", &r.fixture());
    r.scan();
    let (original_id, original_key, original_offset): (i64, String, i64) = r
        .core
        .db
        .query_row(
            "SELECT id,os_file_key,offset FROM source_files WHERE path=?1",
            [path.to_string_lossy().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(!original_key.is_empty());
    // Holding the deleted original open prevents Windows from recycling its file identity.
    let held_original = fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2 | 4)
        .open(&path)
        .unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(
        &path,
        native(THREAD, "new-physical-file", 100).to_string() + "\n",
    )
    .unwrap();
    r.scan();
    let saved: (String, i64) = r
        .core
        .db
        .query_row(
            "SELECT os_file_key,offset FROM source_files WHERE id=?1",
            [original_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved, (original_key.clone(), original_offset));
    let (new_id, new_key): (i64, String) = r
        .core
        .db
        .query_row(
            "SELECT id,os_file_key FROM source_files WHERE path=?1",
            [path.to_string_lossy().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_ne!(new_id, original_id);
    assert!(!new_key.is_empty());
    assert_ne!(new_key, original_key);
    assert_eq!(r.snapshot()["overview"]["totalTokens"], 1320);
    drop(held_original);
}

#[test]
fn companion_settings_boolean_round_trip_survives_restart() {
    let mut r = Rig::new();
    let defaults = r.core.settings().unwrap();
    assert_eq!(defaults["idlePlayEnabled"], true);
    assert_eq!(defaults["workAppLinkEnabled"], true);
    let saved = r.core.action(
        "update_settings",
        &json!({"settings":{"idlePlayEnabled":false,"workAppLinkEnabled":false}}),
    ).unwrap();
    assert_eq!(saved["idlePlayEnabled"], false);
    assert_eq!(saved["workAppLinkEnabled"], false);
    let mut reopened = Core::open(&r.data, &r.source).unwrap();
    let restored = reopened.settings().unwrap();
    assert_eq!(restored["idlePlayEnabled"], false);
    assert_eq!(restored["workAppLinkEnabled"], false);
    let enabled = reopened.action(
        "update_settings",
        &json!({"idlePlayEnabled":true,"workAppLinkEnabled":true}),
    ).unwrap();
    assert_eq!(enabled["idlePlayEnabled"], true);
    assert_eq!(enabled["workAppLinkEnabled"], true);
    assert_eq!(Core::open(&r.data, &r.source).unwrap().settings().unwrap()["idlePlayEnabled"], true);
}

#[test]
fn companion_settings_invalid_types_reject_entire_patch_without_persistence() {
    let mut r = Rig::new();
    r.core.action(
        "update_settings",
        &json!({"idlePlayEnabled":false,"workAppLinkEnabled":false,"muted":true}),
    ).unwrap();
    let expected = r.core.settings().unwrap();
    let stored = get_setting(&r.core.db, "preferences").unwrap();
    for key in ["idlePlayEnabled", "workAppLinkEnabled"] {
        for invalid in [json!("true"), json!(1), Value::Null, json!([]), json!({})] {
            let mut patch = json!({"idlePlayEnabled":true,"workAppLinkEnabled":true,"muted":false});
            patch[key] = invalid;
            assert!(r.core.action("update_settings", &json!({"settings":patch})).is_err(), "{key}");
            assert_eq!(r.core.settings().unwrap(), expected, "{key}");
            assert_eq!(get_setting(&r.core.db, "preferences").unwrap(), stored, "{key}");
        }
    }
    let reopened = Core::open(&r.data, &r.source).unwrap();
    assert_eq!(reopened.settings().unwrap(), expected);
}
