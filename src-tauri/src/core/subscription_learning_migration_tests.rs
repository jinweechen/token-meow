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

fn prior_rows(db: &Connection) -> StoredRows {
    let names: Vec<String> = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') AND name NOT IN ('quota_calibrations','local_schedule_templates','local_schedule_instances','local_manual_consumption') ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            // Identifiers come only from the fixed, test-owned migration DDL.
            // 显式保留 v7 的所有来源旧列；v9 新列不能让 SELECT * 比较失去意义。
            let columns = if name == "source_instances" {
                "id,root,created_at,client_kind,enabled,environment"
            } else {
                "*"
            };
            let mut statement = db
                .prepare(&format!("SELECT {columns} FROM {name} ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            let values = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            (name, values)
        })
        .collect()
}

fn v7_fixture() -> V1MigrationFixture {
    let fixture = v1_migration_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    // Real generations 001..007: a current database with its version changed
    // would leave v8 objects behind and cannot prove an actual v7 upgrade.
    for sql in [
        include_str!("../../migrations/002_multi_source.sql"),
        include_str!("../../migrations/003_fact_references.sql"),
        include_str!("../../migrations/004_usage_lookup_index.sql"),
        include_str!("../../migrations/005_pet_read_models.sql"),
        include_str!("../../migrations/006_retention_time_indexes.sql"),
        include_str!("../../migrations/007_subscription_plans.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute(
        "UPDATE source_files SET source_id='v1-stable-source',channel_id='v1-stable-source:jsonl'",
        [],
    )
    .unwrap();
    register_channel(&db, "v1-stable-source").unwrap();
    put_setting(&db, "primarySourceId", &json!("v1-stable-source")).unwrap();
    put_setting(&db, "subscriptionRevision", &json!(17)).unwrap();
    for (revision, target) in [(1, 100_000), (2, 80_000)] {
        let plan = json!({"id":"v7-plan","revision":revision,"sourceId":"v1-stable-source",
            "title":"preserve-v7-plan","type":"tokens","model":null,"modelDimension":"response",
            "target":target,"effectiveFrom":"2026-10-21T00:00:00.000Z","endPolicy":"time",
            "endsAt":"2026-10-23T00:00:00.000Z","windowId":null,"periodInstanceId":"v7-plan",
            "lifecycle":"active","remindersEnabled":false});
        db.execute(
            "INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES('v7-plan',?1,'v1-stable-source',?2)",
            params![revision,plan.to_string()],
        ).unwrap();
    }
    for (kind, owner, period, data) in [
        (
            "plan",
            "v7-plan",
            "v7-plan",
            json!({"latched":[80],"runtimeId":"old-runtime","used":81_000,"demandHistory":{"status":"invalidated","reasons":["test-owned-old-proof"]}}),
        ),
        (
            "quota_watch",
            "v7-watch",
            "old-official-window",
            json!({"riskEpisode":2,"latched":["risk"],"scope":{"sourceId":"v1-stable-source","runtimeId":"old-runtime"}}),
        ),
    ] {
        db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES(?1,?2,?3,?4)",params![kind,owner,period,data.to_string()]).unwrap();
    }
    db.execute("INSERT INTO account_snapshots(collected_at,data) VALUES('2026-10-21T00:00:00.000Z',?1)",
        [json!({"documentKind":"subscription_observation","schemaVersion":1,"sourceId":"v1-stable-source","runtimeId":"old-runtime","bindingGeneration":"1","clearGeneration":null,"identityEpoch":"1","observationSequence":"18","lastSettledSequence":"18","lastPersistedSequence":"18","outcome":"failure","identityConsistency":"unproven"}).to_string()]).unwrap();
    assert_eq!(version(&db), 7);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name='quota_calibrations'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    fixture
}

fn assert_current(db: &Connection, original: &StoredRows) {
    assert_eq!(version(db), 9);
    assert_eq!(
        &prior_rows(db),
        original,
        "migration must preserve all original facts, cursors, plan revisions, latches and unknown account evidence"
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM quota_calibrations", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "migration cannot fabricate learned capacity from old observations"
    );
    let index_columns: Vec<String> = db
        .prepare("PRAGMA index_info(quota_calibrations_source_time)")
        .unwrap()
        .query_map([], |row| row.get(2))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(index_columns, ["source_id", "updated_at"]);
    for table in [
        "local_schedule_templates",
        "local_schedule_instances",
        "local_manual_consumption",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "迁移不能从旧计划制造周期发生项或手动消费"
        );
    }
    let added_column: (String, i64, Option<String>) = db.query_row(
        "SELECT type,\"notnull\",dflt_value FROM pragma_table_info('source_instances') WHERE name='configuration_revision'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(added_column, ("INTEGER".into(), 1, Some("0".into())));
    assert_eq!(db.query_row("SELECT COUNT(*) FROM source_instances WHERE configuration_revision IS NULL OR configuration_revision<>0", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn subscription_learning_v7_through_v9_keeps_wal_facts_plans_states_and_reopens_idempotently() {
    let fixture = v7_fixture();
    let writer = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    put_setting(
        &writer,
        "subscription-v7-committed-wal",
        &json!({"preserve":true}),
    )
    .unwrap();
    assert!(
        fs::metadata(fixture.data.join("ledger.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = prior_rows(&writer);
    let old_schema = schema(&writer);
    let core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_current(&core.db, &before);
    assert_v1_fixture_retained(&core.db, &fixture, true);
    let copies = backups(&fixture.data);
    assert_eq!(copies.len(), 1);
    let backup =
        Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&backup), 7);
    assert_eq!(schema(&backup), old_schema);
    assert_eq!(prior_rows(&backup), before);
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(backup);
    drop(core);
    drop(writer);
    let reopened = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_current(&reopened.db, &before);
    assert_eq!(backups(&fixture.data).len(), 1);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn subscription_learning_v8_last_ddl_failure_rolls_back_and_v7_backup_recovers() {
    let fixture = v7_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE quota_calibrations_source_time(marker TEXT); INSERT INTO quota_calibrations_source_time VALUES('controlled-final-v8-index-collision');").unwrap();
    let original_schema = schema(&db);
    let original_rows = prior_rows(&db);
    drop(db);
    let error = Core::open(&fixture.data, &fixture.source)
        .err()
        .expect("final v8 index must collide");
    assert!(
        error.contains("quota_calibrations_source_time"),
        "unexpected failure: {error}"
    );
    let assert_original = |db: &Connection| {
        assert_eq!(version(db), 7);
        assert_eq!(
            schema(db),
            original_schema,
            "the new quota_calibrations table must roll back with the failing index"
        );
        assert_eq!(prior_rows(db), original_rows);
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
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(backup);
    let recovered_data = fixture._tmp.path().join("recovered-subscription-learning");
    fs::create_dir(&recovered_data).unwrap();
    fs::copy(&copies[0], recovered_data.join("ledger.sqlite3")).unwrap();
    let recovery = Connection::open(recovered_data.join("ledger.sqlite3")).unwrap();
    recovery
        .execute_batch("DROP TABLE quota_calibrations_source_time;")
        .unwrap();
    let recovered_rows = prior_rows(&recovery);
    drop(recovery);
    let recovered = Core::open(&recovered_data, &fixture.source).unwrap();
    assert_current(&recovered.db, &recovered_rows);
    assert_v1_fixture_retained(&recovered.db, &fixture, true);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
    let original = Connection::open_with_flags(
        fixture.data.join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_original(&original);
}

#[test]
fn subscription_learning_future_v10_does_not_write_or_create_a_backup() {
    let fixture = v7_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    db.execute_batch("PRAGMA user_version=10; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let original_rows = prior_rows(&db);
    let original_schema = schema(&db);
    drop(db);
    let before = fs::read(fixture.data.join("ledger.sqlite3")).unwrap();
    let error = Core::open(&fixture.data, &fixture.source)
        .err()
        .expect("future schema must be rejected");
    assert!(error.contains("账本版本比应用更新"));
    assert_eq!(
        fs::read(fixture.data.join("ledger.sqlite3")).unwrap(),
        before
    );
    assert!(backups(&fixture.data).is_empty());
    let db = Connection::open_with_flags(
        fixture.data.join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(version(&db), 10);
    assert_eq!(prior_rows(&db), original_rows);
    assert_eq!(schema(&db), original_schema);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn subscription_learning_schema_enforces_revision_identity_and_valid_payloads() {
    let r = Rig::new();
    assert_eq!(version(&r.core.db), 9);
    assert!(backups(&r.data).is_empty());
    let insert = |revision, data: &str| {
        r.core.db.execute(
        "INSERT INTO quota_calibrations(calibration_id,revision,source_id,updated_at,data) VALUES('test-calibration',?1,?2,'2026-10-22T00:00:00.000Z',?3)",
        params![revision,r.core.source_id,data],
    )
    };
    insert(1, "{\"quality\":\"conditional\"}").unwrap();
    insert(2, "{\"quality\":\"invalidated\"}").unwrap();
    assert!(
        insert(1, "{}").is_err(),
        "same calibration revision must not overwrite history"
    );
    assert!(insert(0, "{}").is_err(), "revisions must be positive");
    assert!(
        insert(3, "not-json").is_err(),
        "invalid derivation payloads must be rejected"
    );
    let values: Vec<(i64,String)> = r.core.db.prepare("SELECT revision,data FROM quota_calibrations WHERE calibration_id='test-calibration' ORDER BY revision").unwrap()
        .query_map([],|row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(
        values,
        vec![
            (1, "{\"quality\":\"conditional\"}".into()),
            (2, "{\"quality\":\"invalidated\"}".into())
        ]
    );
}
