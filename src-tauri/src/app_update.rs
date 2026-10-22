use crate::update_policy::{self as policy, Release};
use serde_json::{json, Value};
use std::{fs::{self, File, OpenOptions}, io::{Read, Seek, SeekFrom, Write}, path::PathBuf, sync::{atomic::{AtomicBool, Ordering}, Mutex}, time::{Duration, Instant}};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

#[derive(Default)]
pub struct UpdateState { busy: AtomicBool, session: Mutex<Session> }
#[derive(Default)]
struct Session { release: Option<Release>, checked: Option<Instant>, prepared: Option<Prepared> }
struct Prepared { path: PathBuf, held: File }
struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }
fn enter(state: &UpdateState) -> Result<Busy<'_>, String> {
    state.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).map_err(|_| "更新操作正在进行")?;
    Ok(Busy(&state.busy))
}
fn main_window(window: &WebviewWindow) -> Result<(), String> { if window.label()=="main" {Ok(())} else {Err("请从主面板的应用更新中操作".into())} }
fn client() -> Result<reqwest::blocking::Client,String> {
    reqwest::blocking::Client::builder().user_agent(concat!("TokenMeow/",env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(300)).https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let host=attempt.url().host_str().unwrap_or("");
            if attempt.previous().len()>=5 || !(host=="github.com" || host=="release-assets.githubusercontent.com" || host=="objects.githubusercontent.com") {attempt.stop()} else {attempt.follow()}
        })).build().map_err(|_| "无法创建更新连接".into())
}
fn ordinary(path: &std::path::Path) -> Result<(),String> {
    let mut current=Some(path);
    while let Some(p)=current {
        if let Ok(meta)=fs::symlink_metadata(p) {
            #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if meta.file_attributes() & 0x400 != 0 {return Err("更新缓存不能包含链接".into());} }
            if meta.file_type().is_symlink() {return Err("更新缓存不能包含链接".into());}
        }
        current=p.parent();
    }
    Ok(())
}
fn cache(app: &AppHandle) -> Result<PathBuf,String> {
    let root=app.path().app_local_data_dir().map_err(|_| "无法定位更新缓存")?.join("updates");
    ordinary(&root)?; fs::create_dir_all(&root).map_err(|_| "无法创建更新缓存")?;
    // At most one retained setup and one in-progress setup; never recurse or follow links.
    for entry in fs::read_dir(&root).map_err(|_| "无法读取更新缓存")? {
        let entry=entry.map_err(|_| "无法读取更新缓存")?;
        if uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_err() {continue;}
        ordinary(&entry.path())?;
        let file=entry.path().join("setup.exe");
        if file.exists() {ordinary(&file)?; fs::remove_file(file).map_err(|_| "已有更新包仍在使用，请关闭安装器后重试")?;}
        fs::remove_dir(entry.path()).map_err(|_| "更新缓存包含未知文件，已保留")?;
    }
    let path=root.join(uuid::Uuid::new_v4().to_string()); fs::create_dir(&path).map_err(|_| "无法创建更新下载目录")?; Ok(path.join("setup.exe"))
}

#[tauri::command]
pub async fn check_app_update(app: AppHandle, window: WebviewWindow) -> Result<Value,String> {
    main_window(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state=app.state::<UpdateState>(); let _busy=enter(&state)?;
        let mut session=state.session.lock().map_err(|_| "更新状态不可用")?;
        session.release=None; session.prepared=None; session.checked=None;
        let response=client()?.get(policy::ENDPOINT).timeout(Duration::from_secs(25)).send().map_err(|_| "无法连接 GitHub Releases，请检查网络后重试")?;
        if response.status()==reqwest::StatusCode::NOT_FOUND {return Err("GitHub Releases 尚未发布更新清单".into());}
        let response=response.error_for_status().map_err(|_| "更新服务器返回错误，请稍后重试")?;
        let mut body=Vec::new(); response.take(128*1024+1).read_to_end(&mut body).map_err(|_| "读取更新信息失败")?;
        if body.len()>128*1024 {return Err("更新清单超过大小限制".into());}
        let release: Release=serde_json::from_slice(&body).map_err(|_| "更新清单格式无效")?;
        let available=policy::validate_release(&release,env!("CARGO_PKG_VERSION"))?;
        let result=json!({"currentVersion":env!("CARGO_PKG_VERSION"),"available":available,"version":release.version,"notes":release.notes,"repository":policy::REPOSITORY});
        if available {session.release=Some(release);session.checked=Some(Instant::now());}
        Ok(result)
    }).await.map_err(|_| "更新检查任务失败")?
}

