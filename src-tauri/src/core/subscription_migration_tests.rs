use super::*;

type StoredRows = Vec<(String, Vec<Vec<rusqlite::types::Value>>)>;

fn version(db: &Connection) -> i64 {
    db.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn backups(data: &Path) -> Vec<PathBuf> {
    fs::read_dir(data)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ledger-before-v9-")
        })
        .collect()
}

fn schema(db: &Connection) -> Vec<(String, String, Option<String>)> {
    db.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn legacy_rows(db: &Connection) -> StoredRows {
    let names: Vec<String> = db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') AND name NOT IN ('usage_plans','plan_period_state','subscription_observation_sequence','quota_calibrations','local_schedule_templates','local_schedule_instances','local_manual_consumption') ORDER BY name").unwrap()
        .query_map([], |row| row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            // All names come from this test's fixed migration DDL, never from user data.
            // v9 增加来源配置代次；旧列仍逐值检查，新增列在下面独立验证。
            let columns = if name == "source_instances" {
                "id,root,created_at,client_kind,enabled,environment"
            } else {
                "*"
            };
            let mut statement = db
                .prepare(&format!("SELECT {columns} FROM {name} ORDER BY rowid"))
                .unwrap();
            let count = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..count)
                        .map(|i| row.get(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}

fn v6_fixture() -> V1MigrationFixture {
    let fixture = v1_migration_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    // Execute actual old migrations, rather than relabeling a current database.
    for sql in [
        include_str!("../../migrations/002_multi_source.sql"),
        include_str!("../../migrations/003_fact_references.sql"),
        include_str!("../../migrations/004_usage_lookup_index.sql"),
        include_str!("../../migrations/005_pet_read_models.sql"),
        include_str!("../../migrations/006_retention_time_indexes.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute(
        "UPDATE source_files SET source_id='v1-stable-source',channel_id='v1-stable-source:jsonl'",
        [],
    )
    .unwrap();
    // Match normal v6 initialization too, so reopening is not mistaken for
    // migration-driven channel or capability mutation.
    register_channel(&db, "v1-stable-source").unwrap();
    put_setting(&db, "primarySourceId", &json!("v1-stable-source")).unwrap();
    put_setting(&db, "subscription-v6-marker", &json!({"preserve":true})).unwrap();
    db.execute("INSERT INTO account_snapshots(collected_at,data) VALUES('2026-10-21T00:00:00Z','{\"sourceId\":\"legacy-source\",\"usedPercent\":17}')", []).unwrap();
    assert_eq!(version(&db), 6);
    fixture
}

fn assert_current_schema(db: &Connection, before: &StoredRows) {
    assert_eq!(version(db), 9);
    assert_eq!(&legacy_rows(db), before, "all preexisting SQL values, cursor state, feed high water and unknown old identity are preserved");
    for table in [
        "usage_plans",
        "plan_period_state",
        "quota_calibrations",
        "local_schedule_templates",
        "local_schedule_instances",
        "local_manual_consumption",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    let added_column: (String, i64, Option<String>) = db
        .query_row(
            "SELECT type,\"notnull\",dflt_value FROM pragma_table_info('source_instances') WHERE name='configuration_revision'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(added_column, ("INTEGER".into(), 1, Some("0".into())));
    assert_eq!(db.query_row("SELECT COUNT(*) FROM source_instances WHERE configuration_revision IS NULL OR configuration_revision<>0", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    for index in [
        "subscription_observation_scope_time",
        "subscription_observation_sequence",
    ] {
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND name=?1",
                [index],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}

#[test]
fn subscription_v6_upgrade_preserves_wal_facts_cursors_legacy_observation_and_reopen() {
    let fixture = v6_fixture();
    let writer = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    put_setting(&writer, "subscription-committed-wal", &json!("preserved")).unwrap();
    assert!(
        fs::metadata(fixture.data.join("ledger.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = legacy_rows(&writer);
    let core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_current_schema(&core.db, &before);
    assert_v1_fixture_retained(&core.db, &fixture, true);
    let copies = backups(&fixture.data);
    assert_eq!(copies.len(), 1);
    let backup =
        Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&backup), 6);
    assert_eq!(legacy_rows(&backup), before);
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
    assert_current_schema(&reopened.db, &before);
    assert_eq!(backups(&fixture.data).len(), 1);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn subscription_v7_last_ddl_conflict_rolls_back_and_consistent_v6_backup_recovers() {
    let fixture = v6_fixture();
    let original = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    original.execute_batch("CREATE TABLE subscription_observation_sequence(marker TEXT); INSERT INTO subscription_observation_sequence VALUES('test-owned-last-ddl-collision');").unwrap();
    let original_schema = schema(&original);
    let before = legacy_rows(&original);
    drop(original);
    let error = Core::open(&fixture.data, &fixture.source)
        .err()
        .expect("last index collision must reject the migration");
    assert!(
        error.contains("subscription_observation_sequence"),
        "{error}"
    );
    let original = Connection::open_with_flags(
        fixture.data.join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(version(&original), 6);
    assert_eq!(
        schema(&original),
        original_schema,
        "early v7 DDL must roll back too"
    );
    assert_eq!(legacy_rows(&original), before);
    drop(original);
    let copies = backups(&fixture.data);
    assert_eq!(copies.len(), 1);
    let backup =
        Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&backup), 6);
    assert_eq!(schema(&backup), original_schema);
    assert_eq!(legacy_rows(&backup), before);
    drop(backup);
    let recovered_data = fixture._tmp.path().join("recovered-subscription");
    fs::create_dir(&recovered_data).unwrap();
    fs::copy(&copies[0], recovered_data.join("ledger.sqlite3")).unwrap();
    let recovery = Connection::open(recovered_data.join("ledger.sqlite3")).unwrap();
    recovery
        .execute_batch("DROP TABLE subscription_observation_sequence;")
        .unwrap();
    drop(recovery);
    let recovered = Core::open(&recovered_data, &fixture.source).unwrap();
    assert_current_schema(&recovered.db, &before);
    assert_v1_fixture_retained(&recovered.db, &fixture, true);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn subscription_future_v10_is_rejected_without_backup_or_business_writes() {
    let fixture = v6_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    db.execute_batch("PRAGMA user_version=10; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(db);
    let before = fs::read(fixture.data.join("ledger.sqlite3")).unwrap();
    let error = Core::open(&fixture.data, &fixture.source)
        .err()
        .expect("future schema is not migrated");
    assert!(error.contains("账本版本比应用更新"));
    assert_eq!(
        fs::read(fixture.data.join("ledger.sqlite3")).unwrap(),
        before
    );
    assert!(backups(&fixture.data).is_empty());
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}
