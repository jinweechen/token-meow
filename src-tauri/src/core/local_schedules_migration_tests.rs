use super::*;

type StoredRows = Vec<(String, Vec<Vec<rusqlite::types::Value>>)>;
const NEW_TABLES: [&str; 3] = [
    "local_schedule_templates",
    "local_schedule_instances",
    "local_manual_consumption",
];

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
    let names:Vec<String>=db.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND (name NOT LIKE 'sqlite_%' OR name='sqlite_sequence') AND name NOT IN ('local_schedule_templates','local_schedule_instances','local_manual_consumption') ORDER BY name").unwrap()
        .query_map([],|row|row.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            // 唯一扩展的旧表显式比较全部原列，不能用 SELECT * 把新增代次混入原值比较。
            let columns = if name == "source_instances" {
                "id,root,created_at,client_kind,enabled,environment"
            } else {
                "*"
            };
            let mut query = db
                .prepare(&format!("SELECT {columns} FROM {name} ORDER BY rowid"))
                .unwrap();
            let count = query.column_count();
            let values = query
                .query_map([], |row| {
                    (0..count)
                        .map(|index| row.get(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            (name, values)
        })
        .collect()
}

fn v8_fixture() -> V1MigrationFixture {
    let fixture = v1_migration_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    // 依次执行真实 001..008；不能将已经含 v9 列/表的当前库改 PRAGMA 冒充 v8。
    for sql in [
        include_str!("../../migrations/002_multi_source.sql"),
        include_str!("../../migrations/003_fact_references.sql"),
        include_str!("../../migrations/004_usage_lookup_index.sql"),
        include_str!("../../migrations/005_pet_read_models.sql"),
        include_str!("../../migrations/006_retention_time_indexes.sql"),
        include_str!("../../migrations/007_subscription_plans.sql"),
        include_str!("../../migrations/008_subscription_learning.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    db.execute(
        "UPDATE source_files SET source_id='v1-stable-source',channel_id='v1-stable-source:jsonl'",
        [],
    )
    .unwrap();
    db.execute("INSERT INTO source_instances(id,root,created_at,client_kind,enabled,environment) VALUES('v8-secondary','synthetic-secondary-root','2026-10-20T00:00:00Z','claude',0,'synthetic-environment')",[]).unwrap();
    register_channel(&db, "v1-stable-source").unwrap();
    register_channel(&db, "v8-secondary").unwrap();
    put_setting(&db, "subscriptionRevision", &json!(17)).unwrap();
    put_setting(
        &db,
        "v8-migration-marker",
        &json!({"nativeIdentity":"unknown","preserve":true}),
    )
    .unwrap();
    for revision in [1, 2] {
        db.execute("INSERT INTO usage_plans(plan_id,revision,source_id,data) VALUES('v8-plan',?1,'v1-stable-source',?2)",
            params![revision,json!({"id":"v8-plan","revision":revision,"manual":true,"target":100_000-revision*1000}).to_string()]).unwrap();
        db.execute("INSERT INTO quota_calibrations(calibration_id,revision,source_id,updated_at,data) VALUES('v8-calibration',?1,'v1-stable-source','2026-10-22T00:00:00.000Z',?2)",
            params![revision,json!({"archiveId":"v8-calibration","revision":revision,"quality":"invalidated","capacity":null,"preserveOldUnknownEvidence":true}).to_string()]).unwrap();
    }
    db.execute("INSERT INTO plan_period_state(owner_kind,owner_id,period_instance_id,data) VALUES('plan','v8-plan','v8-plan','{\"used\":17,\"latched\":[80]}')",[]).unwrap();
    db.execute("INSERT INTO account_snapshots(collected_at,data) VALUES('2026-10-22T00:00:00.000Z',?1)",
        [json!({"documentKind":"subscription_observation","schemaVersion":1,"sourceId":"v1-stable-source","runtimeId":"old-v8-runtime","bindingGeneration":"1","identityEpoch":"2","observationSequence":"3","clearGeneration":null,"outcome":"failure","identityConsistency":"unproven"}).to_string()]).unwrap();
    assert_eq!(version(&db), 8);
    assert_eq!(db.query_row("SELECT COUNT(*) FROM pragma_table_info('source_instances') WHERE name='configuration_revision'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
    fixture
}

fn assert_v9(db: &Connection, fixture: &V1MigrationFixture, before: &StoredRows) {
    assert_eq!(version(db), 9);
    assert_eq!(
        &prior_rows(db),
        before,
        "v8 原事实、完整游标、计划/校准历史、锁存及未知身份 JSON 必须逐 SQL 值保留"
    );
    assert_v1_fixture_retained(db, fixture, true);
    for table in NEW_TABLES {
        assert_eq!(
            db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "不得把旧计划推断为新日程或消费：{table}"
        );
    }
    let column:(String,i64,Option<String>)=db.query_row(
        "SELECT type,\"notnull\",dflt_value FROM pragma_table_info('source_instances') WHERE name='configuration_revision'",[],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(column, ("INTEGER".into(), 1, Some("0".into())));
    let revisions: Vec<(String, i64)> = db
        .prepare("SELECT id,configuration_revision FROM source_instances ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        revisions,
        vec![("v1-stable-source".into(), 0), ("v8-secondary".into(), 0)]
    );
}

#[test]
fn local_schedules_v8_to_v9_preserves_wal_values_backups_and_reopens_idempotently() {
    let fixture = v8_fixture();
    let writer = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    put_setting(&writer, "v8-committed-wal", &json!({"preserved":true})).unwrap();
    assert!(
        fs::metadata(fixture.data.join("ledger.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = prior_rows(&writer);
    let old_schema = schema(&writer);
    let core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_v9(&core.db, &fixture, &before);
    let copies = backups(&fixture.data);
    assert_eq!(copies.len(), 1);
    let backup =
        Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&backup), 8);
    assert_eq!(schema(&backup), old_schema);
    assert_eq!(prior_rows(&backup), before);
    assert_v1_fixture_retained(&backup, &fixture, true);
    drop(backup);
    drop(core);
    drop(writer);
    let reopened = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_v9(&reopened.db, &fixture, &before);
    assert_eq!(backups(&fixture.data).len(), 1);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

fn final_v9_index() -> String {
    let db = Connection::open_in_memory().unwrap();
    for sql in [
        include_str!("../../migrations/001_initial.sql"),
        include_str!("../../migrations/002_multi_source.sql"),
        include_str!("../../migrations/003_fact_references.sql"),
        include_str!("../../migrations/004_usage_lookup_index.sql"),
        include_str!("../../migrations/005_pet_read_models.sql"),
        include_str!("../../migrations/006_retention_time_indexes.sql"),
        include_str!("../../migrations/007_subscription_plans.sql"),
        include_str!("../../migrations/008_subscription_learning.sql"),
        include_str!("../../migrations/009_local_schedules.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    let index:String=db.query_row("SELECT name FROM sqlite_schema WHERE type='index' AND sql IS NOT NULL AND tbl_name IN ('local_schedule_templates','local_schedule_instances','local_manual_consumption') ORDER BY rowid DESC LIMIT 1",[],|row|row.get(0)).unwrap();
    assert!(
        index
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
        "测试索引名必须来自固定迁移 DDL"
    );
    index
}

#[test]
fn local_schedules_v9_final_index_failure_rolls_back_column_tables_and_all_ddl() {
    let fixture = v8_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    let collision = final_v9_index();
    db.execute_batch(&format!("CREATE TABLE {collision}(marker TEXT); INSERT INTO {collision} VALUES('controlled-v9-last-index-collision');")).unwrap();
    let old_schema = schema(&db);
    let before = prior_rows(&db);
    drop(db);
    let error = Core::open(&fixture.data, &fixture.source)
        .err()
        .expect("v9 最后索引应失败");
    assert!(error.contains(&collision), "{error}");
    let assert_original = |db: &Connection| {
        assert_eq!(version(db), 8);
        assert_eq!(schema(db), old_schema);
        assert_eq!(prior_rows(db), before);
        assert_eq!(db.query_row("SELECT COUNT(*) FROM pragma_table_info('source_instances') WHERE name='configuration_revision'",[],|row|row.get::<_,i64>(0)).unwrap(),0);
        for table in NEW_TABLES {
            assert_eq!(
                db.query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
        }
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
    let recovered_data = fixture._tmp.path().join("recovered-local-schedules");
    fs::create_dir(&recovered_data).unwrap();
    fs::copy(&copies[0], recovered_data.join("ledger.sqlite3")).unwrap();
    let recovery = Connection::open(recovered_data.join("ledger.sqlite3")).unwrap();
    recovery
        .execute_batch(&format!("DROP TABLE {collision};"))
        .unwrap();
    let recovered_before = prior_rows(&recovery);
    drop(recovery);
    let recovered = Core::open(&recovered_data, &fixture.source).unwrap();
    assert_v9(&recovered.db, &fixture, &recovered_before);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn local_schedules_future_v10_rejects_before_backup_or_business_writes() {
    let fixture = v8_fixture();
    let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    db.execute_batch("PRAGMA user_version=10; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let old_schema = schema(&db);
    let before = prior_rows(&db);
    drop(db);
    let bytes = fs::read(fixture.data.join("ledger.sqlite3")).unwrap();
    let error = Core::open(&fixture.data, &fixture.source)
        .err()
        .expect("未来 v10 不能降级写入");
    assert!(error.contains("账本版本比应用更新"));
    assert_eq!(
        fs::read(fixture.data.join("ledger.sqlite3")).unwrap(),
        bytes
    );
    assert!(backups(&fixture.data).is_empty());
    let original = Connection::open_with_flags(
        fixture.data.join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(version(&original), 10);
    assert_eq!(schema(&original), old_schema);
    assert_eq!(prior_rows(&original), before);
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn local_schedules_new_sources_default_to_zero_configuration_revision() {
    let fixture = v8_fixture();
    let core = Core::open(&fixture.data, &fixture.source).unwrap();
    core.db.execute("INSERT INTO source_instances(id,root,created_at) VALUES('post-v9-source','synthetic-post-v9-root','2026-10-23T00:00:00Z')",[]).unwrap();
    assert_eq!(
        core.db
            .query_row(
                "SELECT configuration_revision FROM source_instances WHERE id='post-v9-source'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        core.db
            .query_row("SELECT COUNT(*) FROM quota_calibrations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    for table in NEW_TABLES {
        assert_eq!(
            core.db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
