use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    core: Core,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&temp.path().join("data"), &source).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        Self {
            _temp: temp,
            source,
            core,
        }
    }

    fn write(&self, text: &str) -> PathBuf {
        let path = self.source.join("one.jsonl");
        fs::write(&path, text).unwrap();
        path
    }

    fn scan_discovered(
        &mut self,
        discovered: super::super::adapter::Discovery,
        cancelled: bool,
    ) -> Result<ScanReport> {
        let source = self.core.source_id.clone();
        scan_discovered_source(
            &mut self.core,
            &source,
            &super::super::adapter::CodexAdapter,
            discovered,
            &AtomicBool::new(cancelled),
        )
    }

    fn setting(&self, prefix: &str) -> Option<Value> {
        get_setting(&self.core.db, &format!("{prefix}:{}", self.core.source_id)).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.core
            .db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

fn usage() -> String {
    json!({"timestamp":now(),"type":"token_usage_record","payload":{
        "thread_id":"88000000-0000-4000-8000-000000000001","response_id":"reliability-response",
        "usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":0,
            "cache_write_input_tokens":0,"reasoning_output_tokens":0,"total_tokens":120}
    }})
    .to_string()
        + "\n"
}

#[test]
fn injected_discovery_result_has_an_independent_read_quantum() {
    let mut fixture = Fixture::new();
    let path = fixture.write(&usage());
    // Supplying a completed listing keeps discovery outside the read quantum.
    // No sleep or timing threshold is needed to exercise this boundary.
    let discovered = super::super::adapter::Discovery {
        files: vec![path.clone()],
        unsupported: 2,
        ..Default::default()
    };
    let report = fixture.scan_discovered(discovered, false).unwrap();
    assert_eq!(report.new_responses, 1);
    assert_eq!(fixture.count("usage_responses"), 1);
    let offset: i64 = fixture
        .core
        .db
        .query_row(
            "SELECT offset FROM source_files WHERE path=?1",
            [path.to_string_lossy().to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(offset as u64, fs::metadata(path).unwrap().len());
    let health = fixture.setting("lastSourceCheck").unwrap();
    assert_eq!(health["discoveryComplete"], true);
    assert_eq!(health["unsupportedFiles"], 2);
    assert!(fixture.setting("lastSourceSuccess").is_some());
}

#[test]
fn partial_discovery_reads_available_siblings_and_preserves_incomplete_history() {
    let mut fixture = Fixture::new();
    let path = fixture.write(&usage());
    let source = fixture.core.source_id.clone();
    put_setting(
        &fixture.core.db,
        &format!("historyPending:{source}"),
        &json!(9),
    )
    .unwrap();
    put_setting(
        &fixture.core.db,
        &format!("lastSourceSuccess:{source}"),
        &json!("retained-success"),
    )
    .unwrap();
    let discovered = super::super::adapter::Discovery {
        files: vec![path],
        errors: vec![fixture.source.join("unreadable-directory")],
        ..Default::default()
    };
    let report = fixture.scan_discovered(discovered, false).unwrap();
    assert_eq!(report.new_responses, 1);
    assert!(report.issues > 0);
    assert_eq!(fixture.setting("historyPending"), Some(json!(9)));
    assert_eq!(
        fixture.setting("lastSourceSuccess"),
        Some(json!("retained-success"))
    );
    let health = fixture.setting("lastSourceCheck").unwrap();
    assert_eq!(health["discoveryComplete"], false);
    assert_eq!(health["discoveryErrors"], 1);
    let issue_kind: String = fixture
        .core
        .db
        .query_row("SELECT kind FROM ingestion_issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(issue_kind, "source_discovery_failed");
}

#[test]
fn adapter_semantic_issue_prevents_a_false_success_watermark() {
    let mut fixture = Fixture::new();
    let bad = json!({"timestamp":"invalid-time","type":"token_usage_record","payload":{}});
    let path = fixture.write(&(bad.to_string() + "\n" + &usage()));
    let report = fixture
        .scan_discovered(
            super::super::adapter::Discovery {
                files: vec![path],
                ..Default::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(report.new_responses, 1);
    assert!(report.issues > 0);
    assert_eq!(fixture.count("ingestion_issues"), 1);
    assert!(fixture.setting("lastSourceSuccess").is_none());
    assert!(
        fixture.setting("lastSourceCheck").unwrap()["issues"]
            .as_u64()
            .unwrap()
            > 0
    );
}

#[test]
fn repeated_malformed_line_keeps_explicit_scan_issue_reporting() {
    let mut fixture = Fixture::new();
    let path = fixture.write("{bad}\n");
    for _ in 0..2 {
        fixture.core.db.execute(
            "UPDATE source_files SET offset=0,last_start=0,last_hash='',context='{}',generation=generation+1", [],
        ).unwrap();
        let report = fixture
            .scan_discovered(
                super::super::adapter::Discovery {
                    files: vec![path.clone()],
                    ..Default::default()
                },
                false,
            )
            .unwrap();
        assert!(report.issues > 0);
        assert!(fixture.setting("lastSourceSuccess").is_none());
    }
    assert_eq!(
        fixture.count("ingestion_issues"),
        1,
        "the retained diagnosis stays deduplicated"
    );
}

#[test]
fn repeated_reader_issue_and_new_adapter_issue_are_both_reported() {
    let mut fixture = Fixture::new();
    let path = fixture.write("{bad}\n");
    fixture
        .scan_discovered(
            super::super::adapter::Discovery {
                files: vec![path.clone()],
                ..Default::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(fixture.count("ingestion_issues"), 1);

    let semantic_error =
        json!({"timestamp":"invalid-time","type":"token_usage_record","payload":{}});
    fs::write(&path, format!("{{bad}}\n{semantic_error}\n")).unwrap();
    // Start one new shared physical generation. Both cursors decode the same
    // interval once; elapsed wall-clock time cannot change this test's count.
    fixture.core.db.execute(
        "UPDATE source_files SET offset=0,last_start=0,last_hash='',context='{\"caught_up\":true}',generation=generation+1", [],
    ).unwrap();
    let report = fixture
        .scan_discovered(
            super::super::adapter::Discovery {
                files: vec![path],
                ..Default::default()
            },
            false,
        )
        .unwrap();
    assert_eq!(report.issues, 2);
    assert_eq!(fixture.count("ingestion_issues"), 2);
    let invalid_json_count: i64 = fixture
        .core
        .db
        .query_row(
            "SELECT COUNT(*) FROM ingestion_issues WHERE kind='invalid_json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        invalid_json_count, 1,
        "the old diagnosis remains deduplicated"
    );
    assert_eq!(fixture.setting("lastSourceCheck").unwrap()["issues"], 2);
    assert!(fixture.setting("lastSourceSuccess").is_none());
}

#[test]
fn cancelled_discovery_does_not_advance_success_or_complete_history() {
    let mut fixture = Fixture::new();
    let path = fixture.write(&usage());
    let source = fixture.core.source_id.clone();
    put_setting(
        &fixture.core.db,
        &format!("historyPending:{source}"),
        &json!(7),
    )
    .unwrap();
    let report = fixture
        .scan_discovered(
            super::super::adapter::Discovery {
                files: vec![path],
                cancelled: true,
                ..Default::default()
            },
            true,
        )
        .unwrap();
    assert_eq!(report.new_responses, 0);
    assert_eq!(fixture.count("source_files"), 0);
    assert!(fixture.setting("lastSourceSuccess").is_none());
    assert_eq!(fixture.setting("historyPending"), Some(json!(7)));
    let health = fixture.setting("lastSourceCheck").unwrap();
    assert_eq!(health["cancelled"], true);
    assert_eq!(health["discoveryComplete"], false);
}

#[test]
fn failed_read_batch_rolls_back_discovery_diagnoses_with_facts_and_cursor() {
    let mut fixture = Fixture::new();
    let path = fixture.write(&usage());
    put_setting(&fixture.core.db, "testFailBeforeCommit", &json!(true)).unwrap();
    let result = fixture.scan_discovered(
        super::super::adapter::Discovery {
            files: vec![path],
            errors: vec![fixture.source.join("unreadable")],
            ..Default::default()
        },
        false,
    );
    assert!(result.unwrap_err().contains("injected-before-commit"));
    assert_eq!(fixture.count("usage_responses"), 0);
    assert_eq!(fixture.count("ingested_records"), 0);
    assert_eq!(fixture.count("source_files"), 0);
    assert_eq!(fixture.count("ingestion_issues"), 0);
    assert!(fixture.setting("lastSourceCheck").is_none());
}
