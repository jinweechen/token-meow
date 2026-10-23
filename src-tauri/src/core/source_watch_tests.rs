use super::*;

struct Fixture {
    core: Core,
    source: PathBuf,
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("pet"), &source).unwrap();
        Self { core, source, temp }
    }

    fn add_config(&self, id: &str, kind: &str, enabled: bool) -> PathBuf {
        let root = self.temp.path().join(id);
        fs::create_dir(&root).unwrap();
        self.core.db.execute(
            "INSERT INTO source_instances(id,root,created_at,client_kind,enabled) VALUES(?1,?2,?3,?4,?5)",
            params![id, root.to_string_lossy(), now(), kind, enabled],
        ).unwrap();
        root
    }
}

#[test]
fn watcher_roots_exclude_disabled_and_non_codex_sources() {
    let fixture = Fixture::new();
    let enabled = fixture.add_config("enabled", "codex", true);
    let disabled = fixture.add_config("disabled", "codex", false);
    let other = fixture.add_config("other", "synthetic-other", true);

    let mut roots = fixture.core.enabled_codex_source_roots().unwrap();
    roots.sort();
    let mut expected = vec![fixture.source.clone(), enabled];
    expected.sort();
    assert_eq!(roots, expected);

    let protected = fixture.core.registered_source_roots().unwrap();
    assert!(protected.contains(&disabled));
    assert!(protected.contains(&other));
}

#[test]
fn watcher_roots_follow_relocation_and_reenable() {
    let mut fixture = Fixture::new();
    let id = fixture.core.source_id.clone();
    assert_eq!(
        fixture.core.enabled_codex_source_roots().unwrap(),
        vec![fixture.source.clone()]
    );

    fixture
        .core
        .action("update_source", &json!({"id":id,"enabled":false}))
        .unwrap();
    assert!(fixture
        .core
        .enabled_codex_source_roots()
        .unwrap()
        .is_empty());
    let moved = fixture.temp.path().join("relocated");
    fs::create_dir(&moved).unwrap();
    fixture
        .core
        .action("update_source", &json!({"id":id,"path":moved}))
        .unwrap();
    assert!(fixture
        .core
        .enabled_codex_source_roots()
        .unwrap()
        .is_empty());

    fixture
        .core
        .action("update_source", &json!({"id":id,"enabled":true}))
        .unwrap();
    assert_eq!(
        fixture.core.enabled_codex_source_roots().unwrap(),
        vec![moved.clone()]
    );
    fs::remove_dir(&moved).unwrap();
    assert_eq!(
        fixture.core.enabled_codex_source_roots().unwrap(),
        vec![moved]
    );
    // The watcher retains responsibility for is_dir, retry and watch replacement.
}

#[test]
fn watcher_config_read_survives_broken_health_and_missing_fact_tables() {
    let fixture = Fixture::new();
    let health_key = format!("lastSourceCheck:{}", fixture.core.source_id);
    fixture
        .core
        .db
        .execute(
            "INSERT INTO settings(key,value) VALUES(?1,'not-json')",
            [&health_key],
        )
        .unwrap();
    assert!(fixture.core.sources().is_err());
    assert_eq!(
        fixture.core.enabled_codex_source_roots().unwrap(),
        vec![fixture.source.clone()]
    );

    fixture
        .core
        .db
        .execute("DELETE FROM settings WHERE key=?1", [&health_key])
        .unwrap();
    fixture
        .core
        .db
        .execute_batch("DROP TABLE usage_responses;")
        .unwrap();
    assert!(fixture.core.sources().is_err());
    assert_eq!(
        fixture.core.enabled_codex_source_roots().unwrap(),
        vec![fixture.source]
    );
}

