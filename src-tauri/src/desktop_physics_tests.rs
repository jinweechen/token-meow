use super::*;

fn p(x: f64, y: f64) -> Point {
    Point { x, y }
}
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn sample(time: u64, point: Point, down: bool) -> ReleaseSample {
    ReleaseSample {
        cursor: Some(point),
        left_down: down,
        escape_down: false,
        valid: true,
        now: ms(time),
    }
}

fn release(velocity: Point) -> ReleaseToken {
    let mut tracker = ReleaseTracker::default();
    assert!(tracker.begin(sample(0, p(200.0, 200.0), true)));
    tracker.sample(sample(
        20,
        p(200.0 + velocity.x * 0.02, 200.0 + velocity.y * 0.02),
        true,
    ));
    tracker
        .sample(sample(
            40,
            p(200.0 + velocity.x * 0.04, 200.0 + velocity.y * 0.04),
            false,
        ))
        .unwrap()
}

fn observation() -> Observation {
    Observation {
        now: ms(40),
        gate: Gate::Allowed,
        input_marker: Some(7),
        request: Some(Request {
            scope: Scope {
                pet: 1,
                source: 2,
                renderer: 3,
            },
            epoch: 1,
            generation: 1,
            issued_at: ms(40),
            duration: Duration::from_secs(8),
            kind: MotionKind::DockTop { surface_id: 9 },
        }),
        geometry: Some(Geometry {
            work_area: Rect {
                left: 0.0,
                top: 0.0,
                right: 1_000.0,
                bottom: 900.0,
            },
            window_size: Size {
                width: 80.0,
                height: 100.0,
            },
            scale_factor: 1.0,
            signature: 4,
        }),
        surface: Some(Surface {
            id: 9,
            generation: 1,
            observed_at: ms(40),
            rect: Rect {
                left: 200.0,
                top: 300.0,
                right: 700.0,
                bottom: 650.0,
            },
        }),
        position: Some(p(150.0, 160.0)),
    }
}

fn time(o: &mut Observation, now: Duration) {
    o.now = now;
    if let Some(surface) = o.surface.as_mut() {
        surface.observed_at = now;
    }
}

fn tick(planner: &mut Planner, o: &mut Observation, duration: Duration) -> Option<MoveIntent> {
    let now = o.now + duration;
    time(o, now);
    let intent = planner.step(*o)?;
    let mut target = None;
    assert_eq!(
        planner.apply_if_current(intent, *o, |p| {
            target = Some(p);
            true
        }),
        ApplyOutcome::Applied
    );
    o.position = target;
    Some(intent)
}

#[test]
fn native_release_is_clamped_single_use_and_new_drags_keep_monotonic_ids() {
    let mut tracker = ReleaseTracker::default();
    assert!(tracker.begin(sample(0, p(0.0, 0.0), true)));
    assert!(!tracker.begin(sample(0, p(0.0, 0.0), true)));
    assert!(tracker.sample(sample(20, p(100.0, -100.0), true)).is_none());
    let token = tracker.sample(sample(40, p(200.0, -200.0), false)).unwrap();
    assert_eq!(token.id(), 1);
    assert_eq!(token.issued_at(), ms(40));
    assert!((token.velocity().x.hypot(token.velocity().y) - MAX_RELEASE_SPEED).abs() < 0.001);
    assert!(token.velocity().x > 0.0 && token.velocity().y < 0.0);
    assert!(tracker
        .sample(sample(50, p(250.0, -250.0), false))
        .is_none());
    assert!(tracker.begin(sample(60, p(0.0, 0.0), true)));
    assert_eq!(
        tracker
            .sample(sample(80, p(10.0, 0.0), false))
            .unwrap()
            .id(),
        2
    );
}

#[test]
fn escape_unknown_rollback_timeout_and_teleport_do_not_become_release() {
    let bad: [fn(&mut ReleaseSample); 7] = [
        |s| s.escape_down = true,
        |s| s.valid = false,
        |s| s.cursor = None,
        |s| s.cursor = Some(p(f64::NAN, 0.0)),
        |s| s.now = ms(10),
        |s| s.now = ms(140),
        |s| s.cursor = Some(p(100_000.0, 0.0)),
    ];
    for change in bad {
        let mut tracker = ReleaseTracker::default();
        assert!(tracker.begin(sample(0, p(0.0, 0.0), true)));
        tracker.sample(sample(20, p(10.0, 0.0), true));
        let mut invalid = sample(40, p(20.0, 0.0), false);
        change(&mut invalid);
        assert!(tracker.sample(invalid).is_none());
        assert!(tracker.sample(sample(50, p(30.0, 0.0), false)).is_none());
        assert!(tracker.active.is_none());
    }
    let mut tracker = ReleaseTracker::default();
    assert!(tracker.begin(sample(0, p(0.0, 0.0), true)));
    assert!(tracker.sample(sample(0, p(0.0, 0.0), true)).is_none());
    assert_eq!(tracker.active.as_ref().unwrap().points.len(), 1);
    assert!(tracker.sample(sample(0, p(1.0, 0.0), false)).is_none());
    assert!(tracker.active.is_none());
    assert!(tracker.begin(sample(1, p(0.0, 0.0), true)));
    tracker.active.as_mut().unwrap().started = Duration::ZERO;
    tracker.active.as_mut().unwrap().last = MAX_DRAG;
    tracker.active.as_mut().unwrap().points = VecDeque::from([(MAX_DRAG, p(0.0, 0.0))]);
    assert!(tracker
        .sample(sample(300_020, p(1.0, 0.0), false))
        .is_none());
}

