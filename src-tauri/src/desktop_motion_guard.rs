//! Pure renderer-permission and automatic-position provenance owners.
//! Native token generation, input sampling and window movement stay with desktop.

use std::{collections::VecDeque, time::Duration};

use super::roaming::Point;

const HEARTBEAT_LIFETIME: Duration = Duration::from_millis(1_500);
const TAG_LIFETIME: Duration = Duration::from_secs(1);
const MAX_TAGS: usize = 8;
// One retained predecessor permits a failed latest move to restore provenance.
const MAX_AUTO_HISTORY: usize = MAX_TAGS + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Readiness {
    pub ready: bool,
    pub gait_available: bool,
    pub gait_ready: bool,
}
impl Readiness {
    /// 前端状态不能打开未授权的原生扩展入口。
    pub fn restrict_to_feature(self, enabled: bool) -> Self {
        if enabled { self } else { Self { ready: false, gait_available: false, gait_ready: false } }
    }
}

#[derive(Default)]
pub(super) struct PermissionLease {
    renderer_epoch: Option<String>,
    token: Option<String>,
    sequence: Option<u64>,
    accepted_at: Option<Duration>,
    observed_at: Option<Duration>,
    accepted: Option<Readiness>,
    feature_available: bool,
    closed: bool,
}

impl PermissionLease {
    /// The caller provides a new native-generated token after renderer registration.
    /// No permission or advertised feature survives into an unacknowledged renderer.
    pub fn register(&mut self, renderer_epoch: String, token: String, now: Duration) {
        let backwards = self.observed_at.is_some_and(|previous| now < previous);
        self.renderer_epoch = Some(renderer_epoch);
        self.token = Some(token);
        self.sequence = None;
        self.accepted_at = None;
        self.accepted = None;
        self.feature_available = false;
        self.closed = backwards;
        if !backwards {
            self.observed_at = Some(now);
        }
    }

    fn close(&mut self) {
        self.closed = true;
        self.accepted_at = None;
        self.accepted = None;
    }

    fn observe_time(&mut self, now: Duration) -> bool {
        if self.observed_at.is_some_and(|previous| now < previous) {
            self.close();
            return false;
        }
        self.observed_at = Some(now);
        true
    }

    /// Old renderer identities and duplicate/out-of-order messages cannot revoke
    /// a newer renderer's lease. Invalid current messages fail closed instead.
    pub fn accept(
        &mut self,
        renderer_epoch: &str,
        token: &str,
        sequence: u64,
        flags: Readiness,
        now: Duration,
    ) -> bool {
        if self.renderer_epoch.as_deref() != Some(renderer_epoch)
            || self.token.as_deref() != Some(token)
            || self.sequence.is_some_and(|previous| sequence <= previous)
            || self.closed
        {
            return false;
        }
        if !self.observe_time(now) {
            return false;
        }
        if flags.gait_ready && (!flags.ready || !flags.gait_available) {
            self.close();
            return false;
        }
        self.sequence = Some(sequence);
        self.accepted_at = Some(now);
        self.accepted = Some(flags);
        self.feature_available = flags.gait_available;
        true
    }

    pub fn readiness(&mut self, now: Duration) -> Option<Readiness> {
        if !self.observe_time(now) || self.closed {
            return None;
        }
        let age = now.checked_sub(self.accepted_at?)?;
        (age < HEARTBEAT_LIFETIME).then_some(self.accepted?)
    }

