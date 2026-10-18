//! Real temporary JSONL -> Core.scan -> trusted budget state/claim regressions.
use super::*;
use crate::budget_clock::{test_support::TestClock, Reading};
use std::io::Write;

const THREAD: &str = "00000000-0000-4000-8000-000000000021";
const CHILD: &str = "00000000-0000-4000-8000-000000000022";
const START: &str = "2026-01-01T23:59:54+08:00";

fn utc(text: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

struct Fixture {
    // Drop the database before its temporary directory and restore the clock.
    core: Core,
    host: TestClock,
    host_epoch: u64,
    host_paused: bool,
    temporary: tempfile::TempDir,
    source: PathBuf,
    data: PathBuf,
    time: chrono::DateTime<Utc>,
}

impl Fixture {
    fn new() -> Self {
        let host = TestClock::new(Reading {
            utc: utc(START),
            mono_ms: 0,
        });
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let data = temporary.path().join("pet");
        fs::create_dir(&source).unwrap();
        let mut core = Core::open(&data, &source).unwrap();
        core.notification_gate().publish(true, false);
        // Real actions configure only this fixture, never a real Codex root.
        core.action("update_settings", &json!({"retentionDays":3650}))
            .unwrap();
        core.action("import_history", &json!({"from":"2025-12-01T00:00:00Z"}))
            .unwrap();
        let fixture = Self {
            core,
            host,
            host_epoch: 0,
            host_paused: false,
            temporary,
            source,
            data,
            time: utc(START),
        };
        fixture.meta(&fixture.source, THREAD, "D:/fixture/a", None);
        fixture
    }

    fn at(&mut self, text: &str, monotonic_ms: u64) {
        self.set_host(text, monotonic_ms, 0, false);
    }

    fn set_host(&mut self, text: &str, monotonic_ms: u64, epoch: u64, paused: bool) {
        self.time = utc(text);
        self.host.set(Reading {
            utc: self.time,
            mono_ms: monotonic_ms,
        });
        if epoch != self.host_epoch || paused != self.host_paused {
            self.core.notification_gate().publish(true, paused);
            self.host_epoch = epoch;
            self.host_paused = paused;
        }
    }

    fn append(&self, source: &Path, value: &Value) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(source.join("budget-fixture.jsonl"))
            .unwrap();
        writeln!(file, "{value}").unwrap();
    }

    fn event(&self, kind: &str, payload: Value) -> Value {
        json!({"timestamp":self.time.to_rfc3339(),"type":kind,"payload":payload})
    }

    fn meta(&self, source: &Path, thread: &str, cwd: &str, parent: Option<&str>) {
        self.append(
            source,
            &self.event(
                "session_meta",
                json!({"id":thread,"cwd":cwd,
            "cli_version":"0.147.0","parent_thread_id":parent}),
            ),
        );
    }

    fn usage(&self, thread: &str, response: &str, turn: &str, root: &str, total: i64) -> Value {
        self.event(
            "token_usage_record",
            json!({"thread_id":thread,"response_id":response,
            "turn_id":turn,"root_turn_id":root,"requested_model":"fixture-model",
            "usage":{"input_tokens":total,"output_tokens":0,"total_tokens":total,
                "cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0}}),
        )
    }

    fn daily(&mut self, threshold: i64) {
        self.rule("daily", "daily", None, None, threshold);
    }

    fn rule(
        &mut self,
        id: &str,
        scope: &str,
        source: Option<&str>,
        target: Option<&str>,
        threshold: i64,
    ) {
        self.core
            .action(
                "save_budget",
                &json!({"id":id,"scope":scope,"sourceId":source,
            "target":target,"threshold":threshold,"enabled":true}),
            )
            .unwrap();
    }

    fn observe(&mut self) {
        // Calls real reader::scan -> adapter::ingest -> evaluate_budgets.
        // Do not fill budgetState, r.live, baseline, eligibility or health here.
        self.core.scan().unwrap();
    }

    fn arm(&mut self) {
        self.at("2026-01-01T23:59:56+08:00", 2_000);
        self.observe();
        self.at("2026-01-01T23:59:58+08:00", 4_000);
        self.observe();
    }

    fn budget(&self, id: &str) -> Value {
        self.core.snapshot(json!({})).unwrap()["budgets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["id"] == id)
            .unwrap()
            .clone()
    }

    fn inbox(&self, id: &str) -> Value {
        self.core.snapshot(json!({})).unwrap()["inbox"]
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["budgetId"] == id)
            .unwrap()
            .clone()
    }

    fn history_only(&mut self, id: &str) {
        assert!(self.core.claim_notifications().unwrap().is_empty());
        let item = self.inbox(id);
        assert_eq!(item["notifyEligible"], false);
        assert_eq!(item["notificationState"], "history-only");
    }
}

#[test]
fn midnight_first_live_crossing_claims_once_and_duplicate_or_clear_does_not_refill() {
    let mut f = Fixture::new();
    f.daily(200);
    f.arm();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    let response = f.usage(THREAD, "first-live", "turn", "turn", 250);
    f.append(&f.source, &response);
    f.append(&f.source, &response);
    f.observe();
    assert_eq!(f.budget("daily")["usedTokens"], 250);
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["budgetId"], "daily");
    assert_eq!(claimed[0]["period"], "2026-01-02");
    assert!(f.core.claim_notifications().unwrap().is_empty());
    f.core.action("clear_inbox", &json!({})).unwrap();
    f.at("2026-01-02T00:00:02+08:00", 8_000);
    f.append(&f.source, &response);
    f.observe();
    assert!(f.core.claim_notifications().unwrap().is_empty());
    assert!(f.core.snapshot(json!({})).unwrap()["inbox"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn saving_or_editing_over_existing_same_day_usage_is_history_only() {
    for edit_existing in [false, true] {
        let mut f = Fixture::new();
        if edit_existing {
            f.daily(1_000);
        }
        f.arm();
        f.at("2026-01-02T00:00:01+08:00", 7_000);
        f.append(
            &f.source,
            &f.usage(THREAD, "already-recorded", "turn", "turn", 300),
        );
        f.observe();
        f.at("2026-01-02T00:00:02+08:00", 8_000);
        f.daily(200);
        f.observe();
        f.history_only("daily");
        assert_eq!(
            f.budget("daily")["version"],
            if edit_existing { 2 } else { 1 }
        );
    }
}

#[test]
fn last_moment_save_or_new_version_cannot_borrow_an_old_rollover_certificate() {
    for edit_existing in [false, true] {
        let mut f = Fixture::new();
        if edit_existing {
            f.daily(1_000);
        }
        f.arm();
        f.at("2026-01-01T23:59:59+08:00", 5_000);
        f.daily(200); // A save is a real same-day baseline, but cannot authorize rollover.
        f.at("2026-01-02T00:00:01+08:00", 7_000);
        f.append(
            &f.source,
            &f.usage(THREAD, "post-save", "turn", "turn", 300),
        );
        f.observe();
        f.history_only("daily");
    }
}

#[test]
fn restart_explicit_history_and_long_gap_do_not_turn_the_first_batch_into_live_alerts() {
    for mode in ["restart", "history", "gap"] {
        let mut f = Fixture::new();
        f.daily(200);
        f.arm();
        if mode == "gap" {
            f.at("2026-01-02T00:03:01+08:00", 187_000);
        } else {
            f.at("2026-01-02T00:00:02+08:00", 8_000);
        }
        if mode == "restart" {
            let old_runtime = f.core.runtime_id.clone();
            f.core.shutdown().unwrap();
            f.core = Core::open(&f.data, &f.source).unwrap();
            assert_ne!(f.core.runtime_id, old_runtime);
        }
        let mut response = f.usage(THREAD, mode, "turn", "turn", 300);
        if mode == "history" {
            response["timestamp"] = json!("2026-01-02T00:00:00+08:00");
            f.core
                .action("import_history", &json!({"from":"2026-01-02"}))
                .unwrap();
        }
        f.append(&f.source, &response);
        f.observe();
        f.history_only("daily");
        assert_eq!(f.budget("daily")["usedTokens"], 300);
    }
}

#[test]
fn a_measured_future_day_basis_is_not_replaced_by_zero() {
    let mut f = Fixture::new();
    f.daily(200);
    f.at("2026-01-01T23:59:56+08:00", 2_000);
    let mut early = f.usage(THREAD, "future-day-record", "turn", "turn", 250);
    // This is within the existing five-second source clock allowance.
    early["timestamp"] = json!("2026-01-02T00:00:01+08:00");
    f.append(&f.source, &early);
    f.observe();
    f.at("2026-01-01T23:59:58+08:00", 4_000);
    f.observe();
    f.at("2026-01-02T00:00:02+08:00", 8_000);
    f.append(&f.source, &f.usage(THREAD, "tiny-live", "turn", "turn", 1));
    f.observe();
    assert_eq!(f.budget("daily")["usedTokens"], 251);
    f.history_only("daily");
}

#[test]
fn a_pause_epoch_change_blocks_rollover_and_invalidates_an_unclaimed_budget_alert() {
    for pause_after_evaluation in [false, true] {
        let mut f = Fixture::new();
        f.daily(200);
        f.arm();
        if !pause_after_evaluation {
            // No scan occurs while blocked; comparing only current paused=false
            // or a short wall-clock gap would miss this real host transition.
            f.set_host("2026-01-02T00:00:00+08:00", 6_000, 1, true);
            f.set_host("2026-01-02T00:00:01+08:00", 7_000, 2, false);
        } else {
            f.at("2026-01-02T00:00:01+08:00", 7_000);
        }
        f.append(
            &f.source,
            &f.usage(THREAD, "pause-boundary", "turn", "turn", 250),
        );
        f.observe();
        if pause_after_evaluation {
            assert_eq!(f.inbox("daily")["notifyEligible"], true);
            f.set_host("2026-01-02T00:00:02+08:00", 8_000, 1, true);
            f.set_host("2026-01-02T00:00:03+08:00", 9_000, 2, false);
            assert!(f.core.claim_notifications().unwrap().is_empty());
            assert_eq!(f.inbox("daily")["notificationState"], "suppressed");
        } else {
            f.history_only("daily");
        }
    }
}

#[test]
fn source_identity_and_project_remapping_do_not_replay_other_usage() {
    let mut f = Fixture::new();
    let other_root = f.temporary.path().join("second-source");
    fs::create_dir(&other_root).unwrap();
    let other = f.core.add_source(&other_root).unwrap();
    f.meta(&other_root, THREAD, "D:/fixture/b", None);
    f.core
        .action(
            "set_project_mapping",
            &json!({"path":"D:/fixture/a","project":"Shared"}),
        )
        .unwrap();
    let first = f.core.source_id.clone();
    f.rule("thread-a", "thread", Some(&first), Some(THREAD), 200);
    f.rule("project", "project", None, Some("mapped:Shared"), 200);
    f.arm();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    f.append(
        &other_root,
        &f.usage(THREAD, "source-b-only", "b-turn", "b-turn", 300),
    );
    f.observe();
    assert_eq!(f.budget("thread-a")["usedTokens"], 0);
    assert_eq!(f.budget("project")["usedTokens"], 0);
    assert!(f.core.claim_notifications().unwrap().is_empty());
    f.at("2026-01-02T00:00:02+08:00", 8_000);
    f.core
        .action(
            "set_project_mapping",
            &json!({"path":"D:/fixture/b","project":"Shared"}),
        )
        .unwrap();
    f.observe();
    f.history_only("project"); // Reclassification is not new live consumption.
    f.at("2026-01-02T00:00:03+08:00", 9_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "source-a-live", "a-turn", "a-turn", 250),
    );
    f.observe();
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["budgetId"], "thread-a");
    assert_eq!(claimed[0]["sourceId"], first);
    assert_eq!(f.budget("project")["usedTokens"], 550);
    assert_eq!(f.budget("project")["target"], "mapped:Shared");
    assert_eq!(f.budget("project")["version"], 1);
    assert_ne!(other, first);
}

#[test]
fn root_turn_keeps_its_task_period_and_explicit_child_total_across_midnight() {
    let mut f = Fixture::new();
    f.append(
        &f.source,
        &f.event(
            "event_msg",
            json!({"type":"task_started","thread_id":THREAD,
        "turn_id":"root-task","root_turn_id":"root-task"}),
        ),
    );
    f.meta(&f.source, CHILD, "D:/fixture/a", Some(THREAD));
    f.append(
        &f.source,
        &f.usage(THREAD, "root-100", "root-task", "root-task", 100),
    );
    f.append(
        &f.source,
        &f.usage(CHILD, "child-80", "child-turn", "root-task", 80),
    );
    f.observe();
    let source = f.core.source_id.clone();
    f.rule("root", "rootTurn", Some(&source), Some("root-task"), 200);
    let period = f.budget("root")["period"].clone();
    assert_eq!(f.budget("root")["usedTokens"], 180);
    f.arm();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    f.append(
        &f.source,
        &f.usage(CHILD, "child-30", "child-turn", "root-task", 30),
    );
    f.observe();
    assert_eq!(f.budget("root")["usedTokens"], 210);
    assert_eq!(f.budget("root")["period"], period);
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["budgetId"], "root");
    assert_eq!(claimed[0]["period"], period);
    assert!(f.core.claim_notifications().unwrap().is_empty());
}

#[test]
fn same_day_saved_actual_baseline_allows_only_subsequent_fresh_crossing() {
    let mut f = Fixture::new();
    f.append(
        &f.source,
        &f.usage(THREAD, "pre-save-80", "turn", "turn", 80),
    );
    f.observe();
    f.at("2026-01-01T23:59:55+08:00", 1_000);
    f.daily(200);
    f.at("2026-01-01T23:59:56+08:00", 2_000);
    f.append(&f.source, &f.usage(THREAD, "new-130", "turn", "turn", 130));
    f.observe();
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["usedTokens"], 210);
    let permit = f.core.budget_notification_permit(&claimed[0]).unwrap();
    assert!(permit.is_current());
    f.at("2026-01-01T23:59:57+08:00", 3_000);
    f.observe();
    assert!(permit.is_current());
    f.at("2026-01-02T00:00:12+08:00", 18_000);
    assert!(!permit.is_current()); // The ordinary scan did not extend it.
}

#[test]
fn historical_delta_plus_insufficient_fresh_delta_cannot_borrow_any_live_flag() {
    let mut f = Fixture::new();
    f.daily(200);
    f.arm();
    f.at("2026-01-02T00:00:10+08:00", 16_000);
    f.observe();
    f.at("2026-01-02T00:00:20+08:00", 26_000);
    f.observe();
    f.at("2026-01-02T00:00:30+08:00", 36_000);
    let mut historical = f.usage(THREAD, "late-190", "turn", "turn", 190);
    historical["timestamp"] = json!("2026-01-02T00:00:00+08:00");
    f.append(&f.source, &historical);
    f.append(&f.source, &f.usage(THREAD, "fresh-20", "turn", "turn", 20));
    f.observe();
    assert_eq!(f.budget("daily")["usedTokens"], 210);
    f.history_only("daily");
}

#[test]
fn pre_mutex_host_sample_cannot_borrow_resumed_epoch_or_wait_beyond_lifetime() {
    for boundary in ["epoch", "waiting", "rollback"] {
        let mut f = Fixture::new();
        f.daily(200);
        f.arm();
        let host = f
            .core
            .notification_gate()
            .snapshot()
            .map(|epoch| (epoch, budget_clock::now()));
        match boundary {
            "epoch" => {
                f.set_host("2026-01-02T00:00:00+08:00", 6_000, 1, true);
                f.set_host("2026-01-02T00:00:01+08:00", 7_000, 2, false);
            }
            "waiting" => f.at("2026-01-02T00:00:20+08:00", 26_000),
            _ => f.at("2026-01-01T23:59:57+08:00", 3_000),
        }
        f.append(&f.source, &f.usage(THREAD, boundary, "turn", "turn", 250));
        f.core
            .scan_with_cancel_host(&std::sync::atomic::AtomicBool::new(false), host)
            .unwrap();
        f.history_only("daily");
    }
}

#[test]
fn handled_period_survives_clear_and_a_conflict_drop_below_then_recross() {
    let mut f = Fixture::new();
    f.daily(200);
    f.arm();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "original-250", "turn", "turn", 250),
    );
    f.observe();
    assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
    f.core.action("clear_inbox", &json!({})).unwrap();
    f.at("2026-01-02T00:00:02+08:00", 8_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "original-250", "turn", "turn", 251),
    );
    f.observe();
    assert_eq!(f.budget("daily")["usedTokens"], 0);
    f.at("2026-01-02T00:00:03+08:00", 9_000);
    f.observe();
    f.at("2026-01-02T00:00:04+08:00", 10_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "recross-250", "turn", "turn", 250),
    );
    f.observe();
    assert_eq!(f.budget("daily")["usedTokens"], 250);
    assert!(f.core.claim_notifications().unwrap().is_empty());
    assert!(f.core.snapshot(json!({})).unwrap()["inbox"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn paused_history_backlog_does_not_block_a_committed_live_crossing() {
    let mut f = Fixture::new();
    f.core.action("pause_history", &json!({})).unwrap();
    f.daily(200);
    f.arm();
    // The other file remains unfinished in a bounded live read. Its history
    // backlog/oversized-line diagnosis is not evidence against this live file.
    fs::write(
        f.source.join("old-backlog.jsonl"),
        vec![b' '; 5 * 1024 * 1024],
    )
    .unwrap();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "live-with-backlog", "turn", "turn", 250),
    );
    let report = f.core.scan().unwrap();
    assert!(report.pending_files > 0);
    assert_eq!(
        get_setting(&f.core.db, "historyPaused").unwrap(),
        Some(json!(true))
    );
    assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
}

