//! 有界桌宠物理规划。没有 HWND、IPC、线程、系统设置或业务账本所有权。
//!
//! 宿主只能在单一原生移动 owner 内调用；Allowed 必须已经包括 Core/source、
//! renderer readiness、系统动画、可见性和原生输入等全部许可。本模块不授予许可。
//! scope 来自原生实例身份，scope/source/renderer 变化时宿主必须递增 epoch。
//! 一个 planner 对应一个宠物，epoch/generation 在其整个生命周期内单调递增。
//!
//! 所有外部坐标均为物理像素；Rapier 使用工作区局部坐标以减少 f32 精度损失。
//! MoveIntent 不暴露目标，只有执行时重验全部快照的同步 callback 能得到目标。
//! callback 不得排队、await 或延后移动；成功才返回 true。

use rapier2d::prelude::{ColliderBuilder, PhysicsWorld, RigidBodyBuilder, RigidBodyHandle, Vector};
use std::{collections::VecDeque, time::Duration};

pub(super) const MAX_STEP: Duration = Duration::from_millis(50);
pub(super) const MAX_SURFACE_AGE: Duration = Duration::from_millis(250);
pub(super) const RELEASE_TTL: Duration = Duration::from_millis(150);
const MAX_REQUEST: Duration = Duration::from_secs(8);
const MAX_TICK_GAP: Duration = Duration::from_millis(250);
const MAX_SAMPLE_GAP: Duration = Duration::from_millis(100);
const SAMPLE_WINDOW: Duration = Duration::from_millis(120);
const MAX_DRAG: Duration = Duration::from_secs(300);
const MAX_RELEASE_SPEED: f64 = 1_600.0;
const MAX_RAW_SPEED: f64 = 12_000.0;
const EDGE_SPEED: f64 = 64.0;
const MAX_APPROACH: f64 = 120.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    fn capped(self, limit: f64) -> Self {
        let magnitude = self.x.hypot(self.y);
        if magnitude > limit {
            Self {
                x: self.x * limit / magnitude,
                y: self.y * limit / magnitude,
            }
        } else {
            self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Rect {
    fn valid(self) -> bool {
        [self.left, self.top, self.right, self.bottom]
            .iter()
            .all(|v| v.is_finite() && *v >= f64::from(i32::MIN) && *v <= f64::from(i32::MAX))
            && self.right > self.left
            && self.bottom > self.top
            && self.right - self.left <= 65_536.0
            && self.bottom - self.top <= 65_536.0
    }

    fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && self.right > other.left
            && self.top < other.bottom
            && self.bottom > other.top
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Geometry {
    pub work_area: Rect,
    pub window_size: Size,
    pub scale_factor: f64,
    pub signature: u64,
}

impl Geometry {
    fn valid(self) -> bool {
        let s = self.window_size;
        self.work_area.valid()
            && s.width.is_finite()
            && s.height.is_finite()
            && s.width >= 1.0
            && s.height >= 1.0
            && s.width <= 8_192.0
            && s.height <= 8_192.0
            && self.scale_factor.is_finite()
            && (0.25..=8.0).contains(&self.scale_factor)
            && self.work_area.left.ceil() <= (self.work_area.right - s.width).floor()
            && self.work_area.top.ceil() <= (self.work_area.bottom - s.height).floor()
    }

    fn contains(self, p: Point) -> bool {
        self.valid()
            && p.finite()
            && p.x >= self.work_area.left
            && p.y >= self.work_area.top
            && p.x + self.window_size.width <= self.work_area.right
            && p.y + self.window_size.height <= self.work_area.bottom
    }

    fn target(self, p: Point) -> Option<Point> {
        p.finite()
            .then(|| Point {
                x: p.x.round().clamp(
                    self.work_area.left.ceil(),
                    (self.work_area.right - self.window_size.width).floor(),
                ),
                y: p.y.round().clamp(
                    self.work_area.top.ceil(),
                    (self.work_area.bottom - self.window_size.height).floor(),
                ),
            })
            .filter(|p| self.contains(*p))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scope {
    pub pet: u64,
    pub source: u64,
    pub renderer: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Surface {
    pub id: u64,
    pub generation: u64,
    pub rect: Rect,
    pub observed_at: Duration,
}

impl Surface {
    fn fresh(self, now: Duration) -> bool {
        self.rect.valid()
            && now
                .checked_sub(self.observed_at)
                .is_some_and(|age| age <= MAX_SURFACE_AGE)
    }

    fn same_geometry(self, other: Self) -> bool {
        self.id == other.id
            && self.generation == other.generation
            && self.rect == other.rect
            && other.observed_at >= self.observed_at
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerticalDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MotionKind {
    DockTop {
        surface_id: u64,
    },
    Climb {
        surface_id: u64,
        side: Side,
        direction: VerticalDirection,
    },
    Throw {
        release: ReleaseToken,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Request {
    pub scope: Scope,
    pub epoch: u64,
    pub generation: u64,
    pub issued_at: Duration,
    pub duration: Duration,
    pub kind: MotionKind,
}

impl Request {
    fn key(self) -> (u64, u64) {
        (self.epoch, self.generation)
    }

    fn deadline(self) -> Option<Duration> {
        if self.duration.is_zero() {
            return None;
        }
        self.issued_at.checked_add(self.duration.min(MAX_REQUEST))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gate {
    Allowed,
    Blocked,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Observation {
    pub now: Duration,
    pub gate: Gate,
    pub input_marker: Option<u32>,
    pub request: Option<Request>,
    pub geometry: Option<Geometry>,
    pub surface: Option<Surface>,
    pub position: Option<Point>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Docking,
    Perched,
    Climbing,
    AtEdge,
    Thrown,
    Resting,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MoveIntent {
    serial: u64,
    key: (u64, u64),
    scope: Scope,
    created_at: Duration,
    duration: Duration,
    target: Point,
    phase: Phase,
}

impl MoveIntent {
    pub fn duration(self) -> Duration {
        self.duration
    }
    pub fn phase(self) -> Phase {
        self.phase
    }
    pub fn request_key(self) -> (u64, u64) {
        self.key
    }
    pub fn scope(self) -> Scope {
        self.scope
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ApplyOutcome {
    Applied,
    Rejected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct State {
    pub request: Request,
    pub phase: Phase,
    pub expected_position: Point,
    pub velocity: Point,
    pub elapsed: Duration,
    pub remaining: Duration,
}

struct Session {
    request: Request,
    deadline: Duration,
    geometry: Geometry,
    surface: Option<Surface>,
    input_marker: u32,
    expected: Point,
    stepped_at: Duration,
    phase: Phase,
    goal: Option<Point>,
    world: PhysicsWorld,
    body: RigidBodyHandle,
}

impl Session {
    fn new(o: Observation, request: Request, geometry: Geometry, position: Point) -> Option<Self> {
        let deadline = request.deadline()?;
        if o.gate != Gate::Allowed
            || o.now < request.issued_at
            || o.now >= deadline
            || !geometry.contains(position)
        {
            return None;
        }
        let input_marker = o.input_marker?;
        if o.surface
            .is_some_and(|s| !s.fresh(o.now) || !s.rect.intersects(geometry.work_area))
        {
            return None;
        }
        let s = geometry.window_size;
        let mut goal = None;
        let phase = match request.kind {
            MotionKind::DockTop { surface_id } => {
                let surface = o.surface.filter(|surface| surface.id == surface_id)?;
                let r = surface.rect;
                if r.right - r.left < s.width || position.y + s.height > r.top + 0.01 {
                    return None;
                }
                let target = Point {
                    x: position.x.clamp(r.left, r.right - s.width),
                    y: r.top - s.height,
                };
                if !geometry.contains(target)
                    || position.distance(target) > MAX_APPROACH * geometry.scale_factor
                {
                    return None;
                }
                goal = Some(target);
                Phase::Docking
            }
            MotionKind::Climb {
                surface_id,
                side,
                direction,
            } => {
                let surface = o.surface.filter(|surface| surface.id == surface_id)?;
                let r = surface.rect;
                let x = match side {
                    Side::Left => r.left - s.width,
                    Side::Right => r.right,
                };
                let top = (r.top - s.height).max(geometry.work_area.top);
                let bottom = (r.bottom - s.height).min(geometry.work_area.bottom - s.height);
                let outside = match side {
                    Side::Left => position.x + s.width <= r.left + 0.01,
                    Side::Right => position.x >= r.right - 0.01,
                };
                if !outside
                    || top >= bottom
                    || position.y < top
                    || position.y > bottom
                    || (position.x - x).abs() > MAX_APPROACH * geometry.scale_factor
                {
                    return None;
                }
                let target = Point {
                    x,
                    y: match direction {
                        VerticalDirection::Up => top,
                        VerticalDirection::Down => bottom,
                    },
                };
                if !geometry.contains(target) {
                    return None;
                }
                goal = Some(target);
                Phase::Climbing
            }
            MotionKind::Throw { release } => {
                if !release.fresh(o.now) || request.issued_at < release.issued_at {
                    return None;
                }
                let frame = Rect {
                    left: position.x,
                    top: position.y,
                    right: position.x + s.width,
                    bottom: position.y + s.height,
                };
                if o.surface
                    .is_some_and(|surface| surface.rect.intersects(frame))
                {
                    return None;
                }
                Phase::Thrown
            }
        };
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0.0, (980.0 * geometry.scale_factor) as f32);
        world.integration_parameters.length_unit = (100.0 * geometry.scale_factor) as f32;
        world.integration_parameters.max_ccd_substeps = 2;
        world.integration_parameters.normalized_allowed_linear_error = 0.001;
        world.integration_parameters.normalized_max_linear_velocity = 16.0;
        let w = geometry.work_area.right - geometry.work_area.left;
        let h = geometry.work_area.bottom - geometry.work_area.top;
        let thickness = 64.0;
        for (cx, cy, width, height) in [
            (-thickness / 2.0, h / 2.0, thickness, h + 2.0 * thickness),
            (w + thickness / 2.0, h / 2.0, thickness, h + 2.0 * thickness),
            (w / 2.0, -thickness / 2.0, w + 2.0 * thickness, thickness),
            (w / 2.0, h + thickness / 2.0, w + 2.0 * thickness, thickness),
        ] {
            insert_fixed(&mut world, cx, cy, width, height);
        }
        if let Some(surface) = o.surface {
            let r = surface.rect;
            // 仅工作区交集参与碰撞，绝不创建跨显示器移动路径。
            let left = r.left.max(geometry.work_area.left) - geometry.work_area.left;
            let top = r.top.max(geometry.work_area.top) - geometry.work_area.top;
            let right = r.right.min(geometry.work_area.right) - geometry.work_area.left;
            let bottom = r.bottom.min(geometry.work_area.bottom) - geometry.work_area.top;
            insert_fixed(
                &mut world,
                (left + right) / 2.0,
                (top + bottom) / 2.0,
                right - left,
                bottom - top,
            );
        }
        let dynamic = matches!(request.kind, MotionKind::Throw { .. });
        let builder = if dynamic {
            RigidBodyBuilder::dynamic()
        } else {
            RigidBodyBuilder::kinematic_position_based()
        };
        let mut builder = builder
            .translation(Vector::new(
                (position.x - geometry.work_area.left + s.width / 2.0) as f32,
                (position.y - geometry.work_area.top + s.height / 2.0) as f32,
            ))
            .lock_rotations()
            .ccd_enabled(true)
            .linear_damping(0.4);
        if let MotionKind::Throw { release } = request.kind {
            builder = builder.linvel(Vector::new(
                release.velocity.x as f32,
                release.velocity.y as f32,
            ));
        }
        let (body, _) = world.insert(
            builder,
            ColliderBuilder::cuboid((s.width / 2.0) as f32, (s.height / 2.0) as f32)
                .restitution(0.25)
                .friction(0.8),
        );
        Some(Self {
            request,
            deadline,
            geometry,
            surface: o.surface,
            input_marker,
            expected: position,
            stepped_at: o.now,
            phase,
            goal,
            world,
            body,
        })
    }

    fn matches(&self, o: Observation) -> bool {
        o.gate == Gate::Allowed
            && o.request == Some(self.request)
            && o.geometry == Some(self.geometry)
            && o.position == Some(self.expected)
            && o.input_marker == Some(self.input_marker)
            && o.now >= self.request.issued_at
            && o.now < self.deadline
            && self.geometry.contains(self.expected)
            && match (self.surface, o.surface) {
                (None, None) => true,
                (Some(old), Some(new)) => old.same_geometry(new) && new.fresh(o.now),
                _ => false,
            }
    }

    fn position(&self) -> Point {
        let p = self.world.bodies[self.body].translation();
        Point {
            x: f64::from(p.x) + self.geometry.work_area.left
                - self.geometry.window_size.width / 2.0,
            y: f64::from(p.y) + self.geometry.work_area.top
                - self.geometry.window_size.height / 2.0,
        }
    }

    fn velocity(&self) -> Point {
        let v = self.world.bodies[self.body].linvel();
        Point {
            x: f64::from(v.x),
            y: f64::from(v.y),
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        if matches!(phase, Phase::Perched | Phase::AtEdge | Phase::Resting) {
            self.world.bodies[self.body].set_linvel(Vector::new(0.0, 0.0), false);
        }
    }

    fn advance(&mut self, duration: Duration) -> Option<(Point, Phase)> {
        if matches!(self.phase, Phase::Perched | Phase::AtEdge | Phase::Resting) {
            return Some((self.expected, self.phase));
        }
        let mut next_phase = self.phase;
        if let Some(goal) = self.goal {
            let p = self.position();
            // 攀爬先在矩形外侧接近边缘，再沿边缘垂直运动，不斜穿外部窗口。
            let immediate = if matches!(self.request.kind, MotionKind::Climb { .. })
                && (p.x - goal.x).abs() > 0.01
            {
                Point { x: goal.x, y: p.y }
            } else {
                goal
            };
            let distance = p.distance(immediate);
            let travel = EDGE_SPEED * self.geometry.scale_factor * duration.as_secs_f64();
            let target = if distance <= travel {
                immediate
            } else {
                Point {
                    x: p.x + (immediate.x - p.x) * travel / distance,
                    y: p.y + (immediate.y - p.y) * travel / distance,
                }
            };
            self.world.bodies[self.body].set_next_kinematic_translation(Vector::new(
                (target.x - self.geometry.work_area.left + self.geometry.window_size.width / 2.0)
                    as f32,
                (target.y - self.geometry.work_area.top + self.geometry.window_size.height / 2.0)
                    as f32,
            ));
            if target.distance(goal) <= 0.01 {
                next_phase = match self.request.kind {
                    MotionKind::DockTop { .. } => Phase::Perched,
                    _ => Phase::AtEdge,
                };
            }
        }
        // 50ms 单步的接触弹簧会在撞击帧留下数像素穿透；动态刚体最多拆成
        // 三个 <=1/60s 子步。只拆分本次已获准的 <=50ms，不追赶墙钟停顿。
        let substeps = if matches!(self.request.kind, MotionKind::Throw { .. }) {
            (duration.as_secs_f64() * 60.0).ceil().clamp(1.0, 3.0) as usize
        } else {
            1
        };
        self.world.integration_parameters.dt = duration.as_secs_f32() / substeps as f32;
        for _ in 0..substeps {
            self.world.step();
        }
        let p = self.position();
        let velocity = self.velocity();
        if !p.finite() || !velocity.finite() {
            return None;
        }
        let target = self.geometry.target(p)?;
        // CCD/接触求解负责物理碰撞；最后的整像素边界收敛保证 HWND 全框不越区。
        if !self.geometry.contains(p) {
            let work = self.geometry.work_area;
            let size = self.geometry.window_size;
            let mut v = velocity;
            if (p.x < work.left && v.x < 0.0) || (p.x + size.width > work.right && v.x > 0.0) {
                v.x = 0.0;
            }
            if (p.y < work.top && v.y < 0.0) || (p.y + size.height > work.bottom && v.y > 0.0) {
                v.y = 0.0;
            }
            let body = &mut self.world.bodies[self.body];
            body.set_translation(
                Vector::new(
                    (target.x - work.left + size.width / 2.0) as f32,
                    (target.y - work.top + size.height / 2.0) as f32,
                ),
                true,
            );
            body.set_linvel(Vector::new(v.x as f32, v.y as f32), true);
        }
        if matches!(self.request.kind, MotionKind::Throw { .. }) {
            let v = self
                .velocity()
                .capped(MAX_RELEASE_SPEED * self.geometry.scale_factor);
            self.world.bodies[self.body].set_linvel(Vector::new(v.x as f32, v.y as f32), false);
            let bottom = target.y + self.geometry.window_size.height;
            let supported = (bottom - self.geometry.work_area.bottom).abs() <= 1.0
                || self.surface.is_some_and(|s| {
                    (bottom - s.rect.top).abs() <= 1.0
                        && target.x < s.rect.right
                        && target.x + self.geometry.window_size.width > s.rect.left
                });
            if supported && v.x.hypot(v.y) <= 8.0 * self.geometry.scale_factor {
                next_phase = Phase::Resting;
            }
        }
        Some((target, next_phase))
    }
}

fn insert_fixed(world: &mut PhysicsWorld, x: f64, y: f64, width: f64, height: f64) {
    world.insert(
        RigidBodyBuilder::fixed().translation(Vector::new(x as f32, y as f32)),
        ColliderBuilder::cuboid((width / 2.0) as f32, (height / 2.0) as f32)
            .restitution(0.25)
            .friction(0.8),
    );
}

#[derive(Default)]
pub(super) struct Planner {
    session: Option<Session>,
    pending: Option<MoveIntent>,
    watermark: Option<(u64, u64)>,
    release_watermark: u64,
    observed_at: Option<Duration>,
    serial: u64,
    exhausted: bool,
}

impl Planner {
    /// 撤销后不能用相同/更早的请求或已经消费的 release token 重新开段。
    pub fn cancel(&mut self) {
        self.session = None;
        self.pending = None;
    }

    fn observe_time(&mut self, now: Duration) -> bool {
        if self.observed_at.is_some_and(|old| now < old) {
            self.cancel();
            return false;
        }
        self.observed_at = Some(now);
        true
    }

    pub fn state(&self) -> Option<State> {
        let s = self.session.as_ref()?;
        let now = self.observed_at?;
        Some(State {
            request: s.request,
            phase: s.phase,
            expected_position: s.expected,
            velocity: s.velocity(),
            elapsed: now.checked_sub(s.request.issued_at)?,
            remaining: s.deadline.saturating_sub(now),
        })
    }

    pub fn step(&mut self, o: Observation) -> Option<MoveIntent> {
        let newer = o
            .request
            .is_some_and(|r| self.watermark.is_none_or(|old| r.key() > old));
        let mut unused_release = true;
        if newer {
            self.cancel();
            self.watermark = o.request.map(Request::key);
            if let Some(Request {
                kind: MotionKind::Throw { release },
                ..
            }) = o.request
            {
                unused_release = release.id > self.release_watermark;
                // 首次尝试即消费，门禁关闭、几何未知等失败也不能重放这次松手。
                self.release_watermark = self.release_watermark.max(release.id);
            }
        }
        if !self.observe_time(o.now) || self.exhausted {
            self.cancel();
            return None;
        }
        let (Some(request), Some(geometry), Some(position)) = (o.request, o.geometry, o.position)
        else {
            self.cancel();
            return None;
        };
        if o.gate != Gate::Allowed || self.watermark != Some(request.key()) {
            self.cancel();
            return None;
        }
        if newer {
            if !unused_release {
                return None;
            }
            self.session = Session::new(o, request, geometry, position);
            return None;
        }
        let valid = self.session.as_ref().is_some_and(|s| {
            s.matches(o)
                && o.now
                    .checked_sub(s.stepped_at)
                    .is_some_and(|gap| gap <= MAX_TICK_GAP)
        });
        if !valid {
            self.cancel();
            return None;
        }
        if let Some(pending) = self.pending {
            if o.now
                .checked_sub(pending.created_at)
                .is_none_or(|age| age > MAX_STEP)
            {
                self.cancel();
            }
            return None;
        }
        let s = self.session.as_mut()?;
        let duration = (o.now - s.stepped_at).min(MAX_STEP);
        s.stepped_at = o.now;
        s.surface = o.surface;
        if duration.is_zero() {
            return None;
        }
        let Some((target, phase)) = s.advance(duration) else {
            self.cancel();
            return None;
        };
        if target == s.expected {
            s.set_phase(phase);
            return None;
        }
        let Some(serial) = self.serial.checked_add(1) else {
            self.exhausted = true;
            self.cancel();
            return None;
        };
        self.serial = serial;
        let intent = MoveIntent {
            serial,
            key: request.key(),
            scope: request.scope,
            created_at: o.now,
            duration,
            target,
            phase,
        };
        self.pending = Some(intent);
        Some(intent)
    }

    /// 必须在同一移动 owner 内用刚取得的原生快照调用。回调失败/展开前先撤销。
    pub fn apply_if_current(
        &mut self,
        intent: MoveIntent,
        o: Observation,
        apply: impl FnOnce(Point) -> bool,
    ) -> ApplyOutcome {
        if !self.observe_time(o.now) || !self.session.as_ref().is_some_and(|s| s.matches(o)) {
            self.cancel();
            return ApplyOutcome::Rejected;
        }
        if self.pending != Some(intent) {
            return ApplyOutcome::Rejected;
        }
        if o.now
            .checked_sub(intent.created_at)
            .is_none_or(|age| age > MAX_STEP)
        {
            self.cancel();
            return ApplyOutcome::Rejected;
        }
        let mut s = self.session.take().expect("已检查物理会话");
        self.pending = None;
        if !apply(intent.target) {
            return ApplyOutcome::Failed;
        }
        s.expected = intent.target;
        s.set_phase(intent.phase);
        s.surface = o.surface;
        self.session = Some(s);
        ApplyOutcome::Applied
    }
}

/// 输入只能来自原生光标/按键快照，不能接收 renderer velocity。
#[derive(Clone, Copy, Debug)]
pub(super) struct ReleaseSample {
    pub cursor: Option<Point>,
    pub left_down: bool,
    pub escape_down: bool,
    pub valid: bool,
    pub now: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ReleaseToken {
    id: u64,
    issued_at: Duration,
    velocity: Point,
}

impl ReleaseToken {
    pub fn id(self) -> u64 {
        self.id
    }
    pub fn issued_at(self) -> Duration {
        self.issued_at
    }
    pub fn velocity(self) -> Point {
        self.velocity
    }
    fn fresh(self, now: Duration) -> bool {
        self.velocity.finite()
            && self.velocity.x.hypot(self.velocity.y) <= MAX_RELEASE_SPEED + 0.01
            && now
                .checked_sub(self.issued_at)
                .is_some_and(|age| age <= RELEASE_TTL)
    }
}

struct DragSamples {
    started: Duration,
    last: Duration,
    points: VecDeque<(Duration, Point)>,
}

#[derive(Default)]
pub(super) struct ReleaseTracker {
    active: Option<DragSamples>,
    observed_at: Option<Duration>,
    serial: u64,
    exhausted: bool,
}

impl ReleaseTracker {
    pub fn cancel(&mut self) {
        self.active = None;
    }

    pub fn begin(&mut self, sample: ReleaseSample) -> bool {
        if self.active.is_some() {
            return false;
        }
        if self.exhausted
            || !sample.valid
            || sample.escape_down
            || !sample.left_down
            || sample.cursor.is_none_or(|p| !p.finite())
            || self.observed_at.is_some_and(|old| sample.now < old)
        {
            return false;
        }
        self.observed_at = Some(sample.now);
        let mut points = VecDeque::with_capacity(6);
        points.push_back((sample.now, sample.cursor.expect("已检查原生光标")));
        self.active = Some(DragSamples {
            started: sample.now,
            last: sample.now,
            points,
        });
        true
    }

    /// 只真 Released 输出一次；Escape/隐藏/暂停/失效/回拨/巨跳均直接清空。
    pub fn sample(&mut self, sample: ReleaseSample) -> Option<ReleaseToken> {
        let mut active = self.active.take()?;
        if !sample.valid
            || sample.escape_down
            || sample.cursor.is_none_or(|p| !p.finite())
            || self.observed_at.is_some_and(|old| sample.now < old)
            || sample
                .now
                .checked_sub(active.last)
                .is_none_or(|gap| gap > MAX_SAMPLE_GAP)
            || sample
                .now
                .checked_sub(active.started)
                .is_none_or(|age| age >= MAX_DRAG)
        {
            return None;
        }
        self.observed_at = Some(sample.now);
        let cursor = sample.cursor?;
        let (old_time, old_cursor) = *active.points.back()?;
        if sample.now == old_time {
            // 相同时间的同位置采样不算第二个真实样本；有位移则速度未知，撤销。
            if cursor != old_cursor || !sample.left_down {
                return None;
            }
            self.active = Some(active);
            return None;
        }
        let distance = cursor.distance(old_cursor);
        let delta = sample.now - old_time;
        if !distance.is_finite()
            || distance > 1_024.0
            || distance / delta.as_secs_f64() > MAX_RAW_SPEED
        {
            return None;
        }
        active.last = sample.now;
        active.points.push_back((sample.now, cursor));
        while active.points.len() > 6
            || active
                .points
                .front()
                .is_some_and(|(time, _)| sample.now - *time > SAMPLE_WINDOW)
        {
            active.points.pop_front();
        }
        if sample.left_down {
            self.active = Some(active);
            return None;
        }
        let (first_time, first) = *active.points.front()?;
        let elapsed = sample.now - first_time;
        if active.points.len() < 2 || elapsed < Duration::from_millis(8) {
            return None;
        }
        let velocity = Point {
            x: (cursor.x - first.x) / elapsed.as_secs_f64(),
            y: (cursor.y - first.y) / elapsed.as_secs_f64(),
        }
        .capped(MAX_RELEASE_SPEED);
        let Some(id) = self.serial.checked_add(1) else {
            self.exhausted = true;
            return None;
        };
        self.serial = id;
        Some(ReleaseToken {
            id,
            issued_at: sample.now,
            velocity,
        })
    }
}

#[cfg(test)]
#[path = "desktop_physics_tests.rs"]
mod tests;
