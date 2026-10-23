use super::*;
use crate::budget_clock::{test_support::TestClock, Reading};
use crate::notification_gate::NotificationPermit;

struct Fixture {
    _temp: tempfile::TempDir,
    data: PathBuf,
    source: PathBuf,
    core: Core,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let core = Core::open(&data, &source).unwrap();
        core.notification_gate.publish(true, false);
        Self { _temp: temp, data, source, core }
    }
    fn set(&mut self, minutes: u64) -> Value {
        self.core.action("update_settings", &json!({"temporaryDoNotDisturbMinutes":minutes})).unwrap()
    }
    fn inbox(&self, id: &str, at: Reading) {
        let event = json!({"id":id,"kind":"completion","status":"completed","sourceId":self.core.source_id,
            "threadId":"quiet-thread","turnId":id,"createdAt":at.utc.to_rfc3339(),"read":false,
            "notifyEligible":true,"animationAcknowledged":false,"eligibleRuntime":self.core.runtime_id,
            "notificationState":"pending","notified":false});
        self.core.db.execute("INSERT INTO inbox_events(id,thread_id,turn_id,event_time,data) VALUES(?1,'quiet-thread',?1,?2,?3)",
            params![id,at.utc.to_rfc3339(),event.to_string()]).unwrap();
    }
    fn event(&self, id: &str) -> Value {
        adapter::load_fact(&self.core.db, "inbox_events", id).unwrap().unwrap()
    }
}

fn at(ms: i64) -> Reading {
    let utc = DateTime::parse_from_rfc3339("2026-10-23T02:00:00Z").unwrap().with_timezone(&Utc)
        + chrono::Duration::milliseconds(ms);
    Reading { utc, mono_ms: 10_000_000u64.checked_add_signed(ms).unwrap() }
}

fn active(core: &Core) -> bool { core.settings().unwrap()["temporaryDoNotDisturb"]["active"] == true }

#[test]
fn temporary_dnd_thirty_and_sixty_minutes_expire_at_boundary_and_do_not_revive() {
    for minutes in [30, 60] {
        let clock = TestClock::new(at(0));
        let mut f = Fixture::new();
        let settings = f.set(minutes);
        let duration = minutes as i64 * 60_000;
        assert_eq!(settings["temporaryDoNotDisturb"]["remainingMs"], duration);
        assert_eq!(settings["temporaryDoNotDisturb"]["deadlineMonoMs"], at(duration).mono_ms);
        assert_eq!(settings["doNotDisturb"], false);
        assert_eq!(settings["effectiveDoNotDisturb"], true);
        clock.set(at(duration - 1));
        assert_eq!(f.core.settings().unwrap()["temporaryDoNotDisturb"]["remainingMs"], 1);
        clock.set(at(duration));
        assert!(!active(&f.core));
        assert_eq!(f.core.settings().unwrap()["temporaryDoNotDisturb"],
            json!({"active":false,"until":null,"remainingMs":0,"deadlineMonoMs":null}));
        clock.set(at(duration - 2));
        assert!(!active(&f.core));
        clock.set(at(duration + 1));
        assert!(!active(&f.core));
    }
}

#[test]
fn temporary_dnd_either_deadline_clock_and_clock_anomalies_latch_termination() {
    for reading in [
        Reading { utc: at(1_800_000).utc, mono_ms: at(1_799_000).mono_ms },
        Reading { utc: at(1_799_000).utc, mono_ms: at(1_800_000).mono_ms },
        Reading { utc: at(500).utc, mono_ms: at(1_001).mono_ms },
        Reading { utc: at(6_000).utc, mono_ms: at(1_001).mono_ms },
        Reading { utc: at(1_001).utc, mono_ms: at(500).mono_ms },
    ] {
        let clock = TestClock::new(at(0));
        let mut f = Fixture::new();
        f.set(30);
        clock.set(at(1_000));
        assert!(active(&f.core));
        let epoch = f.core.notification_gate.snapshot();
        clock.set(reading);
        assert!(!active(&f.core));
        assert_ne!(f.core.notification_gate.snapshot(), epoch);
        clock.set(at(1_100));
        assert!(!active(&f.core));
    }
}

