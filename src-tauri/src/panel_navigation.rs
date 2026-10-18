//! One short-lived, in-memory user navigation intent. No window or ledger access.
use std::time::{Duration, Instant};

pub const LIFETIME: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Domain {
    pub day: String,
    pub runtime_epoch: String,
    pub source_scope_revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub revision: u64,
    pub domain: Domain,
    deadline: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub revision: u64,
    deadline: Instant,
}

impl Ticket {
    pub fn remaining(&self, now: Instant) -> Duration {
        self.deadline.saturating_duration_since(now)
    }
}

impl Pending {
    pub fn remaining_ms(&self, now: Instant) -> u64 {
        self.deadline.saturating_duration_since(now).as_millis() as u64
    }
}

#[derive(Default)]
pub struct Mailbox {
    watermark: u64,
    pending: Option<Pending>,
}

impl Mailbox {
    pub fn watermark(&self) -> u64 {
        self.watermark
    }

    fn advance(&mut self) -> Result<u64, String> {
        let Some(next) = self.watermark.checked_add(1) else {
            self.pending = None;
            return Err("面板导航序号已耗尽，请重新启动应用".into());
        };
        self.watermark = next;
        Ok(next)
    }

    pub fn reserve(&mut self, now: Instant) -> Result<Ticket, String> {
        let deadline = now.checked_add(LIFETIME).ok_or("面板导航期限无效")?;
        let revision = self.advance()?;
        self.pending = None;
        Ok(Ticket { revision, deadline })
    }

    pub fn commit(&mut self, ticket: &Ticket, domain: Domain, now: Instant) -> bool {
        if self.watermark != ticket.revision || now >= ticket.deadline {
            return false;
        }
        self.pending = Some(Pending { revision: ticket.revision, domain, deadline: ticket.deadline });
        true
    }

    pub fn is_current(&mut self, ticket: &Ticket, now: Instant) -> bool {
        self.watermark == ticket.revision && now < ticket.deadline
            && self.snapshot(now).is_some_and(|p| p.revision == ticket.revision)
    }

    #[cfg(test)]
    pub fn publish(&mut self, domain: Domain, now: Instant) -> Result<u64, String> {
        let ticket = self.reserve(now)?;
        assert!(self.commit(&ticket, domain, now));
        Ok(ticket.revision)
    }

    pub fn snapshot(&mut self, now: Instant) -> Option<Pending> {
        if self.pending.as_ref().is_some_and(|p| now >= p.deadline) {
            self.pending = None;
        }
        self.pending.clone()
    }

