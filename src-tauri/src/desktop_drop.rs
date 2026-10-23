//! 原生拖放批次的短期保管与显式转交；路径只留在原生内存中。
//! 静态路径和文件身份检查不防御其他进程在最后一次检查后抢换命名空间。
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub(super) const DROP_TTL: Duration = Duration::from_secs(60);
const MAX_FILES: usize = 8;
const MAX_PATH_CHARACTERS: usize = 2_048;
const INVALID_BATCH: &str = "拖入的文件已到期、被取消或不属于当前页面，请重新拖入。";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum DropKind {
    RegisterExecutable,
    OpenFiles,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DropSummary {
    pub batch_id: String,
    pub kind: DropKind,
    pub file_names: Vec<String>,
    pub expires_in_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DropBatchOptions {
    pub batch_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DropOpenOptions {
    pub batch_id: String,
    pub launcher_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    volume: u32,
    file_index: u64,
    length: u64,
    last_write: u64,
}

struct CapturedFile {
    path: PathBuf,
    identity: FileIdentity,
    name: String,
}

#[derive(Clone)]
struct Lease {
    id: String,
    scope: String,
    created_at: Instant,
    revoked: Arc<AtomicBool>,
}

impl Lease {
    fn remaining(&self, now: Instant) -> Option<Duration> {
        if self.revoked.load(Ordering::Acquire) {
            return None;
        }
        now.checked_duration_since(self.created_at)
            .filter(|age| *age < DROP_TTL)
            .map(|age| DROP_TTL - age)
    }

    fn ensure_live(&self, now: Instant) -> Result<(), String> {
        self.remaining(now)
            .map(|_| ())
            .ok_or_else(|| INVALID_BATCH.into())
    }
}

struct Batch {
    lease: Lease,
    kind: DropKind,
    files: Vec<CapturedFile>,
}

/// 不向调用方公开路径，也不能复制成第二次启动许可。
pub(super) struct FileDropSelection {
    files: Vec<CapturedFile>,
    lease: Lease,
}

#[derive(Default)]
pub(super) struct DropOwner {
    pending: Option<Batch>,
    active: Option<Lease>,
}

impl Drop for DropOwner {
    fn drop(&mut self) {
        self.clear();
    }
}

impl DropOwner {
    /// 只允许桌宠原生 DragDropEvent::Drop 处理器调用；不能接收 IPC 路径。
    pub(super) fn capture(
        &mut self,
        paths: Vec<PathBuf>,
        scope: &str,
        now: Instant,
    ) -> Result<DropSummary, String> {
        // 新的拖放尝试即撤销旧批次，包括本次检查失败的情况。
        self.clear();
        if !cfg!(windows) {
            return Err("桌宠文件拖放目前仅支持 Windows。".into());
        }
        if scope.is_empty() || scope.len() > 256 {
            return Err("当前页面尚未准备好接收文件。".into());
        }
        if paths.is_empty() || paths.len() > MAX_FILES {
            return Err("每次请拖入 1 至 8 个文件。".into());
        }
        let files: Vec<_> = paths
            .into_iter()
            .map(capture_file)
            .collect::<Result<_, _>>()?;
        let mut unique = std::collections::HashSet::new();
        if files
            .iter()
            .any(|file| !unique.insert(path_key(&file.path)))
        {
            return Err("同一批不能包含重复文件。".into());
        }
        let executables = files
            .iter()
            .filter(|file| is_executable(&file.path))
            .count();
        let kind = match (executables, files.len()) {
            (0, _) => DropKind::OpenFiles,
            (1, 1) => DropKind::RegisterExecutable,
            _ => return Err("登记应用时请只拖入一个 .exe，不要与其他文件混放。".into()),
        };
        let lease = Lease {
            id: uuid::Uuid::new_v4().to_string(),
            scope: scope.to_owned(),
            created_at: now,
            revoked: Arc::new(AtomicBool::new(false)),
        };
        self.active = Some(lease.clone());
        self.pending = Some(Batch { lease, kind, files });
        self.current(scope, now).ok_or_else(|| INVALID_BATCH.into())
    }

    /// 超时、换域或时钟异常均关闭当前批次，不能由渲染器延长租约。
    pub(super) fn current(&mut self, scope: &str, now: Instant) -> Option<DropSummary> {
        let remaining = self.active.as_ref().and_then(|lease| {
            (lease.scope == scope)
                .then(|| lease.remaining(now))
                .flatten()
        });
        let Some(remaining) = remaining else {
            self.clear();
            return None;
        };
        let batch = self.pending.as_ref()?;
        Some(DropSummary {
            batch_id: batch.lease.id.clone(),
            kind: batch.kind,
            file_names: batch.files.iter().map(|file| file.name.clone()).collect(),
            expires_in_ms: remaining.as_millis() as u64,
        })
    }

    pub(super) fn clear(&mut self) {
        if let Some(lease) = self.active.take() {
            lease.revoked.store(true, Ordering::Release);
        }
        self.pending = None;
    }

    /// 旧页面的迟到取消不能清除用户刚拖入的新批次。
    pub(super) fn cancel(&mut self, batch_id: &str) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|lease| lease.id == batch_id)
        {
            self.clear();
            true
        } else {
            false
        }
    }

    fn consume(&mut self, batch_id: &str, scope: &str, now: Instant) -> Result<Batch, String> {
        self.current(scope, now).ok_or(INVALID_BATCH)?;
        if !self
            .pending
            .as_ref()
            .is_some_and(|batch| batch.lease.id == batch_id)
        {
            return Err(INVALID_BATCH.into());
        }
        // 先消耗令牌，再检查磁盘；失败同样不能重放。
        let batch = self.pending.take().ok_or(INVALID_BATCH)?;
        if let Err(error) = revalidate_files(&batch.files) {
            self.clear();
            return Err(error);
        }
        batch.lease.ensure_live(Instant::now())?;
        Ok(batch)
    }

    pub(super) fn take_registration(
        &mut self,
        batch_id: &str,
        scope: &str,
        now: Instant,
    ) -> Result<PathBuf, String> {
        let mut batch = self.consume(batch_id, scope, now)?;
        if batch.kind != DropKind::RegisterExecutable || batch.files.len() != 1 {
            self.clear();
            return Err("这批文件不能登记为应用，请重新拖入一个 .exe。".into());
        }
        Ok(batch.files.remove(0).path)
    }

    pub(super) fn take_files(
        &mut self,
        batch_id: &str,
        scope: &str,
        now: Instant,
    ) -> Result<FileDropSelection, String> {
        let batch = self.consume(batch_id, scope, now)?;
        if batch.kind != DropKind::OpenFiles {
            self.clear();
            return Err("拖入的 .exe 只能先登记，不能直接运行。".into());
        }
        Ok(FileDropSelection {
            files: batch.files,
            lease: batch.lease,
        })
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}

fn is_executable(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
}

fn visible_file_name(value: &str) -> String {
    value.chars().take(120).map(|character| {
        if character.is_control() || matches!(character,
            '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            '\u{fffd}'
        } else {
            character
        }
    }).collect()
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn validate_path_syntax(path: &Path) -> Result<(), String> {
    let text = path
        .to_str()
        .ok_or("文件路径不是有效 Unicode，无法安全展示。")?;
    if !path.is_absolute()
        || text.contains('\0')
        || text.chars().count() > MAX_PATH_CHARACTERS
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("请拖入路径完整的本机文件；不接受相对路径或上级目录跳转。".into());
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        let mut components = path.components();
        let drive = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
                _ => return Err("不接受网络共享、设备路径或网络映射盘。".into()),
            },
            _ => return Err("请拖入本机磁盘上的文件。".into()),
        };
        if components.any(|part| match part {
            Component::Normal(value) => value.to_str().is_none_or(|value| value.contains(':')),
            _ => false,
        }) {
            return Err("不接受文件的替代数据流路径。".into());
        }
        let root = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
        // GetDriveTypeW 只查询磁盘类型；0/1 是未知/不可用，4 是网络盘。
        let drive_type =
            unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
        if !matches!(drive_type, 2 | 3 | 5 | 6) {
            return Err("不接受网络共享、设备路径或网络映射盘。".into());
        }
    }
    Ok(())
}

