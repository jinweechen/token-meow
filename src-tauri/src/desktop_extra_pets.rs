//! 辅助窗口只读共享活动摘要；独立登记、拖动与形象，不消费业务提醒。
use super::*;
use crate::{pet_registry::{self as registry, PetConfig, PetConfigPatch, PetDescription, PetIdentity, RendererLease}, pet_skin_selection::ActiveSkin};
use std::collections::HashMap;
#[path = "pet_skin_pins.rs"]
mod skin_pins;

#[derive(Default)]
pub(super) struct Runtime { pub registry: registry::Registry, drags: HashMap<String, ExtraDrag>, pins: skin_pins::SkinPins }
struct ExtraDrag { identity: PetIdentity, lease: RendererLease, controller: manual_drag::Controller }
pub(super) fn dragging(shared: &Shared) -> bool {
    shared.extra_pets.try_lock().map(|owner| !owner.drags.is_empty()).unwrap_or(true)
}

fn main_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" { require_p1_development() } else { Err("请在主面板管理辅助桌宠。".into()) }
}
fn changed(app: &AppHandle) { let _ = app.emit("extra-pets-changed", json!({"kind":"refresh"})); }
fn description(shared: &Shared, label: &str) -> Result<PetDescription, String> {
    shared.extra_pets.lock().map_err(|_| lock_error())?.registry.get_by_label(label).ok_or("辅助桌宠实例已关闭。".into())
}
fn extra_command_allowed(command: &str) -> bool {
    matches!(command, "extra_pet_context" | "extra_pet_pulse" | "register_extra_pet_renderer" |
        "unregister_extra_pet_renderer" | "start_extra_pet_drag" | "close_extra_pet" | "read_pet_skin_frame" | "read_vrm_skin_model")
}
pub(super) fn allows_asset_read(shared: &Shared, label: &str, id: &str, vrm: bool) -> bool {
    description(shared, label).is_ok_and(|pet| match pet.skin_choice { ActiveSkin::Vrm(value) if vrm => value == id, ActiveSkin::Png(value) if !vrm => value == id, _ => false })
}
pub(super) fn require_unused_skin(shared: &Shared, id: &str, vrm: bool) -> Result<(), String> {
    let owner = shared.extra_pets.lock().map_err(|_| lock_error())?;
    if owner.pins.uses(id, vrm)? || owner.registry.list().iter().any(|pet| match &pet.skin_choice {
        ActiveSkin::Vrm(value) if vrm => value == id, ActiveSkin::Png(value) if !vrm => value == id, _ => false,
    }) { Err("辅助桌宠正在使用此角色，请先为它切换角色或关闭辅助窗口。".into()) } else { Ok(()) }
}
/// Tauri 自定义命令默认共用路由。额外视图只取得精确白名单，不能调用账本写入/主窗控制。
pub(super) fn restrict_handler<F>(handler: F) -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static
where F: Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    move |invoke| {
        let view = invoke.message.webview();
        if !matches!(view.label(), "main" | "pet") {
            let command = invoke.message.command();
            let allowed = extra_command_allowed(command);
            let registered = view.try_state::<SharedState>().is_some_and(|shared| shared.extra_pets.try_lock().is_ok_and(|owner| owner.registry.get_by_label(view.label()).is_some()));
            if !allowed || !registered { invoke.resolver.reject("此辅助视图不能调用该操作。"); return true; }
        }
        handler(invoke)
    }
}
fn validate_skin(shared: &Shared, skin: &ActiveSkin) -> Result<(), String> {
    match skin {
        ActiveSkin::Builtin => Ok(()),
        ActiveSkin::Png(id) => {
            let state = shared.skins.lock().map_err(|_| lock_error())?;
            if state.as_ref().map_err(Clone::clone)?.state()?.skins.iter().any(|skin| skin.skin_id == *id) { Ok(()) } else { Err("所选 PNG 皮肤不可用，请重新导入。".into()) }
        }
        ActiveSkin::Vrm(id) => { shared.vrm_skins.lock().map_err(|_| lock_error())?.as_ref().map_err(Clone::clone)?.read_model(id)?; Ok(()) }
    }
}
#[tauri::command]
pub(super) fn list_extra_pets(window: WebviewWindow, state: State<'_, SharedState>) -> Result<Vec<PetDescription>, String> {
    main_only(&window)?;
    Ok(state.extra_pets.lock().map_err(|_| lock_error())?.registry.list())
}
#[tauri::command]
pub(super) async fn create_extra_pet(window: WebviewWindow, state: State<'_, SharedState>, config: PetConfig) -> Result<PetDescription, String> {
    main_only(&window)?;
    let app = window.app_handle().clone(); let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (reservation, pin) = {
            let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
            validate_skin(&shared, &config.skin_choice)?;
            let mut owner = shared.extra_pets.lock().map_err(|_| lock_error())?;
            let pin = owner.pins.pin(vec![config.skin_choice.clone()])?;
            (owner.registry.reserve(config.clone())?, pin)
        };
        let task_pin = pin.ticket();
        let task_reservation = reservation.clone(); let task_shared = shared.clone();
        let result = popup_on_main(&app, move |app| {
            if !task_pin.is_live() || !task_reservation.is_live() || task_shared.stop.load(Ordering::Acquire) || task_shared.exiting.load(Ordering::Acquire) { return Err("辅助桌宠创建已取消。".into()); }
            let primary = app.get_webview_window("pet").ok_or("主桌宠窗口尚未就绪。")?;
            let (geometry, position) = native::motion_snapshot(&primary).ok_or("无法取得可用屏幕工作区。")?;
            let view = tauri::WebviewWindowBuilder::new(&app, &task_reservation.window_label, tauri::WebviewUrl::App("index.html?window=companion".into()))
                .title("Token 喵 · 辅助桌宠").inner_size(280.0 * config.scale, 340.0 * config.scale)
                .resizable(false).decorations(false).transparent(true).shadow(false).always_on_top(true).skip_taskbar(true).visible(false).focused(false)
                .build().map_err(|_| "无法创建辅助桌宠窗口。")?;
            let setup = (|| {
                view.set_focusable(false).map_err(|_| "无法设置辅助窗口交互。")?;
                let size = view.outer_size().map_err(|_| "辅助窗口尺寸不可用。")?;
                let bounds = registry::PlacementBounds { work_area: registry::WorkArea { x: geometry.work_area.left, y: geometry.work_area.top,
                    width: (i64::from(geometry.work_area.right)-i64::from(geometry.work_area.left)) as u32, height: (i64::from(geometry.work_area.bottom)-i64::from(geometry.work_area.top)) as u32 },
                    window_size: registry::WindowSize { width: size.width, height: size.height } };
                let pet = {
                    let mut owner = task_shared.extra_pets.lock().map_err(|_| lock_error())?;
                    if !task_pin.is_live() || task_shared.stop.load(Ordering::Acquire) || task_shared.exiting.load(Ordering::Acquire) { return Err("辅助桌宠创建已取消。".into()); }
                    let pet = owner.registry.commit(&task_reservation, view.label())?;
                    owner.registry.set_position(&pet.identity(), registry::PhysicalPoint { x: position.x.saturating_add(40), y: position.y }, bounds)?;
                    owner.registry.get(&pet.identity()).ok_or("辅助桌宠创建已取消。")?
                };
                if let Some(point) = pet.position { if !native::move_pet_without_activation(&view, roaming::Point { x: point.x, y: point.y }) { return Err("无法放置辅助桌宠。".into()); } }
                if pet.visible && !task_shared.native_paused.load(Ordering::Acquire) { show_without_focus(&view)?; }
                if !task_pin.is_live() { return Err("辅助桌宠创建已取消。".into()); }
                Ok(pet)
            })();
            if setup.is_err() {
                if let Ok(mut owner) = task_shared.extra_pets.lock() { let _ = owner.registry.remove(&task_reservation.identity); owner.registry.abort(&task_reservation); }
                let _ = view.destroy();
            }
            setup
        });
        drop(pin);
        if result.is_err() { if let Ok(mut owner) = shared.extra_pets.lock() { owner.registry.abort(&reservation); } }
        let _ = app.emit("extra-pets-changed", json!({"kind":"created"})); result
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) async fn configure_extra_pet(window: WebviewWindow, state: State<'_, SharedState>, identity: PetIdentity, patch: PetConfigPatch) -> Result<PetDescription, String> {
    main_only(&window)?;
    let app = window.app_handle().clone(); let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (expected, pin) = {
            let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
            let expected = shared.extra_pets.lock().map_err(|_| lock_error())?.registry.get(&identity).ok_or("辅助桌宠已关闭。")?;
            if let Some(skin) = &patch.skin_choice { validate_skin(&shared, skin)?; }
            let mut skins = vec![expected.skin_choice.clone()];
            if let Some(skin) = &patch.skin_choice { skins.push(skin.clone()); }
            let pin = shared.extra_pets.lock().map_err(|_| lock_error())?.pins.pin(skins)?;
            (expected, pin)
        };
        let task_pin = pin.ticket();
        let task_shared = shared.clone();
        let result = popup_on_main(&app, move |app| {
            if !task_pin.is_live() || task_shared.stop.load(Ordering::Acquire) || task_shared.exiting.load(Ordering::Acquire) { return Err("辅助桌宠配置已取消。".into()); }
            let previous = task_shared.extra_pets.lock().map_err(|_| lock_error())?.registry.get(&identity).ok_or("辅助桌宠已关闭。")?;
            if !same_configuration(&expected, &previous) { return Err("辅助桌宠配置已变化，请重新读取后再操作。".into()); }
            let view = app.get_webview_window(&previous.label).ok_or("辅助桌宠窗口已关闭。")?;
            let old_position = view.outer_position().map_err(|_| "辅助窗口位置不可用。")?;
            let old_size = view.outer_size().map_err(|_| "辅助窗口尺寸不可用。")?;
            let old_visible = view.is_visible().unwrap_or(false);
            let pet = {
                let mut owner = task_shared.extra_pets.lock().map_err(|_| lock_error())?;
                let current = owner.registry.get(&identity).ok_or("辅助桌宠已关闭。")?;
                if !task_pin.is_live() || !same_configuration(&expected, &current) { return Err("辅助桌宠配置已变化，请重新读取后再操作。".into()); }
                owner.drags.remove(&identity.pet_id);
                owner.registry.configure(&identity, patch)?
            };
            let applied = (|| {
                view.set_size(LogicalSize::new(280.0 * pet.scale, 340.0 * pet.scale)).map_err(|_| "无法调整辅助窗口尺寸。")?;
                let (geometry, position) = native::motion_snapshot(&view).ok_or("辅助窗口屏幕区域不可用。")?;
                let bounds = placement_bounds(geometry);
                let positioned = task_shared.extra_pets.lock().map_err(|_| lock_error())?.registry.set_position(&identity,
                    registry::PhysicalPoint { x: position.x, y: position.y }, bounds)?;
                let point = positioned.position.ok_or("辅助窗口位置不可用。")?;
                if !native::move_pet_without_activation(&view, roaming::Point { x: point.x, y: point.y }) { return Err("无法调整辅助窗口位置。".into()); }
                if pet.visible && !task_shared.native_paused.load(Ordering::Acquire) { show_without_focus(&view)?; } else { view.hide().map_err(|_| "无法隐藏辅助窗口。")?; }
                if !task_pin.is_live() { return Err("辅助桌宠配置已取消。".into()); }
                Ok(())
            })();
            if let Err(error) = applied {
                if let Ok(mut owner) = task_shared.extra_pets.lock() { let _ = owner.registry.configure(&identity, PetConfigPatch { skin_choice: Some(previous.skin_choice), visible: Some(previous.visible), scale: Some(previous.scale) }); }
                let _ = view.set_size(old_size); let _ = view.set_position(old_position);
                // 回滚不能把原本可见的辅助窗口重新带入已暂停/退出的桌面。
                if old_visible && !task_shared.native_paused.load(Ordering::Acquire) &&
                    !task_shared.stop.load(Ordering::Acquire) && !task_shared.exiting.load(Ordering::Acquire) {
                    let _ = show_without_focus(&view);
                } else { let _ = view.hide(); }
                if let Some((geometry, position)) = native::motion_snapshot(&view) {
                    if let Ok(mut owner) = task_shared.extra_pets.lock() { let _ = owner.registry.set_position(&identity,
                        registry::PhysicalPoint { x: position.x, y: position.y }, placement_bounds(geometry)); }
                }
                return Err(error);
            }
            task_shared.extra_pets.lock().map_err(|_| lock_error())?.registry.get(&identity).ok_or("辅助桌宠已关闭。".into())
        });
        drop(pin);
        let _ = app.emit("extra-pets-changed", json!({"kind":"configured"})); result
    }).await.map_err(|error| error.to_string())?
}
fn same_configuration(expected: &PetDescription, current: &PetDescription) -> bool {
    expected.identity() == current.identity() && expected.skin_choice == current.skin_choice &&
        expected.visible == current.visible && expected.scale == current.scale
}
fn placement_bounds(geometry: roaming::Geometry) -> registry::PlacementBounds {
    registry::PlacementBounds { work_area: registry::WorkArea { x: geometry.work_area.left, y: geometry.work_area.top,
        width: (i64::from(geometry.work_area.right)-i64::from(geometry.work_area.left)) as u32, height: (i64::from(geometry.work_area.bottom)-i64::from(geometry.work_area.top)) as u32 },
        window_size: registry::WindowSize { width: geometry.window_width, height: geometry.window_height } }
}
fn remove_on_main(app: &AppHandle, shared: &Shared, identity: &PetIdentity) -> Result<(), String> {
    let pet = {
        let mut owner = shared.extra_pets.lock().map_err(|_| lock_error())?;
        let pet = owner.registry.remove(identity)?; owner.drags.remove(&identity.pet_id); pet
    };
    if let Some(view) = app.get_webview_window(&pet.label) {
        // 先撤销登记再关闭，避免同步 Destroyed 回调重入登记锁；失败时也不留下可见孤窗。
        if view.destroy().is_err() { let _ = view.hide(); return Err("辅助窗口关闭失败，请退出应用后重新打开。".into()); }
    }
    changed(app); Ok(())
}
#[tauri::command]
pub(super) fn remove_extra_pet(window: WebviewWindow, state: State<'_, SharedState>, identity: PetIdentity) -> Result<(), String> {
    main_only(&window)?;
    let shared = state.inner().clone();
    popup_on_main(window.app_handle(), move |app| remove_on_main(&app, &shared, &identity))
}
#[tauri::command]
pub(super) async fn extra_pet_context(window: WebviewWindow, state: State<'_, SharedState>) -> Result<Value, String> {
    require_p1_development()?;
    let shared = state.inner().clone(); let label = window.label().to_owned();
    // 所有窗口查询在任何素材/登记锁之外完成。
    let visible = native::motion_window_available(&window);
    tauri::async_runtime::spawn_blocking(move || {
        let pet = description(&shared, &label)?;
        let mut png = Value::Null; let mut vrm = Value::Null;
        match &pet.skin_choice {
            ActiveSkin::Png(id) => { png = serde_json::to_value(shared.skins.lock().map_err(|_| lock_error())?.as_ref().map_err(Clone::clone)?.state()?.skins.into_iter().find(|skin| &skin.skin_id == id)).map_err(|_| "无法读取皮肤信息。")?; }
            ActiveSkin::Vrm(id) => { vrm = serde_json::to_value(shared.vrm_skins.lock().map_err(|_| lock_error())?.as_ref().map_err(Clone::clone)?.state()?.models.into_iter().find(|skin| &skin.model_id == id)).map_err(|_| "无法读取角色信息。")?; }
            _ => {}
        }
        Ok(json!({"pet":pet,"nativeVisible":visible,"paused":shared.native_paused.load(Ordering::Acquire),"png":png,"vrm":vrm}))
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) async fn extra_pet_pulse(window: WebviewWindow, state: State<'_, SharedState>, cursor: Option<Value>) -> Result<Value, String> {
    require_p1_development()?;
    let shared = state.inner().clone(); let label = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        description(&shared, &label)?;
        if shared.exiting.load(Ordering::Acquire) { return Err("应用正在退出。".into()); }
        let mut pulse = shared.core.lock().map_err(|_| lock_error())?.pet_pulse(cursor)?;
        pulse["prompts"] = json!([]); // 唯一业务提醒 owner 仍是主桌宠。
        Ok(pulse)
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) fn register_extra_pet_renderer(window: WebviewWindow, state: State<'_, SharedState>, renderer_epoch: String) -> Result<RendererLease, String> {
    require_p1_development()?;
    let mut owner = state.extra_pets.lock().map_err(|_| lock_error())?;
    let pet = owner.registry.get_by_label(window.label()).ok_or("辅助桌宠已关闭。")?;
    owner.drags.remove(&pet.pet_id);
    owner.registry.register_renderer(window.label(), &pet.identity(), &renderer_epoch)
}
#[tauri::command]
pub(super) fn unregister_extra_pet_renderer(window: WebviewWindow, state: State<'_, SharedState>, renderer_epoch: String, token: String) -> Result<(), String> {
    let mut owner = state.extra_pets.lock().map_err(|_| lock_error())?;
    if let Some(pet) = owner.registry.get_by_label(window.label()) {
        if let Some(lease) = owner.registry.renderer_for_request(window.label(), &pet.identity(), &renderer_epoch, &token) { lease.cancel(); owner.drags.remove(&pet.pet_id); }
    }
    Ok(())
}
#[tauri::command]
pub(super) fn close_extra_pet(window: WebviewWindow, state: State<'_, SharedState>, renderer_epoch: String, token: String) -> Result<(), String> {
    let identity = {
        let owner = state.extra_pets.lock().map_err(|_| lock_error())?;
        let pet = owner.registry.get_by_label(window.label()).ok_or("辅助桌宠已关闭。")?;
        owner.registry.renderer_for_request(window.label(), &pet.identity(), &renderer_epoch, &token).ok_or("辅助视图许可已失效。")?.identity
    };
    let shared = state.inner().clone(); popup_on_main(window.app_handle(), move |app| remove_on_main(&app, &shared, &identity))
}
#[tauri::command]
pub(super) fn start_extra_pet_drag(window: WebviewWindow, state: State<'_, SharedState>, request: PetDragRequest, renderer_epoch: String, token: String) -> Result<(), String> {
    require_p1_development()?;
    let label = window.label().to_owned(); let shared = state.inner().clone();
    popup_on_main(window.app_handle(), move |app| {
        let view = app.get_webview_window(&label).ok_or("辅助桌宠窗口已关闭。")?;
        let lease = {
            let owner = shared.extra_pets.lock().map_err(|_| lock_error())?;
            let pet = owner.registry.get_by_label(&label).ok_or("辅助桌宠已关闭。")?;
            owner.registry.renderer_for_request(&label, &pet.identity(), &renderer_epoch, &token).ok_or("辅助视图许可已失效。")?
        };
        #[cfg(windows)] {
            if shared.manual_drag.try_lock().map(|owner| owner.is_active()).unwrap_or(true) || dragging(&shared) {
                return Err("已有桌宠正在拖动，请先松开鼠标。".into());
            }
            let input = native::sample_drag_input(native::motion_window_available(&view), manual_drag_runtime_available(&shared));
            if let Some(reason) = manual_drag_input_rejection(input) { return Err(manual_drag_rejection_message(reason)); }
            let anchor = native::map_drag_press(&view, manual_drag::ClientAnchor { x: request.client_x, y: request.client_y, viewport_width: request.viewport_width, viewport_height: request.viewport_height })?;
            let rect = manual_drag_rect(&view).ok_or("辅助窗口位置不可用。")?;
            let mut controller = manual_drag::Controller::default();
            controller.start(manual_drag::Input { cursor: Some(anchor), ..input }, rect, shared.started_at.elapsed()).map_err(|_| "无法开始辅助桌宠拖动。")?;
            {
                let mut owner = shared.extra_pets.lock().map_err(|_| lock_error())?;
                if !owner.registry.validate_renderer(&label, &lease) { return Err("辅助视图许可已失效。".into()); }
                owner.drags.insert(lease.identity.pet_id.clone(), ExtraDrag { identity: lease.identity.clone(), lease, controller });
            }
            step_on_main(&app, &shared); Ok(())
        }
        #[cfg(not(windows))] { let _ = (request, lease); Err("当前平台暂不支持辅助桌宠拖动。".into()) }
    })
}
/// 与主桌宠共用一个 UI tick；每只持有独立 controller，不产生第二套采集器或循环。
pub(super) fn step_on_main(app: &AppHandle, shared: &Shared) {
    #[cfg(windows)] {
        let pets = shared.extra_pets.try_lock().ok().map(|owner| owner.registry.list()).unwrap_or_default();
        for pet in pets {
            let Some(view) = app.get_webview_window(&pet.label) else {
                if let Ok(mut owner) = shared.extra_pets.try_lock() { owner.drags.remove(&pet.pet_id); }
                continue;
            };
            let Some(rect) = manual_drag_rect(&view) else {
                if let Ok(mut owner) = shared.extra_pets.try_lock() { owner.drags.remove(&pet.pet_id); }
                continue;
            };
            let areas = native::work_areas(&view).unwrap_or_default().into_iter().map(|area| manual_drag::Rect { x: area.x, y: area.y, width: area.width, height: area.height }).collect::<Vec<_>>();
            let input = native::sample_drag_input(native::motion_window_available(&view), manual_drag_runtime_available(shared));
            let update = {
                let Ok(mut owner) = shared.extra_pets.try_lock() else { continue; };
                let Some(mut drag) = owner.drags.remove(&pet.pet_id) else { continue; };
                if !owner.registry.validate_renderer(&pet.label, &drag.lease) { continue; }
                let update = drag.controller.tick(input, rect, &areas, shared.started_at.elapsed());
                let lease = drag.lease.clone(); let identity = drag.identity.clone();
                if update.stopped.is_none() { owner.drags.insert(pet.pet_id.clone(), drag); }
                update.target.map(|target| (target, lease, identity))
            };
            if let Some((target, lease, identity)) = update {
                let boundary = native::sample_drag_input(native::motion_window_available(&view), manual_drag_runtime_available(shared));
                let allowed = manual_drag_input_rejection(boundary).is_none() && shared.extra_pets.try_lock().is_ok_and(|owner| owner.registry.validate_renderer(&pet.label, &lease));
                if allowed && native::move_pet_without_activation(&view, roaming::Point { x: target.x, y: target.y }) {
                    if let Some((geometry, position)) = native::motion_snapshot(&view) { if let Ok(mut owner) = shared.extra_pets.try_lock() { let _ = owner.registry.set_position(&identity, registry::PhysicalPoint { x: position.x, y: position.y }, placement_bounds(geometry)); } }
                } else if let Ok(mut owner) = shared.extra_pets.try_lock() { owner.drags.remove(&pet.pet_id); }
            }
        }
    }
    #[cfg(not(windows))] { let _ = (app, shared); }
}
pub(super) fn runtime_changed(app: &AppHandle, shared: &Shared, paused: bool) {
    let pets = {
        let Ok(mut owner) = shared.extra_pets.lock() else { return; };
        if paused { owner.drags.clear(); owner.registry.revoke_renderers(); }
        owner.registry.list()
    };
    for pet in pets { if let Some(view) = app.get_webview_window(&pet.label) { if paused || !pet.visible { let _ = view.hide(); } else { let _ = show_without_focus(&view); } } }
    let _ = app.emit("extra-pets-changed", json!({"kind":"runtime","paused":paused}));
}
pub(super) fn shutdown(app: &AppHandle, shared: &Shared) {
    let labels = {
        let Ok(mut owner) = shared.extra_pets.lock() else { return; };
        let labels = owner.registry.list().into_iter().map(|pet| pet.label).collect::<Vec<_>>();
        owner.drags.clear(); owner.pins.close(); owner.registry.clear(); labels
    };
    for label in labels { if let Some(view) = app.get_webview_window(&label) { let _ = view.hide(); } }
}
pub(super) fn on_window_event(window: &tauri::Window, event: &WindowEvent, shared: &Shared) -> bool {
    if matches!(window.label(), "main" | "pet") { return false; }
    let pet = description(shared, window.label()).ok();
    if let Some(pet) = pet {
        match event {
            WindowEvent::CloseRequested { api, .. } => { api.prevent_close(); let _ = remove_on_main(window.app_handle(), shared, &pet.identity()); },
            WindowEvent::Destroyed => { if let Ok(mut owner) = shared.extra_pets.lock() { owner.drags.remove(&pet.pet_id); owner.registry.revoke_window(window.label(), &pet.instance_epoch); } changed(window.app_handle()); },
            WindowEvent::Resized(_) => { let visible = window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false); let _ = window.emit("window-visibility-changed", json!({"label":window.label(),"visible":visible})); },
            _ => {}
        }
    }
    true // 未登记窗口也不进入主桌宠的拖放/穿透/位置处理。
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pet() -> PetDescription {
        let mut registry = registry::Registry::default();
        let reservation = registry.reserve(PetConfig::default()).unwrap();
        registry.commit(&reservation, &reservation.window_label).unwrap()
    }
    #[test]
    fn pending_config_allows_independent_drag_position_updates() {
        let expected = pet(); let mut current = expected.clone();
        current.position = Some(registry::PhysicalPoint { x: -100, y: 240 });
        assert!(same_configuration(&expected, &current));
    }
    #[test]
    fn pending_config_rejects_intervening_identity_skin_visibility_and_scale_changes() {
        let expected = pet();
        for field in ["pet", "instance", "skin", "visible", "scale"] {
            let mut current = expected.clone();
            match field {
                "pet" => current.pet_id = uuid::Uuid::new_v4().to_string(),
                "instance" => current.instance_epoch = uuid::Uuid::new_v4().to_string(),
                "skin" => current.skin_choice = ActiveSkin::Png("a".repeat(64)),
                "visible" => current.visible = false,
                _ => current.scale = 1.25,
            }
            assert!(!same_configuration(&expected, &current), "{field}");
        }
    }
    #[test]
    fn auxiliary_view_cannot_invoke_primary_window_or_ledger_actions() {
        for command in ["run_action", "get_snapshot", "get_pet_details", "start_pet_drag", "quit_app", "open_thread", "export_csv",
            "set_pet_motion_readiness", "register_pet_pointer_renderer", "select_pet_skin", "select_vrm_skin", "desktop_debug",
            "create_extra_pet", "remove_extra_pet", "configure_extra_pet", "save_usage_plan", "refresh_account", "unknown_new_command"] {
            assert!(!extra_command_allowed(command), "{command}");
        }
        for command in ["extra_pet_context", "extra_pet_pulse", "register_extra_pet_renderer", "unregister_extra_pet_renderer", "start_extra_pet_drag", "close_extra_pet", "read_pet_skin_frame", "read_vrm_skin_model"] {
            assert!(extra_command_allowed(command), "{command}");
        }
    }
}
