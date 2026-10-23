//! 外部工具以真实临时目录、真实扫描事务与可信查询验证；不运行工具 CLI。
use super::*;
use std::io::Write;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    core: Core,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sources");
        let codex = root.join("codex");
        fs::create_dir_all(&codex).unwrap();
        let core = Core::open(&temp.path().join("data"), &codex).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        Self {
            _temp: temp,
            root,
            core,
        }
    }
    fn add(&mut self, kind: &str) -> (String, PathBuf) {
        let path = self.root.join(kind);
        fs::create_dir_all(&path).unwrap();
        let id = self.core.add_source_with_kind(&path, kind).unwrap();
        (id, path)
    }
    fn scan(&mut self, id: &str, root: &Path, kind: &str) -> Result<ScanReport> {
        scan_source(&mut self.core, id, root, kind, &AtomicBool::new(false))
    }
    fn count(&self, table: &str) -> i64 {
        self.core
            .db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    fn total(&self, id: &str) -> i64 {
        self.core.snapshot(json!({"sourceId":id})).unwrap()["overview"]["totalTokens"]
            .as_i64()
            .unwrap_or(0)
    }
}
fn old_time(days: i64) -> String {
    (crate::budget_clock::now().utc - chrono::Duration::days(days)).to_rfc3339()
}
fn claude(time: &str, id: &str) -> Value {
    json!({"type":"assistant","sessionId":"native-claude-session","timestamp":time,
    "requestId":"native-request","message":{"id":id,"role":"assistant","model":"synthetic-claude",
    "usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":50,"cache_read_input_tokens":300},
    "content":[{"type":"text","text":"PRIVATE_CLAUDE_BODY"}]},"title":"PRIVATE_CLAUDE_TITLE"})
}
fn gemini(time: &str, id: &str) -> Value {
    json!({"type":"gemini","id":id,"timestamp":time,"model":"synthetic-gemini",
    "tokens":{"input":100,"output":20,"cached":40,"thoughts":5,"tool":10,"total":135},
    "content":[{"text":"PRIVATE_GEMINI_BODY"}],"toolCalls":[{"args":{"private":"PRIVATE_TOOL_ARGS"}}]})
}
fn snapshot(messages: Vec<Value>) -> Value {
    json!({"sessionId":"native-gemini-session","projectHash":"a".repeat(64),"kind":"main",
    "messages":messages,"summary":"PRIVATE_GEMINI_SUMMARY"})
}
fn claude_path(root: &Path) -> PathBuf {
    let folder = root.join("projects").join("project-a");
    fs::create_dir_all(&folder).unwrap();
    folder.join("session.jsonl")
}
fn gemini_path(root: &Path, extension: &str) -> PathBuf {
    let folder = root.join("tmp").join("project-a").join("chats");
    fs::create_dir_all(&folder).unwrap();
    folder.join(format!("session-a.{extension}"))
}

