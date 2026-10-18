//! Pure horizontal movement policy. No window, input, ledger or timer ownership.
//!
//! The caller supplies an aggregate gate: Allowed means every applicable gate is
//! positively known true, including inactivity and absence of user interaction.
//! `epoch` is a caller-owned monotonic authorization epoch, not a UUID hash; bump
//! it when the authoritative runtime changes. Generation/revision identify a new
//! movement request, not routine informational desktop-state revisions.
//!
//! Call step at most 100ms apart to obtain the nominal 40 logical px/s speed.
//! Slower ticks discard excess elapsed time; they never catch up. A segment's
//! requested wall-clock duration (capped at eight seconds) is never extended by
//! observations or apply. A shorter caller-owned activity lease can cancel it.
//! Coordinates are physical integer pixels; rounding may conservatively reduce
//! speed at unusual scale factors. The existing HD poses do not provide a gait.
//!
//! An Intent cannot directly expose an executable target. The root must acquire
//! fresh observations and call apply_if_current synchronously in its serialized
//! movement owner. Its callback performs the actual native move and returns true
//! only on success. Do not enqueue/await a target after that callback is entered:
//! this pure module cannot fence an unrelated later OS operation or prove native
//! input-stop latency. Automatic positions must not become the user's anchor.

use std::time::Duration;

const SEGMENT_LIMIT: Duration = Duration::from_secs(8);
const STEP_LIMIT: Duration = Duration::from_millis(100);
const SPEED: f64 = 40.0;
const NEARBY_RADIUS: f64 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Point {
    pub x: i32,
    pub y: i32,
}

/// Physical work-area edges, right and bottom exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    pub work_area: Bounds,
    pub window_width: u32,
    pub window_height: u32,
    pub scale_factor: f64,
    /// Caller changes this on topology, window-size or geometry-owner changes.
    pub signature: u64,
}

impl Geometry {
    fn contains(self, point: Point) -> bool {
        let b = self.work_area;
        self.window_width > 0
            && self.window_height > 0
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
            && b.left < b.right
            && b.top < b.bottom
            && point.x >= b.left
            && point.y >= b.top
            && i64::from(point.x) + i64::from(self.window_width) <= i64::from(b.right)
            && i64::from(point.y) + i64::from(self.window_height) <= i64::from(b.bottom)
    }