#[test]
fn temporary_dnd_cancel_only_removes_temporary_source_and_restart_starts_empty() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.core.action("update_settings", &json!({"doNotDisturb":true,"dndStart":"09:00","dndEnd":"11:00"})).unwrap();
    f.set(30);
    let settings = f.set(0);
    assert_eq!(settings["temporaryDoNotDisturb"]["active"], false);
    assert_eq!(settings["doNotDisturb"], true);
    assert_eq!(settings["scheduledDoNotDisturb"], true);
    assert_eq!(settings["effectiveDoNotDisturb"], true);
    f.core.action("update_settings", &json!({"doNotDisturb":false})).unwrap();
    assert_eq!(f.core.settings().unwrap()["effectiveDoNotDisturb"], true);
    f.core.action("update_settings", &json!({"dndStart":"10:00","dndEnd":"10:00"})).unwrap();
    assert_eq!(f.core.settings().unwrap()["effectiveDoNotDisturb"], false);
    f.set(60);
    let saved = get_setting(&f.core.db, "preferences").unwrap().unwrap();
    for key in ["temporaryDoNotDisturbMinutes", "temporaryDoNotDisturb", "scheduledDoNotDisturb", "effectiveDoNotDisturb"] {
        assert!(saved.get(key).is_none());
    }
    clock.set(at(1_000));
    let reopened = Core::open(&f.data, &f.source).unwrap();
    assert!(!active(&reopened));
    assert_eq!(reopened.settings().unwrap()["doNotDisturb"], false);
    assert_eq!(reopened.settings().unwrap()["dndStart"], "10:00");
}

#[test]
fn temporary_dnd_replacement_uses_new_deadline_and_does_not_change_saved_preferences() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    let saved = get_setting(&f.core.db, "preferences").unwrap();
    f.set(30);
    clock.set(at(60_000));
    let replacement = f.set(60);
    assert_eq!(replacement["temporaryDoNotDisturb"]["deadlineMonoMs"], at(3_660_000).mono_ms);
    assert_eq!(get_setting(&f.core.db, "preferences").unwrap(), saved);
    clock.set(at(1_800_000));
    assert!(active(&f.core));
    clock.set(at(3_660_000));
    assert!(!active(&f.core));
}

#[test]
fn temporary_dnd_invalid_command_or_projection_input_keeps_settings_and_runtime() {
    let _clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.set(30);
    let before = f.core.settings().unwrap();
    let saved = get_setting(&f.core.db, "preferences").unwrap();
    for value in [Value::Null, json!("30"), json!(true), json!(-1), json!(1), json!(45), json!(30.0), json!({})] {
        assert!(f.core.action("update_settings", &json!({"temporaryDoNotDisturbMinutes":value,"doNotDisturb":true})).is_err());
    }
    for key in ["temporaryDoNotDisturb", "scheduledDoNotDisturb", "effectiveDoNotDisturb"] {
        let mut patch = json!({"temporaryDoNotDisturbMinutes":60});
        patch[key] = json!(false);
        assert!(f.core.action("save_settings", &json!({"settings":patch})).is_err());
    }
    assert_eq!(get_setting(&f.core.db, "preferences").unwrap(), saved);
    assert_eq!(f.core.settings().unwrap(), before);
}