#[test]
fn typed_registration_preserves_kind_and_default_codex_compatibility() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("claude");
    assert!(fixture.core.add_source_with_kind(&root, "codex").is_err());
    assert!(fixture
        .core
        .action("update_source", &json!({"id":id,"clientKind":"gemini"}))
        .is_err());
    let actual: String = fixture
        .core
        .db
        .query_row(
            "SELECT client_kind FROM source_instances WHERE id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actual, "claude");
    let other = fixture.root.join("default-source");
    fs::create_dir_all(&other).unwrap();
    let default_id = fixture
        .core
        .action("add_source", &json!({"path":other}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let actual: String = fixture
        .core
        .db
        .query_row(
            "SELECT client_kind FROM source_instances WHERE id=?1",
            [default_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actual, "codex");
    assert!(fixture
        .core
        .add_source_with_kind(&other, "unknown-tool")
        .is_err());
}

#[test]
fn jsonl_and_snapshot_flow_into_source_filtered_trusted_queries_without_body() {
    let mut fixture = Fixture::new();
    let (claude_id, claude_root) = fixture.add("claude");
    let (gemini_id, gemini_root) = fixture.add("gemini");
    let mut input = claude(&old_time(1), "msg-a");
    let mut bad = input.clone();
    bad["sessionId"] = json!("bad|identity");
    fs::write(claude_path(&claude_root), format!("{}\n{}\n", bad, input)).unwrap();
    let snapshot_path = gemini_path(&gemini_root, "json");
    fs::write(
        &snapshot_path,
        snapshot(vec![
            json!({"type":"user","content":"PRIVATE_USER_BODY"}),
            gemini(&old_time(1), "gemini-a"),
        ])
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        fixture
            .scan(&claude_id, &claude_root, "claude")
            .unwrap()
            .new_responses,
        1
    );
    assert_eq!(
        fixture
            .scan(&gemini_id, &gemini_root, "gemini")
            .unwrap()
            .new_responses,
        1
    );
    assert_eq!(fixture.total(&claude_id), 470);
    assert_eq!(fixture.total(&gemini_id), 135);
    let trusted = fixture.core.begin_panel_read().unwrap();
    assert_eq!(
        trusted.snapshot(json!({"sourceId":gemini_id})).unwrap()["overview"]["totalTokens"],
        135
    );
    drop(trusted);
    let mut persisted = String::new();
    for table in [
        "usage_responses",
        "thread_metadata",
        "ingestion_issues",
        "source_files",
    ] {
        let column = if table == "ingestion_issues" {
            "message"
        } else if table == "source_files" {
            "context"
        } else {
            "data"
        };
        let text: Option<String> = fixture
            .core
            .db
            .query_row(
                &format!("SELECT group_concat({column}) FROM {table}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        persisted.push_str(text.as_deref().unwrap_or(""));
    }
    assert!(!persisted.contains("PRIVATE_"));
    assert_eq!(fixture.count("turns"), 0);
    assert_eq!(fixture.count("items"), 0);
    let records = fixture.count("ingested_records");
    input["message"]["content"] = json!([{"text":"PRIVATE_CHANGED_BODY"}]);
    fs::write(claude_path(&claude_root), format!("{input}\n")).unwrap();
    let mut unchanged = snapshot(vec![gemini(&old_time(1), "gemini-a")]);
    // 保持白名单时间/字段相同，仅改未知正文，不能授予新计量修订。
    let original: Value =
        serde_json::from_str(&fs::read_to_string(&snapshot_path).unwrap()).unwrap();
    unchanged["messages"] = original["messages"].clone();
    unchanged["summary"] = json!("PRIVATE_NEW_SUMMARY");
    fs::write(&snapshot_path, unchanged.to_string()).unwrap();
    assert_eq!(
        fixture
            .scan(&gemini_id, &gemini_root, "gemini")
            .unwrap()
            .new_responses,
        0
    );
    assert_eq!(fixture.count("ingested_records"), records);
    let invalid_channel:i64=fixture.core.db.query_row("SELECT count(*) FROM fact_observations f LEFT JOIN collection_channels c ON c.id=f.channel_id WHERE c.id IS NULL",[],|row|row.get(0)).unwrap();
    assert_eq!(invalid_channel, 0);
}

#[test]
fn gemini_jsonl_context_links_native_fact_and_does_not_trust_filename() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("gemini");
    let path = gemini_path(&root, "jsonl");
    fs::write(
        path,
        format!(
            "{}\n{}\n",
            json!({"$set":{"sessionId":"native-jsonl-session","kind":"main"}}),
            gemini(&old_time(1), "native-message")
        ),
    )
    .unwrap();
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 1);
    let (thread,linked):(String,i64)=fixture.core.db.query_row("SELECT (SELECT thread_id FROM usage_responses WHERE source_id=?1),(SELECT count(*) FROM ingested_records i JOIN usage_responses u ON u.id=i.fact_id WHERE u.source_id=?1)",[&id],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(thread, "native-jsonl-session");
    assert_eq!(linked, 1);
}

#[test]
fn snapshot_fact_progress_and_fingerprint_rollback_with_outer_transaction() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("gemini");
    fs::write(
        gemini_path(&root, "json"),
        snapshot(vec![gemini(&old_time(1), "rollback-message")]).to_string(),
    )
    .unwrap();
    put_setting(&fixture.core.db, "testFailBeforeCommit", &json!(true)).unwrap();
    assert!(fixture.scan(&id, &root, "gemini").is_err());
    assert_eq!(fixture.count("usage_responses"), 0);
    assert_eq!(fixture.count("source_files"), 0);
    assert_eq!(fixture.count("ingested_records"), 0);
    put_setting(&fixture.core.db, "testFailBeforeCommit", &json!(false)).unwrap();
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 1);
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 0);
}

#[test]
fn rewritten_snapshot_adds_new_native_message_once_and_quarantines_changed_old_counts() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("gemini");
    let path = gemini_path(&root, "json");
    let mut original = snapshot(vec![gemini(&old_time(1), "original-message")]);
    fs::write(&path, original.to_string()).unwrap();
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 1);
    original["messages"]
        .as_array_mut()
        .unwrap()
        .push(gemini(&now(), "new-native-message"));
    fs::write(&path, original.to_string()).unwrap();
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 1);
    assert_eq!(fixture.total(&id), 270);
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 0);
    original["messages"][0]["tokens"]["output"] = json!(25);
    original["messages"][0]["tokens"]["total"] = json!(140);
    fs::write(&path, original.to_string()).unwrap();
    assert_eq!(fixture.scan(&id, &root, "gemini").unwrap().new_responses, 0);
    assert_eq!(fixture.count("usage_responses"), 2);
    assert_eq!(fixture.count("excluded_facts"), 1);
    assert_eq!(fixture.total(&id), 135);
}

