use super::*;

const ADDED_INDEXES: [&str; 5] = [
    "legacy_usage_event_time",
    "usage_aggregate_event_time",
    "context_snapshot_event_time",
    "timing_observation_event_time",
    "fact_observation_event_time",
];
const RETAINED_TABLES: [&str; 9] = [
    "usage_responses",
    "legacy_usage_segments",
    "turns",
    "items",
    "inbox_events",
    "usage_aggregates",
    "context_snapshots",
    "timing_observations",
    "fact_observations",
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
    let mut statement = db
        .prepare(
            "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap();
    rows.collect::<std::result::Result<_, _>>().unwrap()
}

fn rows(db: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    // Only fixed test-owned table names are passed here. Preserve SQL values,
    // rather than normalizing fact JSON or cursor/settings strings.
    // v9 为来源新增修订列；保留旧六列逐值比较，新列在 assert_current_schema 中单独断言。
    let columns = if table == "source_instances" {
        "id,root,created_at,client_kind,enabled,environment"
    } else {
        "*"
    };
    let mut statement = db
        .prepare(&format!("SELECT {columns} FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            (0..columns)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
        })
        .unwrap();
    rows.collect::<std::result::Result<_, _>>().unwrap()
}

fn assert_current_schema(db: &Connection) {
    assert_eq!(version(db), 9);
    let column: (String, i64, String) = db
        .query_row(
            "SELECT type,\"notnull\",dflt_value FROM pragma_table_info('source_instances') WHERE name='configuration_revision'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(column, ("INTEGER".into(), 1, "0".into()));
    assert_eq!(
        db.query_row("SELECT count(*) FROM source_instances WHERE configuration_revision<>0", [], |row| row.get::<_, i64>(0)).unwrap(),
        0
    );
    for table in [
        "local_schedule_templates",
        "local_schedule_instances",
        "local_manual_consumption",
    ] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get::<_, i64>(0)).unwrap(),
            0,
            "旧账本迁移不能制造周期或手动记账事实"
        );
    }
}

fn business_rows(db: &Connection) -> Vec<(&'static str, Vec<Vec<rusqlite::types::Value>>)> {
    [
        "source_instances",
        "source_files",
        "ingested_records",
        "usage_responses",
        "legacy_usage_segments",
        "turns",
        "items",
        "thread_metadata",
        "identity_conflicts",
        "ingestion_issues",
        "budgets",
        "inbox_events",
        "settings",
        "account_snapshots",
        "account_usage_snapshots",
        "usage_aggregates",
        "context_snapshots",
        "timing_observations",
        "account_bindings",
        "fact_observations",
        "fact_aliases",
        "excluded_facts",
    ]
    .into_iter()
    .map(|table| (table, rows(db, table)))
    .collect()
}

fn advance_to(db: &Connection, target: i64) {
    db.execute_batch(include_str!("../../migrations/002_multi_source.sql"))
        .unwrap();
    db.execute(
        "UPDATE source_files SET source_id='v1-stable-source',channel_id='v1-stable-source:jsonl'",
        [],
    )
    .unwrap();
    db.execute_batch(include_str!("../../migrations/003_fact_references.sql"))
        .unwrap();
    db.execute_batch(include_str!("../../migrations/004_usage_lookup_index.sql"))
        .unwrap();
    if target >= 5 {
        db.execute_batch(include_str!("../../migrations/005_pet_read_models.sql"))
            .unwrap();
    }
    assert_eq!(version(db), target);
}

