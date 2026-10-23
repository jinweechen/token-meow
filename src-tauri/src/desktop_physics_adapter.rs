//! 主桌宠的有界物理宿主。IPC 只表达意图，HWND、速度、平台和许可均由原生取得。
//! Core/磁盘刷新只在 worker；UI owner 最后持 Core/settings/selection 锁同步复核和执行。
use super::*;
use crate::{
    desktop_physics as physics, desktop_surfaces as surfaces, pet_skin_selection::ActiveSkin,
};
use std::hash::{Hash, Hasher};

const EVENT: &str = "pet-physics-state";
const CACHE_TTL: Duration = Duration::from_millis(500);
const ACTION_LIMIT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum RendererKind {
    Builtin,
    Png,
    Vrm,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Registration {
    renderer_epoch: String,
    resource_key: String,
    renderer_kind: RendererKind,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Contact {
    viewport_width: f64,
    viewport_height: f64,
    left_inset: f64,
    right_inset: f64,
    bottom_inset: f64,
}

impl Contact {
    fn valid(&self) -> bool {
        [
            self.viewport_width,
            self.viewport_height,
            self.left_inset,
            self.right_inset,
            self.bottom_inset,
        ]
        .iter()
        .all(|v| v.is_finite())
            && (1.0..=32_768.0).contains(&self.viewport_width)
            && (1.0..=32_768.0).contains(&self.viewport_height)
            && self.left_inset >= 0.0
            && self.right_inset >= 0.0
            && self.bottom_inset >= 0.0
            && self.left_inset <= self.viewport_width * 0.8
            && self.right_inset <= self.viewport_width * 0.8
            && self.left_inset + self.right_inset < self.viewport_width * 0.95
            && self.bottom_inset <= self.viewport_height * 0.75
    }

    fn physical(&self, snapshot: NativeSnapshot) -> Option<Insets> {
        if !self.valid() {
            return None;
        }
        let scale = snapshot.geometry.scale_factor;
        let tolerance = scale.max(1.0);
        if (self.viewport_width * scale - snapshot.client.width).abs() > tolerance
            || (self.viewport_height * scale - snapshot.client.height).abs() > tolerance
        {
            return None;
        }
        let size = snapshot.geometry.window_size;
        let insets = Insets {
            left: snapshot.client_offset.x + self.left_inset * scale,
            right: size.width - snapshot.client_offset.x - snapshot.client.width
                + self.right_inset * scale,
            bottom: size.height - snapshot.client_offset.y - snapshot.client.height
                + self.bottom_inset * scale,
        };
        (insets.left >= 0.0
            && insets.right >= 0.0
            && insets.bottom >= 0.0
            && insets.left + insets.right < size.width
            && insets.bottom < size.height)
            .then_some(insets)
    }
}

#[derive(Clone, Copy)]
struct Insets {
    left: f64,
    right: f64,
    bottom: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Kind {
    Dock,
    Climb,
    Throw,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Dock => "dock",
            Self::Climb => "climb",
            Self::Throw => "throw",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FrameAck {
    generation: String,
    kind: Kind,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Readiness {
    renderer_epoch: String,
    token: String,
    sequence: String,
    ready: bool,
    climb_ready: bool,
    contact: Contact,
    ack: Option<FrameAck>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ActionRequest {
    renderer_epoch: String,
    token: String,
    kind: Kind,
    side: Option<String>,
    direction: Option<String>,
    duration_ms: Option<u64>,
}

#[derive(Clone)]
struct Renderer {
    epoch: String,
    token: String,
    resource_key: String,
    kind: RendererKind,
    handle: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct Skin {
    key: String,
    kind: RendererKind,
}

#[derive(Clone)]
struct Evidence {
    runtime: String,
    source_id: String,
    source_root: PathBuf,
    changes: u64,
    scope: u64,
    skin: Skin,
    at: Duration,
}

#[derive(Clone)]
struct Action {
    generation: u64,
    kind: Kind,
    side: Option<physics::Side>,
    direction: Option<physics::VerticalDirection>,
    issued_at: Duration,
    duration: Duration,
    marker: u32,
    revision: u64,
    scope: u64,
    release: Option<physics::ReleaseToken>,
    origin: NativeSnapshot,
}

impl Action {
    fn alive(&self, now: Duration) -> bool {
        now.checked_sub(self.issued_at)
            .is_some_and(|age| age < self.duration)
    }
}

struct Active {
    request: physics::Request,
    geometry: physics::Geometry,
    initial_position: physics::Point,
    contact: Contact,
    surface: Option<surfaces::Snapshot>,
}

#[derive(Clone)]
struct DragOrigin {
    revision: u64,
    scope: u64,
    resource_key: String,
}

pub(super) struct Runtime {
    renderer: Option<Renderer>,
    permission: motion_guard::PermissionLease,
    contact: Option<Contact>,
    presented: Option<(u64, Kind)>,
    climb_ready: bool,
    throw_enabled: bool,
    planner: physics::Planner,
    tracker: physics::ReleaseTracker,
    drag: Option<DragOrigin>,
    action: Option<Action>,
    active: Option<Active>,
    work: Option<Evidence>,
    skin: Option<(u64, Skin)>,
    cancellation_revision: u64,
    epoch: u64,
    generation: u64,
    revision: u64,
    status: &'static str,
    reason: Option<&'static str>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            renderer: None,
            permission: Default::default(),
            contact: None,
            presented: None,
            climb_ready: false,
            throw_enabled: false,
            planner: Default::default(),
            tracker: Default::default(),
            drag: None,
            action: None,
            active: None,
            work: None,
            skin: None,
            cancellation_revision: 0,
            epoch: 1,
            generation: 0,
            revision: 0,
            status: "idle",
            reason: None,
        }
    }
}

impl Runtime {
    fn clear(&mut self, reason: Option<&'static str>) {
        self.planner.cancel();
        self.tracker.cancel();
        self.drag = None;
        self.action = None;
        self.active = None;
        self.presented = None;
        self.status = if reason.is_some() { "blocked" } else { "idle" };
        self.reason = reason;
        self.epoch = self.epoch.checked_add(1).unwrap_or(u64::MAX);
    }

    fn sync(&mut self, revision: u64) {
        if self.cancellation_revision != revision {
            self.clear(Some("动作已取消"));
            self.cancellation_revision = revision;
            self.work = None;
            self.skin = None;
        }
    }

    fn credentials(&self, epoch: &str, token: &str) -> bool {
        self.renderer
            .as_ref()
            .is_some_and(|r| r.epoch == epoch && r.token == token)
    }

    fn fresh_evidence(&self, now: Duration) -> Option<&Evidence> {
        let evidence = self.work.as_ref()?;
        let renderer = self.renderer.as_ref()?;
        (now.checked_sub(evidence.at)
            .is_some_and(|age| age < CACHE_TTL)
            && renderer.resource_key == evidence.skin.key
            && renderer.kind == evidence.skin.kind)
            .then_some(evidence)
    }

    fn next(&mut self) -> Result<u64, String> {
        if self.epoch == u64::MAX {
            return Err("动作代次已耗尽，请重启应用".into());
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("动作代次已耗尽，请重启应用")?;
        Ok(self.generation)
    }

    fn snapshot(&mut self, now: Duration) -> Value {
        self.revision = self.revision.checked_add(1).unwrap_or(u64::MAX);
        let action = self.action.as_ref().map(|a| {
            let phase = self.planner.state().map(|s| phase_name(s.phase))
                .unwrap_or(if self.active.is_some() { "waitingForFrame" } else { "preparing" });
            let mut value = json!({"generation":a.generation.to_string(),"kind":a.kind.name(),"phase":phase,
                "remainingMs":a.duration.saturating_sub(now.saturating_sub(a.issued_at)).as_millis() as u64});
            if let Some(side) = a.side { value["side"] = json!(if side == physics::Side::Left {"left"} else {"right"}); }
            if let Some(direction) = a.direction { value["direction"] = json!(if direction == physics::VerticalDirection::Up {"up"} else {"down"}); }
            value
        });
        json!({"revision":self.revision.to_string(),"rendererEpoch":self.renderer.as_ref().map(|r|r.epoch.as_str()),
            "throwEnabled":self.throw_enabled,"request":action,"status":self.status,"reason":self.reason})
    }
}

fn phase_name(phase: physics::Phase) -> &'static str {
    match phase {
        physics::Phase::Docking => "docking",
        physics::Phase::Perched => "perched",
        physics::Phase::Climbing => "climbing",
        physics::Phase::AtEdge => "atEdge",
        physics::Phase::Thrown => "thrown",
        physics::Phase::Resting => "resting",
    }
}

fn decimal(value: &str) -> Option<u64> {
    (!value.is_empty()
        && value.len() <= 20
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|c| c.is_ascii_digit()))
    .then(|| value.parse().ok())
    .flatten()
}

fn hash(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn pet_only(window: &WebviewWindow) -> Result<(), String> {
    require_p1_development()?;
    if window.label() == "pet" {
        Ok(())
    } else {
        Err("此动作只用于主桌宠窗口".into())
    }
}

fn wake(shared: &Shared) {
    if let Ok(mut wake) = shared.motion_wake.0.try_lock() {
        *wake = true;
        shared.motion_wake.1.notify_one();
    }
}

fn emit(app: &AppHandle, state: Value) {
    let _ = app.emit_to("pet", EVENT, state);
}

/// 先原子撤权，UI/window/源变化回调绝不等待 worker 或物理 owner。
pub(super) fn cancel(shared: &Shared) {
    let _ = shared
        .physics_revision
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1));
    let revision = shared.physics_revision.load(Ordering::Acquire);
    if let Ok(mut owner) = shared.physics.try_lock() {
        owner.sync(revision);
        owner.clear(None);
    }
    wake(shared);
}

pub(super) fn has_work(shared: &Shared) -> bool {
    shared
        .physics
        .try_lock()
        .map(|owner| owner.action.is_some())
        .unwrap_or(true)
}

fn read_skin(shared: &Shared) -> Option<Skin> {
    let _selection = shared.skin_selection.try_lock().ok()?;
    let choice = crate::pet_skin_selection::read(&shared.data_dir);
    if choice.problem.is_some() {
        return None;
    }
    let active = match choice.active {
        Some(active) => active,
        None => {
            // 仅首次/撤销代次重读旧库，正常 200ms 采样不会反复扫描素材。
            let owner = shared.skins.try_lock().ok()?;
            owner
                .as_ref()
                .ok()?
                .state()
                .ok()?
                .selected_skin_id
                .map(ActiveSkin::Png)
                .unwrap_or(ActiveSkin::Builtin)
        }
    };
    Some(match active {
        ActiveSkin::Builtin => Skin {
            key: "builtin".into(),
            kind: RendererKind::Builtin,
        },
        ActiveSkin::Png(id) => Skin {
            key: format!("png:{id}"),
            kind: RendererKind::Png,
        },
        ActiveSkin::Vrm(id) => Skin {
            key: format!("vrm:{id}"),
            kind: RendererKind::Vrm,
        },
    })
}

/// worker 调用：允许路径/SQL读取；不持任何锁发事件或读取 Tauri 窗口。
pub(super) fn poll_work(shared: &Shared) {
    let revision = shared.physics_revision.load(Ordering::Acquire);
    let cached_skin = match shared.physics.try_lock() {
        Ok(owner) if owner.renderer.is_some() => owner
            .skin
            .as_ref()
            .filter(|(r, _)| *r == revision)
            .map(|(_, skin)| skin.clone()),
        _ => return,
    };
    let skin = cached_skin.or_else(|| read_skin(shared));
    let evidence = (|| {
        let skin = skin.clone()?;
        if shared.collector_error.try_lock().ok()?.is_some()
            || shared.watcher_error.try_lock().ok()?.is_some()
        {
            return None;
        }
        let mut core = shared.core.try_lock().ok()?;
        if !core.desktop_motion_idle().ok()? {
            return None;
        }
        let source = core.primary_source().ok()?;
        if source["enabled"].as_bool() != Some(true)
            || source["id"].as_str() != Some(core.source_id.as_str())
            || source["root"].as_str().map(PathBuf::from).as_ref() != Some(&core.source_root)
        {
            return None;
        }
        let kind = source["clientKind"].as_str()?;
        let scope = hash((
            &core.runtime_id,
            &core.source_id,
            &core.source_root,
            kind,
            true,
            &skin.key,
        ));
        Some(Evidence {
            runtime: core.runtime_id.clone(),
            source_id: core.source_id.clone(),
            source_root: core.source_root.clone(),
            changes: core.db.total_changes(),
            scope,
            skin,
            at: shared.started_at.elapsed(),
        })
    })();
    if shared.physics_revision.load(Ordering::Acquire) != revision {
        return;
    }
    if let Ok(mut owner) = shared.physics.try_lock() {
        owner.sync(revision);
        if shared.physics_revision.load(Ordering::Acquire) != revision {
            return;
        }
        if (owner.action.is_some() || owner.drag.is_some())
            && evidence.as_ref().is_none_or(|e| {
                owner.action.as_ref().is_some_and(|a| e.scope != a.scope)
                    || owner
                        .drag
                        .as_ref()
                        .is_some_and(|drag| e.scope != drag.scope)
            })
        {
            owner.clear(Some("任务或来源状态已变化"));
        }
        owner.skin = skin.map(|skin| (revision, skin));
        owner.work = evidence;
    }
}

#[tauri::command]
pub(super) fn register_pet_physics_renderer(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    registration: Registration,
) -> Result<Value, String> {
    pet_only(&window)?;
    if uuid::Uuid::parse_str(&registration.renderer_epoch).is_err()
        || registration.renderer_epoch.len() != 36
        || !resource_matches(&registration.resource_key, registration.renderer_kind)
        || state.exiting.load(Ordering::Acquire)
    {
        return Err("角色物理登记无效".into());
    }
    let handle = raw::handle(&window).ok_or("原生桌宠窗口暂不可用")?;
    cancel(&state);
    let revision = state.physics_revision.load(Ordering::Acquire);
    let token = uuid::Uuid::new_v4().to_string();
    let mut owner = state.physics.try_lock().map_err(|_| lock_error())?;
    owner.sync(revision);
    owner.clear(None);
    owner.permission.register(
        registration.renderer_epoch.clone(),
        token.clone(),
        state.started_at.elapsed(),
    );
    owner.renderer = Some(Renderer {
        epoch: registration.renderer_epoch.clone(),
        token: token.clone(),
        resource_key: registration.resource_key,
        kind: registration.renderer_kind,
        handle,
    });
    owner.contact = None;
    owner.climb_ready = false;
    owner.work = None;
    drop(owner);
    wake(&state);
    Ok(json!({"rendererEpoch":registration.renderer_epoch,"token":token}))
}

fn resource_matches(key: &str, kind: RendererKind) -> bool {
    if kind == RendererKind::Builtin {
        return key == "builtin";
    }
    let prefix = if kind == RendererKind::Png {
        "png:"
    } else {
        "vrm:"
    };
    key.strip_prefix(prefix).is_some_and(|id| {
        id.len() == 64
            && id
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}

#[tauri::command]
pub(super) fn set_pet_physics_readiness(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    readiness: Readiness,
) -> Result<bool, String> {
    pet_only(&window)?;
    let Some(sequence) = decimal(&readiness.sequence) else {
        return Ok(false);
    };
    let mut owner = state.physics.try_lock().map_err(|_| lock_error())?;
    owner.sync(state.physics_revision.load(Ordering::Acquire));
    if !owner.credentials(&readiness.renderer_epoch, &readiness.token) {
        return Ok(false);
    }
    // false 不验证旧 viewport，隐藏/换尺寸时仍必须立刻撤权。
    let ready =
        readiness.ready && readiness.contact.valid() && !state.exiting.load(Ordering::Acquire);
    let ack = readiness
        .ack
        .and_then(|ack| decimal(&ack.generation).map(|generation| (generation, ack.kind)))
        .filter(|ack| {
            owner
                .action
                .as_ref()
                .is_some_and(|a| (a.generation, a.kind) == *ack)
        });
    let flags = motion_guard::Readiness {
        ready,
        gait_available: true,
        gait_ready: ready && ack.is_some(),
    };
    if !owner.permission.accept(
        &readiness.renderer_epoch,
        &readiness.token,
        sequence,
        flags,
        state.started_at.elapsed(),
    ) {
        return Ok(false);
    }
    owner.climb_ready = ready
        && readiness.climb_ready
        && owner
            .renderer
            .as_ref()
            .is_some_and(|r| r.kind == RendererKind::Vrm);
    owner.presented = if ready { ack } else { None };
    if !ready {
        owner.clear(Some("角色尚未就绪"));
    } else {
        owner.contact = Some(readiness.contact);
    }
    drop(owner);
    wake(&state);
    Ok(true)
}

#[tauri::command]
pub(super) fn get_pet_physics_state(
    window: WebviewWindow,
    state: State<'_, SharedState>,
) -> Result<Value, String> {
    pet_only(&window)?;
    let mut owner = state.physics.try_lock().map_err(|_| lock_error())?;
    owner.sync(state.physics_revision.load(Ordering::Acquire));
    Ok(owner.snapshot(state.started_at.elapsed()))
}

#[tauri::command]
pub(super) fn request_pet_physics(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    request: ActionRequest,
) -> Result<Value, String> {
    pet_only(&window)?;
    let duration = request.duration_ms.unwrap_or(8_000);
    if duration == 0 || duration > 8_000 || request.kind == Kind::Throw {
        return Err("动作类型或时长无效".into());
    }
    let (side, direction) = match request.kind {
        Kind::Dock if request.side.is_none() && request.direction.is_none() => (None, None),
        Kind::Climb => (
            Some(match request.side.as_deref() {
                Some("left") => physics::Side::Left,
                Some("right") => physics::Side::Right,
                _ => return Err("请选择有效攀爬边缘".into()),
            }),
            Some(match request.direction.as_deref() {
                Some("up") => physics::VerticalDirection::Up,
                Some("down") => physics::VerticalDirection::Down,
                _ => return Err("请选择有效攀爬方向".into()),
            }),
        ),
        _ => return Err("动作参数无效".into()),
    };
    let shared = state.inner().clone();
    let issued_at = shared.started_at.elapsed();
    {
        let owner = shared.physics.try_lock().map_err(|_| lock_error())?;
        if !owner.credentials(&request.renderer_epoch, &request.token) {
            return Err("角色登记已失效".into());
        }
    }
    popup_on_main(window.app_handle(), move |app| {
        let view = app.get_webview_window("pet").ok_or("主桌宠窗口已关闭")?;
        dismiss_pet_popup_on_main(&view, &shared, "physics", true)?;
        let now = shared.started_at.elapsed();
        let marker = native::motion_input_marker().ok_or("原生输入状态未知")?;
        let handle = raw::handle(&view).ok_or("原生桌宠窗口暂不可用")?;
        let origin = raw::snapshot(handle).ok_or("桌宠几何暂不可用")?;
        // menu 的展开/恢复属于请求之前的几何操作；新动作冻结恢复后的真实快照。
        // 后续外部移动仍由原生位置匹配、取消代次和新的 invalidated 标记撤销。
        shared.motion_invalidated.store(false, Ordering::Release);
        let mut owner = shared.physics.try_lock().map_err(|_| lock_error())?;
        let revision = shared.physics_revision.load(Ordering::Acquire);
        owner.sync(revision);
        if !owner.credentials(&request.renderer_epoch, &request.token) {
            return Err("角色登记已失效".into());
        }
        if now
            .checked_sub(issued_at)
            .is_none_or(|age| age >= Duration::from_millis(duration))
        {
            return Err("动作请求已过期".into());
        }
        let evidence = owner
            .fresh_evidence(now)
            .ok_or("任务、来源或角色状态尚未确认")?
            .clone();
        if request.kind == Kind::Climb
            && (!owner.climb_ready || evidence.skin.kind != RendererKind::Vrm)
        {
            return Err("当前角色没有可用攀爬姿态".into());
        }
        drop(owner);
        // 原生几何在任何 physics/settings/Core 锁外读取；宿主冻结真实动作前位置，
        // 与最终位置 writer 共用 settings 序列，不能保存后来的自动终点。
        let anchor = freeze_position_anchor_on_main(&view, &shared)?;
        let admitted = (|| -> Result<Value, String> {
            let now = shared.started_at.elapsed();
            let mut owner = shared.physics.try_lock().map_err(|_| lock_error())?;
            owner.sync(shared.physics_revision.load(Ordering::Acquire));
            if !owner.credentials(&request.renderer_epoch, &request.token)
                || owner
                    .fresh_evidence(now)
                    .is_none_or(|current| current.scope != evidence.scope)
                || shared.physics_revision.load(Ordering::Acquire) != revision
                || now
                    .checked_sub(issued_at)
                    .is_none_or(|age| age >= Duration::from_millis(duration))
                || raw::snapshot(handle).is_none_or(|current| {
                    current.geometry != origin.geometry || current.position != origin.position
                })
            {
                return Err("动作前的角色、来源或位置已变化，请重新选择".into());
            }
            owner.clear(None);
            let generation = owner.next()?;
            owner.action = Some(Action {
                generation,
                kind: request.kind,
                side,
                direction,
                issued_at,
                duration: Duration::from_millis(duration),
                marker,
                revision,
                scope: evidence.scope,
                release: None,
                origin,
            });
            owner.status = "waitingForFrame";
            Ok(owner.snapshot(now))
        })();
        complete_position_anchor(&shared, anchor, admitted.is_ok());
        let result = admitted?;
        if let Ok(mut motion) = shared.motion.try_lock() {
            motion.cancel(false);
        }
        interrupt_motion(&app, &shared, "interaction");
        emit(&app, result.clone());
        wake(&shared);
        Ok(result)
    })
}

#[tauri::command]
pub(super) fn set_pet_throw_enabled(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    renderer_epoch: String,
    token: String,
    enabled: bool,
) -> Result<Value, String> {
    pet_only(&window)?;
    let now = state.started_at.elapsed();
    let mut owner = state.physics.try_lock().map_err(|_| lock_error())?;
    owner.sync(state.physics_revision.load(Ordering::Acquire));
    if !owner.credentials(&renderer_epoch, &token) {
        return Err("角色登记已失效".into());
    }
    if enabled
        && (!owner.permission.readiness(now).is_some_and(|r| r.ready)
            || owner.fresh_evidence(now).is_none())
    {
        return Err("角色与任务状态尚未就绪".into());
    }
    owner.throw_enabled = enabled;
    if !enabled {
        owner.tracker.cancel();
        owner.drag = None;
        if owner.action.as_ref().is_some_and(|a| a.kind == Kind::Throw) {
            owner.clear(None);
        }
    }
    let result = owner.snapshot(now);
    drop(owner);
    emit(window.app_handle(), result.clone());
    wake(&state);
    Ok(result)
}

#[tauri::command]
pub(super) fn cancel_pet_physics(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    renderer_epoch: String,
    token: String,
) -> Result<Value, String> {
    pet_only(&window)?;
    let mut owner = state.physics.try_lock().map_err(|_| lock_error())?;
    if !owner.credentials(&renderer_epoch, &token) {
        return Err("角色登记已失效".into());
    }
    // 凭据检查和原子撤权在同一 owner 临界区，旧 renderer 的取消不能穿过新登记。
    let _ = state
        .physics_revision
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1));
    owner.sync(state.physics_revision.load(Ordering::Acquire));
    owner.clear(None);
    let result = owner.snapshot(state.started_at.elapsed());
    drop(owner);
    emit(window.app_handle(), result.clone());
    wake(&state);
    Ok(result)
}

fn release_sample(input: manual_drag::Input, now: Duration) -> physics::ReleaseSample {
    physics::ReleaseSample {
        cursor: input.cursor.map(|p| physics::Point {
            x: f64::from(p.x),
            y: f64::from(p.y),
        }),
        left_down: input.left_down,
        escape_down: input.escape_down,
        valid: input.visible && input.runtime_available,
        now,
    }
}

/// 只在原生 Controller::start 成功之后调用，不能从 JS 的 press/velocity 授权。
pub(super) fn begin_drag(shared: &Shared, input: manual_drag::Input, now: Duration) {
    let previous = shared.physics.try_lock().ok().and_then(|owner| {
        owner
            .fresh_evidence(now)
            .cloned()
            .map(|evidence| (evidence, owner.cancellation_revision))
    });
    cancel(shared);
    let Ok(mut owner) = shared.physics.try_lock() else {
        return;
    };
    owner.sync(shared.physics_revision.load(Ordering::Acquire));
    // 手势自身不改变来源：仅跨越本次原子撤销保留刚验证的事实；其他撤销不复用。
    let Some((evidence, prior_revision)) =
        previous.filter(|(_, prior)| prior.checked_add(1) == Some(owner.cancellation_revision))
    else {
        return;
    };
    if !owner.throw_enabled
        || !owner.permission.readiness(now).is_some_and(|r| r.ready)
        || !p1_development_enabled()
        || !native::motion_environment_allowed()
    {
        return;
    }
    let _ = prior_revision;
    owner.skin = Some((owner.cancellation_revision, evidence.skin.clone()));
    owner.work = Some(evidence.clone());
    if owner.tracker.begin(release_sample(input, now)) {
        let resource_key = owner
            .renderer
            .as_ref()
            .map(|r| r.resource_key.clone())
            .unwrap_or_default();
        owner.drag = Some(DragOrigin {
            revision: owner.cancellation_revision,
            scope: evidence.scope,
            resource_key,
        });
    }
}

/// Controller 仍为该真实拖动的 owner 时采样；Released 也必须先送入本函数。
pub(super) fn capture_drag_input(shared: &Shared, input: manual_drag::Input, now: Duration) {
    let Ok(mut owner) = shared.physics.try_lock() else {
        return;
    };
    owner.sync(shared.physics_revision.load(Ordering::Acquire));
    let Some(drag) = owner.drag.clone() else {
        return;
    };
    let evidence = owner.fresh_evidence(now).cloned();
    if !owner.throw_enabled
        || !owner.permission.readiness(now).is_some_and(|r| r.ready)
        || drag.revision != owner.cancellation_revision
        || evidence.as_ref().is_none_or(|e| e.scope != drag.scope)
        || owner
            .renderer
            .as_ref()
            .is_none_or(|r| r.resource_key != drag.resource_key)
        || !native::motion_environment_allowed()
    {
        owner.tracker.cancel();
        owner.drag = None;
        return;
    }
    let Some(token) = owner.tracker.sample(release_sample(input, now)) else {
        return;
    };
    owner.drag = None;
    let Some(evidence) = evidence.filter(|e| e.scope == drag.scope) else {
        return;
    };
    let Some(marker) = native::motion_input_marker() else {
        return;
    };
    let Some(origin) = owner
        .renderer
        .as_ref()
        .and_then(|r| raw::snapshot(r.handle))
    else {
        return;
    };
    let Ok(generation) = owner.next() else {
        return;
    };
    owner.planner.cancel();
    owner.active = None;
    owner.presented = None;
    owner.action = Some(Action {
        generation,
        kind: Kind::Throw,
        side: None,
        direction: None,
        issued_at: token.issued_at(),
        duration: ACTION_LIMIT,
        marker,
        revision: drag.revision,
        scope: evidence.scope,
        release: Some(token),
        origin,
    });
    owner.status = "waitingForFrame";
    owner.reason = None;
    drop(owner);
    wake(shared);
}

#[derive(Clone, Copy)]
struct NativeSnapshot {
    geometry: physics::Geometry,
    position: physics::Point,
    client: physics::Size,
    client_offset: physics::Point,
}

fn surface_rect(geometry: physics::Geometry) -> surfaces::Rect {
    let r = geometry.work_area;
    surfaces::Rect {
        left: r.left as i32,
        top: r.top as i32,
        right: r.right as i32,
        bottom: r.bottom as i32,
    }
}

fn whole_surface(surface: &surfaces::Snapshot) -> bool {
    let edges = surface.edges();
    edges.top && edges.left && edges.right && edges.bottom
}

fn mapped_surface(
    surface: &surfaces::Snapshot,
    action: &Action,
    inset: Insets,
    started: Instant,
) -> Option<physics::Surface> {
    if !whole_surface(surface) {
        return None;
    }
    let raw = surface.rect();
    let shift = match (action.kind, action.side) {
        (Kind::Climb, Some(physics::Side::Left)) => inset.right,
        (Kind::Climb, Some(physics::Side::Right)) => -inset.left,
        _ => 0.0,
    };
    let (left, right) = if matches!(action.kind, Kind::Dock | Kind::Throw) {
        (
            f64::from(raw.left) - inset.left,
            f64::from(raw.right) + inset.right,
        )
    } else {
        (f64::from(raw.left) + shift, f64::from(raw.right) + shift)
    };
    Some(physics::Surface {
        id: surface.signature().opaque_id(),
        generation: surface.signature().opaque_id(),
        rect: physics::Rect {
            left,
            right,
            top: f64::from(raw.top) + inset.bottom,
            bottom: f64::from(raw.bottom) + inset.bottom,
        },
        observed_at: surface.observed_at().checked_duration_since(started)?,
    })
}

fn ready(owner: &mut Runtime, action: &Action, now: Duration) -> bool {
    owner
        .permission
        .readiness(now)
        .is_some_and(|r| r.ready && r.gait_ready)
        && owner.presented == Some((action.generation, action.kind))
        && (action.kind != Kind::Climb || owner.climb_ready)
}

fn core_matches(core: &mut Core, evidence: &Evidence, now: Duration) -> bool {
    now.checked_sub(evidence.at)
        .is_some_and(|age| age < CACHE_TTL)
        && core.runtime_id == evidence.runtime
        && core.source_id == evidence.source_id
        && core.source_root == evidence.source_root
        && core.db.total_changes() == evidence.changes
        && core.desktop_motion_cached_idle()
}

fn native_gate(shared: &Shared, handle: usize, marker: u32) -> bool {
    p1_development_enabled()
        && shared.physics_revision.load(Ordering::Acquire) != u64::MAX
        && !shared.stop.load(Ordering::Acquire)
        && !shared.exiting.load(Ordering::Acquire)
        && !shared.native_paused.load(Ordering::Acquire)
        && !shared.native_dragging.load(Ordering::Acquire)
        && !shared.display_recheck.load(Ordering::Acquire)
        && !native::mouse_down()
        && raw::visible(handle)
        && native::motion_environment_allowed()
        && native::motion_input_marker() == Some(marker)
        && shared
            .pet_interaction
            .try_lock()
            .is_ok_and(|interaction| !interaction.active)
        && shared.collector_error.try_lock().is_ok_and(|e| e.is_none())
        && shared.watcher_error.try_lock().is_ok_and(|e| e.is_none())
}

/// true 表示本 tick 已由物理 owner 接管（包括等待/阻断），root 不能再执行旧 roaming。
pub(super) fn step_on_main(app: &AppHandle, shared: &Shared) -> bool {
    // 没有用户动作时不观察外部前台窗口；锁忙时保守占住本 tick，避免另一个移动 owner。
    let active = match shared.physics.try_lock() {
        Ok(mut owner) => {
            owner.sync(shared.physics_revision.load(Ordering::Acquire));
            owner.action.is_some()
        }
        Err(_) => return true,
    };
    if !active {
        return false;
    }
    let Some(window) = app.get_webview_window("pet") else {
        cancel(shared);
        return false;
    };
    let Some(handle) = raw::handle(&window) else {
        cancel(shared);
        return false;
    };
    let Some(initial) = raw::snapshot(handle) else {
        cancel(shared);
        return false;
    };
    let current_surface = surfaces::sample(surface_rect(initial.geometry)).filter(whole_surface);
    let result = step_locked(shared, handle, initial, current_surface);
    if let Some(state) = result.1 {
        emit(app, state);
    }
    result.0
}

fn step_locked(
    shared: &Shared,
    handle: usize,
    initial: NativeSnapshot,
    current_surface: Option<surfaces::Snapshot>,
) -> (bool, Option<Value>) {
    let Ok(mut owner) = shared.physics.try_lock() else {
        return (true, None);
    };
    owner.sync(shared.physics_revision.load(Ordering::Acquire));
    let Some(action) = owner.action.clone() else {
        return (false, None);
    };
    let now = shared.started_at.elapsed();
    macro_rules! blocked {
        ($reason:expr) => {{
            owner.clear(Some($reason));
            return (true, Some(owner.snapshot(shared.started_at.elapsed())));
        }};
    }
    if !action.alive(now) {
        owner.clear(None);
        return (true, Some(owner.snapshot(now)));
    }
    if owner.renderer.as_ref().is_none_or(|r| r.handle != handle) {
        blocked!("原生桌宠实例已变化");
    }
    if action.revision != shared.physics_revision.load(Ordering::Acquire) {
        blocked!("动作已取消");
    }
    let Ok(_settings) = shared.settings_update.try_lock() else {
        blocked!("设置正在变化");
    };
    let Ok(_selection) = shared.skin_selection.try_lock() else {
        blocked!("角色正在变化");
    };
    let Ok(settings) = shared.projection.try_lock() else {
        blocked!("设置状态未知");
    };
    if !flag(&settings, "petVisible", true) || companion::settings_pause(&settings).is_some() {
        blocked!("桌宠已暂停");
    }
    let Some(evidence) = owner
        .fresh_evidence(now)
        .cloned()
        .filter(|e| e.scope == action.scope)
    else {
        blocked!("任务或来源状态未知");
    };
    let Ok(mut core) = shared.core.try_lock() else {
        blocked!("任务状态正在变化");
    };
    if !core_matches(&mut core, &evidence, now) || !native_gate(shared, handle, action.marker) {
        blocked!("任务、输入或系统状态不允许移动");
    }
    if !owner.permission.readiness(now).is_some_and(|r| r.ready)
        || (action.kind == Kind::Climb && !owner.climb_ready)
    {
        blocked!("角色渲染许可已过期或失效");
    }
    // 真实拖动只留下旧 roaming 的失效标记；物理 action 自己冻结了原生位置/输入代次。
    if shared.motion_invalidated.swap(false, Ordering::AcqRel) {
        if action.kind != Kind::Throw || owner.active.is_some() {
            blocked!("窗口位置或交互已变化");
        }
    }
    let Some(contact) = owner.contact.clone() else {
        owner.status = "waitingForFrame";
        return (true, Some(owner.snapshot(now)));
    };
    let Some(inset) = contact.physical(initial) else {
        if owner.active.is_some() {
            blocked!("角色视口或 DPI 已变化");
        }
        owner.status = "waitingForFrame";
        return (true, Some(owner.snapshot(now)));
    };
    if owner.active.is_none()
        && (initial.geometry != action.origin.geometry
            || initial.position != action.origin.position)
    {
        blocked!("请求后桌宠几何或位置已变化");
    }
    if owner.active.is_none() {
        let source = current_surface.clone();
        if action.kind != Kind::Throw && source.is_none() {
            blocked!("没有完整可用的普通前台窗口边缘");
        }
        let surface = source
            .as_ref()
            .and_then(|s| mapped_surface(s, &action, inset, shared.started_at));
        let kind = match action.kind {
            Kind::Dock => physics::MotionKind::DockTop {
                surface_id: surface.as_ref().expect("已检查平台").id,
            },
            Kind::Climb => physics::MotionKind::Climb {
                surface_id: surface.as_ref().expect("已检查平台").id,
                side: action.side.expect("已检查边缘"),
                direction: action.direction.expect("已检查方向"),
            },
            Kind::Throw => physics::MotionKind::Throw {
                release: action.release.expect("仅原生松手生成"),
            },
        };
        let renderer = owner.renderer.as_ref().expect("证据包含渲染登记");
        let request = physics::Request {
            scope: physics::Scope {
                pet: hash("pet"),
                source: evidence.scope,
                renderer: hash((&renderer.epoch, &renderer.resource_key)),
            },
            epoch: owner.epoch,
            generation: action.generation,
            issued_at: action.issued_at,
            duration: action.duration,
            kind,
        };
        owner.active = Some(Active {
            request,
            geometry: initial.geometry,
            initial_position: initial.position,
            contact: contact.clone(),
            surface: source,
        });
    }
    let active = owner.active.as_ref().expect("已创建动作");
    if active.geometry != initial.geometry || active.contact != contact {
        blocked!("角色几何已变化");
    }
    if owner.planner.state().is_none() && active.initial_position != initial.position {
        blocked!("桌宠已被外部移动");
    }
    let frozen_surface = active.surface.clone();
    if match (frozen_surface.as_ref(), current_surface.as_ref()) {
        (Some(expected), Some(current)) => expected.signature() != current.signature(),
        (None, None) => false,
        _ => true,
    } {
        blocked!("前台窗口边缘已变化");
    }
    let request = active.request;
    let surface = current_surface
        .as_ref()
        .and_then(|s| mapped_surface(s, &action, inset, shared.started_at));
    if !ready(&mut owner, &action, now) {
        if owner.planner.state().is_some() {
            blocked!("当前动作的真实画面已失效");
        }
        owner.status = "waitingForFrame";
        return (true, Some(owner.snapshot(now)));
    }
    let observation = physics::Observation {
        now,
        gate: physics::Gate::Allowed,
        input_marker: Some(action.marker),
        request: Some(request),
        geometry: Some(initial.geometry),
        surface,
        position: Some(initial.position),
    };
    let intent = owner.planner.step(observation);
    if owner.planner.state().is_none() {
        blocked!("当前边缘距离、位置或松手候选不支持该动作");
    }
    owner.status = "moving";
    owner.reason = None;
    let Some(intent) = intent else {
        return (true, Some(owner.snapshot(now)));
    };
    let Some(fresh) = raw::snapshot(handle) else {
        blocked!("窗口几何暂不可用");
    };
    let final_now = shared.started_at.elapsed();
    let fresh_surface = surfaces::sample(surface_rect(fresh.geometry)).filter(whole_surface);
    let surface_current = match (fresh_surface.as_ref(), frozen_surface.as_ref()) {
        (None, None) => true,
        (Some(fresh), Some(prior)) => {
            fresh.signature() == prior.signature()
                && fresh.revalidate(surface_rect(initial.geometry))
        }
        _ => false,
    };
    let allowed = action.revision == shared.physics_revision.load(Ordering::Acquire)
        && !shared.motion_invalidated.load(Ordering::Acquire)
        && ready(&mut owner, &action, final_now)
        && core_matches(&mut core, &evidence, final_now)
        && native_gate(shared, handle, action.marker)
        && surface_current;
    let current = physics::Observation {
        now: final_now,
        gate: if allowed {
            physics::Gate::Allowed
        } else {
            physics::Gate::Blocked
        },
        input_marker: native::motion_input_marker(),
        request: Some(request),
        geometry: Some(fresh.geometry),
        position: Some(fresh.position),
        surface: fresh_surface
            .as_ref()
            .and_then(|s| mapped_surface(s, &action, inset, shared.started_at)),
    };
    // 取出 planner 后，callback 可以同时重验当前 lease；拒绝/失败都会销毁整段。
    let mut planner = std::mem::take(&mut owner.planner);
    let outcome = planner.apply_if_current(intent, current, |target| {
        let at = shared.started_at.elapsed();
        if action.revision != shared.physics_revision.load(Ordering::Acquire)
            || shared.motion_invalidated.load(Ordering::Acquire)
            || at
                .checked_sub(observation.now)
                .is_none_or(|age| age > physics::MAX_STEP)
            || !ready(&mut owner, &action, at)
            || !core_matches(&mut core, &evidence, at)
            || !native_gate(shared, handle, action.marker)
            || raw::snapshot(handle)
                .is_none_or(|s| s.geometry != fresh.geometry || s.position != fresh.position)
            || fresh_surface
                .as_ref()
                .is_some_and(|s| !s.revalidate(surface_rect(fresh.geometry)))
        {
            return false;
        }
        let target = roaming::Point {
            x: target.x as i32,
            y: target.y as i32,
        };
        let Ok(mut positions) = shared.automatic_positions.try_lock() else {
            return false;
        };
        if !positions.tag(target, at) {
            return false;
        }
        drop(positions);
        let applied = raw::move_window(handle, target);
        if !applied {
            if let Ok(mut positions) = shared.automatic_positions.try_lock() {
                positions.failed(target);
            }
        }
        applied
    });
    owner.planner = planner;
    if outcome != physics::ApplyOutcome::Applied {
        blocked!("动作在执行前已撤销或原生移动失败");
    }
    (true, Some(owner.snapshot(shared.started_at.elapsed())))
}

#[cfg(windows)]
mod raw {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL,
        },
        System::Threading::GetCurrentProcessId,
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                GetClientRect, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
                SetWindowPos, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
            },
        },
    };
    pub(super) fn handle(window: &WebviewWindow) -> Option<usize> {
        Some(window.hwnd().ok()?.0 as usize)
    }
    pub(super) fn visible(handle: usize) -> bool {
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(handle as HWND, &mut pid) != 0
                && pid == GetCurrentProcessId()
                && IsWindowVisible(handle as HWND) != 0
                && IsIconic(handle as HWND) == 0
        }
    }
    pub(super) fn snapshot(handle: usize) -> Option<NativeSnapshot> {
        unsafe {
            let hwnd = handle as HWND;
            let mut rect: RECT = std::mem::zeroed();
            let mut client: RECT = std::mem::zeroed();
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
            let dpi = GetDpiForWindow(hwnd);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if monitor.is_null()
                || dpi == 0
                || GetWindowRect(hwnd, &mut rect) == 0
                || GetClientRect(hwnd, &mut client) == 0
                || GetMonitorInfoW(monitor, &mut info) == 0
            {
                return None;
            }
            let mut origin = POINT {
                x: client.left,
                y: client.top,
            };
            if ClientToScreen(hwnd, &mut origin) == 0 {
                return None;
            }
            let width = f64::from(rect.right) - f64::from(rect.left);
            let height = f64::from(rect.bottom) - f64::from(rect.top);
            let cw = f64::from(client.right) - f64::from(client.left);
            let ch = f64::from(client.bottom) - f64::from(client.top);
            let offset = physics::Point {
                x: f64::from(origin.x) - f64::from(rect.left),
                y: f64::from(origin.y) - f64::from(rect.top),
            };
            if width < 1.0
                || height < 1.0
                || cw < 1.0
                || ch < 1.0
                || offset.x < 0.0
                || offset.y < 0.0
                || offset.x + cw > width
                || offset.y + ch > height
            {
                return None;
            }
            Some(NativeSnapshot {
                geometry: physics::Geometry {
                    work_area: physics::Rect {
                        left: f64::from(info.rcWork.left),
                        top: f64::from(info.rcWork.top),
                        right: f64::from(info.rcWork.right),
                        bottom: f64::from(info.rcWork.bottom),
                    },
                    window_size: physics::Size { width, height },
                    scale_factor: f64::from(dpi) / 96.0,
                    signature: monitor as usize as u64,
                },
                position: physics::Point {
                    x: f64::from(rect.left),
                    y: f64::from(rect.top),
                },
                client: physics::Size {
                    width: cw,
                    height: ch,
                },
                client_offset: offset,
            })
        }
    }
    pub(super) fn move_window(handle: usize, target: roaming::Point) -> bool {
        unsafe {
            SetWindowPos(
                handle as HWND,
                std::ptr::null_mut(),
                target.x,
                target.y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSIZE | SWP_NOZORDER,
            ) != 0
        }
    }
}