#[test]
fn unrelated_missing_source_does_not_block_a_committed_live_crossing() {
    let mut f = Fixture::new();
    let missing = f.temporary.path().join("unrelated-source");
    fs::create_dir(&missing).unwrap();
    f.core.add_source(&missing).unwrap();
    fs::remove_dir(&missing).unwrap();
    f.daily(200);
    f.arm();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "known-live", "turn", "turn", 250),
    );
    let report = f.core.scan().unwrap();
    assert!(report.issues > 0);
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["budgetId"], "daily");
}

#[test]
fn disabled_or_absent_budget_path_never_reads_fact_detail_tables() {
    for disabled in [false, true] {
        let mut f = Fixture::new();
        if disabled {
            f.core
                .action(
                    "save_budget",
                    &json!({"id":"off","scope":"daily","threshold":200,"enabled":false}),
                )
                .unwrap();
        }
        // Equivalent query guard without adding rusqlite hook features: any
        // attempted fact/detail projection now fails rather than silently reads.
        f.core.db.execute_batch("ALTER TABLE usage_responses RENAME TO guard_usage; ALTER TABLE thread_metadata RENAME TO guard_metadata; ALTER TABLE turns RENAME TO guard_turns;").unwrap();
        let host = f
            .core
            .notification_gate()
            .snapshot()
            .map(|epoch| (epoch, budget_clock::now()));
        assert!(begin(&mut f.core, host).unwrap().is_none());
        evaluate_without_scan(&mut f.core).unwrap();
    }
}