#[test]
fn release_velocity_uses_only_recent_two_to_six_real_time_samples() {
    let mut tracker = ReleaseTracker::default();
    assert!(tracker.begin(sample(0, p(0.0, 0.0), true)));
    for n in 1..=20 {
        assert!(tracker
            .sample(sample(n * 20, p(n as f64 * 2.0, 0.0), true))
            .is_none());
        assert!(tracker.active.as_ref().unwrap().points.len() <= 6);
    }
    let token = tracker.sample(sample(420, p(42.0, 0.0), false)).unwrap();
    assert!((token.velocity().x - 100.0).abs() < 0.001);
    assert_eq!(token.velocity().y, 0.0);
    let mut too_fast = ReleaseTracker::default();
    assert!(too_fast.begin(sample(0, p(0.0, 0.0), true)));
    assert!(too_fast.sample(sample(1, p(1.0, 0.0), false)).is_none());
    let mut cancelled = ReleaseTracker::default();
    assert!(cancelled.begin(sample(0, p(0.0, 0.0), true)));
    cancelled.cancel();
    assert!(cancelled.sample(sample(20, p(10.0, 0.0), false)).is_none());
}

#[test]
fn docking_and_both_climb_sides_use_real_kinematic_bodies_without_crossing_surface() {
    let mut planner = Planner::default();
    let mut o = observation();
    planner.step(o);
    for _ in 0..40 {
        tick(&mut planner, &mut o, ms(50));
    }
    assert_eq!(o.position, Some(p(200.0, 200.0)));
    assert_eq!(planner.state().unwrap().phase, Phase::Perched);
    for (side, direction, start, end) in [
        (
            Side::Left,
            VerticalDirection::Up,
            p(40.0, 450.0),
            p(120.0, 200.0),
        ),
        (
            Side::Right,
            VerticalDirection::Down,
            p(780.0, 350.0),
            p(700.0, 550.0),
        ),
    ] {
        let mut planner = Planner::default();
        let mut o = observation();
        o.position = Some(start);
        o.request.as_mut().unwrap().kind = MotionKind::Climb {
            surface_id: 9,
            side,
            direction,
        };
        planner.step(o);
        assert!(planner.state().is_some());
        for _ in 0..135 {
            tick(&mut planner, &mut o, ms(50));
            let actual = o.position.unwrap();
            assert!(o.geometry.unwrap().contains(actual));
            match side {
                Side::Left => assert!(actual.x + 80.0 <= 200.0),
                Side::Right => assert!(actual.x >= 700.0),
            }
        }
        assert_eq!(o.position, Some(end));
        assert_eq!(planner.state().unwrap().phase, Phase::AtEdge);
    }
}

#[test]
fn rapier_throw_has_gravity_ccd_and_work_area_collision_on_negative_monitor() {
    let mut planner = Planner::default();
    let mut o = observation();
    o.surface = None;
    o.position = Some(p(-300.0, -350.0));
    o.geometry.as_mut().unwrap().work_area = Rect {
        left: -1_000.0,
        top: -600.0,
        right: 0.0,
        bottom: 200.0,
    };
    o.request.as_mut().unwrap().kind = MotionKind::Throw {
        release: release(p(900.0, -250.0)),
    };
    planner.step(o);
    let s = planner.session.as_ref().unwrap();
    assert!(s.world.bodies[s.body].is_ccd_enabled());
    assert!(s.world.bodies[s.body].is_dynamic());
    assert_eq!(s.world.colliders.len(), 5);
    let mut rose = false;
    let mut fell = false;
    let mut bounced = false;
    let mut previous = o.position.unwrap();
    for _ in 0..130 {
        tick(&mut planner, &mut o, ms(50));
        let actual = o.position.unwrap();
        assert!(o.geometry.unwrap().contains(actual));
        rose |= actual.y < previous.y;
        fell |= actual.y > previous.y;
        bounced |= planner.state().unwrap().velocity.x < -1.0;
        previous = actual;
    }
    assert!(rose && fell && bounced, "必须观察到真实重力和右边界碰撞");
    assert!(o.position.unwrap().y > -350.0);
}

