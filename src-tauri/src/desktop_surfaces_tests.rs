use super::*;
use std::cell::Cell;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
    Rect {
        left,
        top,
        right,
        bottom,
    }
}
fn work() -> Rect {
    rect(-1920, 0, 0, 1080)
}
fn identity() -> NativeIdentity {
    NativeIdentity {
        handle: 123,
        process_id: 456,
        rect: rect(-1700, 160, -500, 800),
    }
}
fn snapshot(at: Instant) -> Snapshot {
    Snapshot::new(identity(), work(), at, at).unwrap()
}

#[test]
fn negative_monitor_coordinates_and_physical_edges_are_kept() {
    let observed = snapshot(Instant::now());
    assert_eq!(observed.rect(), identity().rect);
    assert_eq!(
        observed.edges(),
        VisibleEdges {
            rect: identity().rect,
            top: true,
            left: true,
            right: true,
            bottom: true
        }
    );
}

#[test]
fn clipping_does_not_invent_a_top_or_side_edge() {
    let raw = rect(-2100, -200, -500, 800);
    let edges = visible_edges(raw, work()).unwrap();
    assert_eq!(edges.rect, rect(-1920, 0, -500, 800));
    assert!(!edges.top && !edges.left);
    assert!(edges.right && edges.bottom);
}

#[test]
fn clipping_all_edges_away_gives_no_surface() {
    assert!(visible_edges(rect(-2200, -200, 500, 1300), work()).is_none());
}

#[test]
fn outside_and_merely_touching_windows_are_not_platforms() {
    for raw in [
        rect(0, 50, 600, 400),
        rect(-2500, 50, -1920, 400),
        rect(-1700, -300, -500, 0),
        rect(-1700, 1080, -500, 1300),
    ] {
        assert!(visible_edges(raw, work()).is_none());
    }
}

#[test]
fn thin_visible_strips_and_tiny_windows_are_refused() {
    for raw in [
        rect(-1950, 100, -1900, 500),
        rect(-1700, 1040, -500, 1300),
        rect(-1700, 100, -1650, 500),
        rect(-1700, 100, -500, 140),
    ] {
        assert!(visible_edges(raw, work()).is_none());
    }
}

#[test]
fn reversed_zero_huge_and_extreme_rectangles_are_refused_without_overflow() {
    for raw in [
        rect(0, 0, 0, 100),
        rect(10, 20, 0, 100),
        rect(0, 0, 32_769, 100),
        rect(0, 0, 32_768, 32_768),
        rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
        rect(-1_000_001, 0, -999_000, 100),
    ] {
        assert!(!raw.valid());
        assert!(visible_edges(raw, work()).is_none());
    }
}

#[test]
fn valid_minimum_and_bounded_maximum_sizes_are_accepted() {
    assert!(visible_edges(rect(-1000, 100, -936, 148), work()).is_some());
    assert!(rect(0, 0, 32_768, 8192).valid());
    assert!(!rect(0, 0, 32_768, 8193).valid());
}

#[test]
fn invalid_work_area_and_empty_native_identity_are_refused() {
    let at = Instant::now();
    assert!(Snapshot::new(identity(), rect(0, 0, 0, 0), at, at).is_none());
    let mut current = identity();
    current.handle = 0;
    assert!(Snapshot::new(current, work(), at, at).is_none());
    current = identity();
    current.process_id = 0;
    assert!(Snapshot::new(current, work(), at, at).is_none());
}

#[test]
fn age_is_monotonic_and_ttl_is_strictly_less_than_500ms() {
    let at = Instant::now();
    let observed = snapshot(at);
    assert_eq!(observed.observed_at(), at);
    assert_eq!(observed.age_at(at), Some(Duration::ZERO));
    assert!(observed.fresh_at(at + Duration::from_millis(499)));
    assert!(!observed.fresh_at(at + Duration::from_millis(500)));
    // Windows QPC 的分辨率可能高于 1ns；构造可表达的回退，不把量化到同刻当成回退。
    assert_eq!(observed.age_at(at - Duration::from_millis(1)), None);
    assert!(!observed.fresh_at(at - Duration::from_millis(1)));
}

