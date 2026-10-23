//! 使用真实 Gemini JSON 文件验证快照准入与持久进度，不运行厂商 CLI。
use super::*;
use crate::budget_clock::{test_support::TestClock, Reading};

struct Rig {
    _temp: tempfile::TempDir,
    core: Core,
    source: String,
    root: PathBuf,
    path: PathBuf,
    at: Reading,
    clock: TestClock,
}
impl Rig {
    fn new() -> Self {
        let at = Reading {
            utc: chrono::DateTime::parse_from_rfc3339("2026-10-23T04:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            mono_ms: 100_000,
        };
        let clock = TestClock::new(at);
        let temp = tempfile::tempdir().unwrap();
        let codex = temp.path().join("codex");
        fs::create_dir(&codex).unwrap();
        let mut core = Core::open(&temp.path().join("data"), &codex).unwrap();
        put_setting(&core.db, "importSince", &json!("1970-01-01T00:00:00Z")).unwrap();
        let root = temp.path().join("gemini");
        let folder = root.join("tmp").join("project-a").join("chats");
        fs::create_dir_all(&folder).unwrap();
        let source = core.add_source_with_kind(&root, "gemini").unwrap();
        Self {
            _temp: temp,
            core,
            source,
            root,
            path: folder.join("session-a.json"),
            at,
            clock,
        }
    }
    fn advance(&mut self, seconds: i64) {
        self.at.utc += Duration::seconds(seconds);
        self.at.mono_ms += seconds.unsigned_abs() * 1000;
        self.clock.set(self.at);
    }
    fn write(&self, messages: &[(&str, i64)]) {
        let messages: Vec<Value> = messages.iter().map(|(id, age_seconds)| {
            json!({"type":"gemini","id":id,"timestamp":(self.at.utc-Duration::seconds(*age_seconds)).to_rfc3339(),
                "model":"synthetic-gemini","tokens":{"input":100,"output":20,"cached":40,"thoughts":5,"tool":10,"total":135},
                "content":"PRIVATE_BODY_NOT_PERSISTED"})
        }).collect();
        fs::write(
            &self.path,
            json!({"sessionId":"native-snapshot-session","projectHash":"a".repeat(64),
            "kind":"main","messages":messages})
            .to_string(),
        )
        .unwrap();
    }
    fn scan(&mut self) -> ScanReport {
        self.core.scan().unwrap()
    }
    fn saved(&self) -> (i64, SnapshotProgress) {
        let (generation, context): (i64, String) = self
            .core
            .db
            .query_row(
                "SELECT generation,context FROM source_files WHERE source_id=?1 AND path=?2",
                params![self.source, self.path.to_string_lossy()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let context: Context = serde_json::from_str(&context).unwrap();
        (
            generation,
            serde_json::from_value(context.legacy[SNAPSHOT_CONTEXT_KEY].clone()).unwrap(),
        )
    }
    fn count(&self, table: &str) -> i64 {
        self.core
            .db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
    fn consume(
        &self,
        adapter: &dyn SourceAdapter,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> ScanReport {
        let mut file = File::open(&self.path).unwrap();
        let length = file.metadata().unwrap().len();
        let mut budget = 16 * 1024 * 1024;
        let mut report = ScanReport::default();
        let mut issues = 0;
        let tx = self.core.db.unchecked_transaction().unwrap();
        consume_snapshot(
            &tx,
            adapter,
            &self.source,
            &self.path.to_string_lossy(),
            &mut file,
            length,
            true,
            cutoff,
            &self.core.runtime_start,
            &AtomicBool::new(false),
            &std::time::Instant::now(),
            &mut budget,
            &mut report,
            &mut issues,
        )
        .unwrap();
        tx.commit().unwrap();
        report
    }
}

#[test]
fn static_snapshot_keeps_generation_and_ingestion_rows_as_scan_retention_advances() {
    let mut r = Rig::new();
    r.write(&[("old-message", 2 * 86400), ("recent-message", 3600)]);
    assert_eq!(r.scan().new_responses, 2);
    let (generation, progress) = r.saved();
    let records = r.count("ingested_records");
    assert_eq!(progress.next_record, 3);
    // 第一轮 scan 已经建立大于 importSince 的留存下界，随后它继续随时钟前进。
    for _ in 0..4 {
        r.advance(60);
        let report = r.scan();
        assert_eq!(report.new_responses, 0);
        assert_eq!(report.lines, 0, "完整静态快照不能因留存时间前进而重播");
        assert_eq!(r.saved().0, generation);
        assert_eq!(r.count("ingested_records"), records);
        assert_eq!(r.count("usage_responses"), 2);
    }
}

/// 实际 Gemini 解析/写入保持不变，只在首个计量写入后耗尽本批时间。
/// 这样真实读取器自然提交未完成游标，不伪造或回写进度来制造测试前提。
struct YieldAfterPrefix;
impl SourceAdapter for YieldAfterPrefix {
    fn client_kind(&self) -> &'static str {
        "gemini"
    }
    fn capabilities(&self) -> Value {
        adapter::adapter_for("gemini").unwrap().capabilities()
    }
    fn normalize(&self, observation: &adapter::Observation) -> Result<Vec<adapter::StandardEvent>> {
        adapter::adapter_for("gemini")
            .unwrap()
            .normalize(observation)
    }
    fn snapshot_records(&self, reader: &mut dyn Read) -> Result<Vec<Value>> {
        adapter::adapter_for("gemini")
            .unwrap()
            .snapshot_records(reader)
    }
    fn advance_local_context(&self, observation: &adapter::Observation, state: &mut Context) {
        adapter::adapter_for("gemini")
            .unwrap()
            .advance_local_context(observation, state);
    }
    fn ingest_local(
        &self,
        db: &Connection,
        observation: &adapter::Observation,
        file: &str,
        state: &mut Context,
        cutoff: Option<&chrono::DateTime<Utc>>,
    ) -> Result<usize> {
        let result = adapter::adapter_for("gemini").unwrap().ingest_local(
            db,
            observation,
            file,
            state,
            cutoff,
        )?;
        if observation.event["id"] == "prefix" {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        Ok(result)
    }
    fn fact_reference_with_context(
        &self,
        observation: &adapter::Observation,
        state: &Context,
    ) -> Option<String> {
        adapter::adapter_for("gemini")
            .unwrap()
            .fact_reference_with_context(observation, state)
    }
}

#[test]
fn tighter_retention_continues_unfinished_tail_without_replaying_prefix_or_losing_new_rows() {
    let mut r = Rig::new();
    r.write(&[
        ("prefix", 2 * 86400),
        ("expired-tail", 2 * 86400),
        ("retained-tail", 3600),
    ]);
    let old_cutoff = r.at.utc - Duration::days(3);
    let report = r.consume(&YieldAfterPrefix, Some(&old_cutoff));
    assert_eq!(report.new_responses, 1);
    assert_eq!(report.pending_files, 1);
    let (generation, progress) = r.saved();
    assert_eq!(progress.next_record, 2);
    r.core
        .action("update_settings", &json!({"retentionDays":1}))
        .unwrap();
    r.core.action("apply_retention", &json!({})).unwrap();
    assert_eq!(r.count("usage_responses"), 0);
    let current_cutoff = r.at.utc - Duration::days(1);
    let report = r.consume(
        adapter::adapter_for("gemini").unwrap(),
        Some(&current_cutoff),
    );
    assert_eq!(report.new_responses, 1);
    assert_eq!(report.lines, 2, "只继续尚未处理的两个尾部记录");
    assert_eq!(report.pending_files, 0);
    assert_eq!(r.saved().0, generation);
    assert_eq!(r.saved().1.next_record, 4);
    assert_eq!(r.count("usage_responses"), 1);
    assert_eq!(
        r.count("ingested_records"),
        3,
        "不新增已处理的 header/prefix，仅新增有效尾部"
    );
    assert_eq!(
        r.consume(
            adapter::adapter_for("gemini").unwrap(),
            Some(&current_cutoff)
        )
        .lines,
        0
    );
}

#[test]
fn wider_cutoff_replays_previously_skipped_records_but_subsequent_tightening_is_stable() {
    let r = Rig::new();
    r.write(&[("historical", 2 * 86400), ("recent", 3600)]);
    let narrow = r.at.utc - Duration::days(1);
    let wide = r.at.utc - Duration::days(3);
    assert_eq!(
        r.consume(adapter::adapter_for("gemini").unwrap(), Some(&narrow))
            .new_responses,
        1
    );
    let first = r.saved().0;
    assert_eq!(
        r.consume(adapter::adapter_for("gemini").unwrap(), Some(&wide))
            .new_responses,
        1
    );
    let wider = r.saved().0;
    assert!(wider > first);
    let records = r.count("ingested_records");
    assert_eq!(
        r.consume(adapter::adapter_for("gemini").unwrap(), Some(&narrow))
            .lines,
        0
    );
    assert_eq!(r.saved().0, wider);
    assert_eq!(r.count("ingested_records"), records);
}

#[test]
fn explicit_history_clear_privacy_and_source_revision_remain_admission_fences() {
    let mut r = Rig::new();
    r.write(&[("history", 2 * 86400)]);
    assert_eq!(r.scan().new_responses, 1);
    let before = r.saved().0;
    r.core
        .action(
            "update_source",
            &json!({"id":r.source,"path":r.root,"enabled":true}),
        )
        .unwrap();
    r.scan();
    assert!(r.saved().0 > before);
    let before = r.saved().0;
    r.core
        .action("update_settings", &json!({"privacyMode":true}))
        .unwrap();
    r.scan();
    assert!(r.saved().0 > before);
    r.core
        .action("update_settings", &json!({"privacyMode":false}))
        .unwrap();
    r.core.action("clear_data", &json!({})).unwrap();
    r.scan();
    assert_eq!(r.count("usage_responses"), 0, "清除下界不能被静态快照绕过");
    r.core
        .action(
            "import_history",
            &json!({"from":(r.at.utc-Duration::days(3)).to_rfc3339()}),
        )
        .unwrap();
    assert_eq!(r.scan().new_responses, 1);
    let before = r.saved().0;
    let records = r.count("ingested_records");
    r.advance(60);
    assert_eq!(r.scan().new_responses, 0);
    assert_eq!(r.saved().0, before);
    assert_eq!(r.count("ingested_records"), records);
}

#[test]
fn legacy_snapshot_progress_upgrades_once_and_then_survives_retention_clock_changes() {
    let mut r = Rig::new();
    r.write(&[("legacy-message", 3600)]);
    r.scan();
    let (old_generation, progress) = r.saved();
    let legacy = json!({"fingerprint":progress.fingerprint,"admissionKey":"legacy-cutoff-bound-hash",
        "nextRecord":progress.next_record,"historyGap":progress.history_gap});
    let raw: String = r
        .core
        .db
        .query_row(
            "SELECT context FROM source_files WHERE source_id=?1",
            [&r.source],
            |row| row.get(0),
        )
        .unwrap();
    let mut context: Context = serde_json::from_str(&raw).unwrap();
    context.legacy.insert(SNAPSHOT_CONTEXT_KEY.into(), legacy);
    r.core
        .db
        .execute(
            "UPDATE source_files SET context=?1 WHERE source_id=?2",
            params![serde_json::to_string(&context).unwrap(), r.source],
        )
        .unwrap();
    assert_eq!(r.scan().new_responses, 0);
    let upgraded = r.saved().0;
    assert_eq!(upgraded, old_generation + 1);
    let records = r.count("ingested_records");
    for _ in 0..2 {
        r.advance(60);
        assert_eq!(r.scan().lines, 0);
        assert_eq!(r.saved().0, upgraded);
        assert_eq!(r.count("ingested_records"), records);
    }
    assert_eq!(r.count("usage_responses"), 1);
}