#[cfg(not(windows))]
mod raw {
    use super::*;
    pub(super) fn handle(_: &WebviewWindow) -> Option<usize> {
        None
    }
    pub(super) fn visible(_: usize) -> bool {
        false
    }
    pub(super) fn snapshot(_: usize) -> Option<NativeSnapshot> {
        None
    }
    pub(super) fn move_window(_: usize, _: roaming::Point) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn native_fixture() -> NativeSnapshot {
        NativeSnapshot {
            geometry: physics::Geometry {
                work_area: physics::Rect {
                    left: -1000.0,
                    top: -1000.0,
                    right: 1000.0,
                    bottom: 1000.0,
                },
                window_size: physics::Size {
                    width: 480.0,
                    height: 640.0,
                },
                scale_factor: 2.0,
                signature: 1,
            },
            position: physics::Point { x: 0.0, y: 0.0 },
            client: physics::Size {
                width: 480.0,
                height: 640.0,
            },
            client_offset: physics::Point { x: 0.0, y: 0.0 },
        }
    }
    fn action_fixture(kind: Kind) -> Action {
        Action {
            generation: 3,
            kind,
            side: None,
            direction: None,
            issued_at: Duration::ZERO,
            duration: ACTION_LIMIT,
            marker: 1,
            revision: 0,
            scope: 1,
            release: None,
            origin: native_fixture(),
        }
    }
    #[test]
    fn strict_payloads_and_contact_bounds_reject_untrusted_velocity_and_bad_insets() {
        assert!(serde_json::from_value::<ActionRequest>(
            json!({"rendererEpoch":"e","token":"t","kind":"throw","velocity":{"x":1,"y":2}})
        )
        .is_err());
        let mut contact = Contact {
            viewport_width: 240.0,
            viewport_height: 320.0,
            left_inset: 20.0,
            right_inset: 20.0,
            bottom_inset: 40.0,
        };
        assert!(contact.valid());
        contact.bottom_inset = 241.0;
        assert!(!contact.valid());
        contact.bottom_inset = 40.0;
        contact.left_inset = f64::NAN;
        assert!(!contact.valid());
        assert_eq!(decimal("01"), None);
        assert_eq!(decimal("18446744073709551616"), None);
        assert!(!resource_matches("vrm:../x", RendererKind::Vrm));
    }
    #[test]
    fn cancellation_epoch_and_default_throw_are_fail_closed() {
        let mut owner = Runtime::default();
        assert!(!owner.throw_enabled);
        owner.sync(8);
        assert_eq!(owner.cancellation_revision, 8);
        assert!(owner.work.is_none());
        assert!(owner.action.is_none() && owner.planner.state().is_none());
        owner.epoch = u64::MAX;
        assert!(owner.next().is_err());
    }
    #[test]
    fn native_state_preserves_optional_action_fields_and_exact_generation_strings() {
        let mut owner = Runtime::default();
        owner.action = Some(action_fixture(Kind::Dock));
        let state = owner.snapshot(Duration::from_secs(1));
        assert_eq!(state["request"]["generation"], "3");
        assert_eq!(state["request"]["kind"], "dock");
        assert_eq!(state["request"]["remainingMs"], 7000);
        assert!(state["request"].get("side").is_none());
        assert!(state["request"].get("direction").is_none());
        let action = owner.action.as_mut().unwrap();
        action.kind = Kind::Climb;
        action.side = Some(physics::Side::Right);
        action.direction = Some(physics::VerticalDirection::Up);
        let next = owner.snapshot(Duration::from_secs(1));
        assert_eq!(next["request"]["side"], "right");
        assert_eq!(next["request"]["direction"], "up");
        assert!(
            decimal(next["revision"].as_str().unwrap())
                > decimal(state["revision"].as_str().unwrap())
        );
    }
    #[test]
    fn contact_is_local_css_validated_against_current_native_client_and_dpi() {
        let mut contact = Contact {
            viewport_width: 240.0,
            viewport_height: 320.0,
            left_inset: 20.0,
            right_inset: 30.0,
            bottom_inset: 40.0,
        };
        let insets = contact.physical(native_fixture()).unwrap();
        assert_eq!(
            (insets.left, insets.right, insets.bottom),
            (40.0, 60.0, 80.0)
        );
        contact.viewport_width = 243.0;
        assert!(
            contact.physical(native_fixture()).is_none(),
            "旧的扩展菜单视口不能成为恢复后小窗口的接触点"
        );
        contact.viewport_width = 240.0;
        let mut different_dpi = native_fixture();
        different_dpi.geometry.scale_factor = 1.5;
        assert!(contact.physical(different_dpi).is_none());
    }
    #[test]
    fn current_kind_generation_and_finite_native_lease_are_all_required_for_frame_permission() {
        let mut owner = Runtime::default();
        let action = action_fixture(Kind::Dock);
        owner
            .permission
            .register("renderer".into(), "native-token".into(), Duration::ZERO);
        assert!(owner.permission.accept(
            "renderer",
            "native-token",
            1,
            motion_guard::Readiness {
                ready: true,
                gait_available: true,
                gait_ready: true,
            },
            Duration::ZERO
        ));
        owner.presented = Some((action.generation, Kind::Throw));
        assert!(!ready(&mut owner, &action, Duration::ZERO));
        owner.presented = Some((action.generation - 1, Kind::Dock));
        assert!(!ready(&mut owner, &action, Duration::ZERO));
        owner.presented = Some((action.generation, Kind::Dock));
        assert!(ready(&mut owner, &action, Duration::from_millis(1499)));
        assert!(!ready(&mut owner, &action, Duration::from_millis(1500)));
        owner.clear(None);
        assert!(owner.presented.is_none());
        assert!(!ready(&mut owner, &action, Duration::from_millis(1501)));
    }
}