#[test]
fn incomplete_native_metrics_remain_unknown_for_plan_after_idle_scan_and_replay() {
    let mut fixture = Fixture::new();
    for kind in ["claude", "gemini"] {
        let (source, root) = fixture.add(kind);
        fixture.core.save_usage_plan(&json!({"expectedRevision":null,"title":"计量完整性计划","type":"tokens","sourceId":source,
            "model":null,"modelDimension":"response","target":1000,"endPolicy":"time",
            "endsAt":(crate::budget_clock::now().utc+chrono::Duration::hours(1)).to_rfc3339(),"windowId":null,"remindersEnabled":false})).unwrap();
        let plan_id: String = fixture
            .core
            .db
            .query_row(
                "SELECT plan_id FROM usage_plans WHERE source_id=?1 ORDER BY revision DESC LIMIT 1",
                [&source],
                |row| row.get(0),
            )
            .unwrap();
        if kind == "claude" {
            let mut record = claude(&now(), "incomplete-claude");
            record["message"]["usage"]["output_tokens"] = json!("PRIVATE_INVALID_COUNTER");
            fs::write(claude_path(&root), format!("{record}\n")).unwrap();
        } else {
            let mut record = gemini(&now(), "incomplete-gemini");
            record["tokens"].as_object_mut().unwrap().remove("tool");
            fs::write(
                gemini_path(&root, "json"),
                snapshot(vec![record]).to_string(),
            )
            .unwrap();
        }
        let first = fixture.scan(&source, &root, kind).unwrap();
        assert!(first.issues > 0);
        let second = fixture.scan(&source, &root, kind).unwrap();
        assert_eq!(second.new_responses, 0);
        let check = get_setting(&fixture.core.db, &format!("lastSourceCheck:{source}"))
            .unwrap()
            .unwrap();
        assert_eq!(check["issues"], 0, "静默下一轮必须仍由持久事实护栏挡住假零");
        let plan = fixture.core.test_read_local_usage_plan(&plan_id).unwrap();
        for field in ["used", "remaining", "progressPercent"] {
            assert!(plan[field].is_null(), "{kind}.{field}: {plan}");
        }
        assert_eq!(plan["evaluationState"], "unknown");
        assert!(plan["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "metering_incomplete"));
        if kind == "claude" {
            fixture
                .core
                .db
                .execute("DELETE FROM ingestion_issues", [])
                .unwrap();
            fixture.core.db.execute("UPDATE source_files SET offset=0,last_start=0,last_hash='',context='{}' WHERE source_id=?1",[&source]).unwrap();
            assert!(fixture.scan(&source, &root, kind).unwrap().issues > 0);
            let messages:Option<String>=fixture.core.db.query_row("SELECT group_concat(message) FROM ingestion_issues WHERE kind='claude_metric_contract'",[],|row|row.get(0)).unwrap();
            assert!(messages.is_some_and(|message| !message.contains("PRIVATE_")));
        }
    }
    assert_eq!(fixture.count("usage_responses"), 2);
}

#[test]
fn paused_snapshot_and_new_jsonl_do_not_admit_copied_history_then_resume_does() {
    let mut fixture = Fixture::new();
    let (gid, groot) = fixture.add("gemini");
    let (cid, croot) = fixture.add("claude");
    fs::write(
        gemini_path(&groot, "json"),
        snapshot(vec![gemini(&old_time(2), "old-message")]).to_string(),
    )
    .unwrap();
    fs::write(
        claude_path(&croot),
        format!("{}\n", claude(&old_time(2), "old-claude")),
    )
    .unwrap();
    fixture.core.action("pause_history", &json!({})).unwrap();
    fixture.scan(&gid, &groot, "gemini").unwrap();
    fixture.scan(&cid, &croot, "claude").unwrap();
    assert_eq!(fixture.count("usage_responses"), 0);
    fixture.core.action("resume_history", &json!({})).unwrap();
    fixture.scan(&gid, &groot, "gemini").unwrap();
    fixture.scan(&cid, &croot, "claude").unwrap();
    assert_eq!(fixture.count("usage_responses"), 2);
}

#[test]
fn clear_watermark_blocks_unchanged_snapshot_until_explicit_history_import() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("gemini");
    fs::write(
        gemini_path(&root, "json"),
        snapshot(vec![gemini(&old_time(2), "history-message")]).to_string(),
    )
    .unwrap();
    fixture.scan(&id, &root, "gemini").unwrap();
    assert_eq!(fixture.count("usage_responses"), 1);
    fixture.core.action("clear_data", &json!({})).unwrap();
    fixture.scan(&id, &root, "gemini").unwrap();
    assert_eq!(fixture.count("usage_responses"), 0);
    fixture
        .core
        .action("import_history", &json!({"from":old_time(3)}))
        .unwrap();
    fixture.scan(&id, &root, "gemini").unwrap();
    assert_eq!(fixture.count("usage_responses"), 1);
}

#[test]
fn retention_boundary_cannot_be_undone_by_snapshot_body_rewrite() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("gemini");
    let path = gemini_path(&root, "json");
    let mut original = snapshot(vec![
        gemini(&old_time(2), "expired"),
        gemini(&now(), "retained"),
    ]);
    fs::write(&path, original.to_string()).unwrap();
    fixture.scan(&id, &root, "gemini").unwrap();
    fixture
        .core
        .action("update_settings", &json!({"settings":{"retentionDays":1}}))
        .unwrap();
    fixture.core.action("apply_retention", &json!({})).unwrap();
    assert_eq!(fixture.count("usage_responses"), 1);
    original["messages"][0]["content"] = json!("PRIVATE_REWRITTEN_BODY");
    fs::write(&path, original.to_string()).unwrap();
    fixture.scan(&id, &root, "gemini").unwrap();
    assert_eq!(fixture.count("usage_responses"), 1);
}

