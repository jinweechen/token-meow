//! Finite desktop shell. Codex files and credentials are never written here.
#[path = "desktop_companion.rs"]
mod companion;
#[path = "desktop_roaming.rs"]
mod roaming;
#[path = "desktop_motion_guard.rs"]
mod motion_guard;
#[path = "csv_export.rs"]
mod csv_export;
#[path = "csv_export_directory.rs"]
mod csv_export_directory;
#[path = "panel_navigation.rs"]
mod panel_navigation_state;
#[path = "desktop_pointer.rs"]
mod pointer;
#[path = "desktop_popup_bounds.rs"]
mod popup_bounds;
#[path = "desktop_drag.rs"]
mod manual_drag;

// BEGIN PURE DRAG HANDOFF
/// Native work is supplied by the desktop adapter. Admission has no popup side
/// effects; once geometry starts, final notifications also run on cancellation.
mod drag_handoff {
    pub fn run<C, E>(
        context: &mut C,
        admit: impl FnOnce(&mut C) -> Result<(), E>,
        geometry: impl FnOnce(&mut C) -> Result<(), E>,
        first_step: impl FnOnce(&mut C) -> Result<(), E>,
        finish: impl FnOnce(&mut C) -> Result<(), E>,
    ) -> Result<(), E> {
        admit(context)?;
        let result = geometry(context).and_then(|_| first_step(context));
        let final_result = finish(context);
        result.and(final_result)
    }

    #[cfg(test)]
    mod tests {
        use super::run;

        #[derive(Default)]
        struct Host { calls: Vec<&'static str>, released: bool, paused: bool, moved: bool }
        fn admit(host: &mut Host) -> Result<(), &'static str> {
            host.calls.push("admit");
            if host.released { Err("released") } else { Ok(()) }
        }
        fn geometry(host: &mut Host) -> Result<(), &'static str> {
            host.calls.push("geometry"); Ok(())
        }
        fn step(host: &mut Host) -> Result<(), &'static str> {
            host.calls.push("resample");
            if host.released || host.paused { return Err("cancelled"); }
            host.calls.push("move"); host.moved = true; Ok(())
        }
        fn finish(host: &mut Host) -> Result<(), &'static str> {
            host.calls.push("notify-and-metrics"); Ok(())
        }
        #[test]
        fn released_request_neither_restores_popup_nor_moves_or_notifies() {
            let mut host = Host { released: true, ..Host::default() };
            assert_eq!(run(&mut host, admit, geometry, step, finish), Err("released"));
            assert_eq!(host.calls, ["admit"]);
            assert!(!host.moved);
        }
        #[test]
        fn first_movement_precedes_popup_notifications_and_metrics() {
            let mut host = Host::default();
            assert_eq!(run(&mut host, admit, geometry, step, finish), Ok(()));
            assert_eq!(host.calls, ["admit", "geometry", "resample", "move", "notify-and-metrics"]);
        }
        #[test]
        fn release_during_geometry_cancels_movement_but_finishes_popup() {
            let mut host = Host::default();
            let restore = |host: &mut Host| { geometry(host)?; host.released = true; Ok(()) };
            assert_eq!(run(&mut host, admit, restore, step, finish), Err("cancelled"));
            assert_eq!(host.calls, ["admit", "geometry", "resample", "notify-and-metrics"]);
            assert!(!host.moved);
        }
        #[test]
        fn pause_during_geometry_cannot_authorize_first_movement() {
            let mut host = Host::default();
            let restore = |host: &mut Host| { geometry(host)?; host.paused = true; Ok(()) };
            assert_eq!(run(&mut host, admit, restore, step, finish), Err("cancelled"));
            assert!(!host.moved);
            assert_eq!(host.calls.last(), Some(&"notify-and-metrics"));
        }
        #[test]
        fn native_geometry_failure_still_finishes_and_preserves_original_error() {
            let mut host = Host::default();
            let restore = |host: &mut Host| { host.calls.push("geometry-failed"); Err("geometry") };
            let finalizer = |host: &mut Host| { finish(host)?; Err("notification") };
            assert_eq!(run(&mut host, admit, restore, step, finalizer), Err("geometry"));
            assert_eq!(host.calls, ["admit", "geometry-failed", "notify-and-metrics"]);
        }
    }
}
// END PURE DRAG HANDOFF

use crate::core::adapter::{adapter_for, SourceConfig};
use crate::{account_bridge::AccountBridge, core::Core};
use crate::notification_gate::{self, Gate, QueuedNotification};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Condvar, Mutex, TryLockError,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, State, WebviewWindow,
    WindowEvent,
};
use tauri_plugin_dialog::DialogExt;

const RECOVERY_SHORTCUT: &str = "Ctrl + Alt + M";
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(5);
const CSV_EXPORT_BUSY: &str = "已有导出正在进行，请先完成或取消保存窗口。";
fn p1_development_enabled() -> bool {
    std::env::var_os("TOKEN_MEOW_P1_DEVELOPMENT").as_deref() == Some(std::ffi::OsStr::new("1"))
}
fn require_p1_development() -> Result<(), String> {
    if p1_development_enabled() { Ok(()) }
    else { Err("本地扩展正在开发验收，当前发行入口尚未开放".into()) }
}
#[tauri::command]
fn get_pet_extension_capabilities() -> Value {
    json!({"enabled":p1_development_enabled(),"nativeQualified":false})
}