fn seed_preservation_rows(db: &Connection) {
    // Physical migration sentinels, not metering evidence. Native facts and
    // real cursor/context data are supplied by the existing v1 fixture.
    let at = "2026-10-15T00:00:00.000Z";
    db.execute("INSERT INTO legacy_usage_segments(id,source_id,thread_id,turn_id,event_time,data) VALUES('retained-legacy','v1-stable-source',?1,'v1-turn',?2,'{}')", params![THREAD, at]).unwrap();
    for table in [
        "usage_aggregates",
        "context_snapshots",
        "timing_observations",
    ] {
        db.execute(&format!("INSERT INTO {table}(id,source_id,event_time,data) VALUES('retained-row','v1-stable-source',?1,'{{}}')"), [at]).unwrap();
    }
    db.execute("INSERT INTO fact_observations(id,fact_id,source_id,channel_id,event_time,digest) VALUES('retained-observation','retained-fact','v1-stable-source','v1-stable-source:jsonl',?1,'retained-digest')", [at]).unwrap();
    db.execute("INSERT INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES('retained-inbox',?1,'v1-turn',?2,?3)", params![THREAD, at, json!({"id":"retained-inbox","read":true,"notified":true,"animationAcknowledged":true}).to_string()]).unwrap();
    for (key, value) in [
        ("retentionWatermark", json!("2026-01-01T00:00:00.000Z")),
        ("historyPaused", json!(true)),
        ("historyCancelled", json!(true)),
        ("clearWatermark", json!("2026-01-02T00:00:00.000Z")),
    ] {
        put_setting(db, key, &value).unwrap();
    }
}