    /// ACK/failure/domain invalidation of one intent must not erase its successor.
    pub fn clear_if(&mut self, revision: u64) -> bool {
        if self.pending.as_ref().is_some_and(|p| p.revision == revision) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    pub fn ack(&mut self, revision: u64, now: Instant) -> bool {
        self.snapshot(now);
        self.clear_if(revision)
    }

    /// User close/manual navigation establishes a fence even when the slot is empty.
    pub fn cancel(&mut self) -> Result<u64, String> {
        self.pending = None;
        self.advance()
    }
}

pub fn revision(value: &str) -> Result<u64, String> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|c| c.is_ascii_digit()) {
        return Err("面板导航序号无效".into());
    }
    let parsed: u64 = value.parse().map_err(|_| "面板导航序号超出范围")?;
    if parsed == 0 || parsed.to_string() != value {
        return Err("面板导航序号无效".into());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(source: &str) -> Domain {
        Domain { day: "2026-10-17".into(), runtime_epoch: "runtime".into(),
            source_scope_revision: source.into() }
    }

    #[test]
    fn new_mailbox_is_empty_and_peek_is_not_a_request() {
        let mut slot = Mailbox::default();
        assert!(slot.snapshot(Instant::now()).is_none());
        assert_eq!(slot.watermark(), 0);
    }

    #[test]
    fn latest_request_replaces_one_slot_and_preserves_domain() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let old = slot.publish(domain("old"), now).unwrap();
        let new = slot.publish(domain("new"), now).unwrap();
        assert!(new > old);
        let pending = slot.snapshot(now).unwrap();
        assert_eq!(pending.revision, new);
        assert_eq!(pending.domain, domain("new"));
        assert!(!slot.ack(old, now));
        assert_eq!(slot.snapshot(now).unwrap().revision, new);
    }

    #[test]
    fn original_exclusive_deadline_is_never_renewed_by_peek() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let id = slot.publish(domain("one"), now).unwrap();
        let first = slot.snapshot(now).unwrap();
        assert_eq!(first.remaining_ms(now), 10_000);
        let late = now + Duration::from_millis(9_999);
        assert_eq!(slot.snapshot(late).unwrap().remaining_ms(late), 1);
        assert!(slot.snapshot(now + LIFETIME).is_none());
        assert!(!slot.ack(id, now + LIFETIME));
        assert_eq!(slot.watermark(), id);
    }

    #[test]
    fn repeated_ack_and_wake_are_idempotent() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let id = slot.publish(domain("one"), now).unwrap();
        assert_eq!(slot.snapshot(now), slot.snapshot(now));
        assert!(slot.ack(id, now));
        assert!(!slot.ack(id, now));
        assert!(slot.snapshot(now).is_none());
        assert_eq!(slot.watermark(), id);
    }

    #[test]
    fn close_or_manual_cancel_fences_outstanding_responses() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let id = slot.publish(domain("one"), now).unwrap();
        let cancelled = slot.cancel().unwrap();
        assert!(cancelled > id);
        assert!(slot.snapshot(now).is_none());
        assert!(!slot.ack(id, now));
        let newer = slot.publish(domain("two"), now).unwrap();
        assert!(newer > cancelled);
        assert!(!slot.clear_if(id));
        assert_eq!(slot.snapshot(now).unwrap().revision, newer);
    }

    #[test]
    fn cancellation_of_empty_slot_still_advances_the_watermark() {
        let mut slot = Mailbox::default();
        assert_eq!(slot.cancel().unwrap(), 1);
        assert_eq!(slot.cancel().unwrap(), 2);
        assert!(slot.snapshot(Instant::now()).is_none());
    }

    #[test]
    fn late_show_failure_cannot_remove_the_new_intent() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let failed_show = slot.publish(domain("one"), now).unwrap();
        let new = slot.publish(domain("two"), now).unwrap();
        assert!(!slot.clear_if(failed_show));
        assert_eq!(slot.snapshot(now).unwrap().revision, new);
    }

    #[test]
    fn decimal_revisions_retain_values_above_javascript_safe_integer() {
        let now = Instant::now();
        let mut slot = Mailbox { watermark: 9_007_199_254_740_991, pending: None };
        let id = slot.publish(domain("one"), now).unwrap();
        assert_eq!(id.to_string(), "9007199254740992");
        assert_eq!(revision(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn noncanonical_or_overflowing_ack_revisions_are_rejected() {
        for value in ["", "0", "01", "+1", "-1", "1.0", " 1", "1 ",
            "١", "18446744073709551616"] {
            assert!(revision(value).is_err(), "{value}");
        }
        assert_eq!(revision("18446744073709551615").unwrap(), u64::MAX);
    }

    #[test]
    fn exhausted_revision_fails_closed_without_wrapping() {
        let now = Instant::now();
        let mut slot = Mailbox { watermark: u64::MAX - 1, pending: None };
        assert_eq!(slot.publish(domain("one"), now).unwrap(), u64::MAX);
        assert!(slot.publish(domain("two"), now).is_err());
        assert!(slot.snapshot(now).is_none());
        assert_eq!(slot.watermark(), u64::MAX);
        assert!(slot.cancel().is_err());
        assert!(slot.snapshot(now).is_none());
    }

    #[test]
    fn cancelled_unpublished_worker_cannot_publish_or_show_later() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let waiting_for_core = slot.reserve(now).unwrap();
        slot.cancel().unwrap();
        assert!(!slot.commit(&waiting_for_core, domain("old"), now));
        assert!(!slot.is_current(&waiting_for_core, now));
        assert!(slot.snapshot(now).is_none());
    }

    #[test]
    fn a_late_worker_cannot_replace_a_newer_reserved_or_committed_intent() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let old = slot.reserve(now).unwrap();
        let new = slot.reserve(now).unwrap();
        assert!(!slot.commit(&old, domain("old"), now));
        assert!(slot.commit(&new, domain("new"), now));
        assert!(!slot.commit(&old, domain("old"), now));
        assert!(!slot.is_current(&old, now));
        assert!(slot.is_current(&new, now));
        assert_eq!(slot.snapshot(now).unwrap().domain, domain("new"));
    }

    #[test]
    fn validation_delay_consumes_the_original_request_lifetime() {
        let now = Instant::now();
        let mut slot = Mailbox::default();
        let ticket = slot.reserve(now).unwrap();
        assert!(slot.commit(&ticket, domain("one"), now + Duration::from_secs(9)));
        assert_eq!(ticket.remaining(now + Duration::from_secs(9)), Duration::from_secs(1));
        assert_eq!(slot.snapshot(now + Duration::from_secs(9)).unwrap()
            .remaining_ms(now + Duration::from_secs(9)), 1000);
        assert!(!slot.is_current(&ticket, now + LIFETIME));
        assert!(!slot.commit(&ticket, domain("one"), now + LIFETIME));
    }
}
