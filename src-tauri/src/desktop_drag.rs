//! Bounded manual dragging in physical pixels. Planning never calls HWND APIs.
//!
//! The desktop UI thread samples input and current bounds, drops the controller
//! lock after `tick`, then moves the real window with SWP_NOACTIVATE. This avoids
//! the OS sizing/moving loop without changing the user's global drag settings.
use std::time::Duration;

pub(super) const MAX_SESSION: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= i32::MAX as u32
            && self.height <= i32::MAX as u32
    }

    fn distance_squared(self, cursor: Point) -> u128 {
        let x = i64::from(cursor.x);
        let y = i64::from(cursor.y);
        let left = i64::from(self.x);
        let top = i64::from(self.y);
        let right = left + i64::from(self.width) - 1;
        let bottom = top + i64::from(self.height) - 1;
        let dx = (x - x.clamp(left, right)).unsigned_abs() as u128;
        let dy = (y - y.clamp(top, bottom)).unsigned_abs() as u128;
        dx * dx + dy * dy
    }

    fn clamp(self, x: i64, y: i64, width: u32, height: u32) -> Point {
        let left = i64::from(self.x);
        let top = i64::from(self.y);
        // A window larger than the work area is pinned to its top-left. Fitting
        // its size remains the existing desktop scale/recovery owner's job.
        let max_x = left + i64::from(self.width.saturating_sub(width));
        let max_y = top + i64::from(self.height.saturating_sub(height));
        Point {
            x: x.clamp(left, max_x)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            y: y.clamp(top, max_y)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        }
    }
}

/// Renderer coordinates identify a press inside this pet WebView only. Native
/// supplies the real physical client rectangle and HWND DPI before any popup
/// restoration; renderer screen coordinates are never accepted.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClientAnchor {
    pub x: f64,
    pub y: f64,
    pub viewport_width: f64,
    pub viewport_height: f64,
}