fn validated_file(path: &Path) -> Result<(PathBuf, FileIdentity), String> {
    validate_path_syntax(path)?;
    for (index, ancestor) in path.ancestors().enumerate() {
        let metadata =
            fs::symlink_metadata(ancestor).map_err(|_| "拖入的文件已不存在或无法访问。")?;
        if is_reparse(&metadata)
            || if index == 0 {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            return Err("只接受普通文件；不接受目录、符号链接或经过重解析点的路径。".into());
        }
    }
    let canonical = path.canonicalize().map_err(|_| "无法解析拖入的文件。")?;
    validate_path_syntax(&canonical)?;
    let identity = read_identity(&canonical)?;
    Ok((canonical, identity))
}

#[cfg(windows)]
fn read_identity(path: &Path) -> Result<FileIdentity, String> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    // FILE_READ_ATTRIBUTES；FILE_FLAG_OPEN_REPARSE_POINT 防止最后一段跟随链接。
    let file = fs::OpenOptions::new()
        .read(true)
        .access_mode(0x80)
        .share_mode(7)
        .custom_flags(0x0020_0000)
        .open(path)
        .map_err(|_| "无法确认拖入文件的身份。")?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let success = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) };
    if success == 0
        || info.dwFileAttributes & (0x10 | 0x400) != 0
        || (info.nFileIndexHigh == 0 && info.nFileIndexLow == 0)
    {
        return Err("无法确认拖入文件的身份，或文件已经变成目录或链接。".into());
    }
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        length: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        last_write: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(info.ftLastWriteTime.dwLowDateTime),
    })
}

