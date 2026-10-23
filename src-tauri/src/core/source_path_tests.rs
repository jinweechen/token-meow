use super::*;
use std::io::Write;

struct Rig {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    core: Core,
}
impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sources");
        let codex = root.join("codex");
        fs::create_dir_all(&codex).unwrap();
        let core = Core::open(&tmp.path().join("data"), &codex).unwrap();
        Self {
            _tmp: tmp,
            root,
            core,
        }
    }
    fn folder(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }
    fn configuration(&self, id: &str) -> (String, bool, i64) {
        self.core
            .db
            .query_row(
                "SELECT root,enabled,configuration_revision FROM source_instances WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }
    fn legacy_source(&self, id: &str, root: &Path, created: &str) {
        self.core.db.execute("INSERT INTO source_instances(id,root,client_kind,enabled,created_at) VALUES(?1,?2,'claude',1,?3)", params![id,root.to_string_lossy(),created]).unwrap();
        register_channel(&self.core.db, id).unwrap();
    }
}

#[test]
fn claude_parent_and_projects_reuse_original_registration_without_rebinding_it() {
    let mut rig = Rig::new();
    let root = rig.folder("ClaudeSource");
    let projects = root.join("projects");
    // 首次尚无 CLI 子目录时也能登记，随后子目录存在不改变来源身份。
    let first = rig.core.add_source_with_kind(&root, "claude").unwrap();
    fs::create_dir_all(&projects).unwrap();
    let before = rig.configuration(&first);
    assert_eq!(
        rig.core.add_source_with_kind(&projects, "claude").unwrap(),
        first
    );
    assert_eq!(rig.configuration(&first), before);
    assert_eq!(rig.core.sources().unwrap().len(), 2);
    assert!(rig.core.add_source_with_kind(&root, "codex").is_err());
    assert!(rig.core.add_source_with_kind(&projects, "codex").is_err());
}

#[test]
fn gemini_tmp_aliases_reuse_id_while_independent_account_roots_stay_separate() {
    let mut rig = Rig::new();
    let a = rig.folder("gemini-account-a");
    let b = rig.folder("gemini-account-b");
    fs::create_dir_all(a.join("tmp")).unwrap();
    fs::create_dir_all(b.join("tmp")).unwrap();
    let first = rig
        .core
        .add_source_with_kind(&a.join("tmp"), "gemini")
        .unwrap();
    assert_eq!(rig.core.add_source_with_kind(&a, "gemini").unwrap(), first);
    let second = rig.core.add_source_with_kind(&b, "gemini").unwrap();
    assert_ne!(first, second);
    assert_eq!(rig.core.sources().unwrap().len(), 3);
}

#[cfg(windows)]
#[test]
fn windows_case_and_separator_aliases_never_create_another_source_identity() {
    let mut rig = Rig::new();
    let root = rig.folder("ClaudeSource");
    fs::create_dir_all(root.join("projects")).unwrap();
    let first = rig.core.add_source_with_kind(&root, "claude").unwrap();
    let alias = root.parent().unwrap().join("claudesource");
    assert!(alias.is_dir());
    assert_eq!(
        rig.core.add_source_with_kind(&alias, "claude").unwrap(),
        first
    );
    let forward = PathBuf::from(format!("{}/", root.to_string_lossy().replace('\\', "/")));
    assert_eq!(
        rig.core.add_source_with_kind(&forward, "claude").unwrap(),
        first
    );
    assert_eq!(rig.core.sources().unwrap().len(), 2);
}

#[test]
fn relocation_collision_rolls_back_configuration_and_self_alias_keeps_source_identity() {
    let mut rig = Rig::new();
    let a = rig.folder("claude-a");
    let b = rig.folder("claude-b");
    fs::create_dir_all(a.join("projects")).unwrap();
    fs::create_dir_all(b.join("projects")).unwrap();
    let first = rig.core.add_source_with_kind(&a, "claude").unwrap();
    let second = rig.core.add_source_with_kind(&b, "claude").unwrap();
    let before = rig.configuration(&second);
    assert!(rig
        .core
        .action(
            "update_source",
            &json!({"id":second,"path":a.join("projects"),"enabled":false})
        )
        .is_err());
    assert_eq!(rig.configuration(&second), before);
    rig.core
        .action(
            "update_source",
            &json!({"id":first,"path":a.join("projects")}),
        )
        .unwrap();
    assert_eq!(
        rig.configuration(&first),
        (a.join("projects").to_string_lossy().into_owned(), true, 1)
    );
}

fn append_claude(path: &Path, id: &str) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", json!({"type":"assistant","sessionId":"source-path-session","timestamp":now(),
        "message":{"id":id,"role":"assistant","model":"fixture-model","usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":50,"cache_read_input_tokens":300}}})).unwrap();
}