#[test]
fn slow_sampling_cannot_create_a_fresh_snapshot() {
    let at = Instant::now();
    assert!(Snapshot::new(identity(), work(), at, at + OBSERVATION_TTL).is_none());
    assert!(Snapshot::new(identity(), work(), at, at - Duration::from_millis(1)).is_none());
}

#[test]
fn signature_is_stable_for_same_geometry_and_changes_with_identity_or_work_area() {
    let at = Instant::now();
    let a = snapshot(at);
    let b = snapshot(at + Duration::from_millis(1));
    assert_eq!(a.signature(), b.signature());
    assert_eq!(a.signature().opaque_id(), b.signature().opaque_id());
    let mut different = identity();
    different.process_id += 1;
    assert_ne!(
        a.signature(),
        Snapshot::new(different, work(), at, at)
            .unwrap()
            .signature()
    );
    different = identity();
    different.handle += 1;
    assert_ne!(
        a.signature(),
        Snapshot::new(different, work(), at, at)
            .unwrap()
            .signature()
    );
    different = identity();
    different.rect.left += 1;
    assert_ne!(
        a.signature(),
        Snapshot::new(different, work(), at, at)
            .unwrap()
            .signature()
    );
    assert_ne!(
        a.signature(),
        Snapshot::new(identity(), rect(-1919, 0, 0, 1080), at, at)
            .unwrap()
            .signature()
    );
}

#[test]
fn revalidation_needs_the_exact_live_identity_and_bounds() {
    let at = Instant::now();
    let observed = snapshot(at);
    assert!(observed.revalidate_with(
        work(),
        at,
        || Some(identity()),
        || at + Duration::from_millis(1)
    ));
    for different in [
        NativeIdentity {
            handle: 124,
            ..identity()
        },
        NativeIdentity {
            process_id: 457,
            ..identity()
        },
        NativeIdentity {
            rect: rect(-1699, 160, -500, 800),
            ..identity()
        },
    ] {
        assert!(!observed.revalidate_with(work(), at, || Some(different), || at));
    }
    assert!(!observed.revalidate_with(rect(-1919, 0, 0, 1080), at, || Some(identity()), || at));
}

#[test]
fn failed_native_revalidation_does_not_reuse_the_old_surface() {
    let at = Instant::now();
    let observed = snapshot(at);
    assert!(!observed.revalidate_with(work(), at, || None, || at));
    assert!(!observed.revalidate_with(work(), at, || None, || at));
}

#[test]
fn stale_or_changed_work_area_skips_native_capture() {
    let at = Instant::now();
    let observed = snapshot(at);
    let called = Cell::new(false);
    assert!(!observed.revalidate_with(
        work(),
        at + OBSERVATION_TTL,
        || {
            called.set(true);
            Some(identity())
        },
        || at
    ));
    assert!(!called.get());
    assert!(!observed.revalidate_with(
        rect(-1919, 0, 0, 1080),
        at,
        || {
            called.set(true);
            Some(identity())
        },
        || at
    ));
    assert!(!called.get());
}

#[test]
fn ttl_is_checked_again_after_the_native_calls() {
    let at = Instant::now();
    let observed = snapshot(at);
    assert!(!observed.revalidate_with(
        work(),
        at + Duration::from_millis(499),
        || Some(identity()),
        || at + OBSERVATION_TTL
    ));
    assert!(!observed.revalidate_with(
        work(),
        at,
        || Some(identity()),
        || at - Duration::from_millis(1)
    ));
}

#[test]
fn debug_output_never_contains_raw_handle_or_process_fields() {
    let observed = snapshot(Instant::now());
    let debug = format!("{observed:?}");
    assert!(debug.contains("signature") && debug.contains("edges"));
    assert!(
        !debug.contains("handle")
            && !debug.contains("process_id")
            && !debug.contains("NativeIdentity")
    );
}

#[cfg(not(windows))]
#[test]
fn unsupported_platform_returns_no_surface() {
    assert!(sample(work()).is_none());
}
