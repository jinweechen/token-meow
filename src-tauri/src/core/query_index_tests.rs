use super::*;

const INDEX: &str = "usage_source_turn_thread";

fn version(db: &Connection) -> i64 {
    db.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn backups(data: &Path) -> Vec<PathBuf> {
    fs::read_dir(data)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("ledger-before-v6-")
        })
        .map(|entry| entry.path())
        .collect()
}

fn schema(db: &Connection) -> Vec<(String, String, Option<String>)> {
    db.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
    .unwrap()
    .collect::<std::result::Result<_, _>>()
    .unwrap()
}

fn facts(db: &Connection) -> Vec<(String, String, String)> {
    db.prepare("SELECT id,digest,data FROM usage_responses ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

fn advance_fixture(db: &Connection, target: i64) {
    if target >= 2 {
        db.execute_batch(include_str!("../../migrations/002_multi_source.sql"))
            .unwrap();
        db.execute("UPDATE source_files SET source_id='v1-stable-source',channel_id='v1-stable-source:jsonl'",[]).unwrap();
    }
    if target >= 3 {
        db.execute_batch(include_str!("../../migrations/003_fact_references.sql"))
            .unwrap();
    }
}

#[test]
fn query_index_fresh_schema_and_newer_schema_rejection_do_not_create_backups_or_rewrite_data() {
    let r = Rig::new();
    assert_eq!(version(&r.core.db), 6);
    let columns: Vec<String> = r
        .core
        .db
        .prepare(&format!("PRAGMA index_info({INDEX})"))
        .unwrap()
        .query_map([], |row| row.get(2))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(columns, ["source_id", "turn_id", "thread_id"]);
    assert!(backups(&r.data).is_empty());
    let data = r.data.clone();
    let source = r.source.clone();
    r.core
        .db
        .execute_batch("PRAGMA user_version=7; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(r.core);
    let before = fs::read(data.join("ledger.sqlite3")).unwrap();
    let error = Core::open(&data, &source)
        .err()
        .expect("future schema must be rejected");
    assert!(error.contains("账本版本比应用更新"));
    assert_eq!(fs::read(data.join("ledger.sqlite3")).unwrap(), before);
    assert!(backups(&data).is_empty());
}

#[test]
fn query_index_v3_committed_wal_backup_retains_facts_cursors_preferences_and_reopen_is_idempotent()
{
    let fixture = v1_migration_fixture();
    let writer = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    advance_fixture(&writer, 3);
    let fact = normalized(native(THREAD, "v3-wal-response", 200), "v1-stable-source");
    let data = serde_json::to_string(&fact).unwrap();
    let digest = hash(data.as_bytes());
    writer.execute("INSERT INTO usage_responses(id,source_id,thread_id,response_id,turn_id,event_time,digest,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![response_key(&fact),fact.source_id,fact.thread_id,fact.response_id,fact.turn_id,fact.event_time,digest,data]).unwrap();
    put_setting(&writer, "v4-wal-marker", &json!("committed-before-upgrade")).unwrap();
    assert!(
        fs::metadata(fixture.data.join("ledger.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let original_facts = facts(&writer);
    let original_cursor: (i64, i64, String) = writer
        .query_row(
            "SELECT generation,offset,context FROM source_files WHERE id=17",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_eq!(version(&core.db), 6);
    assert_eq!(facts(&core.db), original_facts);
    assert_eq!(core.settings().unwrap()["petScale"], 1.25);
    assert_eq!(core.source_id, "v1-stable-source");
    assert_eq!(
        core.db
            .query_row(
                "SELECT generation,offset,context FROM source_files WHERE id=17",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?
                ))
            )
            .unwrap(),
        original_cursor
    );
    let copies = backups(&fixture.data);
    assert_eq!(copies.len(), 1);
    let backup =
        Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&backup), 3);
    assert_eq!(facts(&backup), original_facts);
    assert_eq!(
        get_setting(&backup, "v4-wal-marker").unwrap(),
        Some(json!("committed-before-upgrade"))
    );
    assert_eq!(
        backup
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name=?1",
                [INDEX],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(backup);
    drop(writer);
    drop(core);
    let reopened = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_eq!(version(&reopened.db), 6);
    assert_eq!(facts(&reopened.db), original_facts);
    assert_eq!(backups(&fixture.data).len(), 1);
    assert_eq!(
        reopened.snapshot(json!({})).unwrap()["overview"]["totalTokens"],
        1920 // Existing fixture 1700 + authored committed-WAL response 220.
    );
}

#[test]
fn query_index_last_ddl_failure_rolls_back_every_pending_generation_and_backup_can_recover() {
    for original_version in [1, 2, 3] {
        let fixture = v1_migration_fixture();
        {
            let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
            advance_fixture(&db, original_version);
            db.execute_batch(&format!("CREATE TABLE {INDEX}(fixture_marker TEXT); INSERT INTO {INDEX} VALUES('controlled-last-migration-collision');")).unwrap();
        }
        let original = Connection::open_with_flags(
            fixture.data.join("ledger.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let original_schema = schema(&original);
        let original_facts = facts(&original);
        drop(original);
        let error = Core::open(&fixture.data, &fixture.source)
            .err()
            .expect("index-name collision must fail upgrade");
        assert!(error.contains(INDEX), "unexpected upgrade failure: {error}");
        let assert_original = |db: &Connection| {
            assert_eq!(version(db), original_version);
            assert_eq!(schema(db), original_schema);
            assert_eq!(facts(db), original_facts);
            assert_v1_fixture_retained(db, &fixture, true);
        };
        let original = Connection::open_with_flags(
            fixture.data.join("ledger.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_original(&original);
        drop(original);
        let copies = backups(&fixture.data);
        assert_eq!(copies.len(), 1);
        let backup =
            Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_original(&backup);
        drop(backup);
        let recovered_data = fixture._tmp.path().join("recovered-index-fixture");
        fs::create_dir(&recovered_data).unwrap();
        fs::copy(&copies[0], recovered_data.join("ledger.sqlite3")).unwrap();
        {
            let recovered = Connection::open(recovered_data.join("ledger.sqlite3")).unwrap();
            // Remove only this test's deliberate obstruction from a backup copy.
            recovered
                .execute_batch(&format!("DROP TABLE {INDEX};"))
                .unwrap();
        }
        let recovered = Core::open(&recovered_data, &fixture.source).unwrap();
        assert_eq!(version(&recovered.db), 6);
        assert_eq!(facts(&recovered.db), original_facts);
        assert_v1_migrated_snapshot(&recovered);
        let original = Connection::open_with_flags(
            fixture.data.join("ledger.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_original(&original);
    }
}

#[test]
fn query_index_and_source_reuse_keep_snapshots_csv_and_global_health_semantics() {
    let mut r = Rig::new();
    let child = "00000000-0000-4000-8000-000000000002";
    direct(
        &r.core,
        &[
            event(
                "session_meta",
                json!({"id":THREAD,"cli_version":"0.147.0","thread_source":"user"}),
            ),
            native(THREAD, "index-primary", 1000),
            event(
                "session_meta",
                json!({"id":child,"parent_thread_id":THREAD}),
            ),
            native(child, "index-child", 200),
        ],
    );
    let second_root = r._tmp.path().join("disabled-source");
    fs::create_dir(&second_root).unwrap();
    let second = r.core.add_source(&second_root).unwrap();
    r.core
        .action("update_source", &json!({"id":second,"enabled":false}))
        .unwrap();
    put_setting(
        &r.core.db,
        &format!("historyPending:{}", r.core.source_id),
        &json!(2),
    )
    .unwrap();
    put_setting(&r.core.db, &format!("historyPending:{second}"), &json!(99)).unwrap();
    for (id, source_id) in [
        ("disabled-cap", second.as_str()),
        ("orphan-cap", "missing-source"),
    ] {
        let cap =
            json!({"id":id,"sourceId":source_id,"usage":{"support":"已接入","runtime":"可用"}});
        r.core
            .db
            .execute(
                "INSERT INTO source_capabilities(id,source_id,data) VALUES(?1,?2,?3)",
                params![id, source_id, cap.to_string()],
            )
            .unwrap();
    }
    r.core
        .db
        .execute_batch(&format!("DROP INDEX {INDEX};"))
        .unwrap();
    let queries = vec![
        json!({}),
        json!({"agentKind":"primary"}),
        json!({"agentKind":"subagent"}),
        json!({"agentKind":"unknown"}),
        json!({"sourceId":second}),
    ];
    let before: Vec<_> = queries
        .iter()
        .map(|q| {
            (
                r.core.snapshot(q.clone()).unwrap(),
                r.core.export_csv(q.clone()).unwrap(),
            )
        })
        .collect();
    r.core
        .db
        .execute_batch(
            &include_str!("../../migrations/004_usage_lookup_index.sql")
                .replace("PRAGMA user_version=4;", ""),
        )
        .unwrap();
    for (q, (snapshot, csv)) in queries.iter().zip(&before) {
        assert_eq!(&r.core.snapshot(q.clone()).unwrap(), snapshot);
        assert_eq!(&r.core.export_csv(q.clone()).unwrap(), csv);
    }
    let snapshot = &before[0].0;
    assert_eq!(snapshot["health"]["history"]["pendingFiles"], 2);
    let caps = snapshot["sourceCapabilities"].as_array().unwrap();
    let disabled = caps.iter().find(|cap| cap["id"] == "disabled-cap").unwrap();
    assert_eq!(disabled["usage"]["sourceRuntime"], "未配置");
    assert_eq!(disabled["usage"]["runtime"], "未配置");
    let orphan = caps
        .iter()
        .find(|cap| cap["sourceId"] == "missing-source")
        .unwrap();
    assert!(orphan["usage"].get("sourceRuntime").is_none());
    assert_eq!(orphan["usage"]["runtime"], "可用");
    let disabled_source = snapshot["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["id"] == second)
        .unwrap();
    assert!(disabled_source["health"]["unsupportedFiles"].is_null());
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM usage_responses WHERE source_id=?1 AND thread_id=?2 AND turn_id=?3)",
        "SELECT DISTINCT thread_id FROM usage_responses WHERE source_id=?1 AND turn_id=?3 LIMIT 2",
    ] {
        let plan: Vec<String> = r.core.db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap()
            .query_map(params![r.core.source_id,THREAD,"turn-a"],|row|row.get(3)).unwrap()
            .collect::<std::result::Result<_,_>>().unwrap();
        assert!(plan.iter().any(|step|step.contains("COVERING INDEX usage_source_turn_thread")),"{plan:?}");
    }
}
