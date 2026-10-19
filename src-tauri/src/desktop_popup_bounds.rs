//! Temporary popup geometry only. No settings, ledger, HWND, or persistence.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Session {
    pub original: Bounds,
    scale_factor: f64,
}

impl Session {
    pub fn capture(original: Bounds, scale_factor: f64) -> Option<Self> {
        (original.width > 0
            && original.height > 0
            && scale_factor.is_finite()
            && scale_factor > 0.0)
            .then_some(Self {
                original,
                scale_factor,
            })
    }

    fn sized(&self, work: Bounds, dpi: f64, popup: bool) -> Option<Bounds> {
        if work.width == 0 || work.height == 0 || !dpi.is_finite() || dpi <= 0.0 {
            return None;
        }
        let ratio = dpi / self.scale_factor;
        let mut width = (f64::from(self.original.width) * ratio).round().max(1.0);
        let mut height = (f64::from(self.original.height) * ratio).round().max(1.0);
        if popup {
            width = width.max((280.0 * dpi).ceil());
            height = height.max((520.0 * dpi).ceil());
        }
        let width = (width as u32).min(work.width);
        let height = (height as u32).min(work.height);
        let right = i64::from(self.original.x) + i64::from(self.original.width);
        let bottom = i64::from(self.original.y) + i64::from(self.original.height);
        let left_limit = i64::from(work.x);
        let top_limit = i64::from(work.y);
        let right_limit = left_limit + i64::from(work.width - width);
        let bottom_limit = top_limit + i64::from(work.height - height);
        Some(Bounds {
            x: (right - i64::from(width))
                .clamp(left_limit, right_limit)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            y: (bottom - i64::from(height))
                .clamp(top_limit, bottom_limit)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            width,
            height,
        })
    }

    pub fn expanded(&self, work: Bounds, scale_factor: f64) -> Option<Bounds> {
        self.sized(work, scale_factor, true)
    }

    pub fn restored(&self, work: Bounds, scale_factor: f64) -> Option<Bounds> {
        self.sized(work, scale_factor, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> Bounds {
        Bounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        }
    }
    fn base() -> Bounds {
        Bounds {
            x: 1624,
            y: 676,
            width: 280,
            height: 340,
        }
    }

    #[test]
    fn normal_popup_is_520_logical_high_and_keeps_right_bottom() {
        let session = Session::capture(base(), 1.0).unwrap();
        assert_eq!(
            session.expanded(work(), 1.0),
            Some(Bounds {
                y: 496,
                height: 520,
                ..base()
            })
        );
        assert_eq!(session.restored(work(), 1.0), Some(base()));
    }

    #[test]
    fn half_scale_gains_readable_width_without_losing_anchor() {
        let original = Bounds {
            x: 1764,
            y: 846,
            width: 140,
            height: 170,
        };
        let session = Session::capture(original, 1.0).unwrap();
        assert_eq!(
            session.expanded(work(), 1.0),
            Some(Bounds {
                x: 1624,
                y: 496,
                width: 280,
                height: 520
            })
        );
        assert_eq!(session.restored(work(), 1.0), Some(original));
    }

    #[test]
    fn double_scale_does_not_shrink_the_existing_character_window() {
        let original = Bounds {
            x: 1320,
            y: 340,
            width: 560,
            height: 680,
        };
        let session = Session::capture(original, 1.0).unwrap();
        assert_eq!(session.expanded(work(), 1.0), Some(original));
        assert_eq!(session.restored(work(), 1.0), Some(original));
    }

    #[test]
    fn high_dpi_target_uses_logical_minimum_and_restores_physical_base() {
        let original = Bounds {
            x: 1000,
            y: 200,
            width: 420,
            height: 510,
        };
        let session = Session::capture(original, 1.5).unwrap();
        let expanded = session.expanded(work(), 1.5).unwrap();
        assert_eq!((expanded.width, expanded.height), (420, 780));
        assert_eq!(expanded.y, 0);
        assert_eq!(session.restored(work(), 1.5), Some(original));
    }

    #[test]
    fn short_work_area_clamps_without_changing_saved_original() {
        let session = Session::capture(base(), 1.0).unwrap();
        let small = Bounds {
            x: 0,
            y: 40,
            width: 800,
            height: 360,
        };
        assert_eq!(
            session.expanded(small, 1.0),
            Some(Bounds {
                x: 520,
                y: 40,
                width: 280,
                height: 360
            })
        );
        assert_eq!(session.restored(work(), 1.0), Some(base()));
    }

    #[test]
    fn negative_monitor_coordinates_are_preserved_and_clamped() {
        let original = Bounds {
            x: -320,
            y: -400,
            width: 280,
            height: 340,
        };
        let area = Bounds {
            x: -1920,
            y: -1080,
            width: 1920,
            height: 1040,
        };
        let session = Session::capture(original, 1.0).unwrap();
        assert_eq!(
            session.expanded(area, 1.0),
            Some(Bounds {
                x: -320,
                y: -580,
                width: 280,
                height: 520
            })
        );
        assert_eq!(session.restored(area, 1.0), Some(original));
    }

    #[test]
    fn dpi_change_resizes_base_without_reusing_popup_dimensions() {
        let session = Session::capture(base(), 1.0).unwrap();
        let restored = session.restored(work(), 1.5).unwrap();
        assert_eq!((restored.width, restored.height), (420, 510));
        assert_eq!(
            (
                restored.x + restored.width as i32,
                restored.y + restored.height as i32
            ),
            (1904, 1016)
        );
    }

    #[test]
    fn invalid_geometry_and_dpi_do_not_produce_native_bounds() {
        assert!(Session::capture(base(), f64::NAN).is_none());
        let session = Session::capture(base(), 1.0).unwrap();
        assert!(session
            .expanded(Bounds { width: 0, ..work() }, 1.0)
            .is_none());
        assert!(session.restored(work(), 0.0).is_none());
    }
}
