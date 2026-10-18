//! UI-thread adapter for bounded renderer masks and temporary interaction.
use super::*;
use crate::pet_pointer_policy::{FrameMask, PetPointerPolicy, Region, Viewport};

#[derive(Default)]
pub(super) struct Runtime {
    pub policy: PetPointerPolicy,
    applied: Option<(u64, bool)>,
    pub(super) ignore: Option<bool>,
}
fn now(shared: &Shared) -> u64 {
    shared
        .started_at
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn policy_error(error: crate::pet_pointer_policy::PolicyError) -> String {
    format!("透明命中状态已变化：{error:?}")
}

pub(super) fn reset(shared: &Shared) {
    if let Ok(mut pointer) = shared.pointer.lock() {
        pointer.policy.reset_context();
    }
}

/// Every call is on the main thread; no Core mutex is held across HWND calls.
pub(super) fn sync(window: &WebviewWindow, shared: &Shared) -> Result<Option<Viewport>, String> {
    let visible = window.is_visible().unwrap_or(false);
    #[cfg(windows)]
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as _;
    #[cfg(windows)]
    let viewport = crate::pet_pointer_region::read_viewport(hwnd).ok();
    #[cfg(not(windows))]
    let viewport: Option<Viewport> = None;
    let blocked = shared.native_dragging.load(Ordering::Acquire)
        || shared
            .pet_interaction
            .lock()
            .map(|value| value.active)
            .unwrap_or(true);
    let (stamp, ignore, remaining, rects, applied, previous_ignore) = {
        let mut pointer = shared.pointer.lock().map_err(|_| lock_error())?;
        pointer.policy.set_visible(visible);
        pointer.policy.set_viewport(viewport);
        let applied = pointer.applied;
        let previous_ignore = pointer.ignore;
        let mut plan = pointer.policy.plan(now(shared));
        if blocked {
            plan.region = Region::FullWindow;
        }
        (
            (plan.generation, blocked),
            plan.ignore_cursor_events,
            plan.temporary_remaining_ms,
            match plan.region {
                Region::FullWindow => None,
                Region::Rects(rects) => Some(rects.to_vec()),
            },
            applied,
            previous_ignore,
        )
    };
    // SetWindowRgn can synchronously send window messages. Never hold policy or
    // ledger locks during OS calls, including rectangle recovery.
    if applied != Some(stamp) {
        let plan = crate::pet_pointer_policy::Plan {
            viewport,
            generation: stamp.0,
            region: rects.as_deref().map_or(Region::FullWindow, Region::Rects),
            ignore_cursor_events: ignore,
            temporary_remaining_ms: remaining,
        };
        #[cfg(windows)]
        if let Err(error) = crate::pet_pointer_region::apply(hwnd, &plan) {
            if let Ok(mut pointer) = shared.pointer.lock() {
                pointer.policy.set_viewport(None);
                pointer.applied = None;
            }
            let _ = crate::pet_pointer_region::clear(hwnd);
            return Err(error.to_string());
        }
    }
    if previous_ignore != Some(ignore) {
        window
            .set_ignore_cursor_events(ignore)
            .map_err(|e| e.to_string())?;
    }
    let mut pointer = shared.pointer.lock().map_err(|_| lock_error())?;
    let current = pointer.policy.plan(now(shared));
    let unchanged = current.generation == stamp.0 && current.ignore_cursor_events == ignore;
    pointer.applied = unchanged.then_some(stamp);
    pointer.ignore = unchanged.then_some(ignore);
    Ok(viewport)
}

async fn on_main<T: Send + 'static>(
    app: AppHandle,
    action: impl FnOnce(&WebviewWindow, &Shared) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            let shared = handle.state::<SharedState>();
            if shared.exiting.load(Ordering::Acquire) {
                return Err("应用正在退出".into());
            }
            let window = handle.get_webview_window("pet").ok_or("桌宠窗口尚未就绪")?;
            action(&window, &shared)
        })();
        let _ = send.send(result);
    })
    .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        receive
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| "桌宠交互更新超时".to_owned())?
    })
    .await
    .map_err(|e| e.to_string())?
}
fn pet_only(window: &WebviewWindow) -> Result<(), String> {
    require_p1_development()?;
    if window.label() == "pet" {
        Ok(())
    } else {
        Err("仅桌宠窗口可更新透明命中".into())
    }
}
fn viewport_json(viewport: Viewport) -> Value {
    json!({"width":viewport.width,"height":viewport.height,"dpi":viewport.dpi})
}

