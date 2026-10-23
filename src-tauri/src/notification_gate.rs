//! Native-owned notification admission. No desktop, database, or OS calls in this module.
use crate::budget_clock::{self, Reading};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex};

const AVAILABLE: u64 = 1;
const BLOCKED: u64 = 2;
const EPOCH_SHIFT: u32 = 2;
const MAX_EPOCH: u64 = u64::MAX >> EPOCH_SHIFT;

#[derive(Clone, Copy)]
pub(crate) struct SessionObservation {
    pub(crate) locked: bool,
    pub(crate) disconnected: bool,
    pub(crate) known: bool,
}
pub(crate) enum SessionBoundary { Lock, Unlock, Disconnect, Connect }

/// Blocking messages are authoritative even when the synchronous query lags.
/// Unblocking requires an independent, complete session observation.
pub(crate) fn session_after_boundary(prior: SessionObservation, query: Option<(bool, bool)>, boundary: SessionBoundary) -> SessionObservation {
    let mut observation = query.map(|(locked, disconnected)| SessionObservation { locked, disconnected, known: true })
        .unwrap_or(SessionObservation { known: false, ..prior });
    match boundary {
        SessionBoundary::Lock => {
            observation.locked = true;
            observation.disconnected |= prior.disconnected;
        }
        SessionBoundary::Disconnect => {
            observation.disconnected = true;
            observation.locked |= prior.locked;
        }
        SessionBoundary::Unlock => observation.disconnected |= prior.disconnected,
        SessionBoundary::Connect => observation.locked |= prior.locked,
    }
    observation
}

