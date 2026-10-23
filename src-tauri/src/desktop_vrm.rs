//! 离线 VRM 库入口。文件来自系统选择器；渲染器只使用模型摘要 ID。
use super::*;
use crate::vrm_skin::{VrmSkinModel, VrmSkinState};

fn main_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" { Ok(()) } else { Err("请在主面板管理 VRM 角色。".into()) }
}
#[tauri::command]
pub(super) async fn get_vrm_skin_state(state: State<'_, SharedState>) -> Result<VrmSkinState, String> {
    require_p1_development()?;
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
        skin_selection::vrm_state(&shared)
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) async fn choose_import_vrm_skin(window: WebviewWindow, state: State<'_, SharedState>) -> Result<Option<VrmSkinModel>, String> {
    main_only(&window)?; require_p1_development()?;
    let shared = state.inner().clone();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let source = app.dialog().file().set_title("导入 VRM 1.0 角色").add_filter("VRM 角色", &["vrm"]).blocking_pick_file();
        let Some(source) = source else { return Ok(None); };
        let source = source.into_path().map_err(|error| error.to_string())?;
        let model = shared.vrm_skins.lock().map_err(|_| lock_error())?.as_mut().map_err(|error| error.clone())?.import_file(&source)?;
        let _ = app.emit("pet-skin-library-changed", json!({"kind":"imported"}));
        Ok(Some(model))
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) async fn select_vrm_skin(window: WebviewWindow, state: State<'_, SharedState>, model_id: Option<String>) -> Result<VrmSkinState, String> {
    main_only(&window)?; require_p1_development()?;
    let shared = state.inner().clone();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        physics_adapter::cancel(&shared);
        let result = skin_selection::select_vrm(&shared, model_id.as_deref());
        physics_adapter::cancel(&shared);
        drop_adapter::clear(&shared);
        shared.motion_invalidated.store(true, Ordering::Release);
        interrupt_motion(&app, &shared, "animationUnavailable");
        pointer::reset(&shared);
        let _ = app.emit("pet-skin-library-changed", json!({"kind":"selected"}));
        result
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) async fn remove_vrm_skin(window: WebviewWindow, state: State<'_, SharedState>, model_id: String) -> Result<VrmSkinState, String> {
    main_only(&window)?; require_p1_development()?;
    let shared = state.inner().clone();
    let app = window.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = skin_selection::remove_vrm(&shared, &model_id)?;
        let _ = app.emit("pet-skin-library-changed", json!({"kind":"removed"}));
        Ok(state)
    }).await.map_err(|error| error.to_string())?
}
#[tauri::command]
pub(super) async fn read_vrm_skin_model(window: WebviewWindow, state: State<'_, SharedState>, model_id: String) -> Result<tauri::ipc::Response, String> {
    require_p1_development()?;
    if !matches!(window.label(), "main" | "pet") && !extra_pets::allows_asset_read(&state, window.label(), &model_id, true) { return Err("此窗口不能读取角色模型。".into()); }
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || shared.vrm_skins.lock().map_err(|_| lock_error())?.as_ref().map_err(Clone::clone)?
        .read_model(&model_id).map(tauri::ipc::Response::new)).await.map_err(|error| error.to_string())?
}