#[test]
fn snapshot_limits_cancel_and_expired_budget_leave_no_cursor_or_fact() {
    let mut fixture = Fixture::new();
    let (id, root) = fixture.add("gemini");
    let path = gemini_path(&root, "json");
    let mut file = File::create(&path).unwrap();
    file.write_all(&vec![b' '; 16 * 1024 * 1024 + 1]).unwrap();
    drop(file);
    assert!(fixture.scan(&id, &root, "gemini").unwrap().issues > 0);
    assert_eq!(fixture.count("source_files"), 0);
    assert_eq!(fixture.count("usage_responses"), 0);
    fs::write(
        &path,
        snapshot(vec![gemini(&now(), "cancelled")]).to_string(),
    )
    .unwrap();
    let adapter = super::super::adapter::adapter_for("gemini").unwrap();
    for (cancel, expired) in [(true, false), (false, true)] {
        let mut file = File::open(&path).unwrap();
        let length = file.metadata().unwrap().len();
        let mut budget = 16 * 1024 * 1024;
        let mut report = ScanReport::default();
        let mut issues = 0;
        let start = std::time::Instant::now() - std::time::Duration::from_secs(u64::from(expired));
        let tx = fixture.core.db.unchecked_transaction().unwrap();
        consume_snapshot(
            &tx,
            adapter,
            &id,
            &path.to_string_lossy(),
            &mut file,
            length,
            true,
            None,
            &now(),
            &AtomicBool::new(cancel),
            &start,
            &mut budget,
            &mut report,
            &mut issues,
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(fixture.count("source_files"), 0);
        assert_eq!(fixture.count("usage_responses"), 0);
    }
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    use std::os::windows::process::CommandExt;
    let result = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[cfg(windows)]
#[test]
fn frozen_root_and_intermediate_junction_swaps_are_rejected_before_reading() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registered");
    let external = temp.path().join("external");
    fs::create_dir_all(root.join("projects").join("project-a")).unwrap();
    fs::create_dir_all(external.join("project-a")).unwrap();
    fs::write(
        external.join("project-a").join("outside.jsonl"),
        b"PRIVATE_OUTSIDE",
    )
    .unwrap();
    let guard = SourceRootGuard::new(&root).unwrap();
    let projects = root.join("projects");
    let old_projects = root.join("old-projects");
    fs::rename(&projects, &old_projects).unwrap();
    junction(&projects, &external);
    assert!(guard
        .open(&projects.join("project-a").join("outside.jsonl"))
        .is_err());
    fs::remove_dir(&projects).unwrap();
    fs::rename(&old_projects, &projects).unwrap();
    let leaf = projects.join("project-a").join("leaf.jsonl");
    junction(&leaf, &external);
    assert!(guard.open(&leaf).is_err());
    fs::remove_dir(&leaf).unwrap();
    let old_root = temp.path().join("old-root");
    fs::rename(&root, &old_root).unwrap();
    junction(&root, &external);
    assert!(guard
        .open(&root.join("project-a").join("outside.jsonl"))
        .is_err());
    fs::remove_dir(&root).unwrap();
}

#[cfg(unix)]
#[test]
fn root_descriptor_traversal_rejects_intermediate_symlink_swap() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registered");
    let external = temp.path().join("external");
    fs::create_dir_all(root.join("projects")).unwrap();
    fs::create_dir(&external).unwrap();
    fs::write(external.join("outside.jsonl"), b"PRIVATE_OUTSIDE").unwrap();
    let guard = SourceRootGuard::new(&root).unwrap();
    fs::rename(root.join("projects"), root.join("old-projects")).unwrap();
    std::os::unix::fs::symlink(&external, root.join("projects")).unwrap();
    assert!(guard
        .open(&root.join("projects").join("outside.jsonl"))
        .is_err());
}