fn assert_configuration_binding(
    fixture: &Fixture,
    expected_root: &Path,
    revision: i64,
    enabled: bool,
) {
    let id = &fixture.core.source_id;
    let persisted: (String, i64, bool) = fixture
        .core
        .db
        .query_row(
            "SELECT root,configuration_revision,enabled FROM source_instances WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        persisted,
        (
            expected_root.to_string_lossy().into_owned(),
            revision,
            enabled
        )
    );
    assert_eq!(
        fixture.core.source_root.as_path(),
        expected_root,
        "内存来源只能在事务成功提交后改变"
    );
    let primary = fixture.core.primary_source().unwrap();
    let account = fixture
        .core
        .source_account_bindings()
        .unwrap()
        .into_iter()
        .find(|row| row["id"] == *id)
        .unwrap();
    let row = fixture
        .core
        .sources()
        .unwrap()
        .into_iter()
        .find(|row| row["id"] == *id)
        .unwrap();
    for (view, source) in [
        ("primary_source", primary),
        ("source_account_bindings", account),
        ("sourceRows", row),
    ] {
        assert_eq!(
            source["configurationRevision"],
            json!(revision.to_string()),
            "{view} 必须公开同一规范十进制修订"
        );
        assert_eq!(
            source["root"],
            json!(expected_root.to_string_lossy()),
            "{view}"
        );
        assert_eq!(source["enabled"], json!(enabled), "{view}");
    }
}

#[test]
fn source_configuration_revision_survives_a_b_a_and_disable_reenable_in_all_views() {
    let mut fixture = Fixture::new();
    let id = fixture.core.source_id.clone();
    let a = fixture.source.clone();
    let b = fixture.temp.path().join("configuration-b");
    fs::create_dir(&b).unwrap();
    assert_configuration_binding(&fixture, &a, 0, true);
    fixture
        .core
        .action("update_source", &json!({"id":id,"path":b}))
        .unwrap();
    assert_configuration_binding(&fixture, &b, 1, true);
    fixture
        .core
        .action("update_source", &json!({"id":id,"path":a}))
        .unwrap();
    assert_configuration_binding(&fixture, &a, 2, true);
    fixture
        .core
        .action("update_source", &json!({"id":id,"enabled":false}))
        .unwrap();
    assert_configuration_binding(&fixture, &a, 3, false);
    fixture
        .core
        .action("update_source", &json!({"id":id,"enabled":true}))
        .unwrap();
    assert_configuration_binding(&fixture, &a, 4, true);
    assert_eq!(fixture.core.enabled_codex_source_roots().unwrap(), vec![a]);
}

#[test]
fn invalid_source_updates_and_late_transaction_failure_preserve_revision_and_root() {
    let mut fixture = Fixture::new();
    let id = fixture.core.source_id.clone();
    let original = fixture.source.clone();
    let missing = fixture.temp.path().join("not-created");
    let other = fixture.temp.path().join("configuration-b");
    fs::create_dir(&other).unwrap();
    for invalid in [
        json!({"id":id,"clientKind":"gemini"}),
        json!({"id":id,"clientKind":17}),
        json!({"id":id,"path":missing}),
        json!({"id":id,"path":123}),
        json!({"id":id,"path":null}),
        json!({"id":id,"enabled":"bad"}),
    ] {
        assert!(
            fixture.core.action("update_source", &invalid).is_err(),
            "无效配置不得静默成为一次成功修订"
        );
        assert_configuration_binding(&fixture, &original, 0, true);
    }
    // 该触发器只在 counter 已递增后拒绝通道写入，验证早先 root/enabled/counter 一并回滚。
    fixture.core.db.execute_batch("CREATE TRIGGER controlled_configuration_channel_failure BEFORE INSERT ON collection_channels WHEN EXISTS(SELECT 1 FROM source_instances WHERE configuration_revision>0) BEGIN SELECT RAISE(ABORT,'controlled-configuration-channel-failure'); END;").unwrap();
    let error = fixture
        .core
        .action(
            "update_source",
            &json!({"id":id,"path":other,"enabled":false}),
        )
        .unwrap_err();
    assert!(
        error.contains("controlled-configuration-channel-failure"),
        "{error}"
    );
    assert_configuration_binding(&fixture, &original, 0, true);
    fixture
        .core
        .db
        .execute_batch("DROP TRIGGER controlled_configuration_channel_failure;")
        .unwrap();
    fixture
        .core
        .action("update_source", &json!({"id":id,"path":other}))
        .unwrap();
    assert_configuration_binding(&fixture, &other, 1, true);
}
