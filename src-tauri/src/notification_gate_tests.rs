use super::*;
use crate::budget_clock::test_support::TestClock;
use serde_json::json;
use std::cell::{Cell, RefCell};

fn at(ms: i64) -> Reading {
    Reading { utc: DateTime::parse_from_rfc3339("2026-10-16T15:59:59Z").unwrap().with_timezone(&Utc)
        + chrono::Duration::milliseconds(ms), mono_ms: u64::try_from(ms).unwrap() + 1_000 }
}
fn open_gate() -> Arc<Gate> {
    let gate = Arc::new(Gate::new());
    gate.publish(true, false);
    gate
}
fn fresh_permit(gate: &Arc<Gate>) -> NotificationPermit {
    NotificationPermit::new_at(gate.clone(), gate.snapshot().unwrap(), at(15_000).utc, at(15_000).mono_ms, at(0))
}
fn queued(id: &str, kind: &str, permit: Option<NotificationPermit>) -> QueuedNotification {
    QueuedNotification { event: json!({"id":id,"kind":kind}), permit }
}

#[test]
fn strong_session_messages_override_lagging_queries_and_unblock_requires_known_state() {
    let prior = SessionObservation { locked: false, disconnected: false, known: true };
    let locked = session_after_boundary(prior, Some((false, false)), SessionBoundary::Lock);
    assert!(locked.locked, "lagging unlocked query cannot weaken a lock message");
    let disconnected = session_after_boundary(prior, Some((false, false)), SessionBoundary::Disconnect);
    assert!(disconnected.disconnected, "lagging active query cannot weaken disconnect");
    let unknown = session_after_boundary(locked, None, SessionBoundary::Unlock);
    assert!(!unknown.known);
    assert!(unknown.locked);
    let unknown = session_after_boundary(disconnected, None, SessionBoundary::Connect);
    assert!(!unknown.known);
    assert!(unknown.disconnected);
    let confirmed = session_after_boundary(locked, Some((false, false)), SessionBoundary::Unlock);
    assert!(confirmed.known && !confirmed.locked && !confirmed.disconnected);
    let connected = session_after_boundary(locked, Some((false, false)), SessionBoundary::Connect);
    assert!(connected.locked, "connect cannot clear a prior lock via lagging query");
    let unlocked = session_after_boundary(disconnected, Some((false, false)), SessionBoundary::Unlock);
    assert!(unlocked.disconnected, "unlock cannot clear a prior disconnect via lagging query");
}

#[test]
fn unknown_native_state_and_every_hard_boundary_prevent_epoch_aba() {
    let gate = Gate::new();
    assert_eq!(gate.snapshot(), None);
    gate.publish(false, false);
    assert_eq!(gate.snapshot(), None);
    gate.publish(true, true);
    assert_eq!(gate.snapshot(), None);
    gate.publish(true, false);
    let before = gate.snapshot().unwrap();
    gate.publish(true, true);
    gate.publish(true, false);
    let after = gate.snapshot().unwrap();
    assert_ne!(before, after);
    gate.publish(true, false);
    assert_ne!(gate.snapshot(), Some(after), "duplicate resume is still a boundary");
}

#[test]
fn invalidate_keeps_availability_but_exhausted_epoch_stays_closed() {
    let gate = open_gate();
    let prior = gate.snapshot();
    gate.invalidate();
    assert!(gate.snapshot().is_some());
    assert_ne!(gate.snapshot(), prior);
    gate.publish(true, true);
    gate.invalidate();
    assert_eq!(gate.snapshot(), None);
    gate.0.store((MAX_EPOCH << EPOCH_SHIFT) | AVAILABLE, Ordering::Release);
    gate.invalidate();
    assert_eq!(gate.snapshot(), None);
    gate.publish(true, false);
    assert_eq!(gate.snapshot(), None, "overflow cannot reopen a used token");
}

#[test]
fn permit_clone_has_shared_revocation_and_uses_the_injected_budget_clock() {
    let gate = open_gate();
    let clock = TestClock::new(at(0));
    let permit = NotificationPermit::new(gate.clone(), gate.snapshot().unwrap(), at(15_000).utc, at(15_000).mono_ms);
    let clone = permit.clone();
    clock.set(at(1));
    assert!(permit.is_current());
    assert!(clone.is_current());
    clone.revoke();
    assert!(!permit.is_current());
}

#[test]
fn both_utc_and_monotonic_deadlines_are_exclusive_and_never_extended() {
    let gate = open_gate();
    assert!(fresh_permit(&gate).is_current_at(&at(14_999)));
    for expired in [
        at(15_000),
        Reading { utc: at(15_000).utc, mono_ms: at(1).mono_ms },
        Reading { utc: at(1).utc, mono_ms: at(15_000).mono_ms },
    ] {
        let permit = fresh_permit(&gate);
        assert!(!permit.is_current_at(&expired));
        assert!(!permit.is_current_at(&at(1)), "clock movement cannot revive an expired permit");
    }
}