#[tauri::command]
pub(super) async fn register_pet_pointer_renderer(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<Value, String> {
    pet_only(&window)?;
    on_main(app, |window, shared| {
        let owner = shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .register_renderer()
            .map_err(policy_error)?;
        let viewport = sync(window, shared)?;
        Ok(json!({"owner":owner,"viewport":viewport.map(viewport_json)}))
    })
    .await
}
#[tauri::command]
pub(super) async fn begin_pet_pointer_frame(
    window: WebviewWindow,
    app: AppHandle,
    owner: u64,
    revision: u64,
    frame_key: String,
) -> Result<Value, String> {
    pet_only(&window)?;
    on_main(app, move |window, shared| {
        // Read actual geometry before accepting the renderer's frame.
        sync(window, shared)?;
        shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .begin_frame(owner, revision, frame_key)
            .map_err(policy_error)?;
        let viewport = sync(window, shared)?.ok_or("窗口尺寸暂不可用")?;
        Ok(json!({"viewport":viewport_json(viewport)}))
    })
    .await
}
#[tauri::command]
pub(super) async fn publish_pet_pointer_mask(
    window: WebviewWindow,
    app: AppHandle,
    mask: FrameMask,
) -> Result<bool, String> {
    pet_only(&window)?;
    on_main(app, move |window, shared| {
        sync(window, shared)?;
        let result = shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .publish_mask(mask, now(shared));
        sync(window, shared)?; // Invalid new masks also clear the last region.
        result.map_err(policy_error)?;
        Ok(true)
    })
    .await
}
#[tauri::command]
pub(super) async fn renew_pet_pointer_mask(
    window: WebviewWindow,
    app: AppHandle,
    owner: u64,
    revision: u64,
) -> Result<bool, String> {
    pet_only(&window)?;
    on_main(app, move |window, shared| {
        let result = shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .renew(owner, revision, now(shared));
        sync(window, shared)?;
        result.map_err(policy_error)?;
        Ok(true)
    })
    .await
}
#[tauri::command]
pub(super) async fn release_pet_pointer_renderer(
    window: WebviewWindow,
    app: AppHandle,
    owner: u64,
) -> Result<(), String> {
    pet_only(&window)?;
    on_main(app, move |window, shared| {
        shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .release_renderer(owner)
            .map_err(policy_error)?;
        sync(window, shared)?;
        Ok(())
    })
    .await
}
pub(super) fn temporary(app: &AppHandle) -> Result<(), String> {
    require_p1_development()?;
    let shared = app.state::<SharedState>();
    shared
        .pointer
        .lock()
        .map_err(|_| lock_error())?
        .policy
        .begin_temporary(now(&shared), None);
    schedule_pet_motion(app, &shared);
    Ok(())
}
#[tauri::command]
pub(super) async fn begin_pet_temporary_interaction(app: AppHandle) -> Result<Value, String> {
    require_p1_development()?;
    on_main(app, |window, shared| {
        shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .begin_temporary(now(shared), None);
        sync(window, shared)?;
        let remaining = shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .plan(now(shared))
            .temporary_remaining_ms;
        Ok(json!({"remainingMs":remaining}))
    })
    .await
}
#[tauri::command]
pub(super) async fn cancel_pet_temporary_interaction(app: AppHandle) -> Result<(), String> {
    on_main(app, |window, shared| {
        shared
            .pointer
            .lock()
            .map_err(|_| lock_error())?
            .policy
            .cancel_temporary();
        sync(window, shared)?;
        Ok(())
    })
    .await
}
