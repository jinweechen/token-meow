//! Tauri 拖放事件和确认命令；文件路径不作为 IPC 输入。
use super::*;
use sha2::{Digest, Sha256};

#[derive(Default)]
pub(super) struct Runtime {
    owner: file_drop::DropOwner,
    origin: Option<String>,
}

fn revision(shared: &Shared) -> Result<u64, String> {
    shared.drop_revision.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| value.checked_add(1))
        .map(|value| value + 1).map_err(|_| "拖放序号已耗尽，请重新启动应用。".into())
}

pub(super) fn clear(shared: &Shared) {
    let _ = revision(shared);
    // UI 的隐藏/暂停回调不能等待正在查询 HWND 的工作线程；原子代次先撤权。
    if let Ok(mut runtime) = shared.file_drop.try_lock() {
        runtime.owner.clear();
        runtime.origin = None;
    }
}

fn available(app: &AppHandle, shared: &Shared, origin: &str, require_panel: bool) -> bool {
    cfg!(windows) && !shared.stop.load(Ordering::Acquire) && !shared.exiting.load(Ordering::Acquire)
        && shared.drop_revision.load(Ordering::Acquire) != u64::MAX
        && !shared.native_paused.load(Ordering::Acquire)
        && app.get_webview_window(origin).is_some_and(|window| window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(true))
        && (!require_panel || app.get_webview_window("main").is_some_and(|window| window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(true)))
}

/// HWND 在任何 Core/拖放/应用登记锁之前取得；持锁期间只做 Win32 只读查询。
struct Admission { origin: isize, panel: isize }
impl Admission {
    fn new(app: &AppHandle, origin: &str) -> Result<Self, String> {
        #[cfg(windows)] {
            let original = app.get_webview_window(origin).ok_or("拖放来源窗口已关闭。")?;
            let panel = app.get_webview_window("main").ok_or("主面板尚未就绪。")?;
            Ok(Self { origin: original.hwnd().map_err(|_| "拖放来源窗口已关闭。")?.0 as isize,
                panel: panel.hwnd().map_err(|_| "主面板尚未就绪。")?.0 as isize })
        }
        #[cfg(not(windows))] { let _ = (app, origin); Err("当前平台不支持文件拖放。".into()) }
    }
    fn allowed(&self, shared: &Shared) -> bool {
        if shared.stop.load(Ordering::Acquire) || shared.exiting.load(Ordering::Acquire)
            || shared.native_paused.load(Ordering::Acquire) || shared.drop_revision.load(Ordering::Acquire) == u64::MAX { return false; }
        #[cfg(windows)] {
            use windows_sys::Win32::{System::Threading::GetCurrentProcessId, UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsIconic, IsWindowVisible}};
            [self.origin, self.panel].iter().all(|raw| unsafe {
                let hwnd = *raw as windows_sys::Win32::Foundation::HWND;
                let mut process = 0;
                GetWindowThreadProcessId(hwnd, &mut process) != 0 && process == GetCurrentProcessId()
                    && IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0
            })
        }
        #[cfg(not(windows))] { false }
    }
}

fn pending_admission(app: &AppHandle, shared: &Shared) -> Result<Option<(String, Admission)>, String> {
    let origin = shared.file_drop.lock().map_err(|_| lock_error())?.origin.clone();
    origin.map(|label| Admission::new(app, &label).map(|admission| (label, admission))).transpose()
}

fn present(app: &AppHandle, shared: SharedState, origin: String, stamp: u64, error: Option<String>) -> Result<bool, String> {
    popup_on_main(app, move |app| {
        if shared.drop_revision.load(Ordering::Acquire) != stamp || !available(&app, &shared, &origin, false) { return Ok(false); }
        show_panel(&app)?;
        if let Some(message) = error { let _ = app.emit("companion-drop-error", json!({"message":message})); }
        Ok(true)
    })
}

fn scope(core: &Core, stamp: u64) -> Result<String, String> {
    let source = core.primary_source()?;
    let binding = json!([core.runtime_id, source["id"], source["root"], source["clientKind"], source["enabled"], stamp]);
    Ok(format!("{:x}", Sha256::digest(binding.to_string().as_bytes())))
}

fn changed(app: &AppHandle) { let _ = app.emit("companion-drop-changed", ()); }

/// 只接收框架传来的实际 DragDropEvent::Drop，不对渲染器开放路径入口。
pub(super) fn capture(app: AppHandle, shared: SharedState, origin: String, paths: Vec<PathBuf>) {
    if !matches!(origin.as_str(), "main" | "pet") { return; }
    clear(&shared);
    let received = Instant::now();
    let stamp = shared.drop_revision.load(Ordering::Acquire);
    changed(&app);
    shared.motion_invalidated.store(true, Ordering::Release);
    cancel_manual_drag(&shared, manual_drag::StopReason::Cancelled);
    interrupt_motion(&app, &shared, "interaction");
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<(), String> {
            if !available(&app, &shared, &origin, false) { return Err("窗口隐藏或系统暂停时不能接收文件。".into()); }
            shared.companion.lock().map_err(|_| lock_error())?.list()?;
            if shared.drop_revision.load(Ordering::Acquire) != stamp { return Ok(()); }
            // 先显示确认承载窗口，再发布批次；隐藏页面的旧读取不能清掉尚未显示的新批次。
            if !present(&app, shared.clone(), origin.clone(), stamp, None)? { return Ok(()); }
            let core = shared.core.lock().map_err(|_| lock_error())?;
            let mut runtime = shared.file_drop.lock().map_err(|_| lock_error())?;
            if shared.drop_revision.load(Ordering::Acquire) != stamp { return Ok(()); }
            let binding = scope(&core, stamp)?;
            if received.elapsed() >= file_drop::DROP_TTL { return Err("拖放确认已到期，请重新拖入。".into()); }
            runtime.owner.capture(paths, &binding, received)?;
            if runtime.owner.current(&binding, Instant::now()).is_none() { return Err("拖放确认已到期，请重新拖入。".into()); }
            runtime.origin = Some(origin.clone());
            drop(runtime); drop(core);
            if shared.drop_revision.load(Ordering::Acquire) != stamp || !available(&app, &shared, &origin, false) { return Ok(()); }
            changed(&app);
            Ok(())
        })();
        if let Err(message) = result {
            if shared.drop_revision.load(Ordering::Acquire) == stamp {
                if let Ok(mut runtime) = shared.file_drop.try_lock() { runtime.owner.clear(); runtime.origin = None; }
                // 拖入是显式操作；在主面板展示原因，不把路径写进指标或账本。
                let _ = present(&app, shared.clone(), origin, stamp, Some(message));
            }
        }
    });
}