#[test]
fn temporary_dnd_failed_preferences_write_does_not_start_replace_cancel_or_invalidate_gate() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.set(30);
    let until = f.core.settings().unwrap()["temporaryDoNotDisturb"]["until"].clone();
    let epoch = f.core.notification_gate.snapshot();
    let saved = get_setting(&f.core.db, "preferences").unwrap();
    f.core.db.execute_batch("CREATE TEMP TRIGGER fail_temporary_preferences BEFORE UPDATE ON settings
        WHEN NEW.key='preferences' BEGIN SELECT RAISE(ABORT,'临时免打扰写入失败夹具'); END;").unwrap();
    clock.set(at(1_000));
    for minutes in [0, 60] {
        assert!(f.core.action("update_settings", &json!({"temporaryDoNotDisturbMinutes":minutes,"doNotDisturb":true})).is_err());
        assert!(active(&f.core));
        assert_eq!(f.core.settings().unwrap()["temporaryDoNotDisturb"]["until"], until);
    }
    assert_eq!(f.core.notification_gate.snapshot(), epoch);
    assert_eq!(get_setting(&f.core.db, "preferences").unwrap(), saved);
    f.core.db.execute_batch("DROP TRIGGER fail_temporary_preferences;").unwrap();
    f.set(0);
    f.core.db.execute_batch("CREATE TEMP TRIGGER fail_temporary_preferences BEFORE UPDATE ON settings
        WHEN NEW.key='preferences' BEGIN SELECT RAISE(ABORT,'临时免打扰写入失败夹具'); END;").unwrap();
    assert!(f.core.action("update_settings", &json!({"temporaryDoNotDisturbMinutes":30})).is_err());
    assert!(!active(&f.core));
}

#[test]
fn temporary_dnd_expiry_drops_last_second_event_but_keeps_post_expiry_event_and_unread_facts() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    let baseline = f.core.pet_pulse(None).unwrap();
    f.set(30);
    clock.set(at(1_799_000));
    f.inbox("during-quiet", at(1_799_000));
    // 到期后的第一次观察同时看见两条记录，不能把新记录纳入旧静默到达水位。
    clock.set(at(1_800_100));
    f.inbox("after-quiet", at(1_800_100));
    let pulse = f.core.pet_pulse(Some(baseline["promptFeed"]["cursor"].clone())).unwrap();
    let prompts = pulse["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["id"], "after-quiet");
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["id"], "after-quiet");
    let suppressed = f.event("during-quiet");
    assert_eq!(suppressed["notificationState"], "suppressed");
    assert_eq!(suppressed["animationAcknowledged"], true);
    assert_eq!(suppressed["read"], false);
    assert_eq!(f.event("after-quiet")["read"], false);
    assert!(f.core.claim_notifications().unwrap().is_empty());
}

#[test]
fn temporary_dnd_begin_arrival_fence_blocks_future_dated_backlog_after_cancel() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.inbox("prior-future", at(5_000));
    f.set(30);
    clock.set(at(1_000));
    f.set(0);
    clock.set(at(6_000));
    f.inbox("new-after-cancel", at(6_000));
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["id"], "new-after-cancel");
    assert_eq!(f.event("prior-future")["notificationState"], "suppressed");
}

#[test]
fn temporary_dnd_active_reads_advance_arrival_fence_without_database_writes_or_replay() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.set(30);
    clock.set(at(1_000));
    f.inbox("active-future", at(5_000));
    let changes = f.core.db.total_changes();
    assert!(active(&f.core));
    assert_eq!(f.core.db.total_changes(), changes);
    clock.set(at(2_000));
    f.set(0);
    clock.set(at(6_000));
    assert!(f.core.claim_notifications().unwrap().is_empty());
    assert_eq!(f.event("active-future")["animationAcknowledged"], true);
}

#[test]
fn temporary_dnd_transitions_revoke_budget_completion_and_shared_gate_clones() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    let event = json!({"kind":"completion"});
    let completion = f.core.notification_permit(&event).unwrap();
    let completion_clone = completion.clone();
    let budget = f.core.grant_budget_test_permit("budget-before", 7_200_000);
    let epoch = f.core.notification_gate.snapshot().unwrap();
    f.set(30);
    assert!(!completion_clone.is_current());
    assert!(!budget.is_current());
    assert_ne!(f.core.notification_gate.snapshot(), Some(epoch));
    assert!(f.core.notification_permit(&event).is_none());
    let during_epoch = f.core.notification_gate.snapshot().unwrap();
    let deadline = at(7_200_000);
    let shared = NotificationPermit::new(f.core.notification_gate.clone(), during_epoch, deadline.utc, deadline.mono_ms);
    let shared_clone = shared.clone();
    f.set(0);
    assert!(!shared_clone.is_current());
    let fresh_completion = f.core.notification_permit(&event).unwrap();
    assert!(fresh_completion.is_current());
    clock.set(at(30_000));
    assert!(!fresh_completion.is_current());

    f.set(30);
    let epoch = f.core.notification_gate.snapshot().unwrap();
    let expiry_held = NotificationPermit::new(f.core.notification_gate.clone(), epoch, deadline.utc, deadline.mono_ms);
    clock.set(at(1_830_000));
    assert!(!active(&f.core));
    assert!(!expiry_held.is_current());
    assert!(f.core.notification_gate.snapshot().is_some());
}

