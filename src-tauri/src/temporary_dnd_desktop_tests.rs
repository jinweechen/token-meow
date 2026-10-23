use super::*;
use crate::budget_clock::{test_support::TestClock, Reading};

fn reading(utc: &str, mono_ms: u64) -> Reading {
    Reading {
        utc: chrono::DateTime::parse_from_rfc3339(utc).unwrap().with_timezone(&chrono::Utc),
        mono_ms,
    }
}

#[test]
fn temporary_projection_ignores_countdown_but_tracks_start_extension_and_expiry() {
    let active = json!({"temporaryDoNotDisturb":{"active":true,"until":"2026-10-23T08:30:00Z","remainingMs":1000,"deadlineMonoMs":2000}});
    let mut next = active.clone();
    next["temporaryDoNotDisturb"]["remainingMs"] = json!(500);
    next["muted"] = json!(false);
    assert!(!temporary_dnd_projection_changed(&active, &next));
    for (key, value) in [
        ("active", json!(false)),
        ("until", json!("2026-10-23T09:00:00Z")),
        ("deadlineMonoMs", json!(3000)),
    ] {
        let mut changed = active.clone();
        changed["temporaryDoNotDisturb"][key] = value;
        assert!(temporary_dnd_projection_changed(&active, &changed));
    }
    assert!(temporary_dnd_projection_changed(&json!({}), &active));
}

#[test]
fn native_temporary_dnd_expires_on_monotonic_deadline_without_trusting_cached_effective_flag() {
    let clock = TestClock::new(reading("2026-10-23T08:00:00Z", 1999));
    let settings = json!({"effectiveDoNotDisturb":true,"temporaryDoNotDisturb":{"active":true,"deadlineMonoMs":2000}});
    assert_eq!(companion::settings_pause(&settings), Some("doNotDisturb"));
    clock.set(reading("2026-10-23T07:00:00Z", 2000));
    assert_eq!(companion::settings_pause(&settings), None, "墙上时钟回拨不能延长已到期的临时静默");
}

#[test]
fn malformed_active_temporary_deadline_stays_paused_and_inactive_is_ignored() {
    let _clock = TestClock::new(reading("2026-10-23T08:00:00Z", 1000));
    for deadline in [Value::Null, json!("2000"), json!(-1), json!(2.5)] {
        assert_eq!(companion::settings_pause(&json!({"temporaryDoNotDisturb":{"active":true,"deadlineMonoMs":deadline}})), Some("doNotDisturb"));
    }
    assert_eq!(companion::settings_pause(&json!({"temporaryDoNotDisturb":{"active":false,"deadlineMonoMs":null}})), None);
}

#[test]
fn temporary_expiry_preserves_manual_and_daily_dnd() {
    let clock = TestClock::new(reading("2026-10-23T15:30:00Z", 2000));
    let mut settings = json!({"temporaryDoNotDisturb":{"active":true,"deadlineMonoMs":2000},"doNotDisturb":true});
    assert_eq!(companion::settings_pause(&settings), Some("doNotDisturb"));
    settings["doNotDisturb"] = json!(false);
    settings["dndStart"] = json!("22:00");
    settings["dndEnd"] = json!("07:00");
    assert_eq!(companion::settings_pause(&settings), Some("doNotDisturb"));
    clock.set(reading("2026-10-24T00:00:00Z", 3000));
    assert_eq!(companion::settings_pause(&settings), None);
    assert_eq!(settings["dndStart"], "22:00");
    assert_eq!(settings["dndEnd"], "07:00");
}

#[test]
fn claimed_completion_without_a_guard_cannot_reenter_delivery_after_host_resume() {
    let at = reading("2026-10-23T08:00:00Z", 1000);
    let _clock = TestClock::new(at);
    let gate = std::sync::Arc::new(Gate::new());
    gate.publish(true, true);
    let (mut admitted, suppressed) = guard_claimed_notifications(
        vec![json!({"id":"claimed-while-blocked","kind":"completion"})], |_| None);
    assert_eq!(suppressed, vec!["claimed-while-blocked"]);

    gate.publish(true, false);
    let deadline = at.checked_deadline(30_000).unwrap();
    let guard = notification_gate::NotificationPermit::new(gate.clone(), gate.snapshot().unwrap(), deadline.utc, deadline.mono_ms);
    let (fresh, suppressed) = guard_claimed_notifications(
        vec![json!({"id":"fresh-after-resume","kind":"completion"})], |_| Some(guard.clone()));
    assert!(suppressed.is_empty());
    admitted.extend(fresh);
    let delivery = notification_gate::deliver_batch(&admitted, |_| Ok(()), |_| Ok(()), || true, || at);
    assert_eq!(delivery.submitted, vec!["fresh-after-resume"]);
    assert!(delivery.suppressed.is_empty());
}