#[test]
fn a_fourteen_second_old_fresh_event_expires_after_its_original_last_second() {
    for claim_before_expiry in [false, true] {
        let mut f = Fixture::new();
        f.daily(200);
        f.arm();
        f.at("2026-01-02T00:00:00+08:00", 6_000);
        f.observe();
        f.at("2026-01-02T00:00:14+08:00", 20_000);
        let mut response = f.usage(THREAD, "fourteen-seconds-old", "turn", "turn", 250);
        response["timestamp"] = json!("2026-01-02T00:00:00+08:00");
        f.append(&f.source, &response);
        f.observe();
        let item = f.inbox("daily");
        let permit = f.core.budget_notification_permit(&item).unwrap();
        assert_eq!(permit.expires_utc(), utc("2026-01-02T00:00:15+08:00"));
        if claim_before_expiry {
            assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
        }
        f.at("2026-01-02T00:00:15+08:00", 21_000);
        assert!(!permit.is_current());
        assert!(f.core.claim_notifications().unwrap().is_empty());
        if !claim_before_expiry {
            assert_eq!(f.inbox("daily")["notificationState"], "suppressed");
        }
    }
}

#[test]
fn clock_rollback_or_skew_cannot_rearm_from_a_lower_ordinary_anchor() {
    for mode in ["utc", "mono", "skew"] {
        let mut f = Fixture::new();
        f.daily(200);
        f.arm();
        let (first, first_mono, second, second_mono) = match mode {
            "utc" => (
                "2026-01-01T23:59:57+08:00",
                5_000,
                "2026-01-01T23:59:57.500+08:00",
                5_500,
            ),
            "mono" => (
                "2026-01-01T23:59:59+08:00",
                3_000,
                "2026-01-01T23:59:59.500+08:00",
                3_500,
            ),
            _ => (
                "2026-01-02T00:00:10+08:00",
                5_000,
                "2026-01-02T00:00:11+08:00",
                6_000,
            ),
        };
        f.at(first, first_mono);
        f.observe();
        f.at(second, second_mono);
        f.append(&f.source, &f.usage(THREAD, mode, "turn", "turn", 250));
        f.observe();
        f.history_only("daily");
        f.at("2026-01-02T00:00:12+08:00", 18_000);
        f.observe();
        let baseline = f.budget("daily")["usedTokens"].as_i64().unwrap();
        f.daily(baseline + 50); // A new version uses its measured actual baseline.
        f.at("2026-01-02T00:00:13+08:00", 19_000);
        f.append(
            &f.source,
            &f.usage(THREAD, &format!("recovered-{mode}"), "turn", "turn", 60),
        );
        f.observe();
        assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
    }
}