#[test]
fn temporary_dnd_pinned_panel_read_shares_current_runtime_through_start_cancel_and_expiry() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    let session = f.core.begin_panel_read().unwrap();
    assert_eq!(session.snapshot(json!({})).unwrap()["settings"]["temporaryDoNotDisturb"]["active"], false);
    f.set(30);
    assert_eq!(session.snapshot(json!({})).unwrap()["settings"]["temporaryDoNotDisturb"]["active"], true);
    f.set(0);
    assert_eq!(session.snapshot(json!({})).unwrap()["settings"]["temporaryDoNotDisturb"]["active"], false);
    f.set(60);
    clock.set(at(3_600_000));
    assert_eq!(session.snapshot(json!({})).unwrap()["settings"]["temporaryDoNotDisturb"]["active"], false);
    clock.set(at(3_599_999));
    assert!(!active(&f.core));
    session.finish().unwrap();
}

#[test]
fn temporary_dnd_historical_settings_and_repeated_reads_do_not_cancel_current_lease() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.set(30);
    clock.set(at(1_000));
    assert!(active(&f.core));
    let epoch = f.core.notification_gate.snapshot();
    let until = f.core.settings().unwrap()["temporaryDoNotDisturb"]["until"].clone();
    for wall in [at(-60_000).utc, at(3_600_000).utc, at(500).utc] {
        let historic = f.core.settings_at(wall).unwrap();
        assert_eq!(historic["temporaryDoNotDisturb"]["active"], true);
        assert_eq!(historic["temporaryDoNotDisturb"]["until"], until);
        assert_eq!(historic["temporaryDoNotDisturb"]["remainingMs"], 1_799_000);
    }
    for _ in 0..5 { assert!(active(&f.core)); }
    assert_eq!(f.core.notification_gate.snapshot(), epoch);
}

#[test]
fn temporary_dnd_event_exactly_at_deadline_is_new_but_previous_millisecond_is_suppressed() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.set(30);
    clock.set(at(1_800_000));
    f.inbox("quiet-final-ms", at(1_799_999));
    f.inbox("quiet-exact-end", at(1_800_000));
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["id"], "quiet-exact-end");
    assert_eq!(f.event("quiet-final-ms")["notificationState"], "suppressed");
}

#[test]
fn temporary_dnd_wall_rollback_rebases_cutoff_so_new_epoch_events_can_notify() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    let baseline = f.core.pet_pulse(None).unwrap();
    f.set(60);
    clock.set(at(600_000));
    f.inbox("old-arrived-before-rollback", at(599_000));
    assert!(active(&f.core));
    clock.set(Reading { utc: at(-3_000_000).utc, mono_ms: at(601_000).mono_ms });
    assert!(!active(&f.core));
    clock.set(Reading { utc: at(-2_940_000).utc, mono_ms: at(661_000).mono_ms });
    f.inbox("new-rebased-time", at(-2_940_000));
    f.inbox("late-old-quiet-future", at(599_500));
    let pulse = f.core.pet_pulse(Some(baseline["promptFeed"]["cursor"].clone())).unwrap();
    assert_eq!(pulse["prompts"].as_array().unwrap().len(), 1);
    assert_eq!(pulse["prompts"][0]["id"], "new-rebased-time");
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["id"], "new-rebased-time");
    for id in ["old-arrived-before-rollback", "late-old-quiet-future"] {
        assert_eq!(f.event(id)["notificationState"], "suppressed");
        assert_eq!(f.event(id)["animationAcknowledged"], true);
        assert_eq!(f.event(id)["read"], false);
    }
}

#[test]
fn temporary_dnd_rollback_after_expiry_does_not_keep_future_cutoff_over_new_events() {
    let clock = TestClock::new(at(0));
    let mut f = Fixture::new();
    f.set(30);
    clock.set(at(1_800_000));
    assert!(!active(&f.core));
    clock.set(Reading { utc: at(60_000).utc, mono_ms: at(1_801_000).mono_ms });
    assert!(!active(&f.core));
    clock.set(Reading { utc: at(61_000).utc, mono_ms: at(1_802_000).mono_ms });
    f.inbox("after-inactive-clock-rebase", at(61_000));
    let claimed = f.core.claim_notifications().unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0]["id"], "after-inactive-clock-rebase");
}