    /// Build capability is informational. It never substitutes for a fresh lease.
    pub fn feature_available(&self) -> bool {
        self.feature_available
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MoveOrigin {
    Automatic,
    Stale,
    External,
}

#[derive(Clone, Copy)]
struct AutoPosition {
    serial: u64,
    target: Point,
}

#[derive(Clone, Copy)]
struct PositionTag {
    position: AutoPosition,
    at: Duration,
}

#[derive(Default)]
pub(super) struct PositionProvenance {
    tags: VecDeque<PositionTag>,
    // Bounded history retains the latest automatic point after its tag expires.
    // It also excludes a pre-registered move if the synchronous native call fails.
    auto_history: VecDeque<AutoPosition>,
    observed_at: Option<Duration>,
    serial: u64,
}

impl PositionProvenance {
    fn observe_time(&mut self, now: Duration) -> bool {
        if self.observed_at.is_some_and(|previous| now < previous) {
            return false;
        }
        self.observed_at = Some(now);
        self.tags.retain(|tag| {
            now.checked_sub(tag.at).is_some_and(|age| age <= TAG_LIFETIME)
        });
        true
    }

    /// Register before the synchronous native move. A full live queue refuses
    /// another move; it never discards an outstanding automatic event tag.
    pub fn tag(&mut self, target: Point, now: Duration) -> bool {
        if !self.observe_time(now) || self.tags.len() >= MAX_TAGS {
            return false;
        }
        let Some(serial) = self.serial.checked_add(1) else {
            return false;
        };
        self.serial = serial;
        let position = AutoPosition { serial, target };
        self.tags.push_back(PositionTag { position, at: now });
        self.auto_history.push_back(position);
        if self.auto_history.len() > MAX_AUTO_HISTORY {
            self.auto_history.pop_front();
        }
        true
    }

    /// Remove the newest matching pre-registration after native failure. Older
    /// events for the same coordinate remain independently consumable.
    pub fn failed(&mut self, target: Point) {
        let tagged = self.tags.iter().rev().find(|tag| tag.position.target == target)
            .map(|tag| tag.position.serial);
        let historical = self.auto_history.iter().rev().find(|position| position.target == target)
            .map(|position| position.serial);
        let serial = tagged.into_iter().chain(historical).max();
        if let Some(serial) = serial {
            self.tags.retain(|tag| tag.position.serial != serial);
            self.auto_history.retain(|position| position.serial != serial);
        }
    }

    pub fn observe(
        &mut self,
        event: Point,
        actual: Option<Point>,
        dragging: bool,
        now: Duration,
    ) -> MoveOrigin {
        if dragging {
            self.clear();
            self.observed_at = Some(now);
            return MoveOrigin::External;
        }
        let forward = self.observe_time(now);
        if let Some(index) = self.tags.iter().position(|tag| tag.position.target == event) {
            self.tags.remove(index);
            return MoveOrigin::Automatic;
        }
        if actual != Some(event) || !forward {
            return MoveOrigin::Stale;
        }
        if self.auto_history.back().is_some_and(|position| position.target == event) {
            return MoveOrigin::Automatic;
        }
        self.clear();
        self.observed_at = Some(now);
        MoveOrigin::External
    }

    pub fn clear(&mut self) {
        self.tags.clear();
        self.auto_history.clear();
        self.observed_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_flags_cannot_open_a_closed_native_feature() {
        let ready = Readiness { ready: true, gait_available: true, gait_ready: true };
        assert_eq!(ready.restrict_to_feature(true), ready);
        let closed = ready.restrict_to_feature(false);
        assert_eq!(closed, Readiness { ready: false, gait_available: false, gait_ready: false });
        let mut lease = PermissionLease::default();
        lease.register("renderer".into(), "token".into(), Duration::ZERO);
        assert!(lease.accept("renderer", "token", 1, closed, Duration::ZERO));
        assert!(!lease.feature_available());
        assert_eq!(lease.readiness(Duration::ZERO), Some(closed));
    }

    fn ms(value: u64) -> Duration { Duration::from_millis(value) }
    fn point(x: i32) -> Point { Point { x, y: -40 } }
    const READY: Readiness = Readiness { ready: true, gait_available: true, gait_ready: true };

    #[test]
    fn lease_is_finite_and_expired_capability_is_only_informational() {
        let mut lease = PermissionLease::default();
        assert!(lease.readiness(ms(0)).is_none());
        assert!(!lease.feature_available());
        lease.register("renderer-a".into(), "native-a".into(), ms(1));
        assert!(lease.readiness(ms(1)).is_none());
        assert!(lease.accept("renderer-a", "native-a", 0, READY, ms(10)));
        assert_eq!(lease.readiness(ms(1_509)), Some(READY));
        assert!(lease.readiness(ms(1_510)).is_none());
        assert!(lease.feature_available());
        assert!(lease.accept("renderer-a", "native-a", 1, READY, ms(1_600)));
        assert_eq!(lease.readiness(ms(1_600)), Some(READY));
    }

    #[test]
    fn renderer_replacement_and_stale_messages_cannot_regrant_or_revoke_new_permission() {
        let mut lease = PermissionLease::default();
        lease.register("renderer-a".into(), "native-a".into(), ms(0));
        assert!(lease.accept("renderer-a", "native-a", 90, READY, ms(1)));
        lease.register("renderer-b".into(), "native-b".into(), ms(100));
        assert!(lease.readiness(ms(100)).is_none());
        assert!(!lease.feature_available());
        assert!(!lease.accept("renderer-a", "native-a", 91, READY, ms(101)));
        assert!(lease.accept("renderer-b", "native-b", 1, READY, ms(102)));
        let invalid = Readiness { ready: false, ..READY };
        assert!(!lease.accept("renderer-a", "native-a", 92, invalid, ms(1)));
        assert!(!lease.accept("renderer-b", "native-a", 2, invalid, ms(1)));
        assert!(!lease.accept("renderer-b", "native-b", 1, invalid, ms(1)));
        assert!(!lease.accept("renderer-b", "native-b", 0, invalid, ms(1)));
        assert_eq!(lease.readiness(ms(103)), Some(READY));
        assert!(lease.accept("renderer-b", "native-b", 2, READY, ms(104)));
    }

    #[test]
    fn invalid_current_gait_flags_latch_the_token_until_registration() {
        for invalid in [
            Readiness { ready: false, ..READY },
            Readiness { gait_available: false, ..READY },
        ] {
            let mut lease = PermissionLease::default();
            lease.register("renderer".into(), "native-a".into(), ms(0));
            assert!(lease.accept("renderer", "native-a", 1, READY, ms(1)));
            assert!(!lease.accept("renderer", "native-a", 2, invalid, ms(2)));
            assert!(lease.readiness(ms(3)).is_none());
            assert!(!lease.accept("renderer", "native-a", 3, READY, ms(4)));
            lease.register("renderer".into(), "native-b".into(), ms(5));
            let stopped = Readiness { ready: false, gait_available: true, gait_ready: false };
            assert!(lease.accept("renderer", "native-b", 0, stopped, ms(6)));
            assert_eq!(lease.readiness(ms(6)), Some(stopped));
            assert!(lease.feature_available());
            let fallback = Readiness { ready: true, gait_available: false, gait_ready: false };
            assert!(lease.accept("renderer", "native-b", 1, fallback, ms(7)));
            assert!(!lease.feature_available());
        }
    }

    #[test]
    fn backwards_time_closes_current_permission_and_registration_cannot_erase_the_high_watermark() {
        for rollback_by_read in [false, true] {
            let mut lease = PermissionLease::default();
            lease.register("renderer".into(), "native-a".into(), ms(100));
            assert!(lease.accept("renderer", "native-a", 1, READY, ms(110)));
            if rollback_by_read {
                assert!(lease.readiness(ms(109)).is_none());
            } else {
                assert!(!lease.accept("renderer", "native-a", 2, READY, ms(109)));
            }
            assert!(!lease.accept("renderer", "native-a", 3, READY, ms(111)));
            lease.register("renderer".into(), "native-b".into(), ms(109));
            assert!(!lease.accept("renderer", "native-b", 1, READY, ms(112)));
            lease.register("renderer".into(), "native-c".into(), ms(113));
            assert!(lease.accept("renderer", "native-c", 0, READY, ms(114)));
        }
    }

    #[test]
    fn eight_live_tags_are_kept_until_consumed_or_older_than_one_second() {
        let mut provenance = PositionProvenance::default();
        for x in 0..8 {
            assert!(provenance.tag(point(x), ms(0)));
        }
        assert!(!provenance.tag(point(8), ms(1_000)));
        assert_eq!(provenance.tags.len(), 8);
        assert_eq!(provenance.observe(point(2), Some(point(7)), false, ms(1_000)), MoveOrigin::Automatic);
        assert!(provenance.tag(point(8), ms(1_000)));
        assert!(!provenance.tag(point(9), ms(1_000)));
        assert!(provenance.tag(point(9), ms(1_001)));
        assert_eq!(provenance.tags.len(), 2);
        assert!(provenance.auto_history.len() <= MAX_AUTO_HISTORY);
    }

    #[test]
    fn automatic_tags_cover_out_of_order_and_latest_late_events_without_persisting_stale_events() {
        let mut provenance = PositionProvenance::default();
        assert!(provenance.tag(point(1), ms(0)));
        assert!(provenance.tag(point(2), ms(10)));
        assert_eq!(provenance.observe(point(2), Some(point(2)), false, ms(20)), MoveOrigin::Automatic);
        assert_eq!(provenance.observe(point(1), Some(point(2)), false, ms(30)), MoveOrigin::Automatic);
        assert_eq!(provenance.observe(point(1), Some(point(2)), false, ms(40)), MoveOrigin::Stale);
        assert_eq!(provenance.observe(point(2), Some(point(2)), false, ms(2_000)), MoveOrigin::Automatic);
        assert_eq!(provenance.observe(point(2), None, false, ms(2_001)), MoveOrigin::Stale);
        assert!(provenance.tag(point(3), ms(2_010)));
        assert_eq!(provenance.observe(point(3), Some(point(3)), false, ms(3_011)), MoveOrigin::Automatic);
        assert_eq!(provenance.observe(point(2), Some(point(3)), false, ms(3_012)), MoveOrigin::Stale);
    }

    #[test]
    fn dragging_and_external_actual_positions_clear_automatic_provenance() {
        let mut provenance = PositionProvenance::default();
        assert!(provenance.tag(point(1), ms(0)));
        assert_eq!(provenance.observe(point(1), Some(point(1)), true, ms(1)), MoveOrigin::External);
        assert!(provenance.tags.is_empty());
        assert!(provenance.auto_history.is_empty());
        assert!(provenance.tag(point(2), ms(2)));
        assert_eq!(provenance.observe(point(99), Some(point(99)), false, ms(3)), MoveOrigin::External);
        assert!(provenance.tags.is_empty());
        assert!(provenance.auto_history.is_empty());
        assert_eq!(provenance.observe(point(2), Some(point(99)), false, ms(4)), MoveOrigin::Stale);
        assert_eq!(provenance.observe(point(2), Some(point(2)), false, ms(5)), MoveOrigin::External);
    }

    #[test]
    fn failed_moves_remove_their_tag_and_restore_the_previous_automatic_point() {
        let mut provenance = PositionProvenance::default();
        assert!(provenance.tag(point(1), ms(0)));
        assert_eq!(provenance.observe(point(1), Some(point(1)), false, ms(1)), MoveOrigin::Automatic);
        assert!(provenance.tag(point(2), ms(2)));
        provenance.failed(point(2));
        assert_eq!(provenance.observe(point(2), Some(point(1)), false, ms(3)), MoveOrigin::Stale);
        assert_eq!(provenance.observe(point(1), Some(point(1)), false, ms(2_000)), MoveOrigin::Automatic);
        assert!(provenance.tag(point(3), ms(2_001)));
        assert!(provenance.tag(point(3), ms(2_002)));
        provenance.failed(point(3));
        assert_eq!(provenance.tags.len(), 1);
        assert_eq!(provenance.observe(point(3), Some(point(3)), false, ms(2_003)), MoveOrigin::Automatic);
        assert!(provenance.tags.is_empty());
        assert!(provenance.tag(point(4), ms(2_004)));
        assert_eq!(provenance.observe(point(4), Some(point(4)), false, ms(2_005)), MoveOrigin::Automatic);
        provenance.failed(point(4));
        assert_eq!(provenance.observe(point(3), Some(point(3)), false, ms(4_000)), MoveOrigin::Automatic);
    }

    #[test]
    fn position_clock_rollback_refuses_moves_and_never_marks_unknown_positions_external() {
        let mut provenance = PositionProvenance::default();
        assert!(provenance.tag(point(1), ms(100)));
        assert!(!provenance.tag(point(2), ms(99)));
        assert_eq!(provenance.observe(point(9), Some(point(9)), false, ms(99)), MoveOrigin::Stale);
        assert_eq!(provenance.observe(point(1), Some(point(1)), false, ms(98)), MoveOrigin::Automatic);
        assert_eq!(provenance.observe(point(1), Some(point(1)), false, ms(99)), MoveOrigin::Stale);
        assert_eq!(provenance.observe(point(1), Some(point(1)), false, ms(101)), MoveOrigin::Automatic);
        provenance.clear();
        assert!(provenance.tags.is_empty());
        assert!(provenance.auto_history.is_empty());
        assert_eq!(provenance.observe(point(1), Some(point(1)), false, ms(102)), MoveOrigin::External);
    }
}