#[test]
fn clock_rollback_and_native_epoch_change_permanently_revoke_clones() {
    let gate = open_gate();
    let permit = fresh_permit(&gate);
    let clone = permit.clone();
    assert!(!permit.is_current_at(&Reading { utc: at(0).utc - chrono::Duration::milliseconds(1), mono_ms: at(1).mono_ms }));
    assert!(!clone.is_current_at(&at(1)));
    let permit = fresh_permit(&gate);
    gate.publish(true, true);
    gate.publish(true, false);
    assert!(!permit.is_current_at(&at(1)));
}

#[test]
fn rollback_after_issuance_but_below_a_shared_high_watermark_cannot_revive() {
    for rollback in [
        Reading { utc: at(500).utc, mono_ms: at(1_100).mono_ms },
        Reading { utc: at(1_100).utc, mono_ms: at(500).mono_ms },
    ] {
        let gate = open_gate();
        let permit = fresh_permit(&gate);
        let clone = permit.clone();
        assert!(permit.is_current_at(&at(1_000)));
        assert!(!clone.is_current_at(&rollback));
        assert!(!permit.is_current_at(&at(1_200)), "shared rollback revocation is permanent");
    }
}

#[test]
fn missing_budget_permit_is_dropped_without_removing_completion() {
    let mut queue = vec![queued("budget", "budget", None), queued("completion", "completion", None)];
    assert_eq!(discard_invalid(&mut queue, &at(0)), vec!["budget"]);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].event["id"], "completion");
}

#[test]
fn queued_budget_cannot_survive_suspend_resume_during_merge_or_throttle() {
    let gate = open_gate();
    let events = vec![queued("budget", "budget", Some(fresh_permit(&gate))), queued("completion", "completion", None)];
    gate.publish(true, true);
    gate.publish(true, false);
    let sent = RefCell::new(Vec::new());
    let delivery = deliver_batch(&events,
        |items| Ok(items.iter().filter_map(QueuedNotification::id).collect::<Vec<_>>()),
        |ids| { sent.borrow_mut().extend(ids); Ok(()) }, || true, || at(1));
    assert_eq!(delivery.suppressed, vec!["budget"]);
    assert_eq!(delivery.submitted, vec!["completion"]);
    assert_eq!(*sent.borrow(), vec!["completion"]);
    assert!(delivery.failed.is_empty());
}

#[test]
fn native_change_during_com_preparation_rebuilds_only_surviving_completion() {
    let gate = open_gate();
    let events = vec![queued("budget", "budget", Some(fresh_permit(&gate))), queued("completion", "completion", None)];
    let prepared = RefCell::new(Vec::new());
    let submitted = RefCell::new(Vec::new());
    let delivery = deliver_batch(&events, |items| {
        let ids = items.iter().filter_map(QueuedNotification::id).collect::<Vec<_>>();
        prepared.borrow_mut().push(ids.clone());
        if prepared.borrow().len() == 1 { gate.publish(true, true); gate.publish(true, false); }
        Ok(ids)
    }, |ids| { submitted.borrow_mut().extend(ids); Ok(()) }, || true, || at(1));
    assert_eq!(*prepared.borrow(), vec![vec!["budget", "completion"], vec!["completion"]]);
    assert_eq!(*submitted.borrow(), vec!["completion"]);
    assert_eq!(delivery.suppressed, vec!["budget"]);
    assert!(delivery.failed.is_empty());
}

#[test]
fn expiry_during_com_preparation_is_suppressed_and_never_submitted_or_failed() {
    let gate = open_gate();
    let events = vec![queued("budget", "budget", Some(fresh_permit(&gate)))];
    let now = Cell::new(at(1));
    let submitted = Cell::new(0);
    let delivery = deliver_batch(&events, |_| { now.set(at(15_000)); Ok(()) },
        |_| { submitted.set(submitted.get() + 1); Ok(()) }, || true, || now.get());
    assert_eq!(submitted.get(), 0);
    assert_eq!(delivery.suppressed, vec!["budget"]);
    assert!(delivery.failed.is_empty());
    assert!(delivery.error.is_none());
}

#[test]
fn utc_rollback_inside_com_preparation_after_issued_time_suppresses_budget() {
    let gate = open_gate();
    let permit = fresh_permit(&gate);
    assert!(permit.is_current_at(&at(1_000)));
    let events = vec![queued("budget", "budget", Some(permit))];
    let now = Cell::new(at(1_000));
    let submitted = Cell::new(false);
    let delivery = deliver_batch(&events, |_| {
        now.set(Reading { utc: at(500).utc, mono_ms: at(1_100).mono_ms });
        Ok(())
    }, |_| { submitted.set(true); Ok(()) }, || true, || now.get());
    assert!(!submitted.get());
    assert_eq!(delivery.suppressed, vec!["budget"]);
    assert!(delivery.failed.is_empty());
    assert!(delivery.error.is_none());
}