#[test]
fn retention_v6_upgrade_preserves_v5_wal_facts_cursors_settings_and_reopen_is_idempotent() {
    let fixture = v1_migration_fixture();
    let writer = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
    writer
        .execute_batch("PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    advance_to(&writer, 5);
    seed_preservation_rows(&writer);
    assert!(
        fs::metadata(fixture.data.join("ledger.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    let before = business_rows(&writer);
    let feed = rows(&writer, "pet_inbox_feed");
    let sequence = rows(&writer, "sqlite_sequence");
    let core = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_current_schema(&core.db);
    assert_eq!(business_rows(&core.db), before);
    assert_eq!(rows(&core.db, "pet_inbox_feed"), feed);
    assert_eq!(rows(&core.db, "sqlite_sequence"), sequence);
    assert_v1_fixture_retained(&core.db, &fixture, true);
    let copies = backups(&fixture.data);
    assert_eq!(copies.len(), 1);
    let backup =
        Connection::open_with_flags(&copies[0], rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(version(&backup), 5);
    assert_eq!(business_rows(&backup), before);
    assert_eq!(rows(&backup, "pet_inbox_feed"), feed);
    assert_eq!(rows(&backup, "sqlite_sequence"), sequence);
    assert_v1_fixture_retained(&backup, &fixture, true);
    drop(backup);
    drop(core);
    drop(writer);
    let mut reopened = Core::open(&fixture.data, &fixture.source).unwrap();
    assert_current_schema(&reopened.db);
    assert_eq!(business_rows(&reopened.db), before);
    assert_eq!(rows(&reopened.db, "pet_inbox_feed"), feed);
    assert_eq!(rows(&reopened.db, "sqlite_sequence"), sequence);
    assert_eq!(backups(&fixture.data).len(), 1);
    reopened.shutdown().unwrap();
    assert_eq!(fs::read(&fixture.log).unwrap(), fixture.log_bytes);
}

#[test]
fn retention_v6_last_ddl_failure_rolls_back_all_pending_generations_and_backup_recovers() {
    for original_version in [4, 5] {
        let fixture = v1_migration_fixture();
        let db = Connection::open(fixture.data.join("ledger.sqlite3")).unwrap();
        advance_to(&db, original_version);
        seed_preservation_rows(&db);
        db.execute_batch("CREATE TABLE fact_observation_event_time(marker TEXT); INSERT INTO fact_observation_event_time VALUES('controlled-final-ddl-collision');").unwrap();
        let original_schema = schema(&db);
        let original_rows = business_rows(&db);
        let original_feed = (original_version == 5).then(|| rows(&db, "pet_inbox_feed"));
        drop(db);
        let error = Core::open(&fixture.data, &fixture.source)
            .err()
            .expect("last DDL must fail");
        assert!(
            error.contains("fact_observation_event_time"),
            "unexpected error: {error}"
        );
        let assert_original = |db: &Connection| {
            assert_eq!(version(db), original_version);
            assert_eq!(schema(db), original_schema);
            assert_eq!(business_rows(db), original_rows);
            assert_v1_fixture_retained(db, &fixture, true);
            if let Some(feed) = &original_feed {
                assert_eq!(&rows(db, "pet_inbox_feed"), feed);
            }
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
        let recovery_data = fixture._tmp.path().join("recovered-v6-fixture");
        fs::create_dir(&recovery_data).unwrap();
        fs::copy(&copies[0], recovery_data.join("ledger.sqlite3")).unwrap();
        let recovery_db = Connection::open(recovery_data.join("ledger.sqlite3")).unwrap();
        recovery_db
            .execute_batch("DROP TABLE fact_observation_event_time;")
            .unwrap();
        drop(recovery_db);
        let recovered = Core::open(&recovery_data, &fixture.source).unwrap();
        assert_current_schema(&recovered.db);
        assert_eq!(business_rows(&recovered.db), original_rows);
        assert_v1_fixture_retained(&recovered.db, &fixture, true);
        if let Some(feed) = &original_feed {
            assert_eq!(&rows(&recovered.db, "pet_inbox_feed"), feed);
        } else {
            assert_eq!(rows(&recovered.db, "pet_inbox_feed").len(), 1);
        }
        drop(recovered);
        let original = Connection::open_with_flags(
            fixture.data.join("ledger.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_original(&original);
    }
}

fn seed_retention_row(db: &Connection, table: &str, id: &str, at: &str) {
    let sql = match table {
        "usage_responses" => "INSERT INTO usage_responses(id,source_id,thread_id,response_id,turn_id,event_time,digest,data) VALUES(?1,'retention-source','thread',?1,'turn',?2,'digest','{}')".to_owned(),
        "legacy_usage_segments" => "INSERT INTO legacy_usage_segments(id,source_id,thread_id,turn_id,event_time,data) VALUES(?1,'retention-source','thread','turn',?2,'{}')".to_owned(),
        "turns" | "items" => format!("INSERT INTO {table}(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread','turn',?2,'{{}}')"),
        "inbox_events" => "INSERT INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,'thread','turn',?2,'{}')".to_owned(),
        "usage_aggregates" | "context_snapshots" | "timing_observations" => format!("INSERT INTO {table}(id,source_id,event_time,data) VALUES(?1,'retention-source',?2,'{{}}')"),
        "fact_observations" => "INSERT INTO fact_observations(id,fact_id,source_id,channel_id,event_time,digest) VALUES(?1,?1,'retention-source','retention-source:jsonl',?2,'digest')".to_owned(),
        _ => panic!("unrecognized retention fixture table"),
    };
    db.execute(&sql, params![id, at]).unwrap();
}

fn delete_plan(db: &Connection, table: &str, cutoff: &str) -> Vec<String> {
    // This is the exact production DELETE predicate, with no INDEXED BY hint.
    let mut statement = db
        .prepare(&format!(
            "EXPLAIN QUERY PLAN DELETE FROM {table} WHERE event_time<?1"
        ))
        .unwrap();
    let plans = statement
        .query_map([cutoff], |row| row.get::<_, String>(3))
        .unwrap();
    plans.collect::<std::result::Result<_, _>>().unwrap()
}

#[test]
fn retention_delete_plans_use_time_searches_and_preserve_strict_cutoff_and_inbox_cascade() {
    let mut previous = Rig::new();
    let mut indexed = Rig::new();
    for index in ADDED_INDEXES {
        previous
            .core
            .db
            .execute_batch(&format!("DROP INDEX {index};"))
            .unwrap();
    }
    // 此处隔离时间索引对 DELETE 的影响；两侧均使用真实当前 schema，
    // 不把残留后续列/表的数据库改写 user_version 冒充旧版迁移夹具。
    assert_current_schema(&previous.core.db);
    assert_current_schema(&indexed.core.db);
    // A future protected watermark makes the cutoff independent of test wall
    // time. Existing monotonic-watermark code, not an altered DELETE, chooses it.
    let cutoff = "2100-01-01T00:00:00.000Z";
    for r in [&mut previous, &mut indexed] {
        put_setting(&r.core.db, "retentionWatermark", &json!(cutoff)).unwrap();
        for table in RETAINED_TABLES {
            seed_retention_row(&r.core.db, table, "old", "2099-12-31T23:59:59.999Z");
            seed_retention_row(&r.core.db, table, "boundary", cutoff);
            seed_retention_row(&r.core.db, table, "new", "2100-01-01T00:00:00.001Z");
            for n in 0..128 {
                seed_retention_row(
                    &r.core.db,
                    table,
                    &format!("recent-{n:03}"),
                    "2100-01-02T00:00:00.000Z",
                );
            }
        }
    }
    for table in RETAINED_TABLES {
        let before_plan = delete_plan(&previous.core.db, table, cutoff);
        let after_plan = delete_plan(&indexed.core.db, table, cutoff);
        println!("retention DELETE {table}: without_time_indexes={before_plan:?}; with_time_indexes={after_plan:?}");
        if matches!(
            table,
            "legacy_usage_segments"
                | "usage_aggregates"
                | "context_snapshots"
                | "timing_observations"
                | "fact_observations"
        ) {
            assert!(
                before_plan
                    .iter()
                    .any(|line| line.starts_with(&format!("SCAN {table}"))),
                "expected prior full scan: {before_plan:?}"
            );
        }
        assert!(
            after_plan.iter().any(|line| {
                // SQLite may satisfy the same range predicate with a covering index.
                (line.starts_with(&format!("SEARCH {table} USING INDEX "))
                    || line.starts_with(&format!("SEARCH {table} USING COVERING INDEX ")))
                    && line.contains("event_time<?")
            }),
            "missing time-range search: {after_plan:?}"
        );
    }
    queries::apply_retention(&mut previous.core).unwrap();
    queries::apply_retention(&mut indexed.core).unwrap();
    for table in RETAINED_TABLES {
        let retained = rows(&indexed.core.db, table);
        assert_eq!(
            retained,
            rows(&previous.core.db, table),
            "DELETE semantics changed for {table}"
        );
        assert_eq!(retained.len(), 130);
        let ids: Vec<String> = indexed
            .core
            .db
            .prepare(&format!(
                "SELECT id FROM {table} WHERE id IN ('old','boundary','new') ORDER BY id"
            ))
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            ids,
            ["boundary", "new"],
            "strict retention edge changed for {table}"
        );
    }
    for r in [&previous, &indexed] {
        assert_eq!(
            get_setting(&r.core.db, "retentionWatermark").unwrap(),
            Some(json!(cutoff))
        );
        let feed: Vec<String> = r
            .core
            .db
            .prepare("SELECT inbox_id FROM pet_inbox_feed ORDER BY sequence")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(feed.len(), 130);
        assert!(!feed.iter().any(|id| id == "old"));
        assert!(feed.iter().any(|id| id == "boundary"));
    }
    assert_eq!(
        rows(&indexed.core.db, "pet_inbox_feed"),
        rows(&previous.core.db, "pet_inbox_feed")
    );
    // An unchanged watermark cannot skip cleaning old metadata that arrives
    // later. Exercise the same production cleanup again after late insertion.
    for r in [&mut previous, &mut indexed] {
        for table in RETAINED_TABLES {
            seed_retention_row(&r.core.db, table, "late-old", "2099-12-31T23:59:59.999Z");
        }
        queries::apply_retention(&mut r.core).unwrap();
        for table in RETAINED_TABLES {
            assert_eq!(
                rows(&r.core.db, table).len(),
                130,
                "late old row survived in {table}"
            );
        }
        assert_eq!(
            get_setting(&r.core.db, "retentionWatermark").unwrap(),
            Some(json!(cutoff))
        );
    }
    for table in RETAINED_TABLES {
        assert_eq!(
            rows(&indexed.core.db, table),
            rows(&previous.core.db, table)
        );
    }
    assert_eq!(
        rows(&indexed.core.db, "pet_inbox_feed"),
        rows(&previous.core.db, "pet_inbox_feed")
    );
}