#[tauri::command]
async fn get_pet_skin_state(state: State<'_, SharedState>) -> Result<crate::local_pet_skin::PetSkinState, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || shared.skins.lock().map_err(|_| lock_error())?.as_mut().map_err(|error| error.clone())?.state())
        .await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_import_pet_skin(app: AppHandle, state: State<'_, SharedState>)
    -> Result<Option<crate::local_pet_skin::LocalPetSkinPackage>, String>
{
    require_p1_development()?;
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let source = app.dialog().file().set_title("导入本地 PNG 皮肤目录（含 manifest.json）").blocking_pick_folder();
        let Some(source) = source else { return Ok(None); };
        let source = source.into_path().map_err(|e| e.to_string())?;
        let package = shared.skins.lock().map_err(|_| lock_error())?.as_mut().map_err(|error| error.clone())?.import_directory(&source)?;
        app.emit("pet-skin-library-changed", json!({"kind":"imported"})).map_err(|e| e.to_string())?;
        Ok(Some(package))
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn select_pet_skin(app: AppHandle, state: State<'_, SharedState>, skin_id: Option<String>)
    -> Result<crate::local_pet_skin::PetSkinState, String>
{
    require_p1_development()?;
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = shared.skins.lock().map_err(|_| lock_error())?.as_mut().map_err(|error| error.clone())?.select(skin_id.as_deref())?;
        pointer::reset(&shared);
        app.emit("pet-skin-library-changed", json!({"kind":"selected","selectedSkinId":selected.selected_skin_id})).map_err(|e| e.to_string())?;
        Ok(selected)
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn read_pet_skin_frame(state: State<'_, SharedState>, skin_id: String, frame_id: String)
    -> Result<tauri::ipc::Response, String>
{
    require_p1_development()?;
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        shared.skins.lock().map_err(|_| lock_error())?.as_mut().map_err(|error| error.clone())?.read_frame(&skin_id, &frame_id).map(tauri::ipc::Response::new)
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn remove_pet_skin(app: AppHandle, state: State<'_, SharedState>, skin_id: String)
    -> Result<crate::local_pet_skin::PetSkinState, String>
{
    require_p1_development()?;
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let selected = shared.skins.lock().map_err(|_| lock_error())?.as_mut().map_err(|error| error.clone())?.remove(&skin_id)?;
        app.emit("pet-skin-library-changed", json!({"kind":"removed"})).map_err(|e| e.to_string())?;
        Ok(selected)
    }).await.map_err(|e| e.to_string())?
}

struct CsvExportPermit(Arc<AtomicBool>);
impl CsvExportPermit {
    fn try_acquire(active: &Arc<AtomicBool>) -> Result<Self, String> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| CSV_EXPORT_BUSY.to_owned())?;
        Ok(Self(active.clone()))
    }
}
impl Drop for CsvExportPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn scan_followup_delay(
    lines: usize,
    pending_files: usize,
    issues: usize,
    contended: bool,
) -> Duration {
    if lines > 0 && pending_files > 0 && issues == 0 && !contended {
        Duration::from_millis(25)
    } else {
        Duration::from_secs(2)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct WorkArea {
    id: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
    primary: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedPosition {
    monitor_id: String,
    logical_x: f64,
    logical_y: f64,
    scale_factor: f64,
}
#[derive(Default)]
struct PetInteraction {
    revision: u64,
    active: bool,
    previous_foreground: isize,
    popup_bounds: Option<popup_bounds::Session>,
}
struct MotionRuntime {
    permission: motion_guard::PermissionLease,
    planner: roaming::Planner,
    request: Option<roaming::Request>,
    anchor: Option<roaming::Point>,
    direction: roaming::Direction,
    epoch: u64,
    presented: Option<(u64, roaming::Direction)>,
}
impl Default for MotionRuntime {
    fn default() -> Self {
        Self { permission: Default::default(), planner: Default::default(),
            request: None, anchor: None, direction: roaming::Direction::Right, epoch: 1,
            presented: None }
    }
}
impl MotionRuntime {
    fn cancel(&mut self, reanchor: bool) {
        self.planner.cancel();
        self.request = None;
        if reanchor { self.anchor = None; }
    }
    fn direction(&self) -> companion::MotionDirection {
        match self.direction {
            roaming::Direction::Left => companion::MotionDirection::Left,
            roaming::Direction::Right => companion::MotionDirection::Right,
        }
    }
}
struct AccountCache {
    binding: Value,
    snapshot: Value,
    generation: u64,
}
impl AccountCache {
    fn rebind(&mut self, source: &Value) -> bool {
        if self.binding == *source {
            return false;
        }
        // A root/adapter change under the same id also invalidates directory facts
        // and replies from an earlier visit to this binding.
        self.generation = self.generation.wrapping_add(1);
        self.binding = source.clone();
        self.snapshot =
            json!({"status":"loading","sourceId":source["id"],"buckets":[],"collectedAt":null});
        true
    }
    fn clear(&mut self, source: &Value) {
        self.generation = self.generation.wrapping_add(1);
        self.binding = source.clone();
        self.snapshot = json!({"status":"unavailable","sourceId":source["id"],"buckets":[],"collectedAt":null,"error":"本地账户缓存已清除；后续新读取可重新采集"});
    }
    fn publish(
        &mut self,
        source: &Value,
        generation: u64,
        snapshot: Value,
    ) -> Result<Value, String> {
        if self.binding != *source || self.generation != generation {
            return Err("账户绑定已变化，已丢弃旧快照".into());
        }
        self.snapshot = snapshot.clone();
        Ok(snapshot)
    }
    fn for_source(&self, source: &Value) -> Option<Value> {
        (self.binding == *source && flag(source, "enabled", false)).then(|| self.snapshot.clone())
    }
    fn quota_for_source(&self, source: &Value) -> Option<Value> {
        (self.binding == *source && flag(source, "enabled", false)).then(|| {
            json!({
                "sourceId":source["id"],
                "status":self.snapshot["status"],
                "collectedAt":self.snapshot["collectedAt"],
                "buckets":self.snapshot["buckets"]
            })
        })
    }
}
struct Shared {
    pointer: Mutex<pointer::Runtime>,
    skins: Mutex<Result<crate::local_pet_skin::LocalPetSkinStore, String>>,
    panel_navigation: Mutex<panel_navigation_state::Mailbox>,
    companion: Mutex<companion::Companion>,
    motion: Mutex<MotionRuntime>,
    manual_drag: Mutex<manual_drag::Controller>,
    automatic_positions: Mutex<motion_guard::PositionProvenance>,
    motion_invalidated: AtomicBool,
    motion_tick_pending: AtomicBool,
    motion_wake: (Mutex<bool>, Condvar),
    core: Mutex<Core>,
    panel_reads: Mutex<()>,
    panel_read_revision: AtomicU64,
    panel_read_interrupt: Mutex<Option<rusqlite::InterruptHandle>>,
    notification_gate: Arc<Gate>,
    csv_export_active: Arc<AtomicBool>,
    projection: Mutex<Value>,
    account: Mutex<AccountBridge>,
    account_cache: Mutex<AccountCache>,
    account_reset_pending: AtomicBool,
    last_account_attempt: Mutex<Option<Instant>>,
    account_cancel: Arc<AtomicBool>,
    stop: AtomicBool,
    exiting: AtomicBool,
    scanner_done: AtomicBool,
    account_done: AtomicBool,
    hotkey_thread: AtomicU32,
    shortcut_ready: AtomicBool,
    collector_error: Mutex<Option<String>>,
    shell_error: Mutex<Option<String>>,
    position_dirty: Mutex<Option<Instant>>,
    topology: Mutex<Vec<WorkArea>>,
    data_dir: PathBuf,
    started_at: Instant,
    metrics: Mutex<Value>,
    settings_update: Mutex<()>,
    settings_revision: AtomicU64,
    scan_wake: (Mutex<bool>, Condvar),
    display_recheck: AtomicBool,
    native_paused: AtomicBool,
    native_suppress_toast: AtomicBool,
    native_dragging: AtomicBool,
    watcher_error: Mutex<Option<String>>,
    pet_interaction: Mutex<PetInteraction>,
}
type SharedState = Arc<Shared>;
struct PanelReadRegistration<'a>(&'a Shared);
impl Drop for PanelReadRegistration<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.panel_read_interrupt.lock() {
            active.take();
        }
    }
}
fn lock_error() -> String {
    "本地后台状态暂不可用，请重新启动 Token 喵。".into()
}
fn persist_metrics(shared: &Shared, metrics: &Value) {
    if let Ok(bytes) = serde_json::to_vec_pretty(metrics) {
        let pending = shared.data_dir.join("desktop-metrics.pending.json");
        if fs::write(&pending, bytes).is_ok() {
            let _ = fs::rename(pending, shared.data_dir.join("desktop-metrics.json"));
        }
    }
}
fn record_metric(shared: &Shared, key: &str, value: Value) {
    if let Ok(mut metrics) = shared.metrics.lock() {
        if metrics.get(key) != Some(&value) {
            metrics[key] = value;
            metrics["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
            persist_metrics(shared, &metrics);
        }
    }
}
fn record_window_state(window: &WebviewWindow, shared: &Shared) {
    let value = json!({"visible":window.is_visible().unwrap_or(false),"minimized":window.is_minimized().unwrap_or(false),"focused":window.is_focused().unwrap_or(false),"activation":native::window_activation_state(window),"position":window.outer_position().ok().map(|v|json!({"x":v.x,"y":v.y})),"size":window.outer_size().ok().map(|v|json!({"width":v.width,"height":v.height})),"scaleFactor":window.scale_factor().ok()});
    if let Ok(mut metrics) = shared.metrics.lock() {
        if metrics["windows"].get(window.label()) != Some(&value) {
            if !metrics["windows"].is_object() {
                metrics["windows"] = json!({});
            }
            metrics["windows"][window.label()] = value;
            metrics["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
            persist_metrics(shared, &metrics);
        }
    }
}
fn record_visibility_change(window: &WebviewWindow, shared: &Shared) {
    let visible = window.is_visible().unwrap_or(false);
    let minimized = window.is_minimized().unwrap_or(false);
    if window.label() == "pet" && (!visible || minimized) {
        cancel_manual_drag(shared, manual_drag::StopReason::Hidden);
    }
    let changed = shared
        .metrics
        .lock()
        .map(|metrics| {
            metrics["windows"][window.label()]["visible"].as_bool() != Some(visible)
                || metrics["windows"][window.label()]["minimized"].as_bool() != Some(minimized)
        })
        .unwrap_or(false);
    if changed {
        record_window_state(window, shared);
    }
}
fn source_config(source: &Value) -> Result<SourceConfig, String> {
    Ok(SourceConfig {
        id: source["id"].as_str().ok_or("来源缺少标识")?.to_owned(),
        root: PathBuf::from(source["root"].as_str().ok_or("来源缺少目录")?),
        environment: source["environment"].as_str().unwrap_or("local").to_owned(),
    })
}
fn wake_scan(shared: &Shared) {
    if let Ok(mut pending) = shared.scan_wake.0.lock() {
        *pending = true;
        shared.scan_wake.1.notify_one();
    }
}
fn flag(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(default)
}
fn pet_scale(v: &Value) -> f64 {
    v.get("petScale")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(1.0)
        .clamp(0.5, 2.0)
}

fn show_without_focus(window: &WebviewWindow) -> Result<(), String> {
    // Both windows are created with focus:false. Pinned Tao keeps its
    // MARKER_DONT_FOCUS for visibility/style changes and uses SW_SHOWNOACTIVATE.
    // Always go through Tao: a raw ShowWindow would leave its VISIBLE bit false,
    // so later focusable/topmost changes would hide an apparently visible pet.
    window.show().map_err(|e| e.to_string())?;
    let _ = window.emit(
        "window-visibility-changed",
        json!({"label":window.label(),"visible":true}),
    );
    if let Some(shared) = window.try_state::<SharedState>() {
        record_window_state(window, &shared);
    }
    Ok(())
}
fn show_panel(app: &AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("主面板尚未就绪")?;
    window.unminimize().map_err(|e| e.to_string())?;
    fit_main_window(app)?;
    window.show().map_err(|e| e.to_string())?;
    let _ = window.emit(
        "window-visibility-changed",
        json!({"label":"main","visible":true}),
    );
    window.set_focus().map_err(|e| e.to_string())?;
    record_window_state(&window, &app.state::<SharedState>());
    Ok(())
}
// The pinned Wry runtime executes main-thread tasks inline when already on that
// thread. Other callers wait without holding a Core/geometry mutex. This keeps
// open/close/revision and HWND mutations in one serial native event sequence.
fn popup_on_main<T: Send + 'static>(
    app: &AppHandle,
    action: impl FnOnce(AppHandle) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let task_cancelled = cancelled.clone();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        if !task_cancelled.load(Ordering::Acquire) {
            let _ = send.send(action(handle));
        }
    }).map_err(|e| e.to_string())?;
    match receive.recv_timeout(Duration::from_secs(3)) {
        Ok(result) => result,
        Err(_) => {
            cancelled.store(true, Ordering::Release);
            Err("桌宠弹层几何更新超时".into())
        }
    }
}

fn pet_has_transient_bounds(shared: &Shared) -> bool {
    shared.pet_interaction.try_lock()
        .map(|owner| owner.active || owner.popup_bounds.is_some()).unwrap_or(true)
}

fn popup_work_area(window: &WebviewWindow, original: popup_bounds::Bounds) -> Result<WorkArea, String> {
    let monitors = native::work_areas(window)?;
    monitor_for_rect(&monitors, original.x, original.y, original.width, original.height)
        .cloned().ok_or_else(|| "桌宠所在屏幕暂不可用".into())
}

#[derive(Clone, Copy)]
struct PopupGeometryUpdate {
    original: popup_bounds::Bounds,
    target: popup_bounds::Bounds,
    scale_factor: f64,
    active: bool,
}
fn resize_popup_geometry_on_main(window: &WebviewWindow, shared: &Shared, active: bool) -> Result<Option<PopupGeometryUpdate>, String> {
    let existing = shared.pet_interaction.lock().map_err(|_| lock_error())?.popup_bounds;
    let session = if let Some(session) = existing { session }
    else if active {
        let position = window.outer_position().map_err(|e| e.to_string())?;
        let size = window.outer_size().map_err(|e| e.to_string())?;
        let dpi = window.scale_factor().map_err(|e| e.to_string())?;
        let session = popup_bounds::Session::capture(popup_bounds::Bounds {
            x: position.x, y: position.y, width: size.width, height: size.height,
        }, dpi).ok_or("桌宠原始窗口尺寸无效")?;
        // Publish the transient owner BEFORE SetWindowPos can send reentrant
        // move/resize events. Never hold this mutex across an HWND call.
        shared.pet_interaction.lock().map_err(|_| lock_error())?.popup_bounds = Some(session);
        session
    } else { return Ok(None); };
    let monitor = popup_work_area(window, session.original)?;
    let work = popup_bounds::Bounds { x: monitor.x, y: monitor.y, width: monitor.width, height: monitor.height };
    let target = if active { session.expanded(work, monitor.scale_factor) }
        else { session.restored(work, monitor.scale_factor) }.ok_or("屏幕工作区尺寸无效")?;
    // Geometry invalidation must not revoke the 15s temporary interaction
    // lease that allowed a user to open this popup while click-through is on.
    {
        let mut pointer = shared.pointer.lock().map_err(|_| lock_error())?;
        pointer.policy.set_viewport(None);
    }
    native::set_popup_bounds_without_activation(window, target)?;
    if !active {
        shared.pet_interaction.lock().map_err(|_| lock_error())?.popup_bounds = None;
    }
    Ok(Some(PopupGeometryUpdate { original: session.original, target, scale_factor: monitor.scale_factor, active }))
}
fn finish_popup_geometry_on_main(window: &WebviewWindow, shared: &Shared, update: PopupGeometryUpdate, sync_pointer: bool) -> Result<(), String> {
    if sync_pointer { pointer::sync(window, shared)?; }
    record_window_state(window, shared);
    record_metric(shared, "petPopupBounds", json!({"temporary":update.active,"original":{
        "x":update.original.x,"y":update.original.y,"width":update.original.width,"height":update.original.height},
        "current":{"x":update.target.x,"y":update.target.y,"width":update.target.width,"height":update.target.height},
        "scaleFactor":update.scale_factor,"persisted":false}));
    Ok(())
}
fn resize_popup_on_main(window: &WebviewWindow, shared: &Shared, active: bool) -> Result<(), String> {
    if let Some(update) = resize_popup_geometry_on_main(window, shared, active)? {
        finish_popup_geometry_on_main(window, shared, update, true)?;
    }
    Ok(())
}

fn dismiss_pet_popup(
    window: &WebviewWindow,
    _shared: &Shared,
    reason: &str,
    restore_focus: bool,
) -> Result<(), String> {
    let reason = reason.to_owned();
    popup_on_main(window.app_handle(), move |app| {
        let shared = app.state::<SharedState>();
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        dismiss_pet_popup_on_main(&window, &shared, &reason, restore_focus)
    })
}

#[derive(Clone, Copy)]
struct PopupDismissal { was_active: bool, previous: isize, revision: u64 }
fn take_pet_popup(shared: &Shared) -> Result<PopupDismissal, String> {
    if let Ok(mut pointer) = shared.pointer.lock() { pointer.policy.cancel_temporary(); }
    let (was_active, previous, revision) = {
        let mut interaction = shared.pet_interaction.lock().map_err(|_| lock_error())?;
        let result = (
            interaction.active,
            interaction.previous_foreground,
            interaction.revision,
        );
        interaction.active = false;
        interaction.previous_foreground = 0;
        result
    };
    Ok(PopupDismissal { was_active, previous, revision })
}
fn finish_pet_popup_dismissal_on_main(window: &WebviewWindow, shared: &Shared, reason: &str, restore_focus: bool, dismissal: PopupDismissal) -> Result<(), String> {
    if !dismissal.was_active { return Ok(()); }
    native::restore_popup_focus(window, dismissal.previous, restore_focus);
    window.set_focusable(false).map_err(|e| e.to_string())?;
    record_metric(
        shared,
        "petInteraction",
        json!({"active":false,"revision":dismissal.revision,"reason":reason,"at":chrono::Utc::now().to_rfc3339()}),
    );
    let _ = window.emit(
        "pet-popover-dismiss",
        json!({"reason":reason,"revision":dismissal.revision}),
    );
    Ok(())
}
fn dismiss_pet_popup_on_main(window: &WebviewWindow, shared: &Shared, reason: &str, restore_focus: bool) -> Result<(), String> {
    let dismissal = take_pet_popup(shared)?;
    // Restore even after an earlier focus/resize failure made active=false.
    let bounds_result = if reason == "destroyed" {
        shared.pet_interaction.lock().map_err(|_| lock_error())?.popup_bounds = None;
        Ok(())
    } else { resize_popup_on_main(window, shared, false) };
    let final_result = finish_pet_popup_dismissal_on_main(window, shared, reason, restore_focus, dismissal);
    bounds_result.and(final_result)
}
#[tauri::command]
fn set_pet_interaction(
    window: WebviewWindow,
    _state: State<'_, SharedState>,
    active: bool,
    revision: u64,
) -> Result<Value, String> {
    if window.label() != "pet" {
        return Err("仅桌宠窗口可切换弹层交互".into());
    }
    if revision == 0 || revision > 9_007_199_254_740_991 {
        return Err("弹层交互序号无效".into());
    }
    popup_on_main(window.app_handle(), move |app| {
        let state = app.state::<SharedState>();
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        set_pet_interaction_on_main(&window, &state, active, revision)
    })
}
fn set_pet_interaction_on_main(
    window: &WebviewWindow,
    state: &Shared,
    active: bool,
    revision: u64,
) -> Result<Value, String> {
    if state.exiting.load(Ordering::Relaxed) {
        return Err("应用正在退出".into());
    }
    {
        let mut interaction = state.pet_interaction.lock().map_err(|_| lock_error())?;
        if revision <= interaction.revision {
            return Ok(json!({"active":interaction.active,"revision":interaction.revision}));
        }
        interaction.revision = revision;
        if active {
            if !interaction.active {
                interaction.previous_foreground = native::foreground_window();
            }
            interaction.active = true;
        }
    }
    if active {
        cancel_manual_drag(state, manual_drag::StopReason::Cancelled);
        if !window.is_visible().unwrap_or(false) {
            let _ = dismiss_pet_popup(&window, &state, "hidden", false);
            return Err("桌宠隐藏时不能打开弹层".into());
        }
        if let Err(error) = resize_popup_on_main(window, state, true) {
            let _ = dismiss_pet_popup_on_main(window, state, "bounds-unavailable", false);
            return Err(error);
        }
        let focus_result = window.set_focusable(true).and_then(|_| window.set_focus());
        if let Err(error) = focus_result {
            let _ = dismiss_pet_popup(&window, &state, "focus-unavailable", false);
            return Err(error.to_string());
        }
        record_metric(
            &state,
            "petInteraction",
            json!({"active":true,"revision":revision,"at":chrono::Utc::now().to_rfc3339()}),
        );
    } else {
        dismiss_pet_popup(&window, &state, "closed", true)?;
    }
    let interaction = state.pet_interaction.lock().map_err(|_| lock_error())?;
    Ok(json!({"active":interaction.active,"revision":interaction.revision}))
}
fn apply_projection(app: &AppHandle, settings: &Value) -> Result<(), String> {
    let revision = app.state::<SharedState>().settings_revision.load(Ordering::Acquire);
    apply_projection_revision(app, settings, revision)
}
fn apply_projection_revision(app: &AppHandle, settings: &Value, revision: u64) -> Result<(), String> {
    let settings = settings.clone();
    popup_on_main(app, move |app| {
        if app.state::<SharedState>().settings_revision.load(Ordering::Acquire) != revision {
            return Ok(());
        }
        apply_projection_on_main(&app, &settings)
    })
}
fn apply_projection_on_main(app: &AppHandle, settings: &Value) -> Result<(), String> {
    let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
    let state = app.state::<SharedState>();
    let prior = state.projection.lock().map_err(|_| lock_error())?.clone();
    if prior.get("alwaysOnTop") != settings.get("alwaysOnTop") {
        window
            .set_always_on_top(flag(settings, "alwaysOnTop", true))
            .map_err(|e| e.to_string())?;
    }
    if pet_scale(&prior) != pet_scale(settings) || prior.is_null() {
        cancel_manual_drag(&state, manual_drag::StopReason::Cancelled);
        dismiss_pet_popup_on_main(&window, &state, "scale-changed", false)?;
        window
            .set_size(LogicalSize::new(
                280.0 * pet_scale(settings),
                340.0 * pet_scale(settings),
            ))
            .map_err(|e| e.to_string())?;
    }
    let ignore = {
        let mut pointer = state.pointer.lock().map_err(|_| lock_error())?;
        pointer.policy.set_preferences(p1_development_enabled() && flag(settings, "alphaHitTest", false), flag(settings, "clickThrough", false));
        pointer.policy.set_visible(flag(settings, "petVisible", true));
        pointer.ignore = None;
        pointer.policy.plan(state.started_at.elapsed().as_millis() as u64).ignore_cursor_events
    };
    window.set_ignore_cursor_events(ignore).map_err(|e| e.to_string())?;
    let visible = flag(settings, "petVisible", true);
    if prior.get("petVisible") != settings.get("petVisible") || prior.is_null() {
        if visible {
            show_without_focus(&window)?;
        } else {
            cancel_manual_drag(&state, manual_drag::StopReason::Hidden);
            dismiss_pet_popup(&window, &state, "hidden", false)?;
            window.hide().map_err(|e| e.to_string())?;
            let _ = window.emit(
                "window-visibility-changed",
                json!({"label":"pet","visible":false}),
            );
            record_window_state(&window, &state);
        }
    }
    *state.projection.lock().map_err(|_| lock_error())? = settings.clone();
    app.emit("pet-options-changed", settings)
        .map_err(|e| e.to_string())?;
    app.emit("pet-visibility-changed", json!({"visible":visible}))
        .map_err(|e| e.to_string())
}
fn update_settings(app: &AppHandle, shared: &Shared, patch: &Value) -> Result<Value, String> {
    let serial = shared.settings_update.lock().map_err(|_| lock_error())?;
    let old_settings = shared.core.lock().map_err(|_| lock_error())?.settings()?;
    let autostart = patch.get("startAtLogin").and_then(Value::as_bool);
    if let Some(enabled) = autostart {
        native::set_autostart(enabled)?;
    }
    let settings_result = {
        let mut core = shared.core.lock().map_err(|_| lock_error())?;
        let result = core.action("update_settings", &json!({"settings":patch}));
        shared.panel_read_revision.fetch_add(1, Ordering::AcqRel);
        result
    };
    let settings = match settings_result {
        Ok(settings) => settings,
        Err(error) => {
            if autostart.is_some() {
                let _ = native::set_autostart(flag(&old_settings, "startAtLogin", false));
            }
            return Err(error);
        }
    };
    let revision = shared.settings_revision.fetch_add(1, Ordering::AcqRel) + 1;
    drop(serial);
    // Persisted privacy/settings must invalidate other views even if a later
    // native window projection fails. Do not dispatch while holding its mutex.
    let _ = app.emit("pet-options-changed", &settings);
    // A synchronous IPC on the UI thread may itself need settings_update.
    // Never wait for its dispatch while holding that mutex.
    apply_projection_revision(app, &settings, revision)?;
    Ok(settings)
}
fn recover_pet(app: &AppHandle, reposition: bool) -> Result<(), String> {
    let shared = app.state::<SharedState>();
    popup_on_main(app, move |app| {
        let shared = app.state::<SharedState>();
        cancel_manual_drag(&shared, manual_drag::StopReason::Cancelled);
        if let Some(window) = app.get_webview_window("pet") {
            dismiss_pet_popup_on_main(&window, &shared, "recovery", false)?;
        }
        Ok(())
    })?;
    pointer::reset(&shared);
    update_settings(
        app,
        &shared,
        &json!({"clickThrough":false,"petVisible":true}),
    )?;
    if reposition {
        restore_position(app, &shared, true)?;
    }
    Ok(())
}

#[tauri::command]
async fn get_snapshot(
    state: State<'_, SharedState>,
    query: Option<Value>,
    include_tools: Option<bool>,
) -> Result<Value, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || read_panel_snapshot(&shared, query, None, include_tools.unwrap_or(true)))
        .await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn get_usage_dashboard_snapshot(
    state: State<'_, SharedState>,
    query: Option<Value>,
    request: Value,
    include_tools: Option<bool>,
) -> Result<Value, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || read_panel_snapshot(&shared, query, Some(request), include_tools.unwrap_or(true)))
        .await.map_err(|e|e.to_string())?
}
fn read_panel_snapshot(shared: &SharedState, query: Option<Value>, request: Option<Value>, include_tools: bool) -> Result<Value, String> {
        // Serialize heavy panel reads without holding the collector's writer lock.
        let _panel_read = shared.panel_reads.lock().map_err(|_|lock_error())?;
        if shared.exiting.load(Ordering::Relaxed) {
            return Err("应用正在退出，已停止接受新读取".into());
        }
        let query=query.unwrap_or_else(||json!({}));
        let (reader, read_revision) = {
            let core = shared.core.lock().map_err(|_|lock_error())?;
            if shared.exiting.load(Ordering::Relaxed) {
                return Err("应用正在退出，已停止接受新读取".into());
            }
            let reader = core.begin_panel_read()?;
            (reader, shared.panel_read_revision.load(Ordering::Acquire))
        };
        {
            let mut active = shared.panel_read_interrupt.lock().map_err(|_|lock_error())?;
            if shared.exiting.load(Ordering::Acquire) {
                return Err("应用正在退出，已停止接受新读取".into());
            }
            *active = Some(reader.interrupt_handle());
        }
        let _active_reader = PanelReadRegistration(shared);
        let read_result = (|| -> Result<(Value, Value), String> {
            let snapshot=match request {Some(request)=>reader.usage_dashboard_for_panel(query.clone(),request,include_tools)?,None=>reader.snapshot_for_panel(query.clone(),include_tools)?};
            let mut snapshot=snapshot;
            snapshot["usagePriceSync"]=reader.usage_price_sync()?;
            Ok((snapshot,reader.primary_source()?))
        })();
        // Finish the read transaction even when the query fails.
        let close_result = reader.finish();
        let (mut snapshot, primary) = read_result?;
        close_result?;
        if shared.exiting.load(Ordering::Relaxed) {
            return Err("应用正在退出，已停止接受新读取".into());
        }
        if shared.panel_read_revision.load(Ordering::Acquire) != read_revision {
            return Err("查询期间设置或数据范围已变化，请重新读取".into());
        }
        snapshot["primarySourceId"]=primary["id"].clone();
        snapshot["account"]=if query.get("sourceId").and_then(Value::as_str).is_some_and(|id|!id.is_empty()&&Some(id)!=primary["id"].as_str()){
            json!({"status":"unavailable","sourceId":query["sourceId"],"buckets":[],"collectedAt":null,"error":"此来源尚未启用独立账户采集"})
        }else{shared.account_cache.lock().map_err(|_|lock_error())?.for_source(&primary)
            .unwrap_or_else(||json!({"status":"unavailable","sourceId":primary["id"],"buckets":[],"collectedAt":null,"error":"主来源已变化或停用，等待对应来源重新采集"}))};
        let desktop_metrics=shared.metrics.lock().map_err(|_|lock_error())?.clone();
        snapshot["desktop"]=json!({"recoveryShortcut":RECOVERY_SHORTCUT,"shortcutAvailable":shared.shortcut_ready.load(Ordering::Relaxed),
            "collectorError":shared.collector_error.lock().map_err(|_|lock_error())?.clone(),"shellError":shared.shell_error.lock().map_err(|_|lock_error())?.clone(),
            "watcherError":shared.watcher_error.lock().map_err(|_|lock_error())?.clone(),"renderPaused":shared.native_paused.load(Ordering::Relaxed),
            "directory":desktop_metrics.get("directory").cloned().unwrap_or(Value::Null),
            "notifications":desktop_metrics.get("notifications").cloned().unwrap_or(Value::Null),
            "autostart":native::autostart_status(),
            "version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS});
        if shared.exiting.load(Ordering::Relaxed) {
            return Err("应用正在退出，已停止接受新读取".into());
        }
        if shared.panel_read_revision.load(Ordering::Acquire) != read_revision {
            return Err("查询期间设置或数据范围已变化，请重新读取".into());
        }
        Ok(snapshot)
}
#[tauri::command]
async fn save_usage_price(app: AppHandle, state: State<'_, SharedState>, payload: Value) -> Result<Value, String> {
    let shared = state.inner().clone();
    if shared.exiting.load(Ordering::Relaxed) {
        return Err("应用正在退出，已停止接受新操作".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = {
            let mut core = shared.core.lock().map_err(|_|lock_error())?;
            let result = core.save_usage_price(payload);
            shared.panel_read_revision.fetch_add(1, Ordering::AcqRel);
            result?
        };
        let _ = app.emit("ledger-updated", json!({"reason":"usage-price-saved"}));
        Ok(result)
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn save_usage_price_sync(app: AppHandle, state: State<'_, SharedState>, payload: Value) -> Result<Value, String> {
    change_usage_price_sync(app, state.inner().clone(), payload, "save").await
}
#[tauri::command]
async fn apply_usage_price_sync(app: AppHandle, state: State<'_, SharedState>, payload: Value) -> Result<Value, String> {
    change_usage_price_sync(app, state.inner().clone(), payload, "apply").await
}
#[tauri::command]
async fn record_usage_price_sync_error(app: AppHandle, state: State<'_, SharedState>, payload: Value) -> Result<Value, String> {
    change_usage_price_sync(app, state.inner().clone(), payload, "error").await
}
async fn change_usage_price_sync(app: AppHandle, shared: SharedState, payload: Value, operation: &'static str) -> Result<Value, String> {
    if shared.exiting.load(Ordering::Relaxed) {
        return Err("应用正在退出，已停止接受新操作".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result={let mut core=shared.core.lock().map_err(|_|lock_error())?;
            let result = match operation {"save"=>core.save_usage_price_sync(payload),"apply"=>core.apply_usage_price_sync(payload),_=>core.record_usage_price_sync_error(payload)};
            shared.panel_read_revision.fetch_add(1, Ordering::AcqRel);
            result?};
        let _=app.emit("ledger-updated", json!({"reason":"usage-price-sync"}));
        Ok(result)
    }).await.map_err(|e|e.to_string())?
}
#[tauri::command]
async fn get_pet_pulse(
    state: State<'_, SharedState>,
    cursor: Option<Value>,
) -> Result<Value, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shared.exiting.load(Ordering::Relaxed) {
            return Err("应用正在退出，已停止接受新读取".into());
        }
        let core = shared.core.lock().map_err(|_| lock_error())?;
        core.pet_pulse(cursor)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn get_pet_details(state: State<'_, SharedState>, request: Value) -> Result<Value, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shared.exiting.load(Ordering::Relaxed) {
            return Err("应用正在退出，已停止接受新读取".into());
        }
        // Keep the validated Core domain and cached account projection under
        // the same source/clear boundary. Reading this card never starts RPC.
        let core = shared.core.lock().map_err(|_| lock_error())?;
        let mut details = core.pet_details(request)?;
        let primary = core.primary_source()?;
        details["account"] = shared
            .account_cache
            .lock()
            .map_err(|_| lock_error())?
            .quota_for_source(&primary)
            .unwrap_or_else(|| {
                json!({"status":"unavailable","sourceId":primary["id"],"buckets":[],"collectedAt":null})
            });
        Ok(details)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn run_action(
    app: AppHandle,
    state: State<'_, SharedState>,
    name: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    let shared = state.inner().clone();
    if shared.exiting.load(Ordering::Relaxed) {
        return Err("应用正在退出，已停止接受新操作".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let payload = payload.unwrap_or_else(|| json!({}));
        if matches!(name.as_str(), "save_settings" | "update_settings") {
            let patch = payload.get("settings").unwrap_or(&payload);
            let result = update_settings(&app, &shared, patch)?;
            if patch.get("petScale").is_some() {
                restore_position(&app, &shared, false)?;
            }
            Ok(result)
        } else {
            let mut core=shared.core.lock().map_err(|_|lock_error())?;
            let result=core.action(&name,&payload);
            // Explicit user mutations invalidate outstanding views; collector
            // commits remain valid as newer facts after a consistent read.
            shared.panel_read_revision.fetch_add(1, Ordering::AcqRel);
            let result=result?;
            if name=="clear_data" {
                let primary=core.primary_source()?;
                shared.account_cache.lock().map_err(|_|lock_error())?.clear(&primary);
                shared.account_reset_pending.store(true,Ordering::SeqCst);
                *shared.last_account_attempt.lock().map_err(|_|lock_error())?=None;
                record_metric(&shared,"directory",json!({"status":"unavailable","sourceId":primary["id"],"lastSuccessAt":null,"count":null,"error":"本地目录缓存已清除；后续新读取可重新采集"}));
            }
            if matches!(name.as_str(), "clear_data" | "clear_inbox" | "set_primary_source" | "add_source" | "update_source" | "relocate_source") {
                pointer::reset(&shared);
                // Invalidate other windows before waiting on optional bridge
                // cleanup. A pre-mutation pet reply cannot become a new batch.
                let _ = app.emit("ledger-updated", json!({"petDomainChanged":true,"reason":name}));
            }
            drop(core);
            if name=="clear_data" {
                let mut bridge=shared.account.lock().map_err(|_|lock_error())?;
                if shared.account_reset_pending.swap(false,Ordering::SeqCst) {bridge.clear_cached_data();}
            }
            Ok(result)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn set_pet_options(
    app: AppHandle,
    state: State<'_, SharedState>,
    options: Value,
) -> Result<Value, String> {
    let mut options = options;
    if let Some(scale) = options.get("scale").cloned() {
        options["petScale"] = scale;
        options.as_object_mut().map(|v| v.remove("scale"));
    }
    run_action(
        app,
        state,
        "update_settings".into(),
        Some(json!({"settings":options})),
    )
    .await
}
#[tauri::command]
async fn set_pet_visibility(
    app: AppHandle,
    state: State<'_, SharedState>,
    visible: bool,
) -> Result<Value, String> {
    run_action(
        app,
        state,
        "update_settings".into(),
        Some(json!({"settings":{"petVisible":visible}})),
    )
    .await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PetInboxRequest {
    destination: InboxDestination,
    pet_domain: Value,
}

#[derive(Deserialize)]
enum InboxDestination {
    #[serde(rename = "inbox")]
    Inbox,
}

fn panel_domain(request: &Value) -> Result<panel_navigation_state::Domain, String> {
    Ok(panel_navigation_state::Domain {
        day: request["day"].as_str().ok_or("导航日期无效")?.to_owned(),
        runtime_epoch: request["runtimeEpoch"].as_str().ok_or("导航运行域无效")?.to_owned(),
        source_scope_revision: request["sourceScopeRevision"].as_str().ok_or("导航来源域无效")?.to_owned(),
    })
}

fn panel_domain_value(domain: &panel_navigation_state::Domain) -> Value {
    json!({"day":domain.day,"runtimeEpoch":domain.runtime_epoch,
        "sourceScopeRevision":domain.source_scope_revision})
}

fn panel_snapshot_value(
    mailbox: &mut panel_navigation_state::Mailbox,
    now: Instant,
    deliver: bool,
) -> Value {
    let pending = mailbox.snapshot(now);
    json!({"watermark":mailbox.watermark().to_string(),"pending":
        if deliver { pending.map(|p|json!({"revision":p.revision.to_string(),
            "destination":"inbox","petDomain":panel_domain_value(&p.domain),
            "remainingMs":p.remaining_ms(now)})) } else {None}})
}

fn cancel_panel_navigation(shared: &Shared) {
    if let Ok(mut mailbox) = shared.panel_navigation.lock() {
        // Exhaustion also clears the slot; never wrap an IPC watermark.
        let _ = mailbox.cancel();
    }
}

#[derive(Debug, PartialEq)]
enum PanelNavigationAction {
    Peek,
    Ack(u64),
    Cancel,
}

fn panel_navigation_action(request: &Value) -> Result<PanelNavigationAction, String> {
    let object = request.as_object().ok_or("导航操作必须为对象")?;
    match request["action"].as_str() {
        Some("peek") if object.len() == 1 => Ok(PanelNavigationAction::Peek),
        Some("cancel") if object.len() == 1 => Ok(PanelNavigationAction::Cancel),
        Some("ack") if object.len() == 2 => Ok(PanelNavigationAction::Ack(
            panel_navigation_state::revision(request["revision"].as_str().ok_or("导航确认序号无效")?)?)),
        _ => Err("导航操作含未知字段或目标".into()),
    }
}

fn validated_panel_snapshot(shared: &Shared) -> Result<Value, String> {
    let observed = shared.panel_navigation.lock().map_err(|_| lock_error())?
        .snapshot(Instant::now());
    let Some(observed) = observed else {
        let mut mailbox = shared.panel_navigation.lock().map_err(|_| lock_error())?;
        let retry = mailbox.snapshot(Instant::now()).is_some();
        let mut snapshot = panel_snapshot_value(&mut mailbox, Instant::now(), false);
        if retry { snapshot["retry"] = json!(true); }
        return Ok(snapshot);
    };
    // Never hold the navigation lock while waiting for Core or querying its domain.
    let core = shared.core.lock().map_err(|_| lock_error())?;
    let valid = core.pet_validate_domain(&panel_domain_value(&observed.domain));
    let mut mailbox = shared.panel_navigation.lock().map_err(|_| lock_error())?;
    if let Err(error) = valid {
        mailbox.clear_if(observed.revision);
        return Err(error);
    }
    let current = mailbox.snapshot(Instant::now());
    if current.as_ref().is_some_and(|p| p.revision != observed.revision) {
        // A newly published slot has not been validated by this read. The UI
        // coalesces one more pull; it must not apply the unchecked replacement.
        let mut value = panel_snapshot_value(&mut mailbox, Instant::now(), false);
        value["retry"] = json!(true);
        return Ok(value);
    }
    Ok(panel_snapshot_value(&mut mailbox, Instant::now(), true))
}

#[tauri::command]
async fn open_panel(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, SharedState>,
    request: Option<Value>,
) -> Result<(), String> {
    let Some(request) = request else { return show_panel(&app); };
    if window.label() != "pet" { return Err("仅桌宠窗口可请求小卡导航".into()); }
    let request: PetInboxRequest = serde_json::from_value(request).map_err(|_| "小卡导航请求无效")?;
    let InboxDestination::Inbox = request.destination;
    if state.exiting.load(Ordering::Acquire) || state.native_paused.load(Ordering::Acquire)
        || !window.is_visible().unwrap_or(false) {
        return Err("桌宠当前不可进行导航，请稍后重试".into());
    }
    let requested_at = Instant::now();
    let ticket = state.panel_navigation.lock().map_err(|_| lock_error())?.reserve(requested_at)?;
    let worker_ticket = ticket.clone();
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let core = shared.core.lock().map_err(|_| lock_error())?;
        core.pet_validate_domain(&request.pet_domain)?;
        if worker_ticket.remaining(Instant::now()).is_zero()
            || shared.exiting.load(Ordering::Acquire) || shared.native_paused.load(Ordering::Acquire) {
            return Err("小卡导航已过期，请重新点击".to_owned());
        }
        let mut mailbox = shared.panel_navigation.lock().map_err(|_| lock_error())?;
        if !mailbox.commit(&worker_ticket, panel_domain(&request.pet_domain)?, Instant::now()) {
            return Err("小卡导航已被新的操作取消".into());
        }
        Ok(())
    }).await.map_err(|e| e.to_string())??;
    let ui_app = app.clone();
    let ui_shared = state.inner().clone();
    let ui_ticket = ticket.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    // Show and the final cancellation check run on the same UI thread as native
    // CloseRequested. A delayed worker cannot show a window after that close.
    if let Err(error) = app.run_on_main_thread(move || {
        let shown = (|| {
            if ui_shared.exiting.load(Ordering::Acquire) || ui_shared.native_paused.load(Ordering::Acquire)
                || !ui_shared.panel_navigation.lock().map_err(|_| lock_error())?
                    .is_current(&ui_ticket, Instant::now()) {
                return Err("小卡导航已取消或过期".to_owned());
            }
            let pet = ui_app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
            if !pet.is_visible().unwrap_or(false) { return Err("桌宠已隐藏，导航已取消".into()); }
            dismiss_pet_popup(&pet, &ui_shared, "panel-navigation", false)?;
            show_panel(&ui_app)?;
            // The slot is authoritative even when main has not mounted its listener.
            let _ = ui_app.emit_to("main", "panel-navigation-request",
                json!({"revision":ui_ticket.revision.to_string()}));
            Ok(())
        })();
        if shown.is_err() {
            if let Ok(mut mailbox) = ui_shared.panel_navigation.lock() { mailbox.clear_if(ui_ticket.revision); }
        }
        let _ = sender.send(shown);
    }) {
        if let Ok(mut mailbox) = state.panel_navigation.lock() { mailbox.clear_if(ticket.revision); }
        return Err(error.to_string());
    }
    let wait_ticket = ticket.clone();
    let shown = tauri::async_runtime::spawn_blocking(move || {
        receiver.recv_timeout(wait_ticket.remaining(Instant::now()))
            .map_err(|_| "小卡导航未在原期限内完成，请重新点击".to_owned())?
    }).await.map_err(|e| e.to_string())?;
    if let Err(error) = shown {
        if let Ok(mut mailbox) = state.panel_navigation.lock() { mailbox.clear_if(ticket.revision); }
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
async fn panel_navigation(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    request: Value,
) -> Result<Value, String> {
    if window.label() != "main" { return Err("仅主面板可读取导航意图".into()); }
    let action = panel_navigation_action(&request)?;
    let revision = match action { PanelNavigationAction::Ack(value) => Some(value), _ => None };
    let shared = state.inner().clone();
    if action == PanelNavigationAction::Cancel {
        let mut mailbox = shared.panel_navigation.lock().map_err(|_| lock_error())?;
        mailbox.cancel()?;
        return Ok(panel_snapshot_value(&mut mailbox, Instant::now(), false));
    }
    if shared.exiting.load(Ordering::Acquire) { return Err("应用正在退出".into()); }
    if shared.native_paused.load(Ordering::Acquire)
        || !window.is_visible().unwrap_or(false) || window.is_minimized().unwrap_or(false) {
        let mut mailbox = shared.panel_navigation.lock().map_err(|_| lock_error())?;
        return Ok(panel_snapshot_value(&mut mailbox, Instant::now(), false));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = validated_panel_snapshot(&shared)?;
        if let Some(revision) = revision {
            if snapshot["pending"]["revision"].as_str() == Some(revision.to_string().as_str()) {
                shared.panel_navigation.lock().map_err(|_| lock_error())?.ack(revision, Instant::now());
            }
            return validated_panel_snapshot(&shared);
        }
        Ok(snapshot)
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn open_pet_chat(
    window: WebviewWindow,
    state: State<'_, SharedState>,
    request: Value,
) -> Result<Value, String> {
    if window.label() != "pet" { return Err("仅桌宠窗口可打开小卡聊天".into()); }
    if state.exiting.load(Ordering::Acquire) || state.native_paused.load(Ordering::Acquire)
        || !window.is_visible().unwrap_or(false) {
        return Err("桌宠当前不可打开聊天".into());
    }
    dismiss_pet_popup(&window, &state, "pet-chat", false)?;
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let core = shared.core.lock().map_err(|_| lock_error())?;
        let target = core.pet_chat_target(&request)?;
        if shared.exiting.load(Ordering::Acquire) || shared.native_paused.load(Ordering::Acquire) {
            return Err("聊天请求已取消".to_owned());
        }
        // Keep Core serialized through the OS submission, so source rebinding or
        // cleanup cannot interleave after validation. There is no await here;
        // later changes cannot retract an already submitted OS request.
        match target {
            crate::core::PetChatTarget::Open { target, thread_id } => {
                match open::that(target.as_str()) {
                    Ok(()) => Ok(json!({"status":"submitted"})),
                    Err(_) => Ok(json!({"status":"copyRequired","threadId":thread_id,
                        "message":"未能打开聊天，可复制会话标识"})),
                }
            }
            crate::core::PetChatTarget::CopyRequired { thread_id, message } =>
                Ok(json!({"status":"copyRequired","threadId":thread_id,"message":message})),
        }
    }).await.map_err(|e| e.to_string())?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PetDragRequest {
    client_x: f64,
    client_y: f64,
    viewport_width: f64,
    viewport_height: f64,
}
#[tauri::command]
fn start_pet_drag(window: WebviewWindow, request: PetDragRequest) -> Result<(), String> {
    if window.label() != "pet" { return Err("仅桌宠窗口可开始拖动".into()); }
    popup_on_main(window.app_handle(), move |app| {
        let state = app.state::<SharedState>();
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        #[cfg(windows)]
        {
            let mut handoff = ManualDragHandoff { window: &window, shared: &state, request,
                started: Instant::now(), anchor: None, dismissal: None, geometry: None,
                owner_started: false, first_step_seen: false,
                normal_cancellation: false, finish_failure: None,
                attempt: json!({"phase":"entry","entry":null,"afterGeometry":null,"firstMoveApplied":false,"reason":null}) };
            let result = drag_handoff::run(&mut handoff, ManualDragHandoff::admit,
                ManualDragHandoff::restore_geometry, ManualDragHandoff::first_step,
                ManualDragHandoff::finish);
            handoff.attempt["elapsedMs"] = json!(handoff.started.elapsed().as_secs_f64() * 1000.0);
            handoff.attempt["at"] = json!(chrono::Utc::now().to_rfc3339());
            let result = if let Some(error) = handoff.finish_failure.take() { Err(error) }
                else if handoff.normal_cancellation { Ok(()) } else { result };
            handoff.attempt["normalCancellation"] = json!(handoff.normal_cancellation);
            handoff.attempt["error"] = json!(result.as_ref().err());
            // Decision/first movement comes before any attempt telemetry I/O.
            record_metric(&state, "manualDragAttempt", handoff.attempt);
            result
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            dismiss_pet_popup_on_main(&window, &state, "drag", false)?;
            state.motion_invalidated.store(true, Ordering::Release);
            pointer::reset(&state);
            if let Ok(mut positions) = state.automatic_positions.lock() { positions.clear(); }
            window.start_dragging().map_err(|e| e.to_string())
        }
    })
}

#[cfg(windows)]
struct ManualDragHandoff<'a> {
    window: &'a WebviewWindow,
    shared: &'a Shared,
    request: PetDragRequest,
    started: Instant,
    anchor: Option<manual_drag::Point>,
    dismissal: Option<PopupDismissal>,
    geometry: Option<PopupGeometryUpdate>,
    owner_started: bool,
    first_step_seen: bool,
    normal_cancellation: bool,
    finish_failure: Option<String>,
    attempt: Value,
}
#[cfg(windows)]
impl ManualDragHandoff<'_> {
    fn sample(&self) -> manual_drag::Input {
        native::sample_drag_input(native::motion_window_available(self.window), manual_drag_runtime_available(self.shared))
    }
    fn observe(&self, input: manual_drag::Input) -> Value {
        json!({"elapsedMs":self.started.elapsed().as_secs_f64()*1000.0,
            "leftDown":input.left_down,"escapeDown":input.escape_down,
            "cursorAvailable":input.cursor.is_some(),"visible":input.visible,"runtimeAvailable":input.runtime_available})
    }
    fn reject(&mut self, reason: manual_drag::StopReason) -> String {
        self.attempt["reason"] = json!(format!("{reason:?}"));
        self.normal_cancellation = matches!(reason, manual_drag::StopReason::Released | manual_drag::StopReason::Escape);
        manual_drag_rejection_message(reason)
    }
    fn admit(&mut self) -> Result<(), String> {
        let input = self.sample();
        self.attempt["entry"] = self.observe(input);
        if let Some(reason) = manual_drag_input_rejection(input) {
            self.attempt["phase"] = json!("entry-rejected");
            return Err(self.reject(reason));
        }
        if self.shared.manual_drag.lock().map_err(|_| lock_error())?.is_active() {
            self.attempt["phase"] = json!("already-active");
            self.attempt["reason"] = json!("AlreadyActive");
            self.normal_cancellation = true;
            return Err("拖动已经开始，请先松开鼠标".into());
        }
        self.attempt["phase"] = json!("anchor-validation");
        self.anchor = Some(native::map_drag_press(self.window, manual_drag::ClientAnchor {
            x:self.request.client_x,y:self.request.client_y,
            viewport_width:self.request.viewport_width,viewport_height:self.request.viewport_height,
        })?);
        Ok(())
    }
    fn restore_geometry(&mut self) -> Result<(), String> {
        self.attempt["phase"] = json!("geometry-restoration");
        self.dismissal = Some(take_pet_popup(self.shared)?);
        // No pointer sync, focus change, notification or metrics write here.
        self.geometry = resize_popup_geometry_on_main(self.window, self.shared, false)?;
        Ok(())
    }
    fn first_step(&mut self) -> Result<(), String> {
        let input = self.sample();
        self.attempt["afterGeometry"] = self.observe(input);
        self.attempt["phase"] = json!("after-geometry-admission");
        if let Some(reason) = manual_drag_input_rejection(input) {
            return Err(self.reject(reason));
        }
        let rect = manual_drag_rect(self.window).ok_or("桌宠窗口尺寸暂不可用")?;
        let started = self.shared.manual_drag.lock().map_err(|_| lock_error())?
            .start(manual_drag::Input { cursor:self.anchor, ..input }, rect, self.shared.started_at.elapsed());
        if let Err(error) = started {
            self.normal_cancellation = matches!(error, manual_drag::StartError::AlreadyActive |
                manual_drag::StartError::ButtonNotDown | manual_drag::StartError::EscapeHeld);
            self.attempt["reason"] = json!(format!("{error:?}"));
            return Err(format!("当前无法拖动桌宠：{error:?}"));
        }
        self.owner_started = true;
        self.shared.native_dragging.store(true, Ordering::Release);
        self.shared.motion_invalidated.store(true, Ordering::Release);
        pointer::reset(self.shared);
        if let Ok(mut positions) = self.shared.automatic_positions.lock() { positions.clear(); }
        let first = step_manual_drag_on_main(self.window, self.shared);
        self.first_step_seen = first.handled;
        self.attempt["firstMoveApplied"] = json!(first.moved);
        self.attempt["phase"] = json!(if first.moved { "first-movement-applied" } else { "admitted-without-movement" });
        if let Some(reason) = first.stopped {
            self.attempt["phase"] = json!("first-step-cancelled");
            let message = self.reject(reason);
            return Err(first.sync_error.unwrap_or(message));
        }
        if !first.handled { return Err("拖动在首次移动前已取消".into()); }
        if let Ok(mut wake) = self.shared.motion_wake.0.lock() {
            *wake = true; self.shared.motion_wake.1.notify_one();
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        let bounds_result = match self.geometry.take() {
            Some(update) => finish_popup_geometry_on_main(self.window, self.shared, update, !self.first_step_seen),
            None => Ok(()),
        };
        let final_result = match self.dismissal.take() {
            Some(dismissal) => finish_pet_popup_dismissal_on_main(self.window, self.shared, "drag", false, dismissal),
            None => Ok(()),
        };
        if self.owner_started && self.shared.manual_drag.lock().is_ok_and(|owner| owner.is_active()) {
            record_metric(self.shared, "manualDrag", json!({"active":true,"at":chrono::Utc::now().to_rfc3339()}));
        }
        let result = bounds_result.and(final_result);
        if let Err(error) = &result {
            cancel_manual_drag(self.shared, manual_drag::StopReason::NativeMoveFailed);
            self.normal_cancellation = false;
            self.attempt["reason"] = json!("NativeMoveFailed");
            self.attempt["finishError"] = json!(error);
            self.finish_failure = Some(error.clone());
        }
        result
    }
}
#[cfg(windows)]
fn manual_drag_input_rejection(input: manual_drag::Input) -> Option<manual_drag::StopReason> {
    if !input.visible { Some(manual_drag::StopReason::Hidden) }
    else if !input.runtime_available { Some(manual_drag::StopReason::RuntimeUnavailable) }
    else if input.escape_down { Some(manual_drag::StopReason::Escape) }
    else if !input.left_down { Some(manual_drag::StopReason::Released) }
    else if input.cursor.is_none() { Some(manual_drag::StopReason::CursorUnavailable) }
    else { None }
}
#[cfg(windows)]
fn manual_drag_rejection_message(reason: manual_drag::StopReason) -> String {
    match reason {
        manual_drag::StopReason::Released => "鼠标已松开，未继续拖动；请按住鼠标再移动".into(),
        manual_drag::StopReason::Escape => "拖动已由 Escape 取消".into(),
        manual_drag::StopReason::Hidden => "桌宠隐藏时不能拖动".into(),
        manual_drag::StopReason::RuntimeUnavailable => "系统暂停或退出期间不能拖动".into(),
        manual_drag::StopReason::CursorUnavailable => "当前鼠标位置暂不可用".into(),
        _ => format!("拖动已停止：{reason:?}"),
    }
}

fn manual_drag_runtime_available(shared: &Shared) -> bool {
    !shared.native_paused.load(Ordering::Acquire) && !shared.stop.load(Ordering::Acquire) &&
        !shared.exiting.load(Ordering::Acquire)
}
fn finish_manual_drag(shared: &Shared, reason: manual_drag::StopReason) {
    shared.native_dragging.store(false, Ordering::Release);
    shared.motion_invalidated.store(true, Ordering::Release);
    pointer::reset(shared);
    if let Ok(mut dirty) = shared.position_dirty.lock() { *dirty = Some(Instant::now()); }
    record_metric(shared, "manualDrag", json!({"active":false,"reason":format!("{reason:?}"),
        "at":chrono::Utc::now().to_rfc3339()}));
}
fn cancel_manual_drag(shared: &Shared, reason: manual_drag::StopReason) {
    let cancelled = shared.manual_drag.lock().map(|mut owner| owner.cancel(reason)).unwrap_or(false);
    if cancelled { finish_manual_drag(shared, reason); }
}
#[cfg(windows)]
fn manual_drag_rect(window: &WebviewWindow) -> Option<manual_drag::Rect> {
    native::motion_snapshot(window).map(|(geometry, position)| manual_drag::Rect {
        x: position.x, y: position.y, width: geometry.window_width, height: geometry.window_height,
    })
}
/// One physical-cursor sample, one plan, at most one move. No controller lock
/// survives a native call, which may synchronously deliver window messages.
#[cfg(windows)]
#[derive(Default)]
struct ManualDragStep { handled: bool, moved: bool, stopped: Option<manual_drag::StopReason>, sync_error: Option<String> }
#[cfg(windows)]
fn step_manual_drag_on_main(window: &WebviewWindow, shared: &Shared) -> ManualDragStep {
    if !shared.manual_drag.lock().is_ok_and(|owner| owner.is_active()) { return ManualDragStep::default(); }
    let rect = manual_drag_rect(window).unwrap_or(manual_drag::Rect { x:0, y:0, width:0, height:0 });
    let areas = native::work_areas(window).unwrap_or_default().into_iter().map(|area|
        manual_drag::Rect { x:area.x, y:area.y, width:area.width, height:area.height }).collect::<Vec<_>>();
    let input = native::sample_drag_input(native::motion_window_available(window), manual_drag_runtime_available(shared));
    let update = match shared.manual_drag.lock() {
        Ok(mut owner) => owner.tick(input, rect, &areas, shared.started_at.elapsed()),
        Err(_) => return ManualDragStep { handled: true, ..ManualDragStep::default() },
    };
    let mut result = ManualDragStep { handled: true, stopped: update.stopped, moved: false, sync_error: None };
    if let Some(reason) = update.stopped { finish_manual_drag(shared, reason); }
    if let Some(target) = update.target {
        // Geometry queries above may take time. Never apply a planned movement
        // after a release/pause that happened during them or during planning.
        let boundary = native::sample_drag_input(native::motion_window_available(window), manual_drag_runtime_available(shared));
        if let Some(reason) = manual_drag_input_rejection(boundary) {
            cancel_manual_drag(shared, reason);
            result.stopped = Some(reason);
        } else if !native::move_pet_without_activation(window, roaming::Point { x:target.x, y:target.y }) {
            cancel_manual_drag(shared, manual_drag::StopReason::NativeMoveFailed);
            result.stopped = Some(manual_drag::StopReason::NativeMoveFailed);
        } else {
            result.moved = true;
        }
    }
    if let Err(error) = pointer::sync(window, shared) {
        cancel_manual_drag(shared, manual_drag::StopReason::NativeMoveFailed);
        result.stopped = Some(manual_drag::StopReason::NativeMoveFailed);
        result.sync_error = Some(error);
    }
    result
}

fn record_pet_move(window: &tauri::Window, shared: &Shared, point: PhysicalPosition<i32>) {
    // Popup expansion/restoration is not a user drag or a new saved anchor.
    if pet_has_transient_bounds(shared) { return; }
    if shared.display_recheck.load(Ordering::Acquire) {
        shared.motion_invalidated.store(true, Ordering::Release);
        return;
    }
    let event = roaming::Point { x: point.x, y: point.y };
    let actual = window.app_handle().get_webview_window("pet")
        .and_then(|view| native::motion_snapshot(&view).map(|value| value.1));
    let dragging = shared.native_dragging.load(Ordering::Acquire);
    let origin = shared.automatic_positions.lock().map(|mut positions|
        positions.observe(event, actual, dragging, shared.started_at.elapsed()))
        .unwrap_or(motion_guard::MoveOrigin::Stale);
    if origin == motion_guard::MoveOrigin::External {
        shared.motion_invalidated.store(true, Ordering::Release);
        if let Ok(mut dirty) = shared.position_dirty.lock() { *dirty = Some(Instant::now()); }
    }
}
#[tauri::command]
async fn open_thread(
    state: State<'_, SharedState>,
    source_id: String,
    thread_id: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let link = shared
            .core
            .lock()
            .map_err(|_| lock_error())?
            .chat_target(&source_id, &thread_id)?
            .as_str()
            .to_owned();
        open::that(link).map_err(|e| format!("未能打开来源聊天，请复制会话标识：{e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn export_csv(
    app: AppHandle,
    state: State<'_, SharedState>,
    query: Option<Value>,
) -> Result<Option<String>, String> {
    let shared = state.inner().clone();
    let permit = CsvExportPermit::try_acquire(&shared.csv_export_active)?;
    tauri::async_runtime::spawn_blocking(move || {
        // The blocking task owns the permit, including when its async waiter is
        // dropped. Release only after generation, the dialog and writing finish.
        let _permit = permit;
        let csv = shared
            .core
            .lock()
            .map_err(|_| lock_error())?
            .export_csv(query.unwrap_or_else(|| json!({})))?;
        let directory = csv_export_directory::first_available([
            app.path().document_dir().ok(),
            app.path().download_dir().ok(),
            app.path().desktop_dir().ok(),
            app.path().home_dir().ok(),
        ].into_iter().flatten())?;
        let destination = app
            .dialog()
            .file()
            .set_title("导出本机用量账本")
            .set_directory(directory)
            .set_file_name(format!(
                "token-meow-{}.csv",
                chrono::Utc::now().format("%Y%m%d-%H%M%S")
            ))
            .add_filter("CSV 用量账本", &["csv"])
            .blocking_save_file();
        let Some(destination) = destination else {
            return Ok(None);
        };
        let path = destination.into_path().map_err(|e| e.to_string())?;
        // Re-read all registered roots after the dialog, including disabled
        // sources. Keep their configuration stable until the commit finishes.
        let core = shared.core.lock().map_err(|_| lock_error())?;
        let mut protected = vec![shared.data_dir.clone()];
        for root in core.registered_source_roots()? {
            if root.as_os_str().is_empty() {
                return Err("导出失败：来源保护目录无效。".into());
            }
            protected.push(root);
        }
        // The query service owns the BOM, 14 columns and formula escaping.
        csv_export::save(&path, csv.as_bytes(), &protected)?;
        drop(core);
        Ok(Some(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "token-meow.csv".into()),
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}
fn record_account_bridge_resource(shared: &Shared) {
    let diagnostic = {
        let Ok(bridge) = shared.account.lock() else {
            return;
        };
        bridge.resource_diagnostic()
    };
    // Account and diagnostic guards are released before metrics or disk I/O.
    if let Ok(value) = serde_json::to_value(diagnostic) {
        record_metric(shared, "accountBridgeResource", value);
    }
}
fn update_account(shared: &Shared, manual_retry: bool) -> Result<Value, String> {
    let result = update_account_inner(shared, manual_retry);
    record_account_bridge_resource(shared);
    result
}
fn update_account_inner(shared: &Shared, manual_retry: bool) -> Result<Value, String> {
    if shared.stop.load(Ordering::Relaxed) {
        return Err("应用正在退出".into());
    }
    let mut bridge = shared.account.lock().map_err(|_| lock_error())?;
    // A manual read may have held the bridge while the user switched sources.
    // Resolve ownership after acquiring it; never begin another RPC for that old source.
    let (source, changed, generation) = {
        let core = shared.core.lock().map_err(|_| lock_error())?;
        let source = core.primary_source()?;
        let mut cache = shared.account_cache.lock().map_err(|_| lock_error())?;
        let changed = cache.rebind(&source);
        (source, changed, cache.generation)
    };
    if shared.account_reset_pending.swap(false, Ordering::SeqCst) {
        bridge.clear_cached_data();
    }
    if changed {
        // Binding includes the selected source id and adapter, not only its path.
        // A failed first read must not reuse a previous logical source's snapshot.
        bridge.clear_cached_data();
        bridge.set_source_root(&PathBuf::from(
            source["root"].as_str().ok_or("主来源缺少目录")?,
        ));
        *shared
            .last_account_attempt
            .lock()
            .map_err(|_| lock_error())? = None;
    }
    if !flag(&source, "enabled", false) {
        bridge.shutdown();
        let snapshot = json!({"status":"unavailable","sourceId":source["id"],"buckets":[],"collectedAt":null,"error":"主来源已停用，账户采集已停止"});
        return shared
            .account_cache
            .lock()
            .map_err(|_| lock_error())?
            .publish(&source, generation, snapshot);
    }
    let throttled = {
        let mut last = shared
            .last_account_attempt
            .lock()
            .map_err(|_| lock_error())?;
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
            true
        } else {
            *last = Some(Instant::now());
            false
        }
    };
    if throttled {
        let core = shared.core.lock().map_err(|_| lock_error())?;
        let cache = shared.account_cache.lock().map_err(|_| lock_error())?;
        if core.primary_source()? != source || cache.generation != generation {
            return Err("账户绑定已变化，请重新读取".into());
        }
        return cache
            .for_source(&source)
            .ok_or_else(|| "账户绑定已变化，请重新读取".into());
    }
    let config = source_config(&source)?;
    if manual_retry {
        bridge.retry_manually();
    }
    let mut snapshot = match adapter_for(source["clientKind"].as_str().unwrap_or_default()) {
        Some(adapter) => adapter.read_account(&config, &mut *bridge).unwrap_or_else(
            |error| json!({"status":"unavailable","buckets":[],"collectedAt":null,"error":error}),
        ),
        None => {
            bridge.shutdown();
            json!({"status":"unavailable","buckets":[],"collectedAt":null,"error":"主来源适配器尚未发布账户读取能力"})
        }
    };
    drop(bridge);
    let core = shared.core.lock().map_err(|_| lock_error())?;
    if core.primary_source()? != source {
        return Err("账户读取期间主来源已变化，已丢弃旧快照".into());
    }
    snapshot["sourceId"] = source["id"].clone();
    shared
        .account_cache
        .lock()
        .map_err(|_| lock_error())?
        .publish(&source, generation, snapshot)
}
fn directory_failure(previous: &Value, source: &Value, generation: u64, error: &str) -> Value {
    let same_binding = previous["sourceId"] == source["id"]
        && previous["bindingGeneration"].as_u64() == Some(generation);
    json!({"status":"unavailable","sourceId":source["id"],"bindingGeneration":generation,"error":error,
        "lastSuccessAt":if same_binding {previous["lastSuccessAt"].clone()} else {Value::Null},
        "count":if same_binding {previous["count"].clone()} else {Value::Null}})
}

fn apply_resource_stop(
    cache: &mut AccountCache,
    source: &Value,
    generation: u64,
    metrics: &mut Value,
    error: &str,
) -> (Option<Value>, bool) {
    if cache.binding != *source
        || cache.generation != generation
        || !flag(source, "enabled", false)
        || adapter_for(source["clientKind"].as_str().unwrap_or_default()).is_none()
    {
        return (None, false);
    }
    // Patch only the current cache. The bridge's snapshot may belong to an old
    // source or a generation cleared while its transport was running.
    let mut snapshot = cache.snapshot.clone();
    snapshot["status"] = json!(if snapshot["collectedAt"].as_str().is_some() {
        "stale"
    } else {
        "unavailable"
    });
    snapshot["error"] = json!(error);
    let directory = directory_failure(&metrics["directory"], source, generation, error);
    let directory_changed = directory != metrics["directory"];
    if snapshot == cache.snapshot && !directory_changed {
        return (None, false);
    }
    cache.snapshot = snapshot.clone();
    metrics["directory"] = directory;
    (Some(snapshot), directory_changed)
}

fn observe_account_resource_stop(
    shared: &Shared,
    source: &Value,
    generation: u64,
) -> Result<Option<Value>, String> {
    let mut bridge = shared.account.lock().map_err(|_| lock_error())?;
    let Some(error) = bridge.observe_resource_stop() else {
        return Ok(None);
    };
    // Keep the bridge lock through publication so a concurrent manual retry
    // cannot publish a fresh read before this older stop is applied.
    let core = shared.core.lock().map_err(|_| lock_error())?;
    if core.primary_source()? != *source {
        return Ok(None);
    }
    let mut cache = shared.account_cache.lock().map_err(|_| lock_error())?;
    let mut metrics = shared.metrics.lock().map_err(|_| lock_error())?;
    let (snapshot, directory_changed) =
        apply_resource_stop(&mut cache, source, generation, &mut metrics, error);
    // Mutations are complete under the binding guards. Keep only metrics locked
    // during I/O: a later clear/source update cannot be persisted ahead of this one.
    drop(cache);
    drop(core);
    drop(bridge);
    if directory_changed {
        metrics["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
        persist_metrics(shared, &metrics);
    }
    Ok(snapshot)
}

#[tauri::command]
async fn refresh_account(state: State<'_, SharedState>) -> Result<Value, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || update_account(&shared, true))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn choose_source_directory(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择受支持的 Codex 数据目录（只读）")
            .blocking_pick_folder()
            .map(|path| {
                path.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose()
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn frontend_ready(window: WebviewWindow, state: State<'_, SharedState>) -> Result<Value, String> {
    let label = window.label();
    if !matches!(label, "main" | "pet") {
        return Err("未知的应用窗口".into());
    }
    let mut metrics = state.metrics.lock().map_err(|_| lock_error())?;
    if metrics["frontendReadyMs"].get(label).is_none() {
        metrics["frontendReadyMs"][label] =
            json!(state.started_at.elapsed().as_secs_f64() * 1000.0);
        metrics["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
        persist_metrics(&state, &metrics);
    }
    let mut response = metrics.clone();
    response["label"] = json!(label);
    response["visible"] =
        json!(window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false));
    response["paused"] = json!(state.native_paused.load(Ordering::Relaxed));
    response["petInteractionRevision"] = json!(
        state
            .pet_interaction
            .lock()
            .map_err(|_| lock_error())?
            .revision
    );
    Ok(response)
}
#[tauri::command]
fn desktop_metrics(state: State<'_, SharedState>) -> Result<Value, String> {
    Ok(state.metrics.lock().map_err(|_| lock_error())?.clone())
}

fn monitor_for_rect(
    monitors: &[WorkArea],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Option<&WorkArea> {
    monitors.iter().max_by_key(|m| {
        let left = (x as i64).max(m.x as i64);
        let top = (y as i64).max(m.y as i64);
        let right = (x as i64 + width as i64).min(m.x as i64 + m.width as i64);
        let bottom = (y as i64 + height as i64).min(m.y as i64 + m.height as i64);
        (right - left).max(0) * (bottom - top).max(0)
    })
}
fn clamp_position(m: &WorkArea, x: i32, y: i32, width: u32, height: u32) -> (i32, i32) {
    let max_x = m.x.saturating_add(m.width.saturating_sub(width) as i32);
    let max_y = m.y.saturating_add(m.height.saturating_sub(height) as i32);
    (x.clamp(m.x, max_x.max(m.x)), y.clamp(m.y, max_y.max(m.y)))
}
fn fit_scale(m: &WorkArea, requested: f64) -> f64 {
    let available =
        (m.width as f64 / m.scale_factor / 280.0).min(m.height as f64 / m.scale_factor / 340.0);
    requested.min(available).clamp(0.5, 2.0)
}
fn panel_bounds(m: &WorkArea, x: i32, y: i32, width: u32, height: u32) -> (i32, i32, u32, u32) {
    let width = width.min(m.width).max(1);
    let height = height.min(m.height).max(1);
    let (x, y) = clamp_position(m, x, y, width, height);
    (x, y, width, height)
}
fn fit_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("主面板尚未就绪")?;
    let monitors = native::work_areas(&window)?;
    let p = window.outer_position().map_err(|e| e.to_string())?;
    let outer = window.outer_size().map_err(|e| e.to_string())?;
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    let Some(monitor) = monitor_for_rect(&monitors, p.x, p.y, outer.width, outer.height) else {
        return Ok(());
    };
    let (x, y, width, height) = panel_bounds(monitor, p.x, p.y, outer.width, outer.height);
    let frame_width = outer.width.saturating_sub(inner.width);
    let frame_height = outer.height.saturating_sub(inner.height);
    let client_width = width.saturating_sub(frame_width).max(1);
    let client_height = height.saturating_sub(frame_height).max(1);
    window
        .set_min_size(Some(PhysicalSize::new(
            (780.0 * monitor.scale_factor)
                .round()
                .min(client_width as f64) as u32,
            (480.0 * monitor.scale_factor)
                .round()
                .min(client_height as f64) as u32,
        )))
        .map_err(|e| e.to_string())?;
    if width != outer.width || height != outer.height {
        window
            .set_size(PhysicalSize::new(client_width, client_height))
            .map_err(|e| e.to_string())?;
    }
    if (x, y) != (p.x, p.y) {
        window
            .set_position(PhysicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn restored_position(
    monitors: &[WorkArea],
    saved: Option<&SavedPosition>,
    logical_width: f64,
    logical_height: f64,
    reset: bool,
) -> Option<(i32, i32)> {
    let m = if !reset {
        saved.and_then(|s| monitors.iter().find(|m| m.id == s.monitor_id))
    } else {
        None
    }
    .or_else(|| monitors.iter().find(|m| m.primary))
    .or_else(|| monitors.first())?;
    let matching = saved.filter(|s| !reset && s.monitor_id == m.id);
    let width = (logical_width * m.scale_factor).round() as u32;
    let height = (logical_height * m.scale_factor).round() as u32;
    let (x, y) = matching
        .map(|s| {
            (
                m.x.saturating_add((s.logical_x * m.scale_factor).round() as i32),
                m.y.saturating_add((s.logical_y * m.scale_factor).round() as i32),
            )
        })
        .unwrap_or((
            m.x + (m.width as i32 - width as i32 - 16).max(0),
            m.y + (m.height as i32 - height as i32 - 16).max(0),
        ));
    Some(clamp_position(m, x, y, width, height))
}
fn restore_position(app: &AppHandle, shared: &Shared, reset: bool) -> Result<(), String> {
    let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
    popup_on_main(app, move |app| {
        let shared = app.state::<SharedState>();
        cancel_manual_drag(&shared, manual_drag::StopReason::Cancelled);
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        dismiss_pet_popup_on_main(&window, &shared, "display-recovery", false)
    })?;
    let monitors = native::work_areas(&window)?;
    let settings = shared.core.lock().map_err(|_| lock_error())?.settings()?;
    let saved = settings
        .get("petPosition")
        .cloned()
        .and_then(|v| serde_json::from_value::<SavedPosition>(v).ok());
    let target = saved
        .as_ref()
        .filter(|_| !reset)
        .and_then(|s| monitors.iter().find(|m| m.id == s.monitor_id))
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or_else(|| monitors.first());
    let mut scale = pet_scale(&settings);
    if let Some(target) = target {
        let fitted = fit_scale(target, scale);
        if fitted < scale {
            scale = fitted;
            update_settings(app, shared, &json!({"petScale":scale}))?;
        }
    }
    if let Some((x, y)) = restored_position(
        &monitors,
        saved.as_ref(),
        280.0 * scale,
        340.0 * scale,
        reset,
    ) {
        popup_on_main(app, move |app| {
            let shared = app.state::<SharedState>();
            let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
            // The worker can yield between the earlier restoration and this
            // final placement. Revoke any gesture that began in that gap.
            cancel_manual_drag(&shared, manual_drag::StopReason::Cancelled);
            dismiss_pet_popup_on_main(&window, &shared, "display-recovery", false)?;
            window.set_position(PhysicalPosition::new(x, y)).map_err(|e| e.to_string())
        })?;
    }
    *shared.topology.lock().map_err(|_| lock_error())? = monitors;
    *shared.position_dirty.lock().map_err(|_| lock_error())? = Some(Instant::now());
    Ok(())
}
fn persist_settled_position(app: &AppHandle, shared: &Shared) -> Result<(), String> {
    if shared.native_dragging.load(Ordering::Acquire) || pet_has_transient_bounds(shared) { return Ok(()); }
    let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
    let Some(m) = popup_on_main(app, move |app| {
        let shared = app.state::<SharedState>();
        if shared.native_dragging.load(Ordering::Acquire) || pet_has_transient_bounds(&shared) { return Ok(None); }
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        let monitors = native::work_areas(&window)?;
        let p = window.outer_position().map_err(|e| e.to_string())?;
        let size = window.outer_size().map_err(|e| e.to_string())?;
        Ok(monitor_for_rect(&monitors, p.x, p.y, size.width, size.height).cloned())
    })? else { return Ok(()); };
    let settings = shared.core.lock().map_err(|_| lock_error())?.settings()?;
    let fitted = fit_scale(&m, pet_scale(&settings));
    if fitted < pet_scale(&settings) {
        update_settings(app, shared, &json!({"petScale":fitted}))?;
    }
    // A popup may open between the worker's initial check and native reads.
    // Capture ONLY normal geometry in one UI task, before touching settings.
    let Some((m, x, y, size)) = popup_on_main(app, move |app| {
        let shared = app.state::<SharedState>();
        if shared.native_dragging.load(Ordering::Acquire) || pet_has_transient_bounds(&shared) { return Ok(None); }
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        let monitors = native::work_areas(&window)?;
        let p = window.outer_position().map_err(|e| e.to_string())?;
        let size = window.outer_size().map_err(|e| e.to_string())?;
        let Some(m) = monitor_for_rect(&monitors, p.x, p.y, size.width, size.height).cloned() else { return Ok(None); };
        let (x, y) = clamp_position(&m, p.x, p.y, size.width, size.height);
        if (x, y) != (p.x, p.y) {
            window.set_position(PhysicalPosition::new(x, y)).map_err(|e| e.to_string())?;
        }
        Ok(Some((m, x, y, size)))
    })? else { return Ok(()); };
    let saved = SavedPosition {
        monitor_id: m.id.clone(),
        logical_x: (x - m.x) as f64 / m.scale_factor,
        logical_y: (y - m.y) as f64 / m.scale_factor,
        scale_factor: m.scale_factor,
    };
    let serial = shared.settings_update.lock().map_err(|_| lock_error())?;
    let settings = shared
        .core
        .lock()
        .map_err(|_| lock_error())?
        .action("update_settings", &json!({"petPosition":saved}))?;
    drop(serial);
    // Position is not a visual projection acknowledgement. Preserve the last
    // actually-applied visual flags when another settings projection is queued.
    shared.projection.lock().map_err(|_| lock_error())?["petPosition"] = settings["petPosition"].clone();
    record_window_state(&window, shared);
    record_metric(
        shared,
        "positionSettled",
        json!({"at":chrono::Utc::now().to_rfc3339(),"withinWorkArea":x>=m.x&&y>=m.y&&(x as i64+size.width as i64)<=m.x as i64+m.width as i64&&(y as i64+size.height as i64)<=m.y as i64+m.height as i64,"dragging":false,"scaleFactor":m.scale_factor}),
    );
    Ok(())
}
#[tauri::command]
fn reset_pet_position(app: AppHandle) -> Result<(), String> {
    recover_pet(&app, true)
}
fn begin_shutdown(app: &AppHandle) {
    let shared = app.state::<SharedState>().inner().clone();
    if shared.exiting.swap(true, Ordering::SeqCst) {
        return;
    }
    // Registration and this exit check share a mutex, so a late reader cannot
    // start a query after shutdown has observed an empty interrupt slot.
    if let Ok(active) = shared.panel_read_interrupt.lock() {
        if let Some(interrupt) = active.as_ref() {
            interrupt.interrupt();
        }
    }
    cancel_manual_drag(&shared, manual_drag::StopReason::RuntimeUnavailable);
    shared.stop.store(true, Ordering::SeqCst);
    pointer::refresh_runtime_admission(&shared);
    cancel_panel_navigation(&shared);
    wake_scan(&shared);
    shared.account_cancel.store(true, Ordering::SeqCst);
    record_metric(
        &shared,
        "shutdown",
        json!({"stage":"requested","at":chrono::Utc::now().to_rfc3339(),"limitMs":SHUTDOWN_LIMIT.as_millis()}),
    );
    native::stop_hotkey(shared.hotkey_thread.load(Ordering::Relaxed));
    for label in ["main", "pet"] {
        if let Some(window) = app.get_webview_window(label) {
            if label == "pet" {
                let _ = dismiss_pet_popup(&window, &shared, "shutdown", false);
            }
            let _ = window.hide();
            record_window_state(&window, &shared);
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let deadline = started + SHUTDOWN_LIMIT;
        while Instant::now() < deadline
            && (!shared.scanner_done.load(Ordering::Acquire)
                || !shared.account_done.load(Ordering::Acquire)
                || shared.panel_reads.try_lock().is_err())
        {
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Ok(mut bridge) = shared.account.try_lock() {
            bridge.shutdown();
        }
        record_metric(
            &shared,
            "shutdown",
            json!({"stage":"exit-ready","at":chrono::Utc::now().to_rfc3339(),"waitMs":started.elapsed().as_secs_f64()*1000.0,"scannerFinished":shared.scanner_done.load(Ordering::Acquire),"accountFinished":shared.account_done.load(Ordering::Acquire),"panelReadsFinished":shared.panel_reads.try_lock().is_ok(),"deadlineReached":Instant::now()>=deadline}),
        );
        // Unfinished SQLite transactions roll back; never force a partial scan to commit.
        // The Windows JobObject closes on process exit and owns only bridge descendants.
        app.exit(0);
    });
}
#[tauri::command]
fn quit_app(app: AppHandle) {
    begin_shutdown(&app);
}

fn create_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let panel = MenuItem::with_id(app, "panel", "打开统计面板", true, None::<&str>)?;
    let recover = MenuItem::with_id(
        app,
        "recover",
        "恢复桌宠交互  Ctrl+Alt+M",
        true,
        None::<&str>,
    )?;
    let position = MenuItem::with_id(app, "position", "将桌宠移回主屏", true, None::<&str>)?;
    let temporary = MenuItem::with_id(app, "temporary", "临时交互 15 秒  Ctrl+Alt+I", p1_development_enabled(), None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "隐藏角色（继续采集）", true, None::<&str>)?;
    let mute = MenuItem::with_id(app, "mute", "切换静音", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, "exit", "退出 Token 喵", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&panel, &temporary, &recover, &position, &hide, &mute, &exit])?;
    TrayIconBuilder::with_id("token-meow-tray")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/32x32.png"
        ))?)
        .tooltip("Token 喵 · 本地只读用量账本")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let action = event.id.as_ref().to_owned();
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let state = app.state::<SharedState>();
                let result = match action.as_str() {
                    "panel" => show_panel(&app),
                    "recover" => recover_pet(&app, false),
                    "temporary" => pointer::temporary(&app),
                    "position" => recover_pet(&app, true),
                    "hide" => {
                        update_settings(&app, &state, &json!({"petVisible":false})).map(|_| ())
                    }
                    "mute" => {
                        let muted = state
                            .core
                            .lock()
                            .map_err(|_| lock_error())
                            .and_then(|core| core.settings())
                            .map(|s| flag(&s, "muted", true));
                        muted.and_then(|m| {
                            update_settings(&app, &state, &json!({"muted":!m})).map(|_| ())
                        })
                    }
                    "exit" => {
                        begin_shutdown(&app);
                        Ok(())
                    }
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    if let Ok(mut stored) = state.shell_error.lock() {
                        *stored = Some(error);
                    }
                }
            });
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = show_panel(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}
fn start_file_watcher(shared: SharedState) {
    std::thread::spawn(move || {
        let mut watcher = native::DirectoryWatches::new();
        let mut watched: Vec<PathBuf> = vec![];
        while !shared.stop.load(Ordering::Relaxed) {
            let sources = shared
                .core
                .lock()
                .map_err(|_| lock_error())
                .and_then(|core| core.enabled_codex_source_roots())
                .unwrap_or_default();
            let mut desired: Vec<PathBuf> = sources
                .into_iter()
                .filter(|p| p.is_dir())
                .collect();
            desired.sort();
            desired.dedup();
            if desired != watched || watcher.needs_retry() {
                let failed = watcher.replace(&desired);
                watched = desired;
                if let Ok(mut error) = shared.watcher_error.lock() {
                    *error = if failed {
                        Some("部分来源目录无法监听，周期补扫继续".into())
                    } else {
                        None
                    };
                }
                wake_scan(&shared);
            }
            if watcher.wait_changed() {
                wake_scan(&shared);
            }
        }
    });
}

fn notification_summary(events: &[Value]) -> String {
    let mut counts = [0usize; 5];
    for event in events {
        let index = if event["kind"] == "budget" {
            4
        } else {
            match event["status"].as_str().unwrap_or("") {
                "failed" => 0,
                "interrupted" => 1,
                "completed" => 2,
                _ => 3,
            }
        };
        counts[index] += 1;
    }
    let labels = [
        "个回合失败",
        "个回合中断",
        "个回合正常结束",
        "个回合已结束，结果未知",
        "项本地软预算提醒",
    ];
    let parts: Vec<String> = counts
        .iter()
        .zip(labels)
        .filter(|(n, _)| **n > 0)
        .map(|(n, label)| format!("{n}{label}"))
        .collect();
    format!("{}。详情保留在本地收件箱。", parts.join("；"))
}
fn process_notifications(
    shared: &Shared,
    queue: &mut Vec<QueuedNotification>,
    queued_at: &mut Option<Instant>,
    last_sent: &mut Option<Instant>,
) {
    let claimed = shared
        .core
        .lock()
        .ok()
        .and_then(|mut core| core.claim_notifications().ok().map(|events| {
            events.into_iter().map(|event| QueuedNotification {
                permit: core.budget_notification_permit(&event), event,
            }).collect::<Vec<_>>()
        }))
        .unwrap_or_default();
    let admission = notification_gate::admit_notifications(queue, claimed, &crate::budget_clock::now());
    if admission.reset_queued_at { *queued_at = None; }
    if !admission.suppressed.is_empty() {
        if let Ok(mut core) = shared.core.lock() {
            for ids in admission.suppressed.chunks(100) {
                let _ = core.finish_notification(ids, "suppressed", None);
            }
        }
    }
    if queue.is_empty() {
        *queued_at = None;
        return;
    }
    if queued_at.is_none() {
        *queued_at = Some(Instant::now());
    }
    let settings = shared
        .core
        .lock()
        .ok()
        .and_then(|core| core.settings().ok())
        .unwrap_or_default();
    let suppressed = shared.notification_gate.snapshot().is_none()
        || shared.native_suppress_toast.load(Ordering::Acquire)
        || flag(
            &settings,
            "effectiveDoNotDisturb",
            flag(&settings, "doNotDisturb", false),
        );
    if !suppressed
        && (queued_at.is_some_and(|at| at.elapsed() < Duration::from_secs(2))
            || last_sent.is_some_and(|at| at.elapsed() < Duration::from_secs(10)))
    {
        return;
    }
    let result = if suppressed {
        notification_gate::Delivery {
            suppressed: queue.iter().filter_map(|event| event.event["id"].as_str().map(str::to_owned)).collect(),
            ..Default::default()
        }
    } else {
        native::show_notification(
            "Token 喵 · 本地提醒",
            queue,
            flag(&settings, "muted", true),
            || {
                // COM preparation can outlive a pause or settings change. Do not
                // block a native event owner or submit on unknown settings.
                !shared.stop.load(Ordering::Acquire)
                    && shared.notification_gate.snapshot().is_some()
                    && !shared.native_suppress_toast.load(Ordering::Acquire)
                    && shared.core.try_lock().ok().and_then(|core| core.settings().ok())
                        .is_some_and(|settings| !flag(&settings, "effectiveDoNotDisturb", true))
            },
        )
    };
    if let Ok(mut core) = shared.core.lock() {
        for (ids, status) in [(&result.suppressed, "suppressed"), (&result.submitted, "sent"), (&result.failed, "failed")] {
            if !ids.is_empty() { let _ = core.finish_notification(ids, status, result.error.as_deref()); }
        }
    }
    if !result.submitted.is_empty() || !result.failed.is_empty() {
        *last_sent = Some(Instant::now());
    }
    if let Ok(mut metrics) = shared.metrics.lock() {
        metrics["notifications"] = json!({"status":if !result.failed.is_empty(){"failed"}else if !result.submitted.is_empty(){"submitted"}else{"suppressed"},"lastAttemptAt":chrono::Utc::now().to_rfc3339(),"eventCount":result.submitted.len()+result.failed.len()+result.suppressed.len(),"suppressedCount":result.suppressed.len(),"error":result.error});
        persist_metrics(shared, &metrics);
    }
    queue.clear();
    *queued_at = None;
}


// Desktop companion context has its own revision and never changes ledger facts.
fn motion_work_idle(shared: &Shared) -> Option<bool> {
    if shared.collector_error.try_lock().ok()?.is_some() || shared.watcher_error.try_lock().ok()?.is_some() {
        return Some(false);
    }
    Some(shared.core.try_lock().ok()?.desktop_motion_idle().unwrap_or(false))
}

fn interrupt_motion(app: &AppHandle, shared: &Shared, reason: &'static str) {
    let update = shared.companion.try_lock().ok().and_then(|mut owner| owner.interrupt(reason));
    if let Some(update) = update { let _ = app.emit("desktop-companion-state", update); }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MotionReadiness {
    renderer_epoch: String,
    token: String,
    sequence: String,
    ready: bool,
    gait_available: bool,
    gait_ready: bool,
    motion_generation: Option<String>,
    motion_direction: Option<String>,
}

#[tauri::command]
fn register_pet_motion_renderer(
    window: WebviewWindow, app: AppHandle, state: State<'_, SharedState>, renderer_epoch: String,
) -> Result<String, String> {
    if window.label() != "pet" || uuid::Uuid::parse_str(&renderer_epoch).is_err() ||
        state.exiting.load(Ordering::Relaxed) { return Err("行走渲染登记无效".into()); }
    let token = uuid::Uuid::new_v4().to_string();
    {
        // Never wait on the movement owner from a WebView/UI command thread.
        let mut motion = state.motion.try_lock().map_err(|_| lock_error())?;
        motion.epoch = motion.epoch.checked_add(1).ok_or("行走代次已耗尽")?;
        motion.cancel(false);
        motion.presented = None;
        motion.permission.register(renderer_epoch, token.clone(), state.started_at.elapsed());
    }
    interrupt_motion(&app, &state, "animationUnavailable");
    Ok(token)
}

#[tauri::command]
fn set_pet_motion_readiness(
    window: WebviewWindow, app: AppHandle, state: State<'_, SharedState>, readiness: MotionReadiness,
) -> Result<bool, String> {
    if window.label() != "pet" || state.exiting.load(Ordering::Relaxed) ||
        readiness.sequence.is_empty() || readiness.sequence.len() > 20 ||
        !readiness.sequence.bytes().all(|value| value.is_ascii_digit()) { return Ok(false); }
    let Some(sequence) = readiness.sequence.parse::<u64>().ok() else { return Ok(false); };
    let presented = readiness.motion_generation.as_deref()
        .filter(|value| !value.is_empty() && value.len() <= 20 &&
            (*value == "0" || !value.starts_with('0')) && value.bytes().all(|c| c.is_ascii_digit()))
        .and_then(|value| value.parse::<u64>().ok())
        .zip(match readiness.motion_direction.as_deref() {
            Some("left") => Some(roaming::Direction::Left),
            Some("right") => Some(roaming::Direction::Right), _ => None,
        });
    let valid_gait = !readiness.gait_ready || presented.is_some();
    let flags = motion_guard::Readiness { ready: readiness.ready && valid_gait,
        gait_available: readiness.gait_available, gait_ready: readiness.gait_ready && valid_gait };
    let accepted = {
        let mut motion = state.motion.try_lock().map_err(|_| lock_error())?;
        let accepted = motion.permission.accept(&readiness.renderer_epoch, &readiness.token,
            sequence, flags, state.started_at.elapsed());
        if accepted {
            motion.presented = flags.gait_ready.then_some(presented).flatten();
            if !flags.ready { motion.cancel(false); }
        }
        accepted
    };
    if accepted && !flags.ready { interrupt_motion(&app, &state, "animationUnavailable"); }
    Ok(accepted)
}

fn poll_companion(app: &AppHandle, shared: &Shared) {
    let settings = shared.projection.try_lock().ok().map(|value| value.clone());
    let pet_window = app.get_webview_window("pet");
    let visible = pet_window.as_ref().is_some_and(|window| {
        window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false)
    });
    let interacting = shared.pet_interaction.try_lock().map(|value| value.active).unwrap_or(true)
        || shared.native_dragging.load(Ordering::Relaxed);
    let inspect_edge = shared.motion.try_lock().is_ok_and(|motion|
        motion.permission.feature_available() && motion.request.is_none());
    // Sample HWND/geometry without holding motion/Core/settings locks. Facing
    // is presentation metadata only; the UI callback revalidates every gate.
    let edge_snapshot = inspect_edge.then(|| pet_window.as_ref()
        .and_then(native::motion_snapshot)).flatten();
    let (available, renderer_ready, direction) = shared.motion.try_lock().map(|mut motion| {
        let available = motion.permission.feature_available() && cfg!(windows);
        let ready = motion.permission.readiness(shared.started_at.elapsed()).is_some_and(|flags| flags.ready);
        if available && motion.request.is_none() {
            if let Some((geometry, position)) = edge_snapshot {
                if let Some(inward) = roaming::choose_inward_direction(geometry, position, motion.direction) {
                    motion.direction = inward;
                }
            }
        }
        (available, ready, motion.direction())
    }).unwrap_or((false, false, companion::MotionDirection::Right));
    // A short Core lock contention skips an idle sample; it cannot authorize a
    // move, and the movement callback separately fails closed on contention.
    let work_idle = if available && renderer_ready { match motion_work_idle(shared) {
        Some(idle) => idle, None => return,
    } } else { true };
    let blocked = if shared.stop.load(Ordering::Relaxed) || shared.native_paused.load(Ordering::Relaxed) {
        Some("desktopPaused")
    } else if !visible || settings.as_ref().is_some_and(|value| !flag(value, "petVisible", true)) {
        Some("hidden")
    } else if interacting || settings.is_none() {
        Some("interaction")
    } else if available && !renderer_ready {
        Some("animationUnavailable")
    } else if !work_idle {
        Some("workActive")
    } else if available && (!native::motion_environment_allowed() || shared.display_recheck.load(Ordering::Relaxed)) {
        Some("systemBusy")
    } else {
        companion::settings_pause(settings.as_ref().unwrap())
    };
    let update = shared.companion.try_lock().ok()
        .and_then(|mut owner| owner.tick(settings.as_ref().unwrap_or(&Value::Null), blocked,
            companion::RoamingContext { available, direction }));
    if let Some(update) = update { let _ = app.emit("desktop-companion-state", update); }
}

/// Queue only a tick, never a target or a permission. At most one tick can wait
/// for the UI thread; planning and final native execution happen together there.
fn schedule_pet_motion(app: &AppHandle, shared: &SharedState) {
    if shared.motion_tick_pending.swap(true, Ordering::AcqRel) { return; }
    // Any path access / uncached ledger query stays on this worker. The UI
    // callback only consumes a recent cache verdict under the Core mutex.
    if shared.companion.try_lock().is_ok_and(|owner| owner.movement().is_some()) {
        let _ = motion_work_idle(shared);
    }
    struct PendingTick(SharedState);
    impl Drop for PendingTick {
        fn drop(&mut self) { self.0.motion_tick_pending.store(false, Ordering::Release); }
    }
    let pending = PendingTick(shared.clone());
    let handle = app.clone();
    if app.run_on_main_thread(move || {
        step_pet_motion_on_main(&handle, &pending.0);
        drop(pending);
    }).is_err() {
        shared.motion_tick_pending.store(false, Ordering::Release);
    }
}

/// Native calls run on the UI thread, so no synchronous HWND getter or move
/// waits for a UI thread that might itself be waiting for Core/settings locks.
fn step_pet_motion_on_main(app: &AppHandle, shared: &Shared) {
    let Some(window) = app.get_webview_window("pet") else {
        cancel_manual_drag(shared, manual_drag::StopReason::Hidden);
        return;
    };
    #[cfg(windows)]
    if step_manual_drag_on_main(&window, shared).handled { return; }
    // Pointer leases expire even when roaming is disabled or its mutex is busy.
    let _ = pointer::sync(&window, shared);
    let Ok(mut motion) = shared.motion.try_lock() else { return; };
    if shared.motion_invalidated.swap(false, Ordering::AcqRel) {
        motion.cancel(true);
        drop(motion);
        interrupt_motion(app, shared, "interaction");
        return;
    }
    let movement = shared.companion.try_lock().ok().and_then(|owner| owner.movement());
    let Some(movement) = movement else { motion.cancel(false); return; };
    let Ok(_settings_guard) = shared.settings_update.try_lock() else {
        motion.cancel(false); drop(motion); interrupt_motion(app, shared, "interaction"); return;
    };
    let now = shared.started_at.elapsed();
    let permission = motion.permission.readiness(now);
    let settings = shared.projection.try_lock().ok().map(|value| value.clone());
    let interacting = shared.pet_interaction.try_lock().map(|value| value.active).unwrap_or(true);
    let blocked = shared.stop.load(Ordering::Acquire) || shared.exiting.load(Ordering::Acquire) ||
        shared.native_paused.load(Ordering::Acquire) || shared.native_dragging.load(Ordering::Acquire) ||
        shared.display_recheck.load(Ordering::Acquire) || interacting || native::mouse_down() ||
        !native::motion_window_available(&window) || !native::motion_environment_allowed() ||
        native::motion_input_marker() != Some(movement.input_marker) ||
        !permission.is_some_and(|flags| flags.ready && flags.gait_available) ||
        settings.as_ref().is_none_or(|value| !flag(value, "idlePlayEnabled", true) ||
            !flag(value, "idleRoamingEnabled", true) || !flag(value, "petVisible", true) ||
            companion::settings_pause(value).is_some());
    if blocked {
        motion.cancel(false);
        drop(motion);
        interrupt_motion(app, shared, "animationUnavailable");
        return;
    }
    if !permission.is_some_and(|flags| flags.gait_ready) ||
        motion.presented != Some((movement.generation, motion.direction)) {
        // Loading and direction acknowledgement consume the original finite
        // burst; neither waiting nor a heartbeat renews its wall-clock lease.
        return;
    }
    let clean = |error: &Mutex<Option<String>>| error.try_lock().is_ok_and(|value| value.is_none());
    let Ok(mut core) = shared.core.try_lock() else {
        motion.cancel(false); drop(motion); interrupt_motion(app, shared, "workActive"); return;
    };
    if !clean(&shared.collector_error) || !clean(&shared.watcher_error) ||
        !core.desktop_motion_cached_idle() {
        motion.cancel(false); drop(core); drop(motion); interrupt_motion(app, shared, "workActive"); return;
    }
    let Some((geometry, position)) = native::motion_snapshot(&window) else {
        motion.cancel(true); drop(core); drop(motion); interrupt_motion(app, shared, "geometryUnavailable"); return;
    };
    let range = if settings.as_ref().is_some_and(|value| value["idleRoamRange"] == "nearby") {
        roaming::Range::Nearby
    } else { roaming::Range::WorkArea };
    if motion.request.is_some_and(|request| request.range != range) {
        motion.cancel(false); drop(core); drop(motion); interrupt_motion(app, shared, "interaction"); return;
    }
    if motion.request.is_none_or(|request| request.generation != movement.generation) {
        let anchor = *motion.anchor.get_or_insert(position);
        motion.request = Some(roaming::Request { epoch: motion.epoch, generation: movement.generation,
            revision: 1, duration: movement.remaining, anchor, range,
            direction: match movement.direction { companion::MotionDirection::Left => roaming::Direction::Left,
                companion::MotionDirection::Right => roaming::Direction::Right } });
    }
    let observation = roaming::Observation { now, gate: roaming::Gate::Allowed,
        input_marker: native::motion_input_marker().filter(|value| *value == movement.input_marker), request: motion.request,
        geometry: Some(geometry), position: Some(position) };
    let Some(intent) = motion.planner.step(observation) else {
        if motion.planner.state().is_none() {
            motion.cancel(true); drop(core); drop(motion); interrupt_motion(app, shared, "geometryUnavailable");
        }
        return;
    };
    if motion.presented != Some((movement.generation, intent.direction())) {
        motion.direction = intent.direction();
        motion.cancel(false);
        drop(core); drop(motion); interrupt_motion(app, shared, "animationUnavailable");
        return;
    }
    let fresh = native::motion_snapshot(&window);
    let now = shared.started_at.elapsed();
    let fresh_permission = motion.permission.readiness(now);
    let native_allowed = fresh_permission.is_some_and(|flags| flags.ready && flags.gait_available && flags.gait_ready) &&
        motion.presented == Some((movement.generation, intent.direction())) &&
        !shared.stop.load(Ordering::Acquire) && !shared.exiting.load(Ordering::Acquire) &&
        !shared.native_paused.load(Ordering::Acquire) && !shared.native_dragging.load(Ordering::Acquire) &&
        !shared.motion_invalidated.load(Ordering::Acquire) && !shared.display_recheck.load(Ordering::Acquire) &&
        !native::mouse_down() && native::motion_window_available(&window) && native::motion_environment_allowed() &&
        shared.pet_interaction.try_lock().is_ok_and(|value| !value.active) &&
        shared.companion.try_lock().is_ok_and(|owner| owner.movement().is_some_and(|value| value.generation == movement.generation));
    let current = roaming::Observation { now,
        gate: if native_allowed { roaming::Gate::Allowed } else { roaming::Gate::Blocked },
        input_marker: native::motion_input_marker().filter(|value| *value == movement.input_marker), request: motion.request,
        geometry: fresh.map(|value| value.0), position: fresh.map(|value| value.1) };
    let MotionRuntime { planner, permission: permission_owner, .. } = &mut *motion;
    let outcome = planner.apply_if_current(intent, current, |target| {
        // No Core writer can change the admitted work facts while this lock is held.
        let Ok(mut positions) = shared.automatic_positions.try_lock() else { return false; };
        let final_now = shared.started_at.elapsed();
        if final_now.checked_sub(observation.now).is_none_or(|age| age > Duration::from_millis(100)) ||
            !core.desktop_motion_cached_idle() ||
            !permission_owner.readiness(final_now).is_some_and(|value| value.ready && value.gait_available && value.gait_ready) ||
            !shared.companion.try_lock().is_ok_and(|owner| owner.movement().is_some_and(|value|
                value.generation == movement.generation && value.input_marker == movement.input_marker)) ||
            native::motion_input_marker() != Some(movement.input_marker) { return false; }
        let tagged = positions.tag(target, final_now);
        drop(positions);
        if !tagged { return false; }
        let applied = native::move_pet_without_activation(&window, target);
        if !applied {
            if let Ok(mut owner) = shared.automatic_positions.lock() { owner.failed(target); }
        }
        applied
    });
    if outcome != roaming::ApplyOutcome::Applied {
        motion.cancel(false); drop(core); drop(motion); interrupt_motion(app, shared, "interaction"); return;
    }
    if let Some(state) = motion.planner.state() { motion.direction = state.direction; }
    // The next worker poll publishes a turn before another native step is
    // admitted; the renderer must acknowledge the new generation+direction.
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompanionLauncherOptions { launcher_id: String }

#[tauri::command]
async fn get_desktop_companion_state(app: AppHandle, state: State<'_, SharedState>) -> Result<companion::DesktopCompanionState, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        poll_companion(&app, &shared);
        let current = shared.companion.lock().map_err(|_| lock_error())?.state();
        Ok(current)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
fn list_companion_launchers(state: State<'_, SharedState>) -> Result<Vec<companion::LauncherSummary>, String> {
    state.companion.lock().map_err(|_| lock_error())?.list()
}

#[tauri::command]
async fn choose_companion_launcher(app: AppHandle, state: State<'_, SharedState>) -> Result<Option<companion::LauncherSummary>, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shared.exiting.load(Ordering::Relaxed) { return Err("应用正在退出。".into()); }
        let _permit = shared.companion.lock().map_err(|_| lock_error())?.begin_picker()?;
        poll_companion(&app, &shared);
        let selected = app.dialog().file().add_filter("Windows 应用", &["exe"]).blocking_pick_file();
        let Some(selected) = selected else { return Ok(None); };
        if shared.exiting.load(Ordering::Relaxed) { return Err("应用正在退出，未新增启动项。".into()); }
        let selected = selected.into_path().map_err(|_| "请选择本机应用文件。".to_owned())?;
        let entry = shared.companion.lock().map_err(|_| lock_error())?.register(selected)?;
        let listed = shared.companion.lock().map_err(|_| lock_error())?.list()?;
        let _ = app.emit("companion-launchers-changed", listed);
        Ok(Some(entry))
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn remove_companion_launcher(app: AppHandle, state: State<'_, SharedState>, options: CompanionLauncherOptions) -> Result<(), String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shared.exiting.load(Ordering::Relaxed) { return Err("应用正在退出。".into()); }
        shared.companion.lock().map_err(|_| lock_error())?.remove(&options.launcher_id)?;
        let listed = shared.companion.lock().map_err(|_| lock_error())?.list()?;
        let _ = app.emit("companion-launchers-changed", listed);
        poll_companion(&app, &shared);
        Ok(())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn launch_companion_app(state: State<'_, SharedState>, options: CompanionLauncherOptions) -> Result<Value, String> {
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shared.exiting.load(Ordering::Relaxed) { return Err("应用正在退出。".into()); }
        let target = shared.companion.lock().map_err(|_| lock_error())?.target(&options.launcher_id)?;
        companion::launch_selected(&target)?;
        Ok(json!({"accepted":true}))
    }).await.map_err(|error| error.to_string())?
}

fn start_workers(app: AppHandle, shared: SharedState) {
    let s = shared.clone();
    let a = app.clone();
    std::thread::spawn(move || {
        let mut notification_queue = vec![];
        let mut queued_at = None;
        let mut last_sent = None;
        let mut scan_metrics_flushed: Option<Instant> = None;
        let mut attempt_iteration = 0u64;
        let mut previous_notification_phase: Option<(u64, bool, f64, f64)> = None;
        while !s.stop.load(Ordering::Relaxed) {
            attempt_iteration = attempt_iteration.saturating_add(1);
            let scan_started = Instant::now();
            let host = s.notification_gate.snapshot().map(|epoch| (epoch, crate::budget_clock::now()));
            let mut contended = false;
            let core_wait_started = Instant::now();
            let guard = match s.core.try_lock() {
                Ok(guard) => Ok(guard),
                Err(TryLockError::WouldBlock) => {
                    contended = true;
                    s.core.lock().map_err(|_| lock_error())
                }
                Err(TryLockError::Poisoned(_)) => Err(lock_error()),
            };
            let core_lock_wait_ms = core_wait_started.elapsed().as_secs_f64() * 1000.0;
            let core_scan_started = Instant::now();
            let result = guard.and_then(|mut core| core.scan_with_cancel_host(&s.stop, host));
            let core_scan_ms = core_scan_started.elapsed().as_secs_f64() * 1000.0;
            let scan_succeeded = result.is_ok();
            let mut followup_delay = Duration::from_secs(2);
            match result {
                Ok(report) => {
                    // Continue productive history work after a bounded yield. A
                    // half-line, read issue, or contended Core keeps idle backoff.
                    followup_delay = scan_followup_delay(
                        report.lines,
                        report.pending_files,
                        report.issues,
                        contended,
                    );
                    if let Ok(mut error) = s.collector_error.lock() {
                        *error = None;
                    }
                    let report_value = serde_json::to_value(&report).unwrap_or_default();
                    if let Ok(mut metrics) = s.metrics.lock() {
                        let previous = &metrics["collector"];
                        let iterations = previous["iterations"]
                            .as_u64()
                            .unwrap_or(0)
                            .saturating_add(1);
                        let total_lines = previous["totalLines"]
                            .as_u64()
                            .unwrap_or(0)
                            .saturating_add(report.lines as u64);
                        let total_responses = previous["totalNewResponses"]
                            .as_u64()
                            .unwrap_or(0)
                            .saturating_add(report.new_responses as u64);
                        metrics["collector"] = json!({"iterations":iterations,"totalLines":total_lines,"totalNewResponses":total_responses,"lastScanAndWaitMs":scan_started.elapsed().as_secs_f64()*1000.0,"lastReport":report_value,"lastSuccessAt":chrono::Utc::now().to_rfc3339(),
                            "attemptIteration":attempt_iteration,"coreLockWaitMs":core_lock_wait_ms,"coreScanMs":core_scan_ms,
                            // Published one successful scan later without another metrics lock or flush.
                            "previousNotificationPhase":previous_notification_phase.map(|(attempt, succeeded, elapsed, completed)|json!({"attemptIteration":attempt,"scanSucceeded":succeeded,"elapsedMs":elapsed,"completedSinceAppStartMs":completed}))});
                        // The idle collector updates RAM each scan; disk metrics have a bounded cadence.
                        if report.lines > 0
                            || scan_metrics_flushed
                                .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
                        {
                            persist_metrics(&s, &metrics);
                            scan_metrics_flushed = Some(Instant::now());
                        }
                    }
                    let _ = a.emit("ledger-updated", report_value);
                }
                Err(error) => {
                    if let Ok(mut stored) = s.collector_error.lock() {
                        *stored = Some(error);
                    }
                }
            }
            if !s.stop.load(Ordering::Relaxed) {
                let notification_started = Instant::now();
                process_notifications(&s, &mut notification_queue, &mut queued_at, &mut last_sent);
                let notification_ms = notification_started.elapsed().as_secs_f64() * 1000.0;
                previous_notification_phase = Some((attempt_iteration, scan_succeeded, notification_ms,
                    s.started_at.elapsed().as_secs_f64() * 1000.0));
            }
            if let Ok(pending) = s.scan_wake.0.lock() {
                if let Ok((mut pending, _)) =
                    s.scan_wake
                        .1
                        .wait_timeout_while(pending, followup_delay, |pending| {
                            !*pending && !s.stop.load(Ordering::Relaxed)
                        })
                {
                    *pending = false;
                }
            }
        }
        if let Ok(mut core) = s.core.lock() {
            let _ = core.shutdown();
        }
        s.scanner_done.store(true, Ordering::Release);
    });
    let s = shared.clone();
    let a = app.clone();
    std::thread::spawn(move || {
        let mut directory_at: Option<Instant> = None;
        let mut refreshed_at: Option<Instant> = None;
        let mut last_source = Value::Null;
        while !s.stop.load(Ordering::Relaxed) {
            let (source, source_generation) = s
                .core
                .lock()
                .map_err(|_| lock_error())
                .and_then(|core| {
                    let source = core.primary_source()?;
                    let generation = s.account_cache.lock().map_err(|_| lock_error())?.generation;
                    Ok((source, generation))
                })
                .unwrap_or((Value::Null, 0));
            let changed = source != last_source;
            if changed {
                directory_at = None;
                last_source = source.clone();
                record_metric(
                    &s,
                    "directory",
                    json!({"status":"pending","sourceId":source["id"],"lastSuccessAt":null,"count":null,"error":null}),
                );
            }
            if changed || refreshed_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(120)) {
                refreshed_at = Some(Instant::now());
                if let Ok(snapshot) = update_account(&s, false) {
                    let _ = a.emit("account-updated", snapshot);
                }
            }
            if !s.stop.load(Ordering::Relaxed)
                && flag(&source, "enabled", false)
                && directory_at.is_none_or(|at| at.elapsed() >= Duration::from_secs(600))
            {
                directory_at = Some(Instant::now());
                let directory_generation = s
                    .account_cache
                    .lock()
                    .map(|cache| cache.generation)
                    .unwrap_or(0);
                let rows = source_config(&source).and_then(|config| {
                    let adapter = adapter_for(source["clientKind"].as_str().unwrap_or_default())
                        .ok_or("主来源适配器尚未发布会话目录读取能力")?;
                    let mut bridge = s.account.lock().map_err(|_| lock_error())?;
                    let current_core = s.core.lock().map_err(|_| lock_error())?;
                    if current_core.primary_source()? != source
                        || s.account_cache.lock().map_err(|_| lock_error())?.generation
                            != directory_generation
                    {
                        return Err("主来源已变化，已跳过旧来源的目录读取".into());
                    }
                    drop(current_core);
                    if s.account_reset_pending.swap(false, Ordering::SeqCst) {
                        bridge.clear_cached_data();
                    }
                    adapter.list_sessions(&config, &mut *bridge)
                });
                match rows {
                    Ok(rows) => {
                        if let Ok(mut core) = s.core.lock() {
                            if core.primary_source().ok().as_ref() == Some(&source)
                                && s.account_cache
                                    .lock()
                                    .map(|cache| cache.generation == directory_generation)
                                    .unwrap_or(false)
                            {
                                let count = rows.len();
                                let merged = core.action(
                                    "merge_session_directory",
                                    &json!({"sessions":rows,"sourceId":source["id"]}),
                                );
                                if let Ok(mut metrics) = s.metrics.lock() {
                                    let previous = metrics["directory"].clone();
                                    metrics["directory"] = if let Err(error) = merged {
                                        directory_failure(
                                            &previous,
                                            &source,
                                            directory_generation,
                                            &error,
                                        )
                                    } else {
                                        json!({"status":"connected","sourceId":source["id"],"bindingGeneration":directory_generation,"lastSuccessAt":chrono::Utc::now().to_rfc3339(),"count":count,"error":null})
                                    };
                                    persist_metrics(&s, &metrics);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Ok(core) = s.core.lock() {
                            if core.primary_source().ok().as_ref() == Some(&source)
                                && s.account_cache
                                    .lock()
                                    .map(|cache| cache.generation == directory_generation)
                                    .unwrap_or(false)
                            {
                                if let Ok(mut metrics) = s.metrics.lock() {
                                    let previous = metrics["directory"].clone();
                                    metrics["directory"] = directory_failure(
                                        &previous,
                                        &source,
                                        directory_generation,
                                        &error,
                                    );
                                    persist_metrics(&s, &metrics);
                                }
                            }
                        }
                    }
                }
            }
            if let Ok(Some(snapshot)) =
                observe_account_resource_stop(&s, &source, source_generation)
            {
                let _ = a.emit("account-updated", snapshot);
            }
            record_account_bridge_resource(&s);
            for _ in 0..10 {
                if s.stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        if let Ok(mut bridge) = s.account.lock() {
            bridge.shutdown();
        }
        record_account_bridge_resource(&s);
        s.account_done.store(true, Ordering::Release);
    });
    std::thread::spawn(move || {
        let mut checked = Instant::now();
        let mut companion_checked = Instant::now();
        while !shared.stop.load(Ordering::Relaxed) {
            let moving = shared.companion.try_lock().is_ok_and(|owner| owner.movement().is_some());
            let dragging = shared.manual_drag.try_lock().is_ok_and(|owner| owner.is_active());
            let delay = Duration::from_millis(if dragging { 16 } else if moving { 50 } else { 200 });
            // The first manual gesture wakes an idle worker immediately. The
            // predicate also retains a notification sent before it starts waiting.
            if let Ok(wake) = shared.motion_wake.0.lock() {
                if let Ok((mut wake, _)) = shared.motion_wake.1.wait_timeout_while(wake, delay, |wake| !*wake) {
                    *wake = false;
                }
            }
            if shared.stop.load(Ordering::Acquire) { break; }
            if companion_checked.elapsed() >= Duration::from_millis(200) {
                companion_checked = Instant::now();
                poll_companion(&app, &shared);
            }
            schedule_pet_motion(&app, &shared);
            if let Some(window) = app.get_webview_window("pet") {
                if checked.elapsed() >= Duration::from_secs(1) {
                    checked = Instant::now();
                    if let Ok(current) = native::work_areas(&window) {
                        let changed = shared
                            .topology
                            .lock()
                            .map(|old| *old != current)
                            .unwrap_or(false);
                        if !native::mouse_down()
                            && !shared.native_dragging.load(Ordering::Relaxed)
                            && (changed || shared.display_recheck.swap(false, Ordering::Relaxed))
                        {
                            let _ = restore_position(&app, &shared, false);
                            let _ = fit_main_window(&app);
                            record_metric(
                                &shared,
                                "displayRecovery",
                                json!({"at":chrono::Utc::now().to_rfc3339(),"monitorCount":current.len(),"topologyChanged":changed,"dragging":false}),
                            );
                            if let Some(main) = app.get_webview_window("main") {
                                record_window_state(&main, &shared);
                            }
                        }
                    }
                }
                let settled = shared
                    .position_dirty
                    .lock()
                    .ok()
                    .and_then(|v| *v)
                    .is_some_and(|at| at.elapsed() > Duration::from_millis(350));
                if settled
                    && !native::mouse_down()
                    && !shared.native_dragging.load(Ordering::Relaxed)
                    && !pet_has_transient_bounds(&shared)
                {
                    if let Ok(mut dirty) = shared.position_dirty.lock() {
                        *dirty = None;
                    }
                    if let Err(error) = persist_settled_position(&app, &shared) {
                        if let Ok(mut stored) = shared.shell_error.lock() {
                            *stored = Some(error);
                        }
                    }
                }
            }
        }
    });
}

#[cfg(debug_assertions)]
#[tauri::command]
fn desktop_diagnostics(app: AppHandle, state: State<'_, SharedState>) -> Result<Value, String> {
    let mut windows = vec![];
    for label in ["main", "pet"] {
        if let Some(w) = app.get_webview_window(label) {
            windows.push(json!({"label":label,"visible":w.is_visible().unwrap_or(false),"focused":w.is_focused().unwrap_or(false),"activation":native::window_activation_state(&w),"position":w.outer_position().ok().map(|v|json!({"x":v.x,"y":v.y})),"size":w.outer_size().ok().map(|v|json!({"width":v.width,"height":v.height})),"scaleFactor":w.scale_factor().ok()}));
        }
    }
    Ok(
        json!({"windows":windows,"monitors":state.topology.lock().map_err(|_|lock_error())?.clone(),"shortcutAvailable":state.shortcut_ready.load(Ordering::Relaxed),"exiting":state.exiting.load(Ordering::Relaxed),"settings":state.core.lock().map_err(|_|lock_error())?.settings()?,"dataDirectory":state.data_dir,"autostart":native::autostart_status(),"collectorError":state.collector_error.lock().map_err(|_|lock_error())?.clone(),"shellError":state.shell_error.lock().map_err(|_|lock_error())?.clone(),"watcherError":state.watcher_error.lock().map_err(|_|lock_error())?.clone(),"renderPaused":state.native_paused.load(Ordering::Relaxed),"dragging":state.native_dragging.load(Ordering::Relaxed)}),
    )
}
#[cfg(debug_assertions)]
#[tauri::command]
async fn desktop_debug(
    app: AppHandle,
    state: State<'_, SharedState>,
    action: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    match action.as_str() {
        "hide" => {
            update_settings(&app, &state, &json!({"petVisible":false}))?;
        }
        "recover" => recover_pet(&app, true)?,
        "recheck_displays" => {
            restore_position(&app, &state, false)?;
            fit_main_window(&app)?;
        }
        "notification" => {
            native::show_diagnostic_notification(|| {
                state.notification_gate.snapshot().is_some()
                    && !state.native_suppress_toast.load(Ordering::Acquire)
                    && state.core.try_lock().ok().and_then(|core| core.settings().ok())
                        .is_some_and(|settings| !flag(&settings, "effectiveDoNotDisturb", true))
            })?;
        }
        "move" => {
            let p = payload.unwrap_or_default();
            let x = p
                .get("x")
                .and_then(Value::as_i64)
                .filter(|n| (-100000..=100000).contains(n))
                .ok_or("x超出诊断范围")?;
            let y = p
                .get("y")
                .and_then(Value::as_i64)
                .filter(|n| (-100000..=100000).contains(n))
                .ok_or("y超出诊断范围")?;
            app.get_webview_window("pet")
                .ok_or("桌宠未就绪")?
                .set_position(PhysicalPosition::new(x as i32, y as i32))
                .map_err(|e| e.to_string())?;
        }
        _ => return Err("未知的受控桌面诊断操作".into()),
    }
    desktop_diagnostics(app, state)
}

pub fn run() {
    let started_at = Instant::now();
    #[allow(unused_mut)]
    let mut context = tauri::generate_context!();
    #[cfg(debug_assertions)]
    if let Some(port) = std::env::var("TOKEN_MEOW_DEBUG_CDP_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port >= 1024)
    {
        for window in &mut context.config_mut().app.windows {
            let prior = window.additional_browser_args.take().unwrap_or_default();
            window.additional_browser_args = Some(format!(
                "{prior} --remote-debugging-address=127.0.0.1 --remote-debugging-port={port}"
            ));
        }
    }
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if args.iter().any(|arg| arg == "--background") {
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = recover_pet(&app, false);
                let _ = show_panel(&app);
            });
        }))
        .plugin(tauri_plugin_dialog::init());
    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_desktop_companion_state,
        register_pet_motion_renderer,
        set_pet_motion_readiness,
        list_companion_launchers,
        choose_companion_launcher,
        remove_companion_launcher,
        launch_companion_app,
        get_snapshot,
        get_usage_dashboard_snapshot,
        save_usage_price,
        save_usage_price_sync,
        apply_usage_price_sync,
        record_usage_price_sync_error,
        get_pet_pulse,
        get_pet_details,
        run_action,
        set_pet_options,
        set_pet_visibility,
        set_pet_interaction,
        pointer::register_pet_pointer_renderer,
        pointer::begin_pet_pointer_frame,
        pointer::publish_pet_pointer_mask,
        pointer::renew_pet_pointer_mask,
        pointer::release_pet_pointer_renderer,
        pointer::begin_pet_temporary_interaction,
        pointer::cancel_pet_temporary_interaction,
        get_pet_extension_capabilities,
        get_pet_skin_state,
        choose_import_pet_skin,
        select_pet_skin,
        read_pet_skin_frame,
        remove_pet_skin,
        open_panel,
        panel_navigation,
        open_pet_chat,
        open_thread,
        start_pet_drag,
        export_csv,
        refresh_account,
        reset_pet_position,
        quit_app,
        choose_source_directory,
        frontend_ready,
        desktop_metrics,
        desktop_diagnostics,
        desktop_debug
    ]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        get_desktop_companion_state,
        register_pet_motion_renderer,
        set_pet_motion_readiness,
        list_companion_launchers,
        choose_companion_launcher,
        remove_companion_launcher,
        launch_companion_app,
        get_snapshot,
        get_usage_dashboard_snapshot,
        save_usage_price,
        save_usage_price_sync,
        apply_usage_price_sync,
        record_usage_price_sync_error,
        get_pet_pulse,
        get_pet_details,
        run_action,
        set_pet_options,
        set_pet_visibility,
        set_pet_interaction,
        pointer::register_pet_pointer_renderer,
        pointer::begin_pet_pointer_frame,
        pointer::publish_pet_pointer_mask,
        pointer::renew_pet_pointer_mask,
        pointer::release_pet_pointer_renderer,
        pointer::begin_pet_temporary_interaction,
        pointer::cancel_pet_temporary_interaction,
        get_pet_extension_capabilities,
        get_pet_skin_state,
        choose_import_pet_skin,
        select_pet_skin,
        read_pet_skin_frame,
        remove_pet_skin,
        open_panel,
        panel_navigation,
        open_pet_chat,
        open_thread,
        start_pet_drag,
        export_csv,
        refresh_account,
        reset_pet_position,
        quit_app,
        choose_source_directory,
        frontend_ready,
        desktop_metrics
    ]);
    builder.setup(move|app|{
        let data_dir = match std::env::var_os("TOKEN_MEOW_DATA_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
            Some(path) => path,
            None => app.path().app_local_data_dir()?,
        };fs::create_dir_all(&data_dir)?;
        let source_root=std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(||std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(||PathBuf::from(".")).join(".codex"));
        let legacy_path=data_dir.join("pet-options.json");let legacy=fs::read(&legacy_path).ok().and_then(|bytes|serde_json::from_slice::<Value>(&bytes).ok());
        let mut core=Core::open(&data_dir,&source_root).map_err(std::io::Error::other)?;core.migrate_desktop_settings(legacy).map_err(std::io::Error::other)?;
        core.action("update_settings",&json!({"clickThrough":false})).map_err(std::io::Error::other)?;let settings=core.settings().map_err(std::io::Error::other)?;
        let primary=core.primary_source().map_err(std::io::Error::other)?;
        let account=AccountBridge::with_source_root(&PathBuf::from(primary["root"].as_str().unwrap_or_default()));let account_cancel=account.cancel_handle();
        let companion=companion::Companion::new(&data_dir);
        let notification_gate=core.notification_gate();
        let skins=crate::local_pet_skin::LocalPetSkinStore::new(&data_dir);
        let shared=Arc::new(Shared{pointer:Mutex::new(pointer::Runtime::default()),skins:Mutex::new(skins),panel_navigation:Mutex::new(panel_navigation_state::Mailbox::default()),companion:Mutex::new(companion),motion:Mutex::new(MotionRuntime::default()),manual_drag:Mutex::new(manual_drag::Controller::default()),automatic_positions:Mutex::new(motion_guard::PositionProvenance::default()),motion_invalidated:AtomicBool::new(false),motion_tick_pending:AtomicBool::new(false),motion_wake:(Mutex::new(false),Condvar::new()),core:Mutex::new(core),panel_reads:Mutex::new(()),panel_read_revision:AtomicU64::new(0),panel_read_interrupt:Mutex::new(None),notification_gate,csv_export_active:Arc::new(AtomicBool::new(false)),projection:Mutex::new(Value::Null),account:Mutex::new(account),account_cancel,account_cache:Mutex::new(AccountCache{binding:Value::Null,snapshot:json!({"status":"loading","buckets":[],"collectedAt":null}),generation:0}),account_reset_pending:AtomicBool::new(false),last_account_attempt:Mutex::new(None),stop:AtomicBool::new(false),exiting:AtomicBool::new(false),scanner_done:AtomicBool::new(false),account_done:AtomicBool::new(false),hotkey_thread:AtomicU32::new(0),shortcut_ready:AtomicBool::new(false),collector_error:Mutex::new(None),shell_error:Mutex::new(None),position_dirty:Mutex::new(None),topology:Mutex::new(vec![]),data_dir,started_at,settings_update:Mutex::new(()),settings_revision:AtomicU64::new(0),scan_wake:(Mutex::new(false),Condvar::new()),display_recheck:AtomicBool::new(false),native_paused:AtomicBool::new(false),native_suppress_toast:AtomicBool::new(false),native_dragging:AtomicBool::new(false),watcher_error:Mutex::new(None),pet_interaction:Mutex::new(PetInteraction::default()),metrics:Mutex::new(json!({"version":env!("CARGO_PKG_VERSION"),"processId":std::process::id(),"frontendReadyMs":{},"createdAt":chrono::Utc::now().to_rfc3339()}))});
        app.manage(shared.clone());create_tray(app.handle())?;if let Some(pet)=app.get_webview_window("pet"){pet.set_focusable(false)?;}
        apply_projection(app.handle(),&settings).map_err(std::io::Error::other)?;restore_position(app.handle(),&shared,false).map_err(std::io::Error::other)?;fit_main_window(app.handle()).map_err(std::io::Error::other)?;
        if !std::env::args().any(|a|a=="--background"){if let Some(main)=app.get_webview_window("main"){show_without_focus(&main).map_err(std::io::Error::other)?;}}
        if let Err(error)=native::install_runtime_events(app.handle(),shared.clone()){if let Ok(mut stored)=shared.shell_error.lock(){*stored=Some(error);}}
        native::start_hotkey(app.handle().clone(),shared.clone());start_file_watcher(shared.clone());start_workers(app.handle().clone(),shared);Ok(())
    }).on_window_event(|window,event|{
        let Some(state)=window.try_state::<SharedState>()else{return;};if window.label()=="main"&&matches!(event,WindowEvent::Destroyed){cancel_panel_navigation(&state);}let Some(view)=window.app_handle().get_webview_window(window.label())else{return;};match event{
            WindowEvent::CloseRequested{api,..}if !state.exiting.load(Ordering::Relaxed)=>{api.prevent_close();if window.label()=="main"{cancel_panel_navigation(&state);}if window.label()=="pet"{cancel_manual_drag(&state,manual_drag::StopReason::Hidden);let _=dismiss_pet_popup(&view,&state,"closed",false);}let _=window.hide();record_window_state(&view,&state);let _=window.emit("window-visibility-changed",json!({"label":window.label(),"visible":false}));if window.label()=="pet"{let app=window.app_handle().clone();let shared=state.inner().clone();tauri::async_runtime::spawn_blocking(move||{let _=update_settings(&app,&shared,&json!({"petVisible":false}));});}},
            WindowEvent::Resized(_)=>{let visible=window.is_visible().unwrap_or(false)&&!window.is_minimized().unwrap_or(false);let _=window.emit("window-visibility-changed",json!({"label":window.label(),"visible":visible}));record_visibility_change(&view,&state);},
            WindowEvent::Focused(focused)=>{if !focused&&window.label()=="pet"{let _=dismiss_pet_popup(&view,&state,"blur",false);}record_window_state(&view,&state);},
            WindowEvent::Destroyed if window.label()=="pet"=>{let _=dismiss_pet_popup(&view,&state,"destroyed",false);},
            WindowEvent::Moved(point) if window.label()=="pet"=>{record_pet_move(window,&state,*point);},
            WindowEvent::ScaleFactorChanged{..} if window.label()=="pet"=>{state.motion_invalidated.store(true,Ordering::Release);state.display_recheck.store(true,Ordering::Release);},_=>{}
        }
    }).build(context).expect("无法启动 Token 喵").run(|app,event|{if let tauri::RunEvent::ExitRequested{api,..}=event{if !app.state::<SharedState>().exiting.load(Ordering::Relaxed){api.prevent_exit();begin_shutdown(app);}}});
}

mod native {
    use super::*;
    #[cfg(windows)]
    pub fn map_drag_press(window: &WebviewWindow, press: manual_drag::ClientAnchor) -> Result<manual_drag::Point, String> {
        use windows_sys::Win32::{Foundation::{POINT, RECT}, Graphics::Gdi::ClientToScreen,
            UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::GetClientRect}};
        let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as _;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetClientRect(hwnd, &mut rect) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
        let mut origin = POINT { x: rect.left, y: rect.top };
        if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = window.scale_factor().map_err(|e| e.to_string())?;
        if dpi == 0 || !scale.is_finite() || (scale - f64::from(dpi) / 96.0).abs() > 0.01 {
            return Err("桌宠 DPI 正在变化，请重新拖动".into());
        }
        let width = u32::try_from(i64::from(rect.right) - i64::from(rect.left)).map_err(|_| "桌宠客户区宽度无效")?;
        let height = u32::try_from(i64::from(rect.bottom) - i64::from(rect.top)).map_err(|_| "桌宠客户区高度无效")?;
        manual_drag::physical_press_anchor(press, manual_drag::Rect { x: origin.x, y: origin.y, width, height }, scale)
            .ok_or_else(|| "拖动按下坐标或窗口尺寸已失效，请重新拖动".into())
    }
    #[cfg(windows)]
    pub fn sample_drag_input(visible: bool, runtime_available: bool) -> manual_drag::Input {
        use windows_sys::Win32::{Foundation::POINT,
            UI::{WindowsAndMessaging::GetPhysicalCursorPos, Input::KeyboardAndMouse::GetAsyncKeyState}};
        let mut cursor = POINT { x:0, y:0 };
        let available = unsafe { GetPhysicalCursorPos(&mut cursor) } != 0;
        manual_drag::Input {
            cursor:available.then_some(manual_drag::Point { x:cursor.x, y:cursor.y }),
            left_down:unsafe { GetAsyncKeyState(1) } < 0,
            escape_down:unsafe { GetAsyncKeyState(0x1b) } < 0,
            visible, runtime_available,
        }
    }
    #[cfg(windows)]
    pub fn motion_input_marker() -> Option<u32> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut input = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        (unsafe { GetLastInputInfo(&mut input) } != 0).then_some(input.dwTime)
    }
    #[cfg(not(windows))]
    pub fn motion_input_marker() -> Option<u32> { None }

    #[cfg(windows)]
    pub fn motion_environment_allowed() -> bool {
        use windows_sys::Win32::UI::{Shell::SHQueryUserNotificationState,
            WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION}};
        let mut animation: i32 = 0;
        let mut notifications = 0;
        unsafe {
            SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut animation as *mut _ as *mut _, 0) != 0 &&
                animation != 0 && SHQueryUserNotificationState(&mut notifications) == 0 && notifications == 5
        }
    }
    #[cfg(not(windows))]
    pub fn motion_environment_allowed() -> bool { false }

    #[cfg(windows)]
    pub fn motion_window_available(window: &WebviewWindow) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
        window.hwnd().ok().is_some_and(|handle| unsafe {
            IsWindowVisible(handle.0 as _) != 0 && IsIconic(handle.0 as _) == 0
        })
    }
    #[cfg(not(windows))]
    pub fn motion_window_available(_window: &WebviewWindow) -> bool { false }

    #[cfg(windows)]
    pub fn motion_snapshot(window: &WebviewWindow) -> Option<(roaming::Geometry, roaming::Point)> {
        use windows_sys::Win32::{Foundation::RECT,
            Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL},
            UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::GetWindowRect}};
        let hwnd = window.hwnd().ok()?.0 as _;
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) };
        if monitor.is_null() || unsafe { GetWindowRect(hwnd, &mut rect) } == 0 { return None; }
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 || dpi == 0 { return None; }
        let width = u32::try_from(i64::from(rect.right) - i64::from(rect.left)).ok()?;
        let height = u32::try_from(i64::from(rect.bottom) - i64::from(rect.top)).ok()?;
        let work = info.rcWork;
        Some((roaming::Geometry { work_area: roaming::Bounds { left: work.left, top: work.top,
            right: work.right, bottom: work.bottom }, window_width: width, window_height: height,
            scale_factor: f64::from(dpi) / 96.0, signature: monitor as usize as u64 },
            roaming::Point { x: rect.left, y: rect.top }))
    }
    #[cfg(not(windows))]
    pub fn motion_snapshot(_window: &WebviewWindow) -> Option<(roaming::Geometry, roaming::Point)> { None }

    #[cfg(windows)]
    pub fn move_pet_without_activation(window: &WebviewWindow, target: roaming::Point) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE,
            SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER};
        window.hwnd().ok().is_some_and(|handle| unsafe {
            SetWindowPos(handle.0 as _, std::ptr::null_mut(), target.x, target.y, 0, 0,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSIZE | SWP_NOZORDER) != 0
        })
    }
    #[cfg(not(windows))]
    pub fn move_pet_without_activation(_window: &WebviewWindow, _target: roaming::Point) -> bool { false }

    #[cfg(windows)]
    pub fn set_popup_bounds_without_activation(window: &WebviewWindow, target: popup_bounds::Bounds) -> Result<(), String> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE,
            SWP_NOOWNERZORDER, SWP_NOZORDER};
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        let width = i32::try_from(target.width).map_err(|_| "弹层宽度无效")?;
        let height = i32::try_from(target.height).map_err(|_| "弹层高度无效")?;
        if unsafe { SetWindowPos(hwnd.0 as _, std::ptr::null_mut(), target.x, target.y, width, height,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    pub fn set_popup_bounds_without_activation(window: &WebviewWindow, target: popup_bounds::Bounds) -> Result<(), String> {
        window.set_size(PhysicalSize::new(target.width, target.height)).map_err(|e| e.to_string())?;
        window.set_position(PhysicalPosition::new(target.x, target.y)).map_err(|e| e.to_string())
    }

    #[cfg(windows)]
    pub fn window_activation_state(window: &WebviewWindow) -> Value {
        window.hwnd().ok().map(|hwnd|unsafe{
            use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow,GetWindowLongPtrW,GWL_EXSTYLE,WS_EX_NOACTIVATE};
            json!({"noActivate":GetWindowLongPtrW(hwnd.0 as _,GWL_EXSTYLE)&WS_EX_NOACTIVATE as isize!=0,"foreground":GetForegroundWindow()==hwnd.0 as _})
        }).unwrap_or(Value::Null)
    }
    #[cfg(not(windows))]
    pub fn window_activation_state(_window: &WebviewWindow) -> Value {
        Value::Null
    }
    #[cfg(windows)]
    pub fn foreground_window() -> isize {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
    }
    #[cfg(not(windows))]
    pub fn foreground_window() -> isize {
        0
    }
    #[cfg(windows)]
    pub fn restore_popup_focus(window: &WebviewWindow, previous: isize, restore: bool) {
        if !restore || previous == 0 {
            return;
        }
        if let Ok(pet) = window.hwnd() {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetForegroundWindow, IsIconic, IsWindow, IsWindowVisible, SetForegroundWindow,
                };
                let previous = previous as windows_sys::Win32::Foundation::HWND;
                if previous != pet.0 as _
                    && IsWindow(previous) != 0
                    && IsWindowVisible(previous) != 0
                    && IsIconic(previous) == 0
                    && GetForegroundWindow() == pet.0 as _
                {
                    SetForegroundWindow(previous);
                }
            }
        }
    }
    #[cfg(not(windows))]
    pub fn restore_popup_focus(_window: &WebviewWindow, _previous: isize, _restore: bool) {}
    #[cfg(windows)]
    fn same_guid(a: &windows_sys::core::GUID, b: &windows_sys::core::GUID) -> bool {
        a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
    }
    fn xml_text(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }
    #[cfg(windows)]
    struct PreparedToast {
        notifier: windows::UI::Notifications::ToastNotifier,
        toast: windows::UI::Notifications::ToastNotification,
        // Fields drop in declaration order: release COM objects before the apartment.
        _apartment: NotificationApartment,
    }
    #[cfg(windows)]
    impl PreparedToast {
        fn submit(self) -> Result<(), String> {
            self.notifier.Show(&self.toast).map_err(notification_error)
        }
    }
    #[cfg(windows)]
    struct NotificationApartment(bool);
    #[cfg(windows)]
    impl Drop for NotificationApartment {
        fn drop(&mut self) {
            if self.0 { unsafe { windows::Win32::System::WinRT::RoUninitialize(); } }
        }
    }
    #[cfg(windows)]
    fn notification_error(error: windows::core::Error) -> String {
        format!("系统通知未能提交；请检查Windows通知设置或安装状态（{:#x}）。收件箱事实仍保留。", error.code().0)
    }
    #[cfg(windows)]
    fn prepare_toast(title: &str, body: &str, muted: bool, deadline: Option<chrono::DateTime<chrono::Utc>>) -> Result<PreparedToast, String> {
        use windows::{
            core::{HSTRING, Interface}, Data::Xml::Dom::XmlDocument,
            Foundation::{DateTime, IReference, PropertyValue},
            Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED},
            UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager},
        };
        let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        if let Err(error) = &initialized {
            if error.code().0 != 0x80010106u32 as i32 { return Err(notification_error(error.clone())); }
        }
        let apartment = NotificationApartment(initialized.is_ok());
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("local.tokenmeow.desktop")).map_err(notification_error)?;
        if notifier.Setting().map_err(notification_error)? != NotificationSetting::Enabled {
            return Err("系统通知未启用，提醒保留在本地收件箱".into());
        }
        let xml = XmlDocument::new().map_err(notification_error)?;
        xml.LoadXml(&HSTRING::from(format!("<toast duration=\"short\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>{}</toast>",xml_text(title),xml_text(body),if muted{"<audio silent=\"true\"/>"}else{""}))).map_err(notification_error)?;
        let toast = ToastNotification::CreateToastNotification(&xml).map_err(notification_error)?;
        toast.SetTag(&HSTRING::from("token-meow-summary")).map_err(notification_error)?;
        if let Some(deadline) = deadline {
            let ticks = notification_gate::windows_deadline_ticks(deadline).ok_or("系统通知期限超出范围")?;
            // Verified windows 0.61.3 binding: IReference<Foundation::DateTime>.
            let boxed: IReference<DateTime> = PropertyValue::CreateDateTime(DateTime { UniversalTime: ticks })
                .map_err(notification_error)?.cast().map_err(notification_error)?;
            toast.SetExpirationTime(&boxed).map_err(notification_error)?;
        }
        Ok(PreparedToast { notifier, toast, _apartment: apartment })
    }
    #[cfg(windows)]
    pub fn show_notification(title: &str, events: &[QueuedNotification], muted: bool, allowed: impl Fn() -> bool) -> notification_gate::Delivery {
        notification_gate::deliver_batch(events, |current| {
            let values: Vec<Value> = current.iter().map(|item| item.event.clone()).collect();
            let deadline = current.iter().filter_map(|item| item.permit.as_ref().map(|permit| permit.expires_utc())).min();
            prepare_toast(title, &notification_summary(&values), muted, deadline)
        }, PreparedToast::submit, allowed, crate::budget_clock::now)
    }
    #[cfg(not(windows))]
    pub fn show_notification(_title: &str, events: &[QueuedNotification], _muted: bool, allowed: impl Fn() -> bool) -> notification_gate::Delivery {
        notification_gate::deliver_batch(events,
            |_| Err::<(), _>("当前平台未接入系统通知，提醒保留在收件箱".into()),
            |_| Ok(()), allowed, crate::budget_clock::now)
    }
    #[cfg(all(windows, debug_assertions))]
    pub fn show_diagnostic_notification(allowed: impl Fn() -> bool) -> Result<(), String> {
        let prepared = prepare_toast("Token 喵 · 通知诊断", "这是一条手动诊断通知；不会生成回合或用量事实。", true, None)?;
        if !allowed() { return Err("系统通知当前已抑制".into()); }
        prepared.submit()
    }
    #[cfg(all(not(windows), debug_assertions))]
    pub fn show_diagnostic_notification(_allowed: impl Fn() -> bool) -> Result<(), String> {
        Err("当前平台未接入系统通知".into())
    }
    #[cfg(windows)]
    pub struct DirectoryWatches {
        handles: Vec<windows_sys::Win32::Foundation::HANDLE>,
        retry: bool,
    }
    #[cfg(windows)]
    impl DirectoryWatches {
        pub fn new() -> Self {
            Self {
                handles: vec![],
                retry: false,
            }
        }
        pub fn needs_retry(&self) -> bool {
            self.retry
        }
        pub fn replace(&mut self, paths: &[PathBuf]) -> bool {
            use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::*};
            self.close();
            self.retry = false;
            let mut failed = paths.len() > 60;
            for path in paths.iter().take(60) {
                let path: Vec<u16> = path
                    .as_os_str()
                    .to_string_lossy()
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let handle = unsafe {
                    FindFirstChangeNotificationW(
                        path.as_ptr(),
                        1,
                        FILE_NOTIFY_CHANGE_FILE_NAME
                            | FILE_NOTIFY_CHANGE_DIR_NAME
                            | FILE_NOTIFY_CHANGE_SIZE
                            | FILE_NOTIFY_CHANGE_LAST_WRITE,
                    )
                };
                if handle == INVALID_HANDLE_VALUE {
                    failed = true;
                    self.retry = true;
                } else {
                    self.handles.push(handle);
                }
            }
            failed
        }
        fn close(&mut self) {
            for handle in self.handles.drain(..) {
                unsafe {
                    windows_sys::Win32::Storage::FileSystem::FindCloseChangeNotification(handle);
                }
            }
        }
        pub fn wait_changed(&mut self) -> bool {
            use windows_sys::Win32::{
                Foundation::WAIT_OBJECT_0, Storage::FileSystem::FindNextChangeNotification,
                System::Threading::WaitForMultipleObjects,
            };
            if self.handles.is_empty() {
                std::thread::sleep(Duration::from_millis(500));
                return false;
            }
            let status = unsafe {
                WaitForMultipleObjects(self.handles.len() as u32, self.handles.as_ptr(), 0, 500)
            };
            let index = status.wrapping_sub(WAIT_OBJECT_0) as usize;
            if index < self.handles.len() {
                if unsafe { FindNextChangeNotification(self.handles[index]) } == 0 {
                    self.retry = true;
                    self.close();
                }
                true
            } else {
                if status == u32::MAX {
                    self.retry = true;
                    self.close();
                }
                false
            }
        }
    }
    #[cfg(windows)]
    impl Drop for DirectoryWatches {
        fn drop(&mut self) {
            self.close();
        }
    }
    #[cfg(not(windows))]
    pub struct DirectoryWatches;
    #[cfg(not(windows))]
    impl DirectoryWatches {
        pub fn new() -> Self {
            Self
        }
        pub fn replace(&mut self, _paths: &[PathBuf]) -> bool {
            true
        }
        pub fn needs_retry(&self) -> bool {
            false
        }
        pub fn wait_changed(&mut self) -> bool {
            std::thread::sleep(Duration::from_millis(500));
            false
        }
    }
    #[cfg(windows)]
    struct RuntimeEvents {
        app: AppHandle,
        shared: SharedState,
        power: windows_sys::Win32::System::Power::HPOWERNOTIFY,
        saver_notification: windows_sys::Win32::System::Power::HPOWERNOTIFY,
        registered: bool,
        session_known: bool,
        display_known: bool,
        power_saver: bool,
        locked: bool,
        suspended: bool,
        display_off: bool,
        disconnected: bool,
    }
    #[cfg(windows)]
    impl RuntimeEvents {
        fn publish(&self, reason: &str) {
            let ready = self.registered && self.session_known && self.display_known;
            let blocked = self.locked || self.suspended || self.display_off || self.disconnected;
            // Native boundaries invalidate before metrics/disk I/O, frontend
            // messages, or waiting for a scanner. Saver-only changes retain the
            // existing notification policy and do not revoke queued toasts.
            if !matches!(reason, "power-saver-on" | "power-saver-off") {
                self.shared.notification_gate.publish(ready, blocked);
            }
            let paused = blocked || self.power_saver;
            self.shared.native_paused.store(paused, Ordering::Release);
            if paused { cancel_manual_drag(&self.shared, manual_drag::StopReason::RuntimeUnavailable); }
            pointer::refresh_runtime_admission(&self.shared);
            self.shared
                .native_suppress_toast
                .store(blocked || !ready, Ordering::Release);
            if let Ok(mut metrics) = self.shared.metrics.lock() {
                let count = metrics["runtime"]["eventCount"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_add(1);
                metrics["runtime"] = json!({"eventCount":count,"lastEventAt":chrono::Utc::now().to_rfc3339(),"reason":reason,"paused":paused,"toastSuppressed":blocked || !ready,"notificationStateKnown":ready,"locked":self.locked,"suspended":self.suspended,"displayOff":self.display_off,"disconnected":self.disconnected,"powerSaver":self.power_saver});
                persist_metrics(&self.shared, &metrics);
            }
            let _ = self.app.emit(
                "desktop-runtime-state",
                json!({"paused":paused,"reason":reason}),
            );
            if !paused {
                self.shared.display_recheck.store(true, Ordering::Relaxed);
                wake_scan(&self.shared);
            }
        }
        fn refresh_session(&mut self) {
            if let Some((locked, disconnected)) = session_notification_state() {
                // A general/resume query cannot clear an observed blocker.
                // Only its matching, query-confirmed Unlock/Connect may clear it.
                self.locked |= locked;
                self.disconnected |= disconnected;
                self.session_known = true;
            } else {
                self.session_known = false;
            }
        }
        fn observe_session_boundary(&mut self, boundary: notification_gate::SessionBoundary) {
            // Revoke before the query can block; a stale query cannot weaken a
            // lock/disconnect that this HWND has already observed.
            self.shared.notification_gate.publish(false, true);
            self.shared.native_suppress_toast.store(true, Ordering::Release);
            let observed = notification_gate::session_after_boundary(
                notification_gate::SessionObservation { locked: self.locked, disconnected: self.disconnected, known: self.session_known },
                session_notification_state(), boundary);
            self.locked = observed.locked;
            self.disconnected = observed.disconnected;
            self.session_known = observed.known;
        }
    }
    #[cfg(windows)]
    unsafe extern "system" fn runtime_proc(
        hwnd: windows_sys::Win32::Foundation::HWND,
        message: u32,
        wparam: usize,
        lparam: isize,
        _id: usize,
        data: usize,
    ) -> isize {
        use windows_sys::Win32::{
            System::{
                Power::{UnregisterPowerSettingNotification, POWERBROADCAST_SETTING},
                RemoteDesktop::WTSUnRegisterSessionNotification,
            },
            UI::{
                Shell::{DefSubclassProc, RemoveWindowSubclass},
                WindowsAndMessaging::*,
            },
        };
        let context = &mut *(data as *mut RuntimeEvents);
        match message {
            WM_POWERBROADCAST => match wparam as u32 {
                PBT_APMSUSPEND => {
                    context.suspended = true;
                    context.display_known = false;
                    context.publish("suspend");
                }
                PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND | PBT_APMRESUMECRITICAL => {
                    context.shared.notification_gate.publish(false, true);
                    context.shared.native_suppress_toast.store(true, Ordering::Release);
                    context.suspended = false;
                    context.display_known = false;
                    context.refresh_session();
                    context.publish("resume");
                }
                PBT_POWERSETTINGCHANGE if lparam != 0 => {
                    let setting = &*(lparam as *const POWERBROADCAST_SETTING);
                    if setting.DataLength >= 4 {
                        let state = std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32);
                        use windows_sys::Win32::System::SystemServices::{
                            GUID_SESSION_DISPLAY_STATUS, GUID_POWER_SAVING_STATUS,
                        };
                        if same_guid(&setting.PowerSetting, &GUID_SESSION_DISPLAY_STATUS) {
                            context.display_known = state <= 2;
                            context.display_off = state == 0;
                            context.publish(if state == 0 {
                                "display-off"
                            } else {
                                "display-on"
                            });
                        } else if same_guid(&setting.PowerSetting, &GUID_POWER_SAVING_STATUS) {
                            context.power_saver = state != 0;
                            context.publish(if state != 0 {
                                "power-saver-on"
                            } else {
                                "power-saver-off"
                            });
                        }
                    }
                }
                _ => {}
            },
            WM_WTSSESSION_CHANGE => match wparam as u32 {
                WTS_SESSION_LOCK => {
                    context.observe_session_boundary(notification_gate::SessionBoundary::Lock);
                    context.publish("session-lock");
                }
                WTS_SESSION_UNLOCK => {
                    context.observe_session_boundary(notification_gate::SessionBoundary::Unlock);
                    context.publish("session-unlock");
                }
                WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => {
                    context.observe_session_boundary(notification_gate::SessionBoundary::Disconnect);
                    context.publish("session-disconnect");
                }
                WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                    context.observe_session_boundary(notification_gate::SessionBoundary::Connect);
                    context.publish("session-connect");
                }
                _ => {}
            },
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
                context
                    .shared
                    .display_recheck
                    .store(true, Ordering::Relaxed);
            }
            WM_ENTERSIZEMOVE => {
                context
                    .shared
                    .native_dragging
                    .store(true, Ordering::Relaxed);
                context.shared.motion_invalidated.store(true, Ordering::Release);
                if let Ok(mut positions) = context.shared.automatic_positions.lock() { positions.clear(); }
            }
            WM_EXITSIZEMOVE => {
                context
                    .shared
                    .native_dragging
                    .store(false, Ordering::Relaxed);
                if let Ok(mut dirty) = context.shared.position_dirty.lock() {
                    *dirty = Some(Instant::now());
                }
            }
            WM_NCDESTROY => {
                cancel_manual_drag(&context.shared, manual_drag::StopReason::Hidden);
                context.shared.notification_gate.publish(false, true);
                context.shared.native_suppress_toast.store(true, Ordering::Release);
                RemoveWindowSubclass(hwnd, Some(runtime_proc), 0x544d);
                WTSUnRegisterSessionNotification(hwnd);
                if context.power != 0 {
                    UnregisterPowerSettingNotification(context.power);
                }
                if context.saver_notification != 0 {
                    UnregisterPowerSettingNotification(context.saver_notification);
                }
                drop(Box::from_raw(data as *mut RuntimeEvents));
                return DefSubclassProc(hwnd, message, wparam, lparam);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
    #[cfg(windows)]
    fn session_notification_state() -> Option<(bool, bool)> {
        use windows_sys::Win32::System::RemoteDesktop::*;
        unsafe {
            let mut buffer: windows_sys::core::PWSTR = std::ptr::null_mut();
            let mut bytes = 0;
            let success = WTSQuerySessionInformationW(WTS_CURRENT_SERVER_HANDLE,
                WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buffer, &mut bytes) != 0;
            struct Buffer(windows_sys::core::PWSTR);
            impl Drop for Buffer {
                fn drop(&mut self) { if !self.0.is_null() { unsafe { WTSFreeMemory(self.0.cast()); } } }
            }
            let _buffer = Buffer(buffer);
            if !success || buffer.is_null() || (bytes as usize) < std::mem::size_of::<WTSINFOEXW>() { return None; }
            let info = std::ptr::read_unaligned(buffer.cast::<WTSINFOEXW>());
            if info.Level != 1 { return None; }
            let session = info.Data.WTSInfoExLevel1;
            // Windows 10/11: explicit SessionFlags, never infer unlocked from WTSActive.
            let locked = match session.SessionFlags as u32 {
                WTS_SESSIONSTATE_LOCK => true,
                WTS_SESSIONSTATE_UNLOCK => false,
                _ => return None,
            };
            Some((locked, session.SessionState != WTSActive))
        }
    }
    #[cfg(windows)]
    pub fn install_runtime_events(app: &AppHandle, shared: SharedState) -> Result<(), String> {
        use windows_sys::Win32::{
            System::{
                Power::{GetSystemPowerStatus, RegisterPowerSettingNotification, SYSTEM_POWER_STATUS},
                RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION},
                SystemServices::{GUID_SESSION_DISPLAY_STATUS, GUID_POWER_SAVING_STATUS},
            },
            UI::{Shell::SetWindowSubclass, WindowsAndMessaging::DEVICE_NOTIFY_WINDOW_HANDLE},
        };
        shared.notification_gate.publish(false, true);
        let window = app.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
        let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as _;
        unsafe {
            let mut power_status: SYSTEM_POWER_STATUS = std::mem::zeroed();
            let power_saver = GetSystemPowerStatus(&mut power_status) != 0 && power_status.SystemStatusFlag == 1;
            let raw = Box::into_raw(Box::new(RuntimeEvents {
                app: app.clone(), shared, power: 0, saver_notification: 0,
                registered: false, session_known: false, display_known: false,
                // These placeholders are not facts: session_known remains false
                // and the atomic gate stays unavailable until a full query.
                power_saver, locked: false, suspended: false, display_off: true, disconnected: false,
            }));
            // Subscribe first so an initial power-state notification cannot be
            // lost during registration. Keep admission unknown until all facts exist.
            if SetWindowSubclass(hwnd, Some(runtime_proc), 0x544d, raw as usize) == 0 {
                (&(*raw).shared).notification_gate.publish(false, true);
                drop(Box::from_raw(raw));
                return Err("Windows运行时事件订阅失败".into());
            }
            // Do not hold a Rust &mut across registration APIs, which may deliver
            // messages reentrantly to the installed subclass.
            let power = RegisterPowerSettingNotification(hwnd, &GUID_SESSION_DISPLAY_STATUS, DEVICE_NOTIFY_WINDOW_HANDLE);
            (*raw).power = power;
            let session_ok = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) != 0;
            let saver = RegisterPowerSettingNotification(hwnd, &GUID_POWER_SAVING_STATUS, DEVICE_NOTIFY_WINDOW_HANDLE);
            (*raw).saver_notification = saver;
            (*raw).registered = power != 0 && session_ok;
            (*raw).refresh_session();
            (*raw).publish("initial-native-state");
            if power == 0 || !session_ok {
                return Err("部分Windows电源或会话事件不可用；系统通知保持关闭".into());
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    pub fn install_runtime_events(_app: &AppHandle, _shared: SharedState) -> Result<(), String> {
        Ok(())
    }
    #[cfg(windows)]
    pub fn work_areas(_window: &WebviewWindow) -> Result<Vec<WorkArea>, String> {
        use windows_sys::{
            core::BOOL,
            Win32::{
                Foundation::{LPARAM, RECT},
                Graphics::Gdi::{
                    EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEW,
                    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
                },
                UI::{
                    HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
                    WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME,
                },
            },
        };
        unsafe extern "system" fn callback(
            monitor: HMONITOR,
            _dc: HDC,
            _rect: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let list = &mut *(data as *mut Vec<WorkArea>);
            let mut info: MONITORINFOEXW = std::mem::zeroed();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) == 0 {
                return 1;
            }
            let mut dx = 96;
            let mut dy = 96;
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let end = info
                .szDevice
                .iter()
                .position(|n| *n == 0)
                .unwrap_or(info.szDevice.len());
            let mut device: DISPLAY_DEVICEW = std::mem::zeroed();
            device.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            let device_found = EnumDisplayDevicesW(
                info.szDevice.as_ptr(),
                0,
                &mut device,
                EDD_GET_DEVICE_INTERFACE_NAME,
            ) != 0;
            let device_end = device
                .DeviceID
                .iter()
                .position(|n| *n == 0)
                .unwrap_or(device.DeviceID.len());
            let id = if device_found && device_end > 0 {
                String::from_utf16_lossy(&device.DeviceID[..device_end])
            } else {
                String::from_utf16_lossy(&info.szDevice[..end])
            };
            let r = info.monitorInfo.rcWork;
            list.push(WorkArea {
                id,
                x: r.left,
                y: r.top,
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
                scale_factor: dx as f64 / 96.0,
                primary: info.monitorInfo.dwFlags & 1 != 0,
            });
            1
        }
        let mut monitors: Vec<WorkArea> = Vec::new();
        let ok = unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(callback),
                &mut monitors as *mut _ as isize,
            )
        };
        if ok == 0 {
            return Err("无法读取Windows显示器工作区".into());
        }
        monitors.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(monitors)
    }
    #[cfg(not(windows))]
    pub fn work_areas(window: &WebviewWindow) -> Result<Vec<WorkArea>, String> {
        window
            .available_monitors()
            .map_err(|e| e.to_string())
            .map(|list| {
                list.into_iter()
                    .enumerate()
                    .map(|(i, m)| WorkArea {
                        id: m.name().cloned().unwrap_or_else(|| format!("monitor-{i}")),
                        x: m.position().x,
                        y: m.position().y,
                        width: m.size().width,
                        height: m.size().height,
                        scale_factor: m.scale_factor(),
                        primary: i == 0,
                    })
                    .collect()
            })
    }
    #[cfg(windows)]
    pub fn mouse_down() -> bool {
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(1) < 0 }
    }
    #[cfg(not(windows))]
    pub fn mouse_down() -> bool {
        false
    }
    #[cfg(windows)]
    pub fn stop_hotkey(id: u32) {
        if id != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                    id,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT,
                    0,
                    0,
                );
            }
        }
    }
    #[cfg(not(windows))]
    pub fn stop_hotkey(_id: u32) {}
    pub fn start_hotkey(app: AppHandle, shared: SharedState) {
        #[cfg(windows)]
        std::thread::spawn(move || unsafe {
            use windows_sys::Win32::{
                System::Threading::GetCurrentThreadId,
                UI::{
                    Input::KeyboardAndMouse::{
                        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
                    },
                    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
                },
            };
            let mut message: MSG = std::mem::zeroed();
            shared
                .hotkey_thread
                .store(GetCurrentThreadId(), Ordering::Relaxed);
            let registered = RegisterHotKey(
                std::ptr::null_mut(),
                1,
                MOD_ALT | MOD_CONTROL | MOD_NOREPEAT,
                b'M' as u32,
            ) != 0;
            shared.shortcut_ready.store(registered, Ordering::Relaxed);
            let temporary_registered = p1_development_enabled() && RegisterHotKey(std::ptr::null_mut(), 2,
                MOD_ALT | MOD_CONTROL | MOD_NOREPEAT, b'I' as u32) != 0;
            if !registered {
                if let Ok(mut error) = shared.shell_error.lock() {
                    *error =
                        Some("Ctrl+Alt+M注册失败（可能被其他程序占用）；请从托盘恢复交互。".into());
                }
            }
            record_metric(&shared, "temporaryShortcutReady", json!(temporary_registered));
            if !registered && !temporary_registered { return; }
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                if message.message == WM_HOTKEY {
                    let temporary = message.wParam == 2;
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        if temporary { let _ = pointer::temporary(&app); }
                        else { let _ = recover_pet(&app, false); }
                    });
                }
            }
            UnregisterHotKey(std::ptr::null_mut(), 1);
            UnregisterHotKey(std::ptr::null_mut(), 2);
        });
        #[cfg(not(windows))]
        {
            let _ = (app, shared);
        }
    }
    #[cfg(windows)]
    fn wide(v: &str) -> Vec<u16> {
        v.encode_utf16().chain(Some(0)).collect()
    }
    #[cfg(windows)]
    pub fn set_autostart(enabled: bool) -> Result<(), String> {
        use windows_sys::Win32::System::Registry::*;
        let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let name = wide("TokenMeow");
        let mut handle = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                std::ptr::null(),
                &mut handle,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(format!("无法配置当前用户开机启动（Windows {status}）"));
        }
        let result = if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = wide(&format!("\"{}\" --background", exe.display()));
            unsafe {
                RegSetValueExW(
                    handle,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr() as *const u8,
                    (value.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(handle, name.as_ptr()) }
        };
        unsafe {
            RegCloseKey(handle);
        }
        if result == 0 || (!enabled && result == 2) {
            Ok(())
        } else {
            Err(format!("开机启动设置失败（Windows {result}）"))
        }
    }
    #[cfg(not(windows))]
    pub fn set_autostart(enabled: bool) -> Result<(), String> {
        if enabled {
            Err("当前平台未开放开机启动".into())
        } else {
            Ok(())
        }
    }
    pub fn autostart_status() -> Value {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Registry::*;
            let mut handle = std::ptr::null_mut();
            let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
            let name = wide("TokenMeow");
            let opened = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    0,
                    KEY_QUERY_VALUE,
                    &mut handle,
                )
            };
            if opened != 0 {
                return json!({"supported":true,"enabled":false,"scope":"currentUser","error":if opened==2{None}else{Some("无法读取开机启动状态")}});
            }
            let mut data = vec![0u16; 16384];
            let mut bytes = (data.len() * 2) as u32;
            let status = unsafe {
                RegQueryValueExW(
                    handle,
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    data.as_mut_ptr() as *mut u8,
                    &mut bytes,
                )
            };
            unsafe {
                RegCloseKey(handle);
            }
            let end = data.iter().position(|c| *c == 0).unwrap_or(data.len());
            let expected = std::env::current_exe()
                .ok()
                .map(|p| format!("\"{}\" --background", p.display()));
            let actual = String::from_utf16_lossy(&data[..end]);
            json!({"supported":true,"enabled":status==0&&expected.as_deref()==Some(actual.as_str()),"scope":"currentUser","differentRegistration":status==0&&expected.as_deref()!=Some(actual.as_str()),"error":if status==0||status==2{None}else{Some("无法读取开机启动状态")}})
        }
        #[cfg(not(windows))]
        {
            json!({"supported":false,"enabled":false})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_navigation_requests_reject_unknown_fields_and_lossy_revisions() {
        assert_eq!(panel_navigation_action(&json!({"action":"peek"})).unwrap(), PanelNavigationAction::Peek);
        assert_eq!(panel_navigation_action(&json!({"action":"cancel"})).unwrap(), PanelNavigationAction::Cancel);
        assert_eq!(panel_navigation_action(&json!({"action":"ack","revision":"9007199254740993"})).unwrap(),
            PanelNavigationAction::Ack(9_007_199_254_740_993));
        for request in [Value::Null, json!([]), json!({"action":"show"}),
            json!({"action":"peek","destination":"settings"}),
            json!({"action":"cancel","revision":"1"}), json!({"action":"ack"}),
            json!({"action":"ack","other":"1"}), json!({"action":"ack","revision":1}),
            json!({"action":"ack","revision":"01"}),
            json!({"action":"ack","revision":"18446744073709551616"})] {
            assert!(panel_navigation_action(&request).is_err(), "{request}");
        }
    }

    #[test]
    fn pet_inbox_destination_and_domain_are_finite_without_arbitrary_urls() {
        let domain = json!({"day":"2026-10-17","runtimeEpoch":"epoch","sourceScopeRevision":"scope"});
        let request: PetInboxRequest = serde_json::from_value(json!({"destination":"inbox","petDomain":domain})).unwrap();
        assert_eq!(panel_domain_value(&panel_domain(&request.pet_domain).unwrap()), domain);
        for value in [json!({"destination":"settings","petDomain":domain}),
            json!({"destination":"https://example.invalid","petDomain":domain}),
            json!({"destination":"inbox","petDomain":domain,"url":"codex://threads/forged"}),
            json!({"destination":"inbox"})] {
            assert!(serde_json::from_value::<PetInboxRequest>(value).is_err());
        }
        assert!(panel_domain(&json!({"day":"2026-10-17","runtimeEpoch":7,"sourceScopeRevision":"scope"})).is_err());
        // Full field/date/epoch validation belongs to Core, never this serializer.
    }

    #[test]
    fn paused_panel_snapshot_preserves_slot_without_renewing_the_deadline() {
        let start = Instant::now();
        let mut mailbox = panel_navigation_state::Mailbox::default();
        let ticket = mailbox.reserve(start).unwrap();
        let domain = panel_navigation_state::Domain { day:"2026-10-17".into(),
            runtime_epoch:"epoch".into(), source_scope_revision:"scope".into() };
        assert!(mailbox.commit(&ticket, domain, start));
        let paused = panel_snapshot_value(&mut mailbox, start + Duration::from_secs(4), false);
        assert_eq!(paused["pending"], Value::Null);
        let resumed = panel_snapshot_value(&mut mailbox, start + Duration::from_secs(6), true);
        assert_eq!(resumed["watermark"], ticket.revision.to_string());
        assert_eq!(resumed["pending"]["revision"], ticket.revision.to_string());
        assert_eq!(resumed["pending"]["remainingMs"], 4000);
        assert_eq!(resumed["pending"]["destination"], "inbox");
        assert_eq!(panel_snapshot_value(&mut mailbox, start + panel_navigation_state::LIFETIME, true)["pending"], Value::Null);
    }

    #[test]
    fn directory_failure_preserves_success_only_for_same_source_generation() {
        let old = json!({"sourceId":"source-a","bindingGeneration":7,"lastSuccessAt":"old-success","count":42});
        let source = json!({"id":"source-a"});
        let same = directory_failure(&old, &source, 7, "deferred");
        assert_eq!(same["lastSuccessAt"], "old-success");
        assert_eq!(same["count"], 42);
        for (source, generation) in [(json!({"id":"source-b"}), 7), (source, 8)] {
            let failed = directory_failure(&old, &source, generation, "deferred");
            assert!(failed["lastSuccessAt"].is_null());
            assert!(failed["count"].is_null());
            assert_eq!(failed["status"], "unavailable");
            assert_eq!(failed["error"], "deferred");
        }
    }

    #[test]
    fn resource_stop_sync_preserves_current_facts_and_directory_success_once() {
        let source = json!({"id":"source-a","root":"C:/fixture/a","clientKind":"codex","environment":"local","enabled":true});
        let facts = json!({"sourceId":"source-a","status":"connected","error":null,"collectedAt":"old-success","lastAttemptAt":"old-attempt","authMode":"old-mode","accountScope":"old-scope","buckets":[{"id":"quota","primary":{"usedPercent":25}}],"usage":{"summary":{"totalTokens":42}}});
        let mut cache = AccountCache {
            binding: source.clone(),
            snapshot: facts.clone(),
            generation: 7,
        };
        let mut metrics = json!({"directory":{"status":"connected","sourceId":"source-a","bindingGeneration":7,"lastSuccessAt":"directory-success","count":42,"error":null},"collector":{"lastSuccessAt":"collector-success"}});
        let (updated, directory_changed) =
            apply_resource_stop(&mut cache, &source, 7, &mut metrics, "resource-stop");
        let mut expected = facts;
        expected["status"] = json!("stale");
        expected["error"] = json!("resource-stop");
        assert_eq!(updated, Some(expected.clone()));
        assert_eq!(cache.snapshot, expected);
        assert!(directory_changed);
        assert_eq!(metrics["directory"]["status"], "unavailable");
        assert_eq!(metrics["directory"]["lastSuccessAt"], "directory-success");
        assert_eq!(metrics["directory"]["count"], 42);
        assert_eq!(metrics["collector"]["lastSuccessAt"], "collector-success");
        let prior_metrics = metrics.clone();
        assert_eq!(
            apply_resource_stop(&mut cache, &source, 7, &mut metrics, "resource-stop"),
            (None, false)
        );
        assert_eq!(metrics, prior_metrics);
        assert_eq!(cache.snapshot, expected);
    }

    #[test]
    fn resource_stop_sync_after_clear_or_rebind_never_restores_old_facts() {
        let source = json!({"id":"source-a","root":"C:/fixture/a","clientKind":"codex","environment":"local","enabled":true});
        let mut cache = AccountCache {
            binding: source.clone(),
            snapshot: json!({"status":"connected","collectedAt":"old-success","buckets":[{"id":"old-quota"}],"lastAttemptAt":"old-attempt"}),
            generation: 7,
        };
        let mut metrics = json!({"directory":{"status":"connected","sourceId":"source-a","bindingGeneration":7,"lastSuccessAt":"old-directory-success","count":42}});
        cache.clear(&source);
        let cleared = cache.snapshot.clone();
        let old_metrics = metrics.clone();
        assert_eq!(
            apply_resource_stop(&mut cache, &source, 7, &mut metrics, "resource-stop"),
            (None, false)
        );
        assert_eq!(cache.snapshot, cleared);
        assert_eq!(metrics, old_metrics);
        let generation = cache.generation;
        assert!(apply_resource_stop(
            &mut cache,
            &source,
            generation,
            &mut metrics,
            "resource-stop"
        )
        .0
        .is_some());
        assert_eq!(cache.snapshot["status"], "unavailable");
        assert_eq!(cache.snapshot["buckets"], json!([]));
        assert!(cache.snapshot["collectedAt"].is_null());
        assert!(cache.snapshot["lastAttemptAt"].is_null());
        assert!(metrics["directory"]["lastSuccessAt"].is_null());
        assert!(metrics["directory"]["count"].is_null());
        let mut relocated = source.clone();
        relocated["root"] = json!("C:/fixture/relocated");
        assert!(cache.rebind(&relocated));
        assert_eq!(cache.snapshot["status"], "loading");
        assert_eq!(
            apply_resource_stop(&mut cache, &source, generation, &mut metrics, "old-stop"),
            (None, false)
        );
        let generation = cache.generation;
        assert!(apply_resource_stop(
            &mut cache,
            &relocated,
            generation,
            &mut metrics,
            "resource-stop"
        )
        .0
        .is_some());
        assert_eq!(cache.snapshot["status"], "unavailable");
        assert_eq!(cache.snapshot["buckets"], json!([]));
        assert!(cache.snapshot["collectedAt"].is_null());
        assert!(metrics["directory"]["lastSuccessAt"].is_null());
    }

    #[test]
    fn resource_stop_sync_rejects_wrong_binding_generation_disabled_and_unsupported() {
        let source = json!({"id":"source-a","root":"C:/fixture/a","clientKind":"codex","environment":"local","enabled":true});
        let snapshot =
            json!({"status":"connected","collectedAt":"success","buckets":[{"id":"keep"}]});
        let directory = json!({"directory":{"status":"connected","sourceId":"source-a","bindingGeneration":7,"lastSuccessAt":"success","count":1}});
        for field in ["id", "root", "clientKind", "environment", "enabled"] {
            let mut wrong = source.clone();
            wrong[field] = if field == "enabled" {
                json!(false)
            } else {
                json!("different")
            };
            let mut cache = AccountCache {
                binding: source.clone(),
                snapshot: snapshot.clone(),
                generation: 7,
            };
            let mut metrics = directory.clone();
            assert_eq!(
                apply_resource_stop(&mut cache, &wrong, 7, &mut metrics, "resource-stop"),
                (None, false)
            );
            assert_eq!(cache.snapshot, snapshot);
            assert_eq!(metrics, directory);
        }
        let mut cache = AccountCache {
            binding: source.clone(),
            snapshot: snapshot.clone(),
            generation: 8,
        };
        let mut metrics = directory.clone();
        assert_eq!(
            apply_resource_stop(&mut cache, &source, 7, &mut metrics, "resource-stop"),
            (None, false)
        );
        assert_eq!(cache.snapshot, snapshot);
        assert_eq!(metrics, directory);
        for (field, value, error) in [
            ("enabled", json!(false), "disabled"),
            ("clientKind", json!("future-adapter"), "unsupported"),
        ] {
            let mut unavailable = source.clone();
            unavailable[field] = value;
            let snapshot =
                json!({"status":"unavailable","error":error,"collectedAt":null,"buckets":[]});
            let mut cache = AccountCache {
                binding: unavailable.clone(),
                snapshot: snapshot.clone(),
                generation: 7,
            };
            let mut metrics = json!({"directory":{"status":"unavailable","error":error}});
            let before = metrics.clone();
            assert_eq!(
                apply_resource_stop(&mut cache, &unavailable, 7, &mut metrics, "resource-stop"),
                (None, false)
            );
            assert_eq!(cache.snapshot, snapshot);
            assert_eq!(metrics, before);
        }
    }

    #[test]
    fn csv_export_gate_stays_busy_after_waiter_drop_until_blocking_work_finishes() {
        let active = Arc::new(AtomicBool::new(false));
        let permit = CsvExportPermit::try_acquire(&active).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let waiter = tauri::async_runtime::spawn_blocking(move || {
            {
                let _permit = permit;
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            finished_tx.send(()).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(waiter);
        assert_eq!(
            CsvExportPermit::try_acquire(&active).err().as_deref(),
            Some(CSV_EXPORT_BUSY),
            "dropping the async waiter must not release an active save operation"
        );
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let next = CsvExportPermit::try_acquire(&active).unwrap();
        assert!(CsvExportPermit::try_acquire(&active).is_err());
        drop(next);
        assert!(CsvExportPermit::try_acquire(&active).is_ok());
    }
    #[test]
    fn csv_export_gate_releases_after_cancel_error_and_unwind() {
        let active = Arc::new(AtomicBool::new(false));
        let finish = |outcome: Result<Option<String>, String>| {
            let _permit = CsvExportPermit::try_acquire(&active)?;
            outcome
        };
        assert_eq!(finish(Ok(None)).unwrap(), None);
        assert_eq!(
            finish(Err("synthetic write failure".into())).unwrap_err(),
            "synthetic write failure"
        );
        assert!(std::panic::catch_unwind(|| {
            let _permit = CsvExportPermit::try_acquire(&active).unwrap();
            panic!("synthetic blocking-task unwind");
        })
        .is_err());
        assert!(CsvExportPermit::try_acquire(&active).is_ok());
    }
    #[test]
    fn history_followup_requires_progress_without_contention_or_errors() {
        assert_eq!(
            scan_followup_delay(100, 3, 0, false),
            Duration::from_millis(25)
        );
        // Idle, an unfinished half-line, source read errors, and lock contention
        // must all retain the original backoff rather than busy-looping.
        for (lines, pending, issues, contended) in [
            (0, 0, 0, false),
            (0, 3, 0, false),
            (100, 0, 0, false),
            (100, 3, 1, false),
            (100, 3, 0, true),
        ] {
            assert_eq!(
                scan_followup_delay(lines, pending, issues, contended),
                Duration::from_secs(2)
            );
        }
    }
    #[test]
    fn account_cache_never_relabels_old_root_or_late_reply_as_current_source() {
        let old = json!({"id":"same-source","root":"C:/fixture/a","clientKind":"codex","environment":"local","enabled":true});
        let new = json!({"id":"same-source","root":"C:/fixture/b","clientKind":"codex","environment":"local","enabled":true});
        let mut cache = AccountCache {
            binding: Value::Null,
            snapshot: Value::Null,
            generation: 0,
        };
        assert!(cache.rebind(&old));
        let old_generation = cache.generation;
        cache
            .publish(
                &old,
                old_generation,
                json!({"status":"connected","buckets":[{"id":"old-account"}]}),
            )
            .unwrap();
        assert_eq!(cache.for_source(&old).unwrap()["status"], "connected");
        assert!(cache.for_source(&new).is_none());
        assert!(cache.rebind(&new));
        let new_generation = cache.generation;
        assert_ne!(new_generation, old_generation);
        let loading = cache.for_source(&new).unwrap();
        assert_eq!(loading["status"], "loading");
        assert_eq!(loading["buckets"], json!([]));
        assert!(cache
            .publish(
                &old,
                old_generation,
                json!({"buckets":[{"id":"late-old-account"}]})
            )
            .is_err());
        assert_eq!(cache.for_source(&new).unwrap()["status"], "loading");
        let mut disabled = new.clone();
        disabled["enabled"] = json!(false);
        assert!(cache.for_source(&disabled).is_none());
        cache
            .publish(
                &new,
                new_generation,
                json!({"status":"connected","buckets":[{"id":"before-clear"}]}),
            )
            .unwrap();
        cache.clear(&new);
        assert_eq!(cache.for_source(&new).unwrap()["buckets"], json!([]));
        assert!(cache
            .publish(
                &new,
                new_generation,
                json!({"status":"connected","buckets":[{"id":"old-inflight-after-clear"}]})
            )
            .is_err());
        let cleared_generation = cache.generation;
        cache
            .publish(
                &new,
                cleared_generation,
                json!({"status":"connected","buckets":[{"id":"fresh-read-after-clear"}]}),
            )
            .unwrap();
        assert_eq!(
            cache.for_source(&new).unwrap()["buckets"][0]["id"],
            "fresh-read-after-clear"
        );
        let prior_visit = cache.generation;
        assert!(cache.rebind(&old));
        assert!(cache.rebind(&new));
        assert!(cache
            .publish(&new, prior_visit, json!({"status":"connected"}))
            .is_err());
    }
    #[test]
    fn pet_quota_projection_preserves_all_windows_without_daily_usage_or_old_binding() {
        let source = json!({"id":"quota-source","root":"C:/quota/a","enabled":true});
        let windows = json!([
            {"id":"first","name":"first","primary":{"usedPercent":30,"resetsAt":100}},
            {"id":"second","name":"second","secondary":{"usedPercent":null,"resetsAt":200}}
        ]);
        let mut cache = AccountCache {
            binding: source.clone(),
            snapshot: json!({"sourceId":"quota-source","status":"stale","collectedAt":"2026-10-15T00:00:00Z","buckets":windows,"usage":{"dailyUsageBuckets":[{"tokens":999}]},"authMode":"not-needed-by-pet"}),
            generation: 0,
        };
        let projected = cache.quota_for_source(&source).unwrap();
        assert_eq!(projected["buckets"], windows);
        assert_eq!(projected["status"], "stale");
        assert_eq!(projected["collectedAt"], "2026-10-15T00:00:00Z");
        assert_eq!(projected["sourceId"], "quota-source");
        assert!(projected.get("usage").is_none());
        assert!(projected.get("authMode").is_none());
        let mut relocated = source.clone();
        relocated["root"] = json!("C:/quota/b");
        assert!(cache.quota_for_source(&relocated).is_none());
        let mut disabled = source.clone();
        disabled["enabled"] = json!(false);
        assert!(cache.quota_for_source(&disabled).is_none());
        cache.clear(&source);
        assert_eq!(
            cache.quota_for_source(&source).unwrap()["buckets"],
            json!([])
        );
        assert!(cache
            .publish(&source, 0, json!({"buckets":windows}))
            .is_err());
    }
    fn area(
        id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        scale: f64,
        primary: bool,
    ) -> WorkArea {
        WorkArea {
            id: id.into(),
            x,
            y,
            width,
            height,
            scale_factor: scale,
            primary,
        }
    }
    #[test]
    fn negative_monitor_and_dpi_restore_from_logical_work_area() {
        let screens = vec![
            area("left", -2560, -100, 2560, 1400, 1.5, false),
            area("primary", 0, 0, 1920, 1040, 1.0, true),
        ];
        let saved = SavedPosition {
            monitor_id: "left".into(),
            logical_x: 100.0,
            logical_y: 50.0,
            scale_factor: 1.0,
        };
        assert_eq!(
            restored_position(&screens, Some(&saved), 280.0, 340.0, false),
            Some((-2410, -25))
        );
    }
    #[test]
    fn removed_monitor_returns_to_primary_and_keeps_entire_window_visible() {
        let screens = vec![area("primary", 0, 0, 1280, 680, 2.0, true)];
        let saved = SavedPosition {
            monitor_id: "removed".into(),
            logical_x: 9999.0,
            logical_y: -500.0,
            scale_factor: 1.0,
        };
        assert_eq!(
            restored_position(&screens, Some(&saved), 280.0, 340.0, false),
            Some((704, 0))
        );
    }
    #[test]
    fn panel_is_limited_to_small_high_dpi_work_area() {
        let m = area("rdp", -1920, 0, 1920, 1040, 2.0, true);
        assert_eq!(
            panel_bounds(&m, -2000, -20, 2480, 1640),
            (-1920, 0, 1920, 1040)
        );
        assert!((fit_scale(&m, 2.0) * 340.0 * m.scale_factor) <= m.height as f64);
    }
    #[test]
    fn notification_text_preserves_unknown_and_failure_and_hides_source_text() {
        let events = vec![
            json!({"status":"failed","title":"private project"}),
            json!({"status":"unknown","cwd":"secret path"}),
            json!({"kind":"budget"}),
        ];
        let summary = notification_summary(&events);
        assert!(summary.contains("1个回合失败"));
        assert!(summary.contains("结果未知"));
        assert!(summary.contains("软预算"));
        assert!(!summary.contains("private"));
        assert!(!summary.contains("secret"));
        assert!(!summary.contains("正常结束"));
    }
    #[test]
    fn drag_end_chooses_monitor_by_overlap_and_clamps_work_area() {
        let screens = vec![
            area("left", -1920, 0, 1920, 1040, 1.0, false),
            area("right", 0, 0, 1920, 1040, 1.0, true),
        ];
        let m = monitor_for_rect(&screens, -100, 900, 280, 340).unwrap();
        assert_eq!(m.id, "right");
        assert_eq!(clamp_position(m, -100, 900, 280, 340), (0, 700));
    }
}