#[test]
fn legacy_overlapping_registrations_block_extra_scan_and_preserve_history_and_enabled_flags() {
    let mut rig = Rig::new();
    let root = rig.folder("claude");
    let logs = root.join("projects").join("project-a");
    fs::create_dir_all(&logs).unwrap();
    let path = logs.join("session.jsonl");
    append_claude(&path, "message-one");
    let owner = rig.core.add_source_with_kind(&root, "claude").unwrap();
    rig.core.scan().unwrap();
    // 模拟旧版本已存在的父子入口登记；不给它更改 enabled 或删除历史的隐式授权。
    rig.legacy_source(
        "legacy-duplicate",
        &root.join("projects"),
        "2000-01-01T00:00:00Z",
    );
    let admitted = rig.core.scan_source_bindings().unwrap();
    assert!(admitted.iter().any(|row| row.0 == owner));
    assert!(!admitted.iter().any(|row| row.0 == "legacy-duplicate"));
    append_claude(&path, "message-two");
    for _ in 0..3 {
        rig.core.scan().unwrap();
    }
    let count: i64 = rig
        .core
        .db
        .query_row("SELECT count(*) FROM usage_responses", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        rig.core.snapshot(json!({})).unwrap()["overview"]["totalTokens"],
        940
    );
    assert_eq!(rig.configuration("legacy-duplicate").1, true);
    let blocked = rig
        .core
        .sources()
        .unwrap()
        .into_iter()
        .find(|row| row["id"] == "legacy-duplicate")
        .unwrap();
    assert_eq!(blocked["runtime"], "不可用");
    assert_eq!(blocked["health"]["blockedReason"], "source_path_duplicate");
    assert_eq!(blocked["health"]["duplicateOf"], owner);
    assert!(rig.core.snapshot(json!({})).unwrap()["health"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["kind"] == "source_path_duplicate"));
    assert!(rig
        .core
        .action(
            "update_source",
            &json!({"id":"legacy-duplicate","enabled":true})
        )
        .is_err());
    rig.core
        .action(
            "update_source",
            &json!({"id":"legacy-duplicate","enabled":false}),
        )
        .unwrap();
    assert_eq!(rig.configuration("legacy-duplicate").1, false);
    assert_eq!(
        rig.core
            .db
            .query_row("SELECT count(*) FROM usage_responses", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn disabled_owner_with_existing_cursor_blocks_an_enabled_legacy_alias_without_moving_history() {
    let mut rig = Rig::new();
    let root = rig.folder("claude");
    let logs = root.join("projects").join("project-a");
    fs::create_dir_all(&logs).unwrap();
    let path = logs.join("session.jsonl");
    append_claude(&path, "message-one");
    let owner = rig.core.add_source_with_kind(&root, "claude").unwrap();
    rig.core.scan().unwrap();
    rig.core
        .action("update_source", &json!({"id":owner,"enabled":false}))
        .unwrap();
    rig.legacy_source(
        "legacy-enabled",
        &root.join("projects"),
        "2000-01-01T00:00:00Z",
    );
    append_claude(&path, "message-two");
    for _ in 0..2 {
        rig.core.scan().unwrap();
    }
    assert_eq!(
        rig.core
            .db
            .query_row("SELECT count(*) FROM usage_responses", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let blocked = rig
        .core
        .sources()
        .unwrap()
        .into_iter()
        .find(|row| row["id"] == "legacy-enabled")
        .unwrap();
    assert_eq!(blocked["runtime"], "不可用");
    assert_eq!(blocked["health"]["duplicateOf"], owner);
    assert!(blocked["health"]["error"]
        .as_str()
        .unwrap()
        .contains("旧游标属于已停用来源"));
    // 明确停用别名后可重新启用原身份，不复制或迁移旧游标。
    rig.core
        .action(
            "update_source",
            &json!({"id":"legacy-enabled","enabled":false}),
        )
        .unwrap();
    let before = rig.configuration("legacy-enabled");
    let error = rig
        .core
        .action(
            "update_source",
            &json!({"id":"legacy-enabled","enabled":true}),
        )
        .unwrap_err();
    assert!(error.contains("旧游标属于来源"));
    assert_eq!(
        rig.configuration("legacy-enabled"),
        before,
        "别名不能抢占已停用原身份的游标"
    );
    rig.core
        .action("update_source", &json!({"id":owner,"enabled":true}))
        .unwrap();
    rig.core.scan().unwrap();
    assert_eq!(
        rig.core
            .db
            .query_row("SELECT count(*) FROM usage_responses", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        rig.core
            .db
            .query_row(
                "SELECT count(*) FROM source_files WHERE source_id='legacy-enabled'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn historical_cursors_from_a_previous_root_do_not_claim_a_different_current_entry() {
    let mut rig = Rig::new();
    let a = rig.folder("claude-a");
    let b = rig.folder("claude-b");
    let a_logs = a.join("projects").join("project-a");
    let b_logs = b.join("projects").join("project-b");
    fs::create_dir_all(&a_logs).unwrap();
    fs::create_dir_all(&b_logs).unwrap();
    append_claude(&a_logs.join("a.jsonl"), "message-a");
    append_claude(&b_logs.join("b.jsonl"), "message-b-one");
    let a_id = rig.core.add_source_with_kind(&a, "claude").unwrap();
    let b_id = rig.core.add_source_with_kind(&b, "claude").unwrap();
    for _ in 0..2 {
        rig.core.scan().unwrap();
    }
    // 旧版本可把 A 改到 B/projects；A 的历史游标仍在此前独立的目录 A。
    rig.core
        .db
        .execute(
            "UPDATE source_instances SET root=?1 WHERE id=?2",
            params![b.join("projects").to_string_lossy(), a_id],
        )
        .unwrap();
    let admitted = rig.core.scan_source_bindings().unwrap();
    assert!(admitted.iter().any(|row| row.0 == b_id));
    assert!(!admitted.iter().any(|row| row.0 == a_id));
    append_claude(&b_logs.join("b.jsonl"), "message-b-two");
    rig.core.scan().unwrap();
    assert_eq!(
        rig.core
            .db
            .query_row(
                "SELECT count(*) FROM usage_responses WHERE source_id=?1",
                [&a_id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        rig.core
            .db
            .query_row(
                "SELECT count(*) FROM usage_responses WHERE source_id=?1",
                [&b_id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
}