#[test]
fn thrown_body_collides_with_external_rectangle_top_instead_of_falling_through() {
    let mut planner = Planner::default();
    let mut o = observation();
    o.position = Some(p(300.0, 100.0));
    o.request.as_mut().unwrap().kind = MotionKind::Throw {
        release: release(p(0.0, 0.0)),
    };
    planner.step(o);
    for _ in 0..100 {
        tick(&mut planner, &mut o, ms(50));
        assert!(
            o.position.unwrap().y + 100.0 <= 302.0,
            "外部平台穿透: now={:?}, native={:?}, physical={:?}, velocity={:?}",
            o.now,
            o.position,
            planner.session.as_ref().map(Session::position),
            planner.state().map(|s| s.velocity)
        );
    }
    assert!((o.position.unwrap().y - 200.0).abs() <= 2.0);
}

#[test]
fn execution_rechecks_authority_scope_input_geometry_surface_and_actual_position() {
    let changes: [fn(&mut Observation); 15] = [
        |o| o.gate = Gate::Blocked,
        |o| o.gate = Gate::Unknown,
        |o| o.input_marker = None,
        |o| o.input_marker = Some(8),
        |o| o.position.as_mut().unwrap().x += 1.0,
        |o| o.geometry.as_mut().unwrap().signature += 1,
        |o| o.geometry.as_mut().unwrap().window_size.width += 1.0,
        |o| o.request.as_mut().unwrap().scope.source += 1,
        |o| o.request.as_mut().unwrap().scope.renderer += 1,
        |o| o.request.as_mut().unwrap().generation += 1,
        |o| o.surface = None,
        |o| o.surface.as_mut().unwrap().generation += 1,
        |o| o.surface.as_mut().unwrap().rect.left += 1.0,
        |o| o.surface.as_mut().unwrap().observed_at = o.now + ms(1),
        |o| {
            o.now += ms(251);
        },
    ];
    for change in changes {
        let mut planner = Planner::default();
        let mut o = observation();
        planner.step(o);
        time(&mut o, ms(90));
        let intent = planner.step(o).unwrap();
        let mut invalid = o;
        change(&mut invalid);
        let mut called = false;
        assert_eq!(
            planner.apply_if_current(intent, invalid, |_| {
                called = true;
                true
            }),
            ApplyOutcome::Rejected
        );
        assert!(!called);
        assert!(planner.state().is_none());
        planner.step(o);
        assert!(planner.state().is_none());
    }
}

#[test]
fn delayed_duplicate_and_replaced_intents_cannot_replay_callbacks() {
    let mut planner = Planner::default();
    let mut o = observation();
    planner.step(o);
    time(&mut o, ms(90));
    let intent = planner.step(o).unwrap();
    time(&mut o, ms(141));
    assert_eq!(
        planner.apply_if_current(intent, o, |_| panic!("过期目标不能执行")),
        ApplyOutcome::Rejected
    );
    assert!(planner.state().is_none());
    o.request.as_mut().unwrap().generation += 1;
    o.request.as_mut().unwrap().issued_at = o.now;
    planner.step(o);
    assert_eq!(
        planner.apply_if_current(intent, o, |_| panic!("旧代次不能执行")),
        ApplyOutcome::Rejected
    );
    assert!(
        planner.state().is_some(),
        "旧 token 不能关闭当前有效的新代次"
    );
    let applied = tick(&mut planner, &mut o, ms(50)).unwrap();
    assert_eq!(
        planner.apply_if_current(applied, o, |_| panic!("同一目标只能消费一次")),
        ApplyOutcome::Rejected
    );
    assert!(planner.state().is_some());
}

#[test]
fn failed_or_unwinding_native_callback_cancels_before_the_callback_runs() {
    for unwind in [false, true] {
        let mut planner = Planner::default();
        let mut o = observation();
        planner.step(o);
        time(&mut o, ms(90));
        let intent = planner.step(o).unwrap();
        if unwind {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                planner.apply_if_current(intent, o, |_| panic!("模拟原生异常"));
            }));
            assert!(outcome.is_err());
        } else {
            assert_eq!(
                planner.apply_if_current(intent, o, |_| false),
                ApplyOutcome::Failed
            );
        }
        assert!(planner.state().is_none());
        planner.step(o);
        assert!(planner.state().is_none());
        assert_eq!(
            planner.apply_if_current(intent, o, |_| panic!("失败后不能重放")),
            ApplyOutcome::Rejected
        );
    }
}