#[test]
fn a_fresh_append_before_the_previous_scan_finished_is_still_new_live_usage() {
    let mut f = Fixture::new();
    f.daily(200);
    f.arm();
    f.at("2026-01-02T00:00:01+08:00", 7_000);
    f.observe();
    f.at("2026-01-02T00:00:02+08:00", 8_000);
    let mut response = f.usage(THREAD, "delayed-file-tail", "turn", "turn", 250);
    // The previous reader could have sampled file length before this source
    // event was appended. It is still inside the same continuous fresh window.
    response["timestamp"] = json!("2026-01-02T00:00:00.900+08:00");
    f.append(&f.source, &response);
    f.observe();
    assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
}

#[test]
fn a_stable_forward_clock_domain_can_rebuild_a_measured_new_version_baseline() {
    let mut f = Fixture::new();
    f.daily(200);
    f.arm();
    f.at("2026-01-02T00:01:00+08:00", 6_000); // Real discontinuity: reject old carry.
    f.observe();
    f.at("2026-01-02T00:01:01+08:00", 7_000); // New clocks advance together.
    f.observe();
    f.daily(200);
    f.at("2026-01-02T00:01:02+08:00", 8_000);
    f.append(
        &f.source,
        &f.usage(THREAD, "stable-new-clock", "turn", "turn", 250),
    );
    f.observe();
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(f.budget("daily")["version"], 2);
}

