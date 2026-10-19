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
    assert_eq!(fixture.core.enabled_codex_source_roots().unwrap(), vec![fixture.source.clone()]);

    fixture.core.action("update_source", &json!({"id":id,"enabled":false})).unwrap();
    assert!(fixture.core.enabled_codex_source_roots().unwrap().is_empty());
    let moved = fixture.temp.path().join("relocated");
    fs::create_dir(&moved).unwrap();
    fixture.core.action("update_source", &json!({"id":id,"path":moved})).unwrap();
    assert!(fixture.core.enabled_codex_source_roots().unwrap().is_empty());

    fixture.core.action("update_source", &json!({"id":id,"enabled":true})).unwrap();
    assert_eq!(fixture.core.enabled_codex_source_roots().unwrap(), vec![moved.clone()]);
    fs::remove_dir(&moved).unwrap();
    assert_eq!(fixture.core.enabled_codex_source_roots().unwrap(), vec![moved]);
    // The watcher retains responsibility for is_dir, retry and watch replacement.
}

#[test]
fn watcher_config_read_survives_broken_health_and_missing_fact_tables() {
    let fixture = Fixture::new();
    let health_key = format!("lastSourceCheck:{}", fixture.core.source_id);
    fixture.core.db.execute(
        "INSERT INTO settings(key,value) VALUES(?1,'not-json')",
        [&health_key],
    ).unwrap();
    assert!(fixture.core.sources().is_err());
    assert_eq!(fixture.core.enabled_codex_source_roots().unwrap(), vec![fixture.source.clone()]);

    fixture.core.db.execute("DELETE FROM settings WHERE key=?1", [&health_key]).unwrap();
    fixture.core.db.execute_batch("DROP TABLE usage_responses;").unwrap();
    assert!(fixture.core.sources().is_err());
    assert_eq!(fixture.core.enabled_codex_source_roots().unwrap(), vec![fixture.source]);
}