#[test]
fn wall_clock_deadline_is_fixed_ticks_never_catch_up_and_pause_requires_new_request() {
    let mut planner = Planner::default();
    let mut o = observation();
    o.request.as_mut().unwrap().duration = Duration::from_secs(60);
    time(&mut o, ms(1_040));
    planner.step(o);
    assert_eq!(planner.state().unwrap().remaining, Duration::from_secs(7));
    let before = o.position.unwrap();
    let intent = tick(&mut planner, &mut o, ms(200)).unwrap();
    assert_eq!(intent.duration(), MAX_STEP);
    assert!(o.position.unwrap().distance(before) <= 4.0);
    tick(&mut planner, &mut o, ms(251));
    assert!(planner.state().is_none());
    o.request.as_mut().unwrap().generation += 1;
    o.request.as_mut().unwrap().issued_at = o.now;
    planner.step(o);
    assert!(planner.state().is_some());
    o.gate = Gate::Blocked;
    tick(&mut planner, &mut o, ms(10));
    assert!(planner.state().is_none());
    o.gate = Gate::Allowed;
    tick(&mut planner, &mut o, ms(10));
    assert!(planner.state().is_none());
    o.request.as_mut().unwrap().generation += 1;
    o.request.as_mut().unwrap().issued_at = ms(40);
    time(&mut o, ms(8_039));
    planner.step(o);
    assert!(planner.state().is_some());
    tick(&mut planner, &mut o, ms(1));
    assert!(planner.state().is_none());
}

#[test]
fn rollback_and_pending_tick_expiry_revoke_instead_of_replacing_old_targets() {
    let mut planner = Planner::default();
    let mut o = observation();
    planner.step(o);
    time(&mut o, ms(90));
    let intent = planner.step(o).unwrap();
    time(&mut o, ms(100));
    assert!(planner.step(o).is_none());
    assert_eq!(planner.pending, Some(intent));
    time(&mut o, ms(141));
    assert!(planner.step(o).is_none());
    assert!(planner.state().is_none());
    o.request.as_mut().unwrap().generation += 1;
    o.request.as_mut().unwrap().issued_at = o.now;
    planner.step(o);
    time(&mut o, ms(140));
    assert!(planner.step(o).is_none());
    assert!(planner.state().is_none());
    time(&mut o, ms(200));
    assert!(planner.step(o).is_none());
    assert!(planner.state().is_none());
}

#[test]
fn release_attempt_is_consumed_even_when_permission_is_closed_or_geometry_is_unknown() {
    let token = release(p(400.0, 0.0));
    for gate in [Gate::Allowed, Gate::Blocked, Gate::Unknown] {
        let mut planner = Planner::default();
        let mut o = observation();
        o.surface = None;
        o.request.as_mut().unwrap().kind = MotionKind::Throw { release: token };
        o.gate = gate;
        let geometry = o.geometry;
        if gate == Gate::Allowed {
            o.geometry = None;
        }
        planner.step(o);
        assert!(planner.state().is_none());
        o.gate = Gate::Allowed;
        o.geometry = geometry;
        o.request.as_mut().unwrap().generation += 1;
        planner.step(o);
        assert!(
            planner.state().is_none(),
            "同一原生松手不能通过新请求代次重放"
        );
    }
    let mut expired = Planner::default();
    let mut o = observation();
    o.surface = None;
    o.request.as_mut().unwrap().kind = MotionKind::Throw { release: token };
    time(&mut o, ms(191));
    expired.step(o);
    assert!(expired.state().is_none());
}

#[test]
fn invalid_geometries_and_ineligible_edges_are_refused_without_starting_a_world() {
    let changes: [fn(&mut Observation); 8] = [
        |o| o.geometry.as_mut().unwrap().window_size.width = 0.0,
        |o| o.geometry.as_mut().unwrap().scale_factor = f64::NAN,
        |o| o.geometry.as_mut().unwrap().work_area.right = f64::INFINITY,
        |o| o.position = Some(p(-1.0, 0.0)),
        |o| o.position = Some(p(200.0, 250.0)),
        |o| o.position = Some(p(0.0, 0.0)),
        |o| o.surface.as_mut().unwrap().id += 1,
        |o| o.request.as_mut().unwrap().duration = Duration::ZERO,
    ];
    for change in changes {
        let mut o = observation();
        change(&mut o);
        let mut planner = Planner::default();
        assert!(planner.step(o).is_none());
        assert!(planner.state().is_none());
    }
}