#[test]
fn dnd_enable_then_disable_revokes_claimed_and_unclaimed_budget_permits() {
    for claimed_before_dnd in [false, true] {
        let mut f = Fixture::new();
        f.daily(200);
        f.arm();
        f.at("2026-01-02T00:00:01+08:00", 7_000);
        f.append(
            &f.source,
            &f.usage(THREAD, "before-dnd", "turn", "turn", 250),
        );
        f.observe();
        let permit = f
            .core
            .budget_notification_permit(&f.inbox("daily"))
            .unwrap();
        if claimed_before_dnd {
            assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
        }
        f.core
            .action("update_settings", &json!({"doNotDisturb":true}))
            .unwrap();
        f.core
            .action("update_settings", &json!({"doNotDisturb":false}))
            .unwrap();
        assert!(
            !permit.is_current(),
            "queued clones cannot outlive a DND round trip"
        );
        assert!(f.core.claim_notifications().unwrap().is_empty());
        if !claimed_before_dnd {
            assert_eq!(f.inbox("daily")["notificationState"], "suppressed");
        }

        // Clearing authorization does not disable a subsequent fresh crossing.
        f.observe();
        f.daily(300);
        f.at("2026-01-02T00:00:02+08:00", 8_000);
        f.append(&f.source, &f.usage(THREAD, "after-dnd", "turn", "turn", 60));
        f.observe();
        assert_eq!(f.core.claim_notifications().unwrap().len(), 1);
    }
}