#[derive(Debug, Default)]
pub(crate) struct Gate(AtomicU64);
impl Gate {
    pub(crate) fn new() -> Self { Self::default() }
    /// Every accepted hard runtime boundary changes the token, even an ABA or duplicate.
    pub(crate) fn publish(&self, available: bool, blocked: bool) {
        self.change(Some((available, blocked)));
    }
    pub(crate) fn invalidate(&self) { self.change(None); }
    fn change(&self, flags: Option<(bool, bool)>) {
        let _ = self.0.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
            let epoch = old >> EPOCH_SHIFT;
            if epoch == MAX_EPOCH { return Some((MAX_EPOCH << EPOCH_SHIFT) | BLOCKED); }
            let flags = flags.map(|(available, blocked)|
                u64::from(available) * AVAILABLE | u64::from(blocked) * BLOCKED)
                .unwrap_or(old & (AVAILABLE | BLOCKED));
            Some(((epoch + 1) << EPOCH_SHIFT) | flags)
        });
    }
    pub(crate) fn snapshot(&self) -> Option<u64> {
        let state = self.0.load(Ordering::Acquire);
        (state & (AVAILABLE | BLOCKED) == AVAILABLE).then_some(state >> EPOCH_SHIFT)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NotificationPermit(Arc<Permit>);
#[derive(Debug)]
struct Permit {
    gate: Arc<Gate>,
    epoch: u64,
    last_checked: Mutex<Reading>,
    expires_utc: DateTime<Utc>,
    expires_mono_ms: u64,
    revoked: AtomicBool,
}
impl NotificationPermit {
    pub(crate) fn new(gate: Arc<Gate>, epoch: u64, expires_utc: DateTime<Utc>, expires_mono_ms: u64) -> Self {
        Self::new_at(gate, epoch, expires_utc, expires_mono_ms, budget_clock::now())
    }
    fn new_at(gate: Arc<Gate>, epoch: u64, expires_utc: DateTime<Utc>, expires_mono_ms: u64, at: Reading) -> Self {
        Self(Arc::new(Permit { gate, epoch, last_checked: Mutex::new(at),
            expires_utc, expires_mono_ms, revoked: AtomicBool::new(false) }))
    }
    pub(crate) fn revoke(&self) { self.0.revoked.store(true, Ordering::Release); }
    pub(crate) fn is_current(&self) -> bool { self.is_current_at(&budget_clock::now()) }
    pub(crate) fn is_current_at(&self, at: &Reading) -> bool {
        let Ok(mut last) = self.0.last_checked.lock() else { self.revoke(); return false; };
        // Clones share a paired high watermark. A rollback can happen entirely
        // after issuance while COM is preparing, before Core next observes time.
        let current = !self.0.revoked.load(Ordering::Acquire) && self.0.gate.snapshot() == Some(self.0.epoch) &&
            at.utc >= last.utc && at.mono_ms >= last.mono_ms &&
            at.utc < self.0.expires_utc && at.mono_ms < self.0.expires_mono_ms;
        if current { *last = *at; } else { self.revoke(); }
        current
    }
    pub(crate) fn expires_utc(&self) -> DateTime<Utc> { self.0.expires_utc }
}

#[derive(Clone)]
pub(crate) struct QueuedNotification {
    pub(crate) event: Value,
    pub(crate) permit: Option<NotificationPermit>,
}
/// Completion has its existing delivery semantics. Every other reminder domain
/// must supply an explicit live permit, including newly introduced plan kinds.
pub(crate) fn requires_domain_permit(event: &Value) -> bool {
    event["kind"] != "completion"
}
impl QueuedNotification {
    fn current_at(&self, at: &Reading) -> bool {
        match &self.permit {
            // 完成提醒也可带宿主守卫；免打扰往返后不能复活队列里的旧提醒。
            Some(permit) => permit.is_current_at(at),
            None => !requires_domain_permit(&self.event),
        }
    }
    fn id(&self) -> Option<String> { self.event["id"].as_str().map(str::to_owned) }
}

pub(crate) fn discard_invalid(queue: &mut Vec<QueuedNotification>, at: &Reading) -> Vec<String> {
    let mut suppressed = Vec::new();
    queue.retain(|event| {
        if event.current_at(at) { true } else {
            if let Some(id) = event.id() { suppressed.push(id); }
            false
        }
    });
    suppressed
}

const NOTIFICATION_QUEUE_LIMIT: usize = 100;

pub(crate) struct Admission {
    pub(crate) suppressed: Vec<String>,
    /// No prior item survived: newly admitted items need their own merge origin.
    pub(crate) reset_queued_at: bool,
}

/// Prune before capacity decisions. Newly expired budgets never consume a slot.
/// Time is injected; this helper does not touch Instant, SQLite, or the OS.
pub(crate) fn admit_notifications(
    queue: &mut Vec<QueuedNotification>,
    incoming: Vec<QueuedNotification>,
    at: &Reading,
) -> Admission {
    let mut suppressed = discard_invalid(queue, at);
    let reset_queued_at = queue.is_empty();
    for event in incoming {
        if event.current_at(at) && queue.len() < NOTIFICATION_QUEUE_LIMIT {
            queue.push(event);
        } else if let Some(id) = event.id() {
            suppressed.push(id);
        }
    }
    Admission { suppressed, reset_queued_at }
}

#[derive(Default, Debug)]
pub(crate) struct Delivery {
    pub(crate) submitted: Vec<String>,
    pub(crate) suppressed: Vec<String>,
    pub(crate) failed: Vec<String>,
    pub(crate) error: Option<String>,
}

/// 准备阶段可能等待 COM；完成后重新校验所有守卫。
/// 移除失效提醒后重建剩余批次，保留没有守卫的完成提醒兼容行为。
pub(crate) fn deliver_batch<P>(
    events: &[QueuedNotification],
    mut prepare: impl FnMut(&[QueuedNotification]) -> Result<P, String>,
    mut submit: impl FnMut(P) -> Result<(), String>,
    allowed: impl Fn() -> bool,
    reading: impl Fn() -> Reading,
) -> Delivery {
    let mut delivery = Delivery::default();
    let mut remaining = events.to_vec();
    delivery.suppressed.extend(discard_invalid(&mut remaining, &reading()));
    // Each retry removes at least one item. There is no retry of an OS submission.
    for _ in 0..=events.len() {
        if remaining.is_empty() { return delivery; }
        if !allowed() {
            delivery.suppressed.extend(remaining.iter().filter_map(QueuedNotification::id));
            return delivery;
        }
        let prepared = match prepare(&remaining) {
            Ok(prepared) => prepared,
            Err(error) => {
                delivery.suppressed.extend(discard_invalid(&mut remaining, &reading()));
                delivery.failed.extend(remaining.iter().filter_map(QueuedNotification::id));
                if !delivery.failed.is_empty() { delivery.error = Some(error); }
                return delivery;
            }
        };
        if !allowed() {
            delivery.suppressed.extend(remaining.iter().filter_map(QueuedNotification::id));
            return delivery;
        }
        let removed = discard_invalid(&mut remaining, &reading());
        if !removed.is_empty() { delivery.suppressed.extend(removed); continue; }
        // Submission linearization: all original permits were current after COM
        // preparation. A boundary observed later cannot retract an OS submission.
        let ids = remaining.iter().filter_map(QueuedNotification::id);
        match submit(prepared) {
            Ok(()) => delivery.submitted.extend(ids),
            Err(error) => { delivery.failed.extend(ids); delivery.error = Some(error); }
        }
        return delivery;
    }
    delivery.suppressed.extend(remaining.iter().filter_map(QueuedNotification::id));
    delivery
}

pub(crate) fn windows_deadline_ticks(at: DateTime<Utc>) -> Option<i64> {
    at.timestamp().checked_add(11_644_473_600)?.checked_mul(10_000_000)?
        .checked_add(i64::from(at.timestamp_subsec_nanos() / 100))
}

#[cfg(test)]
#[path = "notification_gate_tests.rs"]
mod tests;