pub(super) fn physical_press_anchor(
    press: ClientAnchor,
    client: Rect,
    scale_factor: f64,
) -> Option<Point> {
    if !client.valid()
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
        || ![press.x, press.y, press.viewport_width, press.viewport_height]
            .iter().all(|value| value.is_finite())
        || press.viewport_width < 1.0
        || press.viewport_height < 1.0
        || press.viewport_width > 32_768.0
        || press.viewport_height > 32_768.0
        || press.x < 0.0
        || press.y < 0.0
        || press.x >= press.viewport_width
        || press.y >= press.viewport_height
    {
        return None;
    }
    // innerWidth/Height are integer CSS pixels; permit at most their rounding
    // error, not another popup size or a stale monitor/DPI viewport.
    let tolerance = scale_factor.max(1.0);
    if (press.viewport_width * scale_factor - f64::from(client.width)).abs() > tolerance
        || (press.viewport_height * scale_factor - f64::from(client.height)).abs() > tolerance
    {
        return None;
    }
    let x = i64::from(client.x) + (press.x * f64::from(client.width) / press.viewport_width)
        .round().min(f64::from(client.width - 1)) as i64;
    let y = i64::from(client.y) + (press.y * f64::from(client.height) / press.viewport_height)
        .round().min(f64::from(client.height - 1)) as i64;
    Some(Point { x: i32::try_from(x).ok()?, y: i32::try_from(y).ok()? })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Input {
    pub cursor: Option<Point>,
    pub left_down: bool,
    pub escape_down: bool,
    pub visible: bool,
    pub runtime_available: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartError {
    AlreadyActive,
    ButtonNotDown,
    EscapeHeld,
    Hidden,
    RuntimeUnavailable,
    CursorUnavailable,
    InvalidGeometry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StopReason {
    Released,
    Escape,
    Hidden,
    RuntimeUnavailable,
    CursorUnavailable,
    InvalidGeometry,
    Timeout,
    ClockRegression,
    Cancelled,
    NativeMoveFailed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Update {
    pub target: Option<Point>,
    pub stopped: Option<StopReason>,
}

#[derive(Clone, Copy, Debug)]
struct Session {
    cursor: Point,
    origin: Rect,
    started: Duration,
    observed: Duration,
}

#[derive(Default)]
pub(super) struct Controller {
    session: Option<Session>,
}

impl Controller {
    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    pub fn start(&mut self, input: Input, window: Rect, now: Duration) -> Result<(), StartError> {
        if self.is_active() {
            return Err(StartError::AlreadyActive);
        }
        if !input.visible {
            return Err(StartError::Hidden);
        }
        if !input.runtime_available {
            return Err(StartError::RuntimeUnavailable);
        }
        if input.escape_down {
            return Err(StartError::EscapeHeld);
        }
        if !input.left_down {
            return Err(StartError::ButtonNotDown);
        }
        let cursor = input.cursor.ok_or(StartError::CursorUnavailable)?;
        if !window.valid() {
            return Err(StartError::InvalidGeometry);
        }
        self.session = Some(Session {
            cursor,
            origin: window,
            started: now,
            observed: now,
        });
        Ok(())
    }

    /// Explicit hidden/pause/exit/native-move-error paths can revoke immediately,
    /// even if the GUI worker is about to stop scheduling further ticks.
    pub fn cancel(&mut self, _reason: StopReason) -> bool {
        self.session.take().is_some()
    }

    fn stop(&mut self, reason: StopReason) -> Update {
        self.session = None;
        Update {
            target: None,
            stopped: Some(reason),
        }
    }

    /// A single current sample produces at most one target. Nothing queues old
    /// cursor positions, and all deltas use the original physical-pixel anchor.
    pub fn tick(&mut self, input: Input, window: Rect, work: &[Rect], now: Duration) -> Update {
        let Some(mut session) = self.session else {
            return Update::default();
        };
        if !input.visible {
            return self.stop(StopReason::Hidden);
        }
        if !input.runtime_available {
            return self.stop(StopReason::RuntimeUnavailable);
        }
        if input.escape_down {
            return self.stop(StopReason::Escape);
        }
        if !input.left_down {
            return self.stop(StopReason::Released);
        }
        let Some(cursor) = input.cursor else {
            return self.stop(StopReason::CursorUnavailable);
        };
        if now < session.observed {
            return self.stop(StopReason::ClockRegression);
        }
        if now.saturating_sub(session.started) >= MAX_SESSION {
            return self.stop(StopReason::Timeout);
        }
        if !window.valid() {
            return self.stop(StopReason::InvalidGeometry);
        }
        let Some(area) = work
            .iter()
            .copied()
            .filter(|area| area.valid())
            .min_by_key(|area| area.distance_squared(cursor))
        else {
            return self.stop(StopReason::InvalidGeometry);
        };
        // A per-monitor DPI/scale change can resize the real window mid-drag.
        // Rebase using its current physical rect instead of applying stale size
        // or scaling cursor deltas a second time.
        if window.width != session.origin.width || window.height != session.origin.height {
            session.cursor = cursor;
            session.origin = window;
        }
        session.observed = now;
        self.session = Some(session);
        let x = i64::from(session.origin.x) + i64::from(cursor.x) - i64::from(session.cursor.x);
        let y = i64::from(session.origin.y) + i64::from(cursor.y) - i64::from(session.cursor.y);
        let target = area.clamp(x, y, window.width, window.height);
        Update {
            target: (target
                != Point {
                    x: window.x,
                    y: window.y,
                })
            .then_some(target),
            stopped: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(x: i32, y: i32) -> Point {
        Point { x, y }
    }
    fn window() -> Rect {
        Rect {
            x: 100,
            y: 100,
            width: 280,
            height: 340,
        }
    }
    fn area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        }
    }
    fn input(x: i32, y: i32) -> Input {
        Input {
            cursor: Some(point(x, y)),
            left_down: true,
            escape_down: false,
            visible: true,
            runtime_available: true,
        }
    }
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    fn active_owner() -> Controller {
        let mut owner = Controller::default();
        owner.start(input(120, 130), window(), ms(0)).unwrap();
        owner
    }
    fn press(x: f64, y: f64, width: f64, height: f64) -> ClientAnchor {
        ClientAnchor { x, y, viewport_width: width, viewport_height: height }
    }
    #[test]
    fn original_press_anchor_preserves_move_completed_before_native_takeover() {
        let rect = Rect { x: 1624, y: 676, ..window() };
        let anchor = physical_press_anchor(press(145.0, 130.0, 280.0, 340.0), rect, 1.0).unwrap();
        let mut owner = Controller::default();
        owner.start(Input { cursor: Some(anchor), ..input(0, 0) }, rect, ms(0)).unwrap();
        assert_eq!(owner.tick(input(1689, 726), rect, &[area()], ms(1)).target,
            Some(point(1544, 596)));
    }
    #[test]
    fn physical_press_handles_high_dpi_fractional_css_and_negative_monitor_origin() {
        let rect = Rect { x: -1920, y: -240, width: 420, height: 510 };
        assert_eq!(physical_press_anchor(press(100.5, 150.5, 280.0, 340.0), rect, 1.5),
            Some(point(-1769, -14)));
        let rect = Rect { width: 350, height: 425, ..rect };
        assert_eq!(physical_press_anchor(press(100.0, 80.0, 280.0, 340.0), rect, 1.25),
            Some(point(-1795, -140)));
    }
    #[test]
    fn popup_press_is_mapped_before_restoration_changes_client_origin() {
        let expanded = Rect { x: 1624, y: 496, width: 280, height: 520 };
        let anchor = physical_press_anchor(press(145.0, 310.0, 280.0, 520.0), expanded, 1.0).unwrap();
        assert_eq!(anchor, point(1769, 806));
        let restored = Rect { x: 1624, y: 676, ..window() };
        let mut owner = Controller::default();
        owner.start(Input { cursor: Some(anchor), ..input(0, 0) }, restored, ms(0)).unwrap();
        assert_eq!(owner.tick(input(1689, 726), restored, &[area()], ms(1)).target,
            Some(point(1544, 596)));
    }
    #[test]
    fn stale_viewport_dpi_and_invalid_geometry_cannot_supply_an_anchor() {
        let request = press(145.0, 130.0, 280.0, 340.0);
        assert!(physical_press_anchor(request, Rect { height: 520, ..window() }, 1.0).is_none());
        assert!(physical_press_anchor(request, window(), 1.5).is_none());
        assert!(physical_press_anchor(request, Rect { width: 0, ..window() }, 1.0).is_none());
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(physical_press_anchor(request, window(), scale).is_none());
        }
        // Integer innerWidth can round one CSS pixel at fractional DPI.
        let rect = Rect { width: 421, height: 511, ..window() };
        assert!(physical_press_anchor(press(140.0, 170.0, 281.0, 341.0), rect, 1.5).is_some());
    }
    #[test]
    fn nonfinite_outside_or_unbounded_renderer_coordinates_are_rejected() {
        let invalid = [
            press(f64::NAN, 1.0, 280.0, 340.0),
            press(1.0, f64::INFINITY, 280.0, 340.0),
            press(1.0, 1.0, f64::NAN, 340.0),
            press(1.0, 1.0, 280.0, f64::INFINITY),
            press(-0.1, 1.0, 280.0, 340.0),
            press(1.0, -0.1, 280.0, 340.0),
            press(280.0, 1.0, 280.0, 340.0),
            press(1.0, 340.0, 280.0, 340.0),
            press(0.0, 0.0, 0.0, 340.0),
            press(0.0, 0.0, 280.0, -1.0),
            press(0.0, 0.0, 32769.0, 340.0),
        ];
        for request in invalid {
            assert!(physical_press_anchor(request, window(), 1.0).is_none());
        }
    }
    #[test]
    fn client_edges_stay_inside_physical_pixels_and_screen_coordinates_do_not_overflow() {
        assert_eq!(physical_press_anchor(press(0.0, 0.0, 280.0, 340.0), window(), 1.0), Some(point(100, 100)));
        assert_eq!(physical_press_anchor(press(279.99, 339.99, 280.0, 340.0), window(), 1.0), Some(point(379, 439)));
        assert!(physical_press_anchor(press(279.0, 0.0, 280.0, 340.0),
            Rect { x: i32::MAX - 100, ..window() }, 1.0).is_none());
    }
    #[test]
    fn starts_only_for_real_held_button_visible_available_cursor_and_valid_size() {
        let cases = [
            (
                Input {
                    left_down: false,
                    ..input(0, 0)
                },
                window(),
                StartError::ButtonNotDown,
            ),
            (
                Input {
                    escape_down: true,
                    ..input(0, 0)
                },
                window(),
                StartError::EscapeHeld,
            ),
            (
                Input {
                    visible: false,
                    ..input(0, 0)
                },
                window(),
                StartError::Hidden,
            ),
            (
                Input {
                    runtime_available: false,
                    ..input(0, 0)
                },
                window(),
                StartError::RuntimeUnavailable,
            ),
            (
                Input {
                    cursor: None,
                    ..input(0, 0)
                },
                window(),
                StartError::CursorUnavailable,
            ),
            (
                input(0, 0),
                Rect {
                    width: 0,
                    ..window()
                },
                StartError::InvalidGeometry,
            ),
        ];
        for (sample, rect, error) in cases {
            let mut owner = Controller::default();
            assert_eq!(owner.start(sample, rect, ms(0)), Err(error));
            assert!(!owner.is_active());
        }
    }
    #[test]
    fn second_start_cannot_replace_active_anchor() {
        let mut owner = active_owner();
        assert_eq!(
            owner.start(input(900, 900), area(), ms(1)),
            Err(StartError::AlreadyActive)
        );
        assert_eq!(
            owner
                .tick(input(130, 140), window(), &[area()], ms(2))
                .target,
            Some(point(110, 110))
        );
    }
    #[test]
    fn direct_anchor_delta_has_no_accumulation_or_queued_history() {
        let mut owner = active_owner();
        let first = owner.tick(input(170, 180), window(), &[area()], ms(1));
        assert_eq!(first.target, Some(point(150, 150)));
        let current = Rect {
            x: 150,
            y: 150,
            ..window()
        };
        assert_eq!(
            owner
                .tick(input(180, 190), current, &[area()], ms(2))
                .target,
            Some(point(160, 160))
        );
        assert_eq!(
            owner
                .tick(input(120, 130), current, &[area()], ms(3))
                .target,
            Some(point(100, 100))
        );
    }
    #[test]
    fn negative_coordinate_monitor_is_selected_by_cursor_and_clamped() {
        let mut owner = active_owner();
        let left = Rect {
            x: -1920,
            y: -200,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            owner
                .tick(input(-1900, -180), window(), &[area(), left], ms(1))
                .target,
            Some(point(-1920, -200))
        );
    }
    #[test]
    fn cursor_crosses_to_second_monitor_without_sticking_to_first() {
        let mut owner = active_owner();
        let second = Rect {
            x: 1920,
            y: 0,
            width: 2560,
            height: 1400,
        };
        assert_eq!(
            owner
                .tick(input(2200, 200), window(), &[area(), second], ms(1))
                .target,
            Some(point(2180, 170))
        );
    }
    #[test]
    fn gap_uses_nearest_work_area_and_honors_taskbar_edge() {
        let mut owner = active_owner();
        let right = Rect {
            x: 2100,
            y: 0,
            width: 1000,
            height: 900,
        };
        assert_eq!(
            owner
                .tick(input(2000, 1000), window(), &[area(), right], ms(1))
                .target,
            Some(point(1640, 700))
        );
    }
    #[test]
    fn physical_resize_rebases_instead_of_rescaling_cursor_delta() {
        let mut owner = active_owner();
        let resized = Rect {
            x: 150,
            y: 150,
            width: 560,
            height: 680,
        };
        assert_eq!(
            owner
                .tick(input(170, 180), resized, &[area()], ms(1))
                .target,
            None
        );
        assert_eq!(
            owner
                .tick(input(180, 190), resized, &[area()], ms(2))
                .target,
            Some(point(160, 160))
        );
    }
    #[test]
    fn releasing_escape_hidden_pause_or_missing_cursor_stops_once() {
        let cases = [
            (
                Input {
                    left_down: false,
                    ..input(999, 999)
                },
                StopReason::Released,
            ),
            (
                Input {
                    escape_down: true,
                    ..input(999, 999)
                },
                StopReason::Escape,
            ),
            (
                Input {
                    visible: false,
                    ..input(999, 999)
                },
                StopReason::Hidden,
            ),
            (
                Input {
                    runtime_available: false,
                    ..input(999, 999)
                },
                StopReason::RuntimeUnavailable,
            ),
            (
                Input {
                    cursor: None,
                    ..input(999, 999)
                },
                StopReason::CursorUnavailable,
            ),
        ];
        for (sample, reason) in cases {
            let mut owner = active_owner();
            assert_eq!(
                owner.tick(sample, window(), &[area()], ms(1)),
                Update {
                    target: None,
                    stopped: Some(reason)
                }
            );
            assert!(!owner.is_active());
            assert_eq!(
                owner.tick(input(999, 999), window(), &[area()], ms(2)),
                Update::default()
            );
        }
    }
    #[test]
    fn invalid_geometry_or_missing_work_areas_fail_closed() {
        let mut owner = active_owner();
        assert_eq!(
            owner.tick(input(1, 1), window(), &[], ms(1)).stopped,
            Some(StopReason::InvalidGeometry)
        );
        let mut owner = active_owner();
        assert_eq!(
            owner
                .tick(
                    input(1, 1),
                    Rect {
                        height: 0,
                        ..window()
                    },
                    &[area()],
                    ms(1)
                )
                .stopped,
            Some(StopReason::InvalidGeometry)
        );
    }
    #[test]
    fn deadline_and_clock_regression_do_not_leave_an_owner() {
        let mut owner = active_owner();
        assert_eq!(
            owner
                .tick(input(1, 1), window(), &[area()], MAX_SESSION)
                .stopped,
            Some(StopReason::Timeout)
        );
        let mut owner = Controller::default();
        owner.start(input(1, 1), window(), ms(10)).unwrap();
        assert_eq!(
            owner.tick(input(2, 2), window(), &[area()], ms(9)).stopped,
            Some(StopReason::ClockRegression)
        );
    }
    #[test]
    fn oversized_window_and_extreme_coordinates_do_not_overflow() {
        let mut owner = active_owner();
        let tiny = Rect {
            x: -100,
            y: -100,
            width: 20,
            height: 20,
        };
        assert_eq!(
            owner
                .tick(input(i32::MIN, i32::MIN), window(), &[tiny], ms(1))
                .target,
            Some(point(-100, -100))
        );
        let mut owner = Controller::default();
        let start = Rect {
            x: i32::MAX - 300,
            y: i32::MAX - 400,
            ..window()
        };
        owner
            .start(input(i32::MIN, i32::MIN), start, ms(0))
            .unwrap();
        assert_eq!(
            owner
                .tick(input(i32::MAX, i32::MAX), start, &[area()], ms(1))
                .target,
            Some(point(1640, 700))
        );
    }
    #[test]
    fn explicit_cancel_releases_owner_and_allows_new_session() {
        let mut owner = active_owner();
        assert!(owner.cancel(StopReason::Cancelled));
        assert!(!owner.cancel(StopReason::NativeMoveFailed));
        assert!(owner.start(input(120, 130), window(), ms(1)).is_ok());
        assert!(owner.cancel(StopReason::NativeMoveFailed));
        assert!(!owner.is_active());
    }
}