#[cfg(not(windows))]
fn read_identity(_path: &Path) -> Result<FileIdentity, String> {
    Err("桌宠文件拖放目前仅支持 Windows。".into())
}

fn capture_file(path: PathBuf) -> Result<CapturedFile, String> {
    let (path, identity) = validated_file(&path)?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("文件名称无法展示。")?;
    Ok(CapturedFile {
        name: visible_file_name(name),
        path,
        identity,
    })
}

fn revalidate_files(files: &[CapturedFile]) -> Result<(), String> {
    for file in files {
        let (path, identity) = validated_file(&file.path)?;
        if path_key(&path) != path_key(&file.path) || identity != file.identity {
            return Err("拖入后文件已被修改或替换，请重新拖入确认。".into());
        }
    }
    Ok(())
}

const PRIVATE_KEYS: &[&str] = &[
    "CODEX_HOME",
    "CODEX_ACCESS_TOKEN",
    "CODEX_API_KEY",
    "CODEX_APP_TOOLS_PIPE_PATH",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
    "CODEX_THREAD_ID",
    "CODEX_SESSION_ID",
    "CODEX_TOOL_CALL_ID",
    "CODEX_TASK_WORKSPACE_VERIFYING_IDENTITY",
];

fn remove_private_environment(command: &mut Command) {
    for key in PRIVATE_KEYS {
        command.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        if PRIVATE_KEYS
            .iter()
            .any(|blocked| key.to_string_lossy().eq_ignore_ascii_case(blocked))
        {
            command.env_remove(key);
        }
    }
}

fn prepare_file_command(target: &Path, selection: &FileDropSelection) -> Result<Command, String> {
    selection.lease.ensure_live(Instant::now())?;
    let (target, _) =
        validated_file(target).map_err(|_| "登记的应用已不存在或路径不可用，请重新选择。")?;
    if !is_executable(&target) || selection.files.is_empty() || selection.files.len() > MAX_FILES {
        return Err("请选择已经登记的本机 .exe 应用。".into());
    }
    revalidate_files(&selection.files)?;
    selection.lease.ensure_live(Instant::now())?;
    let mut command = Command::new(&target);
    command.args(selection.files.iter().map(|file| file.path.as_os_str()));
    if let Some(parent) = target.parent() {
        command.current_dir(parent);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    remove_private_environment(&mut command);
    Ok(command)
}

/// 目标必须来自 Companion::target；调用前还要检查当前可见性、停止状态和页面域。
pub(super) fn launch_files(target: &Path, selection: FileDropSelection) -> Result<(), String> {
    launch_files_if(target, selection, || true)
}

/// 原生拥有者在全部磁盘复验后、spawn 前读取最新的停止、暂停、域与窗口状态。
pub(super) fn launch_files_if(
    target: &Path,
    selection: FileDropSelection,
    allow: impl FnOnce() -> bool,
) -> Result<(), String> {
    let mut command = prepare_file_command(target, &selection)?;
    selection.lease.ensure_live(Instant::now())?;
    if !allow() {
        return Err("当前桌宠已隐藏、暂停或上下文发生变化，请重新拖入。".into());
    }
    command
        .spawn()
        .map(|child| drop(child))
        .map_err(|_| "未能把文件交给登记的应用，请检查应用支持的文件类型和系统权限。".into())
}

#[cfg(test)]
#[path = "desktop_drop_tests.rs"]
mod tests;