#[tauri::command]
pub async fn download_app_update(app: AppHandle, window: WebviewWindow) -> Result<Value,String> {
    main_window(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state=app.state::<UpdateState>(); let _busy=enter(&state)?;
        let mut session=state.session.lock().map_err(|_| "更新状态不可用")?;
        if !session.checked.is_some_and(|t|t.elapsed()<Duration::from_secs(3600)) {return Err("更新检查已过期，请重新检查".into());}
        let release=session.release.clone().ok_or("请先检查新版本")?;
        let artifact=release.platforms.get("windows-x86_64").ok_or("缺少 Windows 安装包")?;
        session.prepared=None;
        let path=cache(&app)?;
        let result=(|| -> Result<File,String> {
            let mut response=client()?.get(&artifact.url).send().map_err(|_| "下载失败，请检查网络后重试")?.error_for_status().map_err(|_| "安装包下载失败")?;
            let total=response.content_length();
            if total.is_some_and(|n|n>policy::MAX_INSTALLER_BYTES) {return Err("更新安装包超过大小限制".into());}
            let mut output=OpenOptions::new().create_new(true).write(true).open(&path).map_err(|_| "无法写入更新缓存")?;
            let mut received=0u64; let mut buf=[0u8;65536]; let mut last=Instant::now();
            loop {
                let n=response.read(&mut buf).map_err(|_| "下载中断，请重试")?; if n==0 {break;}
                received+=n as u64; if received>policy::MAX_INSTALLER_BYTES {return Err("更新安装包超过大小限制".into());}
                output.write_all(&buf[..n]).map_err(|_| "更新缓存写入失败，请检查磁盘空间")?;
                if last.elapsed()>Duration::from_millis(150) {let _=app.emit("app-update-progress",json!({"received":received,"total":total}));last=Instant::now();}
            }
            if total.is_some_and(|n|n!=received) {return Err("更新包下载不完整".into());}
            output.sync_all().map_err(|_| "更新缓存保存失败")?;drop(output);
            let mut options=OpenOptions::new();options.read(true);
            #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(1).custom_flags(0x00200000);}
            let mut held=options.open(&path).map_err(|_| "无法保护下载后的更新文件")?;
            ordinary(&path)?;
            policy::verify_reader(&mut held,&artifact.signature,&release.version,policy::PUBLIC_KEY)?;
            held.seek(SeekFrom::Start(0)).map_err(|_| "无法读取更新包")?;
            let mut magic=[0u8;2];held.read_exact(&mut magic).map_err(|_| "更新包无效")?;
            if magic!=*b"MZ" {return Err("更新包不是 Windows 安装程序".into());}
            let _=app.emit("app-update-progress",json!({"received":received,"total":Some(received)}));
            Ok(held)
        })();
        match result {
            Ok(held)=>{session.prepared=Some(Prepared{path,held});Ok(json!({"ready":true,"version":release.version}))},
            Err(error)=>{let _=fs::remove_file(&path);let _=fs::remove_dir(path.parent().unwrap());Err(error)}
        }
    }).await.map_err(|_| "更新下载任务失败")?
}

#[tauri::command]
pub fn launch_app_update(app:AppHandle,window:WebviewWindow,state:State<UpdateState>)->Result<(),String>{
    main_window(&window)?;let _busy=enter(&state)?;
    let mut session=state.session.lock().map_err(|_| "更新状态不可用")?;
    let prepared=session.prepared.as_mut().ok_or("请先下载并校验更新包")?;
    let _=&prepared.held; // Deny replacement until CreateProcess has opened the verified image.
    std::process::Command::new(&prepared.path).spawn().map_err(|_| "无法打开更新安装器，应用仍保持运行")?;
    drop(session);
    crate::desktop::quit_app(app);
    Ok(())
}