#[test]
fn final_native_or_dnd_denial_after_preparation_is_suppressed_not_failed() {
    let allowed = Cell::new(true);
    let submitted = Cell::new(false);
    let events = vec![queued("completion", "completion", None)];
    let delivery = deliver_batch(&events, |_| { allowed.set(false); Ok(()) },
        |_| { submitted.set(true); Ok(()) }, || allowed.get(), || at(1));
    assert!(!submitted.get());
    assert_eq!(delivery.suppressed, vec!["completion"]);
    assert!(delivery.failed.is_empty());
}

#[test]
fn preparation_failure_separates_newly_stale_budget_from_actual_failed_completion() {
    let gate = open_gate();
    let events = vec![queued("budget", "budget", Some(fresh_permit(&gate))), queued("completion", "completion", None)];
    let delivery = deliver_batch(&events, |_| {
        gate.invalidate();
        Err::<(), _>("synthetic COM preparation failure".into())
    }, |_| panic!("failure cannot submit"), || true, || at(1));
    assert_eq!(delivery.suppressed, vec!["budget"]);
    assert_eq!(delivery.failed, vec!["completion"]);
    assert_eq!(delivery.error.as_deref(), Some("synthetic COM preparation failure"));
}

#[test]
fn submission_failure_is_attempted_once_and_is_not_replayed() {
    let attempts = Cell::new(0);
    let events = vec![queued("completion", "completion", None)];
    let delivery = deliver_batch(&events, |_| Ok(()), |_| {
        attempts.set(attempts.get() + 1);
        Err("synthetic OS submission failure".into())
    }, || true, || at(1));
    assert_eq!(attempts.get(), 1);
    assert_eq!(delivery.failed, vec!["completion"]);
    assert!(delivery.submitted.is_empty());
}

#[test]
fn windows_expiration_ticks_use_1601_epoch_and_preserve_subsecond_precision() {
    let unix = DateTime::from_timestamp(0, 123_456_700).unwrap();
    assert_eq!(windows_deadline_ticks(unix), Some(116_444_736_001_234_567));
    assert_eq!(windows_deadline_ticks(DateTime::parse_from_rfc3339("1601-01-01T00:00:00Z").unwrap().with_timezone(&Utc)), Some(0));
}

#[test]
fn expired_full_budget_queue_admits_new_completion_and_restarts_merge_window() {
    let gate = open_gate();
    let mut queue = (0..100).map(|i| queued(&format!("old-budget-{i}"), "budget", Some(fresh_permit(&gate))))
        .collect::<Vec<_>>();
    let admission = admit_notifications(&mut queue,
        vec![queued("new-completion", "completion", None)], &at(15_000));
    assert_eq!(admission.suppressed.len(), 100);
    assert!(admission.suppressed.iter().all(|id| id.starts_with("old-budget-")));
    assert!(admission.reset_queued_at, "old merge origin cannot carry into a new batch");
    assert_eq!(queue.iter().filter_map(QueuedNotification::id).collect::<Vec<_>>(), vec!["new-completion"]);

    // Both the old queue and a newly claimed batch may expire together. The
    // caller must finish all returned IDs in Core's bounded batches of 100.
    let mut expired = (0..100).map(|i| queued(&format!("old-{i}"), "budget", Some(fresh_permit(&gate))))
        .collect::<Vec<_>>();
    let incoming = (0..100).map(|i| queued(&format!("incoming-{i}"), "budget", Some(fresh_permit(&gate))))
        .collect::<Vec<_>>();
    let all_expired = admit_notifications(&mut expired, incoming, &at(15_000));
    assert_eq!(all_expired.suppressed.len(), 200);
    assert!(all_expired.reset_queued_at);
    assert!(expired.is_empty());
}

#[test]
fn surviving_completion_keeps_merge_window_and_only_current_full_queue_overflows() {
    let gate = open_gate();
    let mut queue = vec![queued("old-completion-a", "completion", None), queued("old-completion-b", "completion", None)];
    queue.extend((0..98).map(|i| queued(&format!("old-budget-{i}"), "budget", Some(fresh_permit(&gate)))));
    let mut incoming = vec![queued("expired-new-budget", "budget", Some(fresh_permit(&gate)))];
    incoming.extend((0..98).map(|i| queued(&format!("new-completion-{i}"), "completion", None)));
    incoming.push(queued("real-overflow-completion", "completion", None));
    assert_eq!(incoming.len(), 100, "same maximum claimed batch as Core");
    let admission = admit_notifications(&mut queue, incoming, &at(15_000));
    assert!(!admission.reset_queued_at, "surviving old completions keep their merge origin");
    assert_eq!(queue.len(), 100);
    assert_eq!(queue[0].event["id"], "old-completion-a");
    assert_eq!(queue[1].event["id"], "old-completion-b");
    for i in 0..98 { assert_eq!(queue[i + 2].event["id"], format!("new-completion-{i}")); }
    assert_eq!(admission.suppressed.len(), 100);
    assert_eq!(&admission.suppressed[98..], ["expired-new-budget", "real-overflow-completion"]);
    assert!(queue.iter().all(|event| event.event["kind"] == "completion"));
}