fn main_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" { Ok(()) } else { Err("请在主面板确认拖入的文件。".into()) }
}

#[tauri::command]
pub(super) async fn get_companion_drop(window: WebviewWindow, state: State<'_, SharedState>) -> Result<Option<file_drop::DropSummary>, String> {
    main_only(&window)?;
    let app = window.app_handle().clone();
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some((origin, admission)) = pending_admission(&app, &shared)? else { return Ok(None); };
        let core = shared.core.lock().map_err(|_| lock_error())?;
        let mut runtime = shared.file_drop.lock().map_err(|_| lock_error())?;
        if runtime.origin.as_deref() != Some(origin.as_str()) { return Ok(None); }
        if !admission.allowed(&shared) { runtime.owner.clear(); runtime.origin = None; return Ok(None); }
        let binding = scope(&core, shared.drop_revision.load(Ordering::Acquire))?;
        Ok(runtime.owner.current(&binding, Instant::now()))
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub(super) async fn cancel_companion_drop(window: WebviewWindow, state: State<'_, SharedState>, options: file_drop::DropBatchOptions) -> Result<(), String> {
    main_only(&window)?;
    let mut runtime = state.file_drop.lock().map_err(|_| lock_error())?;
    if runtime.owner.cancel(&options.batch_id) { runtime.origin = None; }
    drop(runtime);
    changed(window.app_handle());
    Ok(())
}

#[tauri::command]
pub(super) async fn register_companion_drop(window: WebviewWindow, state: State<'_, SharedState>, options: file_drop::DropBatchOptions) -> Result<Value, String> {
    main_only(&window)?;
    let app = window.app_handle().clone();
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            let (origin, admission) = pending_admission(&app, &shared)?.ok_or("拖放确认已到期，请重新拖入。")?;
            let core = shared.core.lock().map_err(|_| lock_error())?;
            let mut runtime = shared.file_drop.lock().map_err(|_| lock_error())?;
            if runtime.origin.as_deref() != Some(origin.as_str()) { return Err("拖放上下文已变化，请重新拖入。".into()); }
            let stamp = shared.drop_revision.load(Ordering::Acquire);
            if !admission.allowed(&shared) { runtime.owner.clear(); return Err("窗口隐藏或系统暂停，已取消添加。".into()); }
            let selected = runtime.owner.take_registration(&options.batch_id, &scope(&core, stamp)?, Instant::now())?;
            if !admission.allowed(&shared) || stamp != shared.drop_revision.load(Ordering::Acquire) { runtime.owner.clear(); return Err("拖放上下文已变化，请重新拖入。".into()); }
            let entry = shared.companion.lock().map_err(|_| lock_error())?.register(selected)?;
            runtime.owner.clear(); runtime.origin = None;
            drop(runtime); drop(core);
            let listed = shared.companion.lock().map_err(|_| lock_error())?.list()?;
            let _ = app.emit("companion-launchers-changed", listed);
            Ok(json!({"accepted":true,"launcher":entry}))
        })();
        changed(&app);
        result
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub(super) async fn open_companion_drop(window: WebviewWindow, state: State<'_, SharedState>, options: file_drop::DropOpenOptions) -> Result<Value, String> {
    main_only(&window)?;
    let app = window.app_handle().clone();
    let shared = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            let (origin, admission) = pending_admission(&app, &shared)?.ok_or("拖放确认已到期，请重新拖入。")?;
            let core = shared.core.lock().map_err(|_| lock_error())?;
            let mut runtime = shared.file_drop.lock().map_err(|_| lock_error())?;
            if runtime.origin.as_deref() != Some(origin.as_str()) { return Err("拖放上下文已变化，请重新拖入。".into()); }
            let stamp = shared.drop_revision.load(Ordering::Acquire);
            if !admission.allowed(&shared) { runtime.owner.clear(); return Err("窗口隐藏或系统暂停，已取消打开。".into()); }
            let files = runtime.owner.take_files(&options.batch_id, &scope(&core, stamp)?, Instant::now())?;
            let target = shared.companion.lock().map_err(|_| lock_error())?.target(&options.launcher_id)?;
            file_drop::launch_files_if(&target, files, || admission.allowed(&shared) && stamp == shared.drop_revision.load(Ordering::Acquire))?;
            runtime.owner.clear(); runtime.origin = None;
            Ok(json!({"accepted":true}))
        })();
        changed(&app);
        result
    }).await.map_err(|error| error.to_string())?
}