    fn horizontal_limits(self, request: Request, position: Point) -> Option<(i32, i32)> {
        if !self.contains(position) || !self.contains(request.anchor)
            || position.y != request.anchor.y
        {
            return None;
        }
        let mut left = self.work_area.left;
        let mut right = i32::try_from(
            i64::from(self.work_area.right) - i64::from(self.window_width),
        ).ok()?;
        if request.range == Range::Nearby {
            let radius = NEARBY_RADIUS * self.scale_factor;
            if !radius.is_finite() {
                return None;
            }
            left = (f64::from(request.anchor.x) - radius).ceil().max(f64::from(left)) as i32;
            right = (f64::from(request.anchor.x) + radius).floor().min(f64::from(right)) as i32;
        }
        (left < right && position.x >= left && position.x <= right).then_some((left, right))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
}

impl Direction {
    fn reverse(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Choose a direction with room for the full window before publishing a gait.
/// Work-area edges are physical coordinates and may be negative. This does not
/// grant movement permission or change an existing segment's frozen request.
pub(crate) fn choose_inward_direction(
    geometry: Geometry,
    position: Point,
    preferred: Direction,
) -> Option<Direction> {
    if !geometry.contains(position) {
        return None;
    }
    let left = geometry.work_area.left;
    let right = i32::try_from(
        i64::from(geometry.work_area.right) - i64::from(geometry.window_width),
    ).ok()?;
    if left >= right {
        return None;
    }
    Some(if position.x == left {
        Direction::Right
    } else if position.x == right {
        Direction::Left
    } else {
        preferred
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Range {
    Nearby,
    WorkArea,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Allowed,
    Blocked,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub epoch: u64,
    pub generation: u64,
    pub revision: u64,
    /// Requested wall-clock segment duration; zero is refused, over 8s is capped.
    pub duration: Duration,
    pub anchor: Point,
    pub range: Range,
    pub direction: Direction,
}

impl Request {
    fn key(self) -> (u64, u64, u64) {
        (self.epoch, self.generation, self.revision)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Observation {
    pub now: Duration,
    pub gate: Gate,
    /// Opaque native last-input marker. Missing is unknown, not unchanged input.
    pub input_marker: Option<u32>,
    pub request: Option<Request>,
    pub geometry: Option<Geometry>,
    pub position: Option<Point>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MoveIntent {
    serial: u64,
    key: (u64, u64, u64),
    created_at: Duration,
    duration: Duration,
    target: Point,
    direction: Direction,
    next_direction: Direction,
    next_fraction: f64,
}

impl MoveIntent {
    /// Effective movement time, never more than 100ms; not native call duration.
    pub fn duration(self) -> Duration { self.duration }
    pub fn direction(self) -> Direction { self.direction }
    pub fn request_key(self) -> (u64, u64, u64) { self.key }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Applied,
    Rejected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SegmentState {
    pub request_key: (u64, u64, u64),
    /// Direction for the next step; reverses only after an edge move is applied.
    pub direction: Direction,
    pub expected_position: Point,
    pub elapsed: Duration,
    pub remaining: Duration,
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    request: Request,
    geometry: Geometry,
    input_marker: u32,
    expected: Point,
    left: i32,
    right: i32,
    direction: Direction,
    started_at: Duration,
    limit: Duration,
    stepped_at: Duration,
    fraction: f64,
}

impl Segment {
    fn matches(self, observation: Observation) -> bool {
        observation.gate == Gate::Allowed
            && observation.request == Some(self.request)
            && observation.geometry == Some(self.geometry)
            && observation.input_marker == Some(self.input_marker)
            && observation.position == Some(self.expected)
            && self.geometry.contains(self.expected)
            && observation.now >= self.started_at
            && observation.now - self.started_at < self.limit
    }
}

#[derive(Default)]
pub(crate) struct Planner {
    segment: Option<Segment>,
    pending: Option<MoveIntent>,
    watermark: Option<(u64, u64, u64)>,
    observed_at: Option<Duration>,
    serial: u64,
    exhausted: bool,
}

impl Planner {
    /// Cancels the current authorization, including any outstanding intent.
    /// The same or an older request cannot start again after cancellation.
    pub fn cancel(&mut self) {
        self.segment = None;
        self.pending = None;
    }

    fn time_is_forward(&mut self, now: Duration) -> bool {
        if self.observed_at.is_some_and(|old| now < old) {
            self.cancel();
            return false;
        }
        self.observed_at = Some(now);
        true
    }

    pub fn state(&self) -> Option<SegmentState> {
        let segment = self.segment?;
        let elapsed = self.observed_at?.checked_sub(segment.started_at)?;
        Some(SegmentState {
            request_key: segment.request.key(),
            direction: segment.direction,
            expected_position: segment.expected,
            elapsed,
            remaining: segment.limit.saturating_sub(elapsed),
        })
    }

    pub fn step(&mut self, observation: Observation) -> Option<MoveIntent> {
        let newer = observation.request.is_some_and(|r| {
            self.watermark.is_none_or(|old| r.key() > old)
        });
        if newer {
            self.cancel();
            self.watermark = observation.request.map(Request::key);
        }
        if !self.time_is_forward(observation.now) || self.exhausted {
            self.cancel();
            return None;
        }
        let (Some(request), Some(geometry), Some(position), Some(input_marker)) = (
            observation.request, observation.geometry, observation.position,
            observation.input_marker,
        ) else {
            self.cancel();
            return None;
        };
        if observation.gate != Gate::Allowed || self.watermark != Some(request.key()) {
            self.cancel();
            return None;
        }
        if newer {
            if request.duration.is_zero() {
                return None;
            }
            let Some((left, right)) = geometry.horizontal_limits(request, position) else {
                return None;
            };
            self.segment = Some(Segment {
                request, geometry, input_marker, expected: position, left, right,
                direction: request.direction, started_at: observation.now,
                limit: request.duration.min(SEGMENT_LIMIT),
                stepped_at: observation.now, fraction: 0.0,
            });
            return None;
        }
        let mut segment = self.segment?;
        if !segment.matches(observation) {
            self.cancel();
            return None;
        }
        // An unacknowledged intent is neither renewed nor replaced by a tick.
        if self.pending.is_some() {
            return None;
        }
        let duration = (observation.now - segment.stepped_at).min(STEP_LIMIT);
        segment.stepped_at = observation.now;
        let budget = SPEED * segment.geometry.scale_factor * duration.as_secs_f64();
        let cap = (SPEED * segment.geometry.scale_factor * STEP_LIMIT.as_secs_f64()).floor();
        if !budget.is_finite() || !cap.is_finite() {
            self.cancel();
            return None;
        }
        let pixels = (budget + segment.fraction).floor().min(cap);
        let fraction = (budget + segment.fraction - pixels).clamp(0.0, 0.999_999_999);
        if pixels < 1.0 {
            segment.fraction = fraction;
            self.segment = Some(segment);
            return None;
        }
        let mut direction = segment.direction;
        if (direction == Direction::Left && segment.expected.x == segment.left)
            || (direction == Direction::Right && segment.expected.x == segment.right)
        {
            direction = direction.reverse();
        }
        let room = match direction {
            Direction::Left => i64::from(segment.expected.x) - i64::from(segment.left),
            Direction::Right => i64::from(segment.right) - i64::from(segment.expected.x),
        };
        let distance = pixels.min(room as f64) as i64;
        let x = match direction {
            Direction::Left => i64::from(segment.expected.x) - distance,
            Direction::Right => i64::from(segment.expected.x) + distance,
        };
        let target = Point { x: i32::try_from(x).ok()?, y: segment.expected.y };
        let Some(serial) = self.serial.checked_add(1) else {
            self.cancel();
            self.exhausted = true;
            return None;
        };
        self.serial = serial;
        let intent = MoveIntent {
            serial, key: request.key(), created_at: observation.now, duration,
            target, direction,
            next_direction: if target.x == segment.left || target.x == segment.right {
                direction.reverse()
            } else { direction },
            // Do not carry unused boundary distance into a later tick.
            next_fraction: if distance < pixels as i64 { 0.0 } else { fraction },
        };
        self.segment = Some(segment);
        self.pending = Some(intent);
        Some(intent)
    }

    /// Consumes one exact pending intent after a fresh execution-time recheck.
    /// Rejected intents never call `apply`. `apply` must execute synchronously;
    /// true means the native operation succeeded, not that UI/latency is proven.
    pub fn apply_if_current(
        &mut self,
        intent: MoveIntent,
        observation: Observation,
        apply: impl FnOnce(Point) -> bool,
    ) -> ApplyOutcome {
        // Even a stale-token call carries an observed stop condition. Recheck
        // the current segment first so unknown/closed gates cannot leave it live.
        if !self.time_is_forward(observation.now)
            || !self.segment.is_some_and(|s| s.matches(observation))
        {
            self.cancel();
            return ApplyOutcome::Rejected;
        }
        if self.pending != Some(intent) {
            return ApplyOutcome::Rejected;
        }
        if observation.now.checked_sub(intent.created_at).is_none_or(|age| age > STEP_LIMIT)
        {
            self.cancel();
            return ApplyOutcome::Rejected;
        }
        // Take first: a failed or unwinding native callback cannot leave a live
        // segment or replayable intent behind. Root supplies any native action.
        let mut segment = self.segment.take().expect("checked segment");
        self.pending = None;
        if !apply(intent.target) {
            return ApplyOutcome::Failed;
        }
        segment.expected = intent.target;
        segment.direction = intent.next_direction;
        segment.fraction = intent.next_fraction;
        self.segment = Some(segment);
        ApplyOutcome::Applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(ms: u64) -> Observation {
        let anchor = Point { x: 500, y: 300 };
        Observation {
            now: Duration::from_millis(ms), gate: Gate::Allowed,
            input_marker: Some(7), position: Some(anchor),
            request: Some(Request { epoch: 1, generation: 1, revision: 1, anchor,
                duration: SEGMENT_LIMIT,
                range: Range::Nearby, direction: Direction::Right }),
            geometry: Some(Geometry {
                work_area: Bounds { left: 0, top: 0, right: 1600, bottom: 1000 },
                window_width: 200, window_height: 300, scale_factor: 1.0, signature: 1,
            }),
        }
    }

    fn apply(planner: &mut Planner, observation: &mut Observation) -> Option<MoveIntent> {
        let intent = planner.step(*observation)?;
        let mut target = None;
        assert_eq!(planner.apply_if_current(intent, *observation, |p| {
            target = Some(p); true
        }), ApplyOutcome::Applied);
        observation.position = target;
        Some(intent)
    }

    #[test]
    fn closed_or_unknown_authority_never_starts_or_resumes_the_same_request() {
        for gate in [Gate::Blocked, Gate::Unknown] {
            let mut planner = Planner::default();
            let mut o = observation(0); o.gate = gate;
            assert!(planner.step(o).is_none());
            o.gate = Gate::Allowed; o.now = Duration::from_millis(100);
            assert!(planner.step(o).is_none()); assert!(planner.state().is_none());
            o.request.as_mut().unwrap().revision += 1;
            assert!(planner.step(o).is_none());
            o.now += STEP_LIMIT; assert!(apply(&mut planner, &mut o).is_some());
        }
    }

    #[test]
    fn negative_monitor_and_dpi_preserve_the_full_window_and_nearby_radius() {
        let mut planner = Planner::default(); let mut o = observation(0);
        let anchor = Point { x: -1200, y: -500 };
        o.position = Some(anchor); o.request.as_mut().unwrap().anchor = anchor;
        o.geometry = Some(Geometry { work_area: Bounds { left: -1920, top: -900,
            right: 0, bottom: 300 }, window_width: 420, window_height: 510,
            scale_factor: 1.5, signature: 9 });
        planner.step(o); let mut reversed = false;
        for n in 1..80 {
            o.now = Duration::from_millis(n * 100);
            if let Some(intent) = apply(&mut planner, &mut o) {
                assert!(intent.duration() <= STEP_LIMIT);
                reversed |= intent.direction() == Direction::Left;
            }
            let point = o.position.unwrap();
            assert!(o.geometry.unwrap().contains(point));
            assert!((i64::from(point.x) - i64::from(anchor.x)).abs() <= 240);
            assert_eq!(point.y, anchor.y);
        }
        assert!(reversed);
    }

    #[test]
    fn work_area_mode_extends_the_route_beyond_the_nearby_anchor_band() {
        let mut maxima = Vec::new();
        for range in [Range::Nearby, Range::WorkArea] {
            let mut planner = Planner::default(); let mut o = observation(0);
            o.request.as_mut().unwrap().range = range; planner.step(o);
            let mut maximum = 500;
            for n in 1..80 {
                o.now = Duration::from_millis(n * 100); apply(&mut planner, &mut o);
                maximum = maximum.max(o.position.unwrap().x);
            }
            maxima.push(maximum);
        }
        assert_eq!(maxima[0], 660); assert!(maxima[1] > 660);
    }

    #[test]
    fn long_scheduler_pause_discards_catch_up_and_does_not_extend_eight_seconds() {
        let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
        o.now = Duration::from_secs(5); let intent = apply(&mut planner, &mut o).unwrap();
        assert_eq!(intent.duration(), STEP_LIMIT); assert_eq!(o.position.unwrap().x, 504);
        o.now = Duration::from_secs(8); assert!(planner.step(o).is_none());
        assert!(planner.state().is_none());
        o.now = Duration::from_secs(20); assert!(planner.step(o).is_none());
        for duration in [Duration::from_millis(2400), Duration::from_secs(60)] {
            let mut planner = Planner::default(); let mut o = observation(0);
            o.request.as_mut().unwrap().duration = duration; planner.step(o);
            let limit = duration.min(SEGMENT_LIMIT);
            assert_eq!(planner.state().unwrap().remaining, limit);
            o.now = limit; assert!(planner.step(o).is_none());
            assert!(planner.state().is_none());
        }
    }

    #[test]
    fn repeated_revision_does_not_renew_a_segment_or_reopen_a_completed_one() {
        let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
        for n in 1..=80 {
            o.now = Duration::from_millis(n * 100); apply(&mut planner, &mut o);
        }
        assert!(planner.state().is_none());
        o.now += STEP_LIMIT; assert!(planner.step(o).is_none());
        o.request.as_mut().unwrap().revision += 1;
        planner.step(o); assert_eq!(planner.state().unwrap().elapsed, Duration::ZERO);
    }

    #[test]
    fn input_change_missing_input_and_gate_changes_reject_the_pending_native_call() {
        for mode in 0..4 {
            let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
            o.now += STEP_LIMIT; let intent = planner.step(o).unwrap();
            match mode { 0 => o.input_marker = Some(8), 1 => o.input_marker = None,
                2 => o.gate = Gate::Blocked, _ => o.gate = Gate::Unknown }
            assert_eq!(planner.apply_if_current(intent, o, |_| panic!("late move")), ApplyOutcome::Rejected);
            assert!(planner.state().is_none());
            o.gate = Gate::Allowed; o.input_marker = Some(7); o.now += STEP_LIMIT;
            assert!(planner.step(o).is_none());
        }
    }

    #[test]
    fn monotonic_authority_replacement_fences_old_epoch_generation_and_revision() {
        for mode in 0..3 {
            let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
            o.now += STEP_LIMIT; let old_intent = planner.step(o).unwrap(); let old = o;
            let request = o.request.as_mut().unwrap();
            match mode { 0 => request.epoch += 1, 1 => request.generation += 1, _ => request.revision += 1 }
            planner.step(o); assert!(planner.state().is_some());
            assert_eq!(planner.apply_if_current(old_intent, o, |_| panic!("old authority")), ApplyOutcome::Rejected);
            let mut stale = old; stale.now += STEP_LIMIT; planner.step(stale);
            assert!(planner.state().is_none());
            o.now = stale.now + STEP_LIMIT; assert!(planner.step(o).is_none());
        }
    }

    #[test]
    fn geometry_changes_cancel_even_when_the_caller_reuses_its_signature() {
        for signature_change in [false, true] {
            let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
            o.now += STEP_LIMIT; let intent = planner.step(o).unwrap();
            if signature_change { o.geometry.as_mut().unwrap().signature += 1; }
            else { o.geometry.as_mut().unwrap().window_width += 1; }
            assert_eq!(planner.apply_if_current(intent, o, |_| panic!("stale geometry")), ApplyOutcome::Rejected);
            assert!(planner.state().is_none());
        }
    }

    #[test]
    fn invalid_geometry_and_a_window_larger_than_the_work_area_never_emit() {
        for mode in 0..5 {
            let mut planner = Planner::default(); let mut o = observation(0);
            match mode { 0 => o.geometry = None, 1 => o.position = None,
                2 => o.geometry.as_mut().unwrap().scale_factor = f64::NAN,
                3 => o.geometry.as_mut().unwrap().scale_factor = 0.0,
                _ => o.geometry.as_mut().unwrap().window_width = u32::MAX }
            planner.step(o); o.now += STEP_LIMIT;
            assert!(planner.step(o).is_none()); assert!(planner.state().is_none());
        }
    }

    #[test]
    fn external_window_movement_cancels_instead_of_correcting_or_chasing_it() {
        let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
        o.now += STEP_LIMIT; let intent = planner.step(o).unwrap();
        o.position.as_mut().unwrap().x += 1;
        assert_eq!(planner.apply_if_current(intent, o, |_| panic!("external drag")), ApplyOutcome::Rejected);
        o.now += STEP_LIMIT; assert!(planner.step(o).is_none());
        assert!(planner.state().is_none());
    }

    #[test]
    fn backwards_clock_and_expired_intent_cannot_publish_late_targets() {
        for backwards in [false, true] {
            let mut planner = Planner::default(); let mut o = observation(1000); planner.step(o);
            o.now += STEP_LIMIT; let intent = planner.step(o).unwrap();
            o.now = if backwards { Duration::from_millis(1099) } else { Duration::from_millis(1201) };
            assert_eq!(planner.apply_if_current(intent, o, |_| panic!("late time")), ApplyOutcome::Rejected);
            o.now = Duration::from_secs(2); assert!(planner.step(o).is_none());
        }
    }

    #[test]
    fn execution_is_once_only_and_failed_or_unwinding_callbacks_leave_no_live_route() {
        let mut planner = Planner::default(); let mut o = observation(0); planner.step(o);
        o.now += STEP_LIMIT; let intent = planner.step(o).unwrap();
        assert_eq!(planner.apply_if_current(intent, o, |_| false), ApplyOutcome::Failed);
        assert_eq!(planner.apply_if_current(intent, o, |_| panic!("replayed")), ApplyOutcome::Rejected);
        o.request.as_mut().unwrap().revision += 1; planner.step(o);
        o.now += STEP_LIMIT; let intent = planner.step(o).unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            planner.apply_if_current(intent, o, |_| panic!("native callback panic"));
        }));
        assert!(result.is_err()); assert!(planner.state().is_none());
        o.request.as_mut().unwrap().revision += 1; planner.step(o);
        o.now += STEP_LIMIT; let intent = apply(&mut planner, &mut o).unwrap();
        assert_eq!(intent.request_key(), o.request.unwrap().key());
        let state = planner.state().unwrap();
        assert_eq!(state.elapsed, STEP_LIMIT);
        assert_eq!(state.remaining, Duration::from_millis(7900));
        assert_eq!(planner.apply_if_current(intent, o, |_| panic!("duplicate")), ApplyOutcome::Rejected);
        o.now += STEP_LIMIT;
        let current = planner.step(o).unwrap();
        o.gate = Gate::Unknown;
        assert_eq!(planner.apply_if_current(intent, o, |_| panic!("stale token")), ApplyOutcome::Rejected);
        assert!(planner.state().is_none());
        o.gate = Gate::Allowed;
        assert_eq!(planner.apply_if_current(current, o, |_| panic!("revived after observed stop")), ApplyOutcome::Rejected);
    }

    #[test]
    fn initial_direction_faces_inward_at_either_work_area_edge_and_keeps_interior_preference() {
        let geometry = observation(0).geometry.unwrap();
        for preferred in [Direction::Left, Direction::Right] {
            assert_eq!(choose_inward_direction(geometry, Point { x: 0, y: 300 }, preferred), Some(Direction::Right));
            assert_eq!(choose_inward_direction(geometry, Point { x: 1400, y: 300 }, preferred), Some(Direction::Left));
            assert_eq!(choose_inward_direction(geometry, Point { x: 700, y: 300 }, preferred), Some(preferred));
        }
    }

    #[test]
    fn initial_direction_uses_physical_negative_monitor_edges_at_fractional_dpi() {
        let geometry = Geometry {
            work_area: Bounds { left: -1920, top: -900, right: 0, bottom: 300 },
            window_width: 420, window_height: 510, scale_factor: 1.5, signature: 9,
        };
        assert_eq!(choose_inward_direction(geometry, Point { x: -1920, y: -500 }, Direction::Left), Some(Direction::Right));
        assert_eq!(choose_inward_direction(geometry, Point { x: -420, y: -500 }, Direction::Right), Some(Direction::Left));
        for preferred in [Direction::Left, Direction::Right] {
            assert_eq!(choose_inward_direction(geometry, Point { x: -1000, y: -500 }, preferred), Some(preferred));
        }
    }

    #[test]
    fn initial_direction_rejects_invalid_geometry_and_any_partial_window_overflow() {
        let valid = observation(0).geometry.unwrap();
        for geometry in [
            Geometry { window_width: 0, ..valid },
            Geometry { window_height: 0, ..valid },
            Geometry { window_width: 1601, ..valid },
            Geometry { window_height: 1001, ..valid },
            Geometry { window_width: u32::MAX, ..valid },
            Geometry { scale_factor: 0.0, ..valid },
            Geometry { scale_factor: -1.0, ..valid },
            Geometry { scale_factor: f64::NAN, ..valid },
            Geometry { scale_factor: f64::INFINITY, ..valid },
            Geometry { work_area: Bounds { right: 0, ..valid.work_area }, ..valid },
            Geometry { work_area: Bounds { bottom: 0, ..valid.work_area }, ..valid },
        ] {
            assert!(choose_inward_direction(geometry, Point { x: 0, y: 0 }, Direction::Right).is_none());
        }
        for position in [
            Point { x: -1, y: 0 },
            Point { x: 1401, y: 0 },
            Point { x: 0, y: -1 },
            Point { x: 0, y: 701 },
        ] {
            assert!(choose_inward_direction(valid, position, Direction::Right).is_none());
        }
    }

    #[test]
    fn initial_direction_requires_horizontal_room_but_accepts_exact_vertical_fit() {
        let geometry = Geometry {
            work_area: Bounds { left: -2, top: -2, right: 2, bottom: 2 },
            window_width: 3, window_height: 4, scale_factor: 2.0, signature: 1,
        };
        assert_eq!(choose_inward_direction(geometry, Point { x: -2, y: -2 }, Direction::Left), Some(Direction::Right));
        assert_eq!(choose_inward_direction(geometry, Point { x: -1, y: -2 }, Direction::Right), Some(Direction::Left));
        assert!(choose_inward_direction(Geometry { window_width: 4, ..geometry }, Point { x: -2, y: -2 }, Direction::Right).is_none());
        assert!(choose_inward_direction(Geometry { window_width: 5, ..geometry }, Point { x: -2, y: -2 }, Direction::Right).is_none());
    }

    #[test]
    fn initial_direction_handles_full_integer_coordinate_span_without_overflow() {
        let geometry = Geometry {
            work_area: Bounds { left: i32::MIN, top: i32::MIN, right: i32::MAX, bottom: i32::MAX },
            window_width: u32::MAX - 1, window_height: u32::MAX,
            scale_factor: 1.25, signature: u64::MAX,
        };
        let left = Point { x: i32::MIN, y: i32::MIN };
        let right = Point { x: i32::MIN + 1, y: i32::MIN };
        assert_eq!(choose_inward_direction(geometry, left, Direction::Left), Some(Direction::Right));
        assert_eq!(choose_inward_direction(geometry, right, Direction::Right), Some(Direction::Left));
        assert!(choose_inward_direction(Geometry { window_width: u32::MAX, ..geometry }, left, Direction::Right).is_none());
        let narrow = Geometry { window_width: 1, ..geometry };
        assert_eq!(choose_inward_direction(narrow, Point { x: i32::MAX - 1, y: i32::MIN }, Direction::Right), Some(Direction::Left));
        assert!(choose_inward_direction(narrow, Point { x: i32::MAX, y: i32::MIN }, Direction::Left).is_none());
    }

    #[test]
    fn initial_inward_direction_matches_the_first_real_planner_step_at_both_edges() {
        for (x, preferred, expected) in [
            (0, Direction::Left, Direction::Right),
            (1400, Direction::Right, Direction::Left),
        ] {
            let mut planner = Planner::default();
            let mut o = observation(0);
            let position = Point { x, y: 300 };
            let direction = choose_inward_direction(o.geometry.unwrap(), position, preferred).unwrap();
            o.position = Some(position);
            let request = o.request.as_mut().unwrap();
            request.anchor = position;
            request.direction = direction;
            request.range = Range::WorkArea;
            assert!(planner.step(o).is_none());
            o.now += STEP_LIMIT;
            let intent = apply(&mut planner, &mut o).unwrap();
            assert_eq!(direction, expected);
            assert_eq!(intent.direction(), direction);
            assert_ne!(o.position, Some(position));
            assert!(o.geometry.unwrap().contains(o.position.unwrap()));
        }
    }
}
