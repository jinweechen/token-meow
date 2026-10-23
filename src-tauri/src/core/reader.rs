use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Context {
    pub owner: String,
    pub active_thread: String,
    pub current_turn: String,
    pub own_start: Option<u64>,
    pub model: String,
    pub parent: String,
    pub legacy: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub observed_settings: std::collections::BTreeMap<String, Value>,
    // 仅当前原生记录使用；不把文件位置变成可持久化或可由JSON声明的事实。
    #[serde(skip)]
    pub record_position: Option<(i64, i64, u64)>,
    #[serde(default)]
    pub notify_eligible: bool,
    #[serde(default)]
    pub runtime_start: String,
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub discard_offset: u64,
    #[serde(default)]
    pub caught_up: bool,
    // Optional JSON cursor is an additive migration of existing v3 file state.
    // The outer cursor always remains the recoverable historical position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_cursor: Option<Box<Cursor>>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub offset: u64,
    pub last_start: u64,
    pub last_hash: String,
    pub context: Context,
    #[serde(default)]
    pub bootstrap_end: Option<u64>,
    #[serde(default)]
    pub bootstrap_since: String,
}
impl Context {
    pub fn thread(&self, v: &Value) -> String {
        if self
            .own_start
            .map(|n| {
                v.get("ordinal")
                    .and_then(Value::as_u64)
                    .map(|x| x >= n)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
        {
            return self.owner.clone();
        }
        if self.active_thread.is_empty() {
            self.owner.clone()
        } else {
            self.active_thread.clone()
        }
    }
}

fn shared_open(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1 | 2 | 4);
    }
    options.open(path)
}

/// 新来源仅授权登记目录的原生对象；发现列表不是打开文件的授权凭据。
struct SourceRootGuard {
    path: PathBuf,
    canonical: PathBuf,
    handle: File,
    identity: String,
}
fn source_path_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "来源路径或原生文件身份已变化",
    )
}
fn source_is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
fn source_directory_chain(path: &Path) -> std::io::Result<()> {
    for ancestor in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || source_is_link(&metadata) {
            return Err(source_path_error());
        }
    }
    Ok(())
}
#[cfg(windows)]
fn source_final_path(file: &File) -> std::io::Result<PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut value = vec![0_u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            value.as_mut_ptr(),
            value.len() as u32,
            0,
        )
    };
    if length == 0 || length as usize >= value.len() {
        return Err(source_path_error());
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(
        &value[..length as usize],
    )))
}
fn source_paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        // ASCII 大小写等价；非 ASCII 大小写差异拒绝而不扩大权限匹配。
        let normalize = |path: &Path| {
            path.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .trim_end_matches('\\')
                .to_ascii_lowercase()
        };
        normalize(left) == normalize(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}
#[cfg(windows)]
fn source_open_directory(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .access_mode(0x80)
        .share_mode(7)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)
}
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn source_open_directory(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let (_, directory, _, _) = source_unix_flags();
    OpenOptions::new()
        .read(true)
        .custom_flags(directory)
        .open(path)
}
#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "macos"
)))]
fn source_open_directory(_path: &Path) -> std::io::Result<File> {
    Err(source_path_error())
}
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn source_unix_flags() -> (i32, i32, i32, i32) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        (0x20000, 0x20000 | 0x10000 | 0x80000, 0x80000, 0)
    }
    #[cfg(target_os = "macos")]
    {
        (0x100, 0x100 | 0x100000 | 0x1000000, 0x1000000, 0)
    }
}
impl SourceRootGuard {
    fn new(path: &Path) -> std::io::Result<Self> {
        use std::path::Component;
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err(source_path_error());
        }
        #[cfg(windows)]
        {
            use std::path::Prefix;
            if !matches!(path.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_)))
            {
                return Err(source_path_error());
            }
        }
        source_directory_chain(path)?;
        let canonical = fs::canonicalize(path)?;
        if !source_paths_equal(path, &canonical) {
            return Err(source_path_error());
        }
        let handle = source_open_directory(path)?;
        if source_is_link(&handle.metadata()?) || !handle.metadata()?.is_dir() {
            return Err(source_path_error());
        }
        #[cfg(windows)]
        if !source_paths_equal(&source_final_path(&handle)?, &canonical) {
            return Err(source_path_error());
        }
        let identity = file_key(&handle);
        if identity.is_empty() {
            return Err(source_path_error());
        }
        let guard = Self {
            path: path.to_path_buf(),
            canonical,
            handle,
            identity,
        };
        guard.revalidate()?;
        Ok(guard)
    }
    fn revalidate(&self) -> std::io::Result<()> {
        source_directory_chain(&self.path)?;
        let current = source_open_directory(&self.path)?;
        if source_is_link(&current.metadata()?)
            || file_key(&current) != self.identity
            || file_key(&self.handle) != self.identity
        {
            return Err(source_path_error());
        }
        if !source_paths_equal(&fs::canonicalize(&self.path)?, &self.canonical) {
            return Err(source_path_error());
        }
        #[cfg(windows)]
        if !source_paths_equal(&source_final_path(&self.handle)?, &self.canonical)
            || !source_paths_equal(&source_final_path(&current)?, &self.canonical)
        {
            return Err(source_path_error());
        }
        Ok(())
    }
    fn open(&self, path: &Path) -> std::io::Result<File> {
        use std::path::Component;
        let relative = path
            .strip_prefix(&self.path)
            .map_err(|_| source_path_error())?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(source_path_error());
        }
        self.revalidate()?;
        source_directory_chain(path.parent().ok_or_else(source_path_error)?)?;
        let file = self.open_relative(relative, path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || source_is_link(&metadata) {
            return Err(source_path_error());
        }
        #[cfg(windows)]
        if !source_paths_equal(&source_final_path(&file)?, &self.canonical.join(relative)) {
            return Err(source_path_error());
        }
        let path_metadata = fs::symlink_metadata(path)?;
        if !path_metadata.is_file() || source_is_link(&path_metadata) {
            return Err(source_path_error());
        }
        source_directory_chain(path.parent().ok_or_else(source_path_error)?)?;
        self.revalidate()?;
        Ok(file)
    }
    fn validate_read(&self, file: &File, path: &Path) -> std::io::Result<()> {
        let current = self.open(path)?;
        if file_key(&current) != file_key(file) {
            return Err(source_path_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    fn open_relative(&self, _relative: &Path, path: &Path) -> std::io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .share_mode(7)
            .custom_flags(0x0020_0000)
            .open(path)
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    fn open_relative(&self, relative: &Path, _path: &Path) -> std::io::Result<File> {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        };
        unsafe extern "C" {
            fn openat(dirfd: i32, path: *const std::ffi::c_char, flags: i32, ...) -> i32;
        }
        let (nofollow, directory, cloexec, read) = source_unix_flags();
        let parts: Vec<_> = relative.components().collect();
        let mut held = Vec::<File>::new();
        for (index, part) in parts.iter().enumerate() {
            let name = std::ffi::CString::new(part.as_os_str().as_bytes())
                .map_err(|_| source_path_error())?;
            let parent = held.last().unwrap_or(&self.handle).as_raw_fd();
            let flags = if index + 1 == parts.len() {
                read | nofollow | cloexec
            } else {
                read | directory
            };
            let raw = unsafe { openat(parent, name.as_ptr(), flags) };
            if raw < 0 {
                return Err(std::io::Error::last_os_error());
            }
            held.push(unsafe { File::from_raw_fd(raw) });
        }
        let file = held.pop().ok_or_else(source_path_error)?;
        use std::os::unix::fs::MetadataExt;
        let actual = file.metadata()?;
        let expected = fs::symlink_metadata(self.path.join(relative))?;
        if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            return Err(source_path_error());
        }
        Ok(file)
    }
    #[cfg(not(any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "macos"
    )))]
    fn open_relative(&self, _relative: &Path, _path: &Path) -> std::io::Result<File> {
        Err(source_path_error())
    }
}
fn same_anchor(file: &mut File, start: u64, expected: &str) -> bool {
    if expected.is_empty() {
        return true;
    }
    if file.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut r = BufReader::new(file);
    limited_line(&mut r, 4 * 1024 * 1024, &AtomicBool::new(false), None)
        .map(|line| line.complete && line.digest == expected)
        .unwrap_or(false)
}
struct Line {
    bytes: Vec<u8>,
    total: usize,
    complete: bool,
    digest: String,
    oversize: bool,
}
fn limited_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
    cancel: &AtomicBool,
    started: Option<&std::time::Instant>,
) -> std::io::Result<Line> {
    let mut bytes = Vec::new();
    let mut total = 0usize;
    let mut digest = Sha256::new();
    let mut complete = false;
    loop {
        if cancel.load(Ordering::Relaxed)
            || started.is_some_and(|s| s.elapsed() > std::time::Duration::from_millis(250))
        {
            break;
        }
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let allowed = (4 * 1024 * 1024 + 1usize).saturating_sub(total);
        if allowed == 0 {
            break;
        }
        let n = available
            .iter()
            .position(|c| *c == b'\n')
            .map(|p| p + 1)
            .unwrap_or(available.len())
            .min(allowed);
        let end = available[n - 1] == b'\n';
        digest.update(&available[..n]);
        let copy = n.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&available[..copy]);
        reader.consume(n);
        total += n;
        if end {
            complete = true;
            break;
        }
    }
    Ok(Line {
        bytes,
        total,
        complete,
        digest: format!("{:x}", digest.finalize()),
        oversize: limit > 0 && total > limit,
    })
}

fn file_key(file: &File) -> String {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } != 0 {
            return format!(
                "win:{}:{}:{}",
                info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(m) = file.metadata() {
            return format!("unix:{}:{}", m.dev(), m.ino());
        }
    }
    String::new()
}
fn make_live_cursor(
    file: &mut File,
    len: u64,
    historical: &Context,
    new_runtime_file: bool,
    runtime_start: &str,
) -> Result<Cursor> {
    let mut context = Context {
        owner: historical.owner.clone(),
        active_thread: historical.owner.clone(),
        parent: historical.parent.clone(),
        own_start: historical.own_start,
        ..Context::default()
    };
    if new_runtime_file {
        return Ok(Cursor {
            context,
            ..Cursor::default()
        });
    }
    // A bounded metadata bootstrap also catches runtime events written before
    // this old file received its first fair scheduling turn. It never advances
    // the historical cursor or stores old usage/lifecycle facts.
    let window_start = len.saturating_sub(2 * 1024 * 1024);
    file.seek(SeekFrom::Start(window_start)).map_err(err)?;
    let mut window = vec![0; (len - window_start) as usize];
    file.read_exact(&mut window).map_err(err)?;
    let offset = if window_start == 0 {
        0
    } else {
        window
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| window_start + n as u64 + 1)
            .unwrap_or(len)
    };
    if window_start > 0 && !window.contains(&b'\n') {
        // We started inside an oversized/unfinished line. Discard its suffix,
        // rather than parsing a fragment as an independent source observation.
        context.discard_offset = len;
    }
    Ok(Cursor {
        offset,
        context,
        bootstrap_end: Some(len),
        bootstrap_since: runtime_start.into(),
        ..Cursor::default()
    })
}

fn report_reader_issue(
    db: &Connection,
    report: &mut ScanReport,
    new_reader_issues: &mut usize,
    kind: &str,
    file: &str,
    digest: &str,
    message: &str,
) -> Result<()> {
    issue(db, kind, file, digest, message)?;
    // Capture the INSERT OR IGNORE result before another statement changes it.
    // Repeated reader diagnoses still count as observations in this scan, but
    // only actual inserted rows overlap the batch's new-diagnosis SQL count.
    let inserted = usize::try_from(db.changes()).map_err(err)?;
    *new_reader_issues = new_reader_issues
        .checked_add(inserted)
        .ok_or("新增读取诊断计数超出整数范围")?;
    report.issues = report
        .issues
        .checked_add(1)
        .ok_or("读取诊断计数超出整数范围")?;
    Ok(())
}

const SNAPSHOT_CONTEXT_KEY: &str = "foreign.snapshot.v1";
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotProgress {
    fingerprint: String,
    admission_key: String,
    // 下界收紧只影响尚未读取的尾部；扩大范围才需要重新遍历已跳过的前缀。
    // 旧进度没有此字段，准入指纹的版本变化会让它保守重建一次。
    #[serde(default)]
    admission_cutoff: Option<String>,
    next_record: usize,
    history_gap: bool,
}
fn snapshot_cutoff_expanded(
    previous: Option<&str>,
    current: Option<&chrono::DateTime<Utc>>,
) -> bool {
    match previous {
        None => false,
        Some(previous) => chrono::DateTime::parse_from_rfc3339(previous)
            .ok()
            .map(|previous| current.is_none_or(|current| *current < previous.with_timezone(&Utc)))
            .unwrap_or(true),
    }
}
struct SnapshotRead<'a> {
    file: &'a mut File,
    remaining: usize,
    consumed: usize,
    cancel: &'a AtomicBool,
    started: &'a std::time::Instant,
    interrupted: bool,
}
impl Read for SnapshotRead<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed)
            || self.started.elapsed() > std::time::Duration::from_millis(250)
        {
            self.interrupted = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "快照读取批次已结束",
            ));
        }
        let limit = buffer.len().min(self.remaining).min(64 * 1024);
        if limit == 0 {
            return Ok(0);
        }
        let read = self.file.read(&mut buffer[..limit])?;
        self.remaining -= read;
        self.consumed += read;
        Ok(read)
    }
}
#[allow(clippy::too_many_arguments)]
fn consume_snapshot(
    db: &Connection,
    adapter: &dyn SourceAdapter,
    source: &str,
    path: &str,
    file: &mut File,
    length: u64,
    history_enabled: bool,
    cutoff: Option<&chrono::DateTime<Utc>>,
    live_start: &str,
    cancel: &AtomicBool,
    started: &std::time::Instant,
    budget: &mut usize,
    report: &mut ScanReport,
    new_reader_issues: &mut usize,
) -> Result<()> {
    if length > 16 * 1024 * 1024 {
        report_reader_issue(
            db,
            report,
            new_reader_issues,
            "snapshot_too_large",
            path,
            "snapshot-limit-v1",
            "整体 JSON 快照超过 16 MiB 上限，未导入且未保存正文",
        )?;
        report.pending_files += 1;
        return Ok(());
    }
    if length as usize > *budget {
        report.pending_files += 1;
        return Ok(());
    }
    let before = file.metadata().map_err(err)?.modified().ok();
    file.seek(SeekFrom::Start(0)).map_err(err)?;
    let mut bounded = SnapshotRead {
        file,
        remaining: *budget,
        consumed: 0,
        cancel,
        started,
        interrupted: false,
    };
    let parsed = adapter.snapshot_records(&mut bounded);
    *budget = budget.saturating_sub(bounded.consumed);
    let bytes = bounded.consumed;
    let interrupted = bounded.interrupted;
    if interrupted
        || cancel.load(Ordering::Relaxed)
        || started.elapsed() > std::time::Duration::from_millis(250)
    {
        report.pending_files += 1;
        return Ok(());
    }
    let records = match parsed {
        Ok(records) if records.len() <= 8193 => records,
        _ => {
            report_reader_issue(
                db,
                report,
                new_reader_issues,
                "invalid_snapshot",
                path,
                "snapshot-schema-v1",
                "整体 JSON 快照不是支持的完整白名单格式，未保存原文，下轮重试",
            )?;
            report.pending_files += 1;
            return Ok(());
        }
    };
    let after = bounded.file.metadata().map_err(err)?;
    if bytes as u64 != length || after.len() != length || before != after.modified().ok() {
        report.pending_files += 1;
        return Ok(());
    }
    let physical_key = file_key(bounded.file);
    drop(bounded);
    // 只有已投影的原生元数据参与 fingerprint；正文改动不会改写计量身份或制造新代次。
    let fingerprint = hash(&serde_json::to_vec(&records).map_err(err)?);
    // retentionWatermark 会随扫描时钟前进，不能作为整份快照的重播身份。
    // 显式补导、清除、来源配置和隐私变化仍各自构成准入围栏。
    let source_binding: Value = db.query_row(
        "SELECT root,client_kind,enabled,environment,configuration_revision FROM source_instances WHERE id=?1",
        [source],
        |row| Ok(json!([row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,bool>(2)?,row.get::<_,String>(3)?,row.get::<_,i64>(4)?.to_string()])),
    ).map_err(err)?;
    let admission_key=hash(json!({"version":2,"historyEnabled":history_enabled,"liveStart":live_start,
        "importSince":get_setting(db,"importSince")?,"clearWatermark":get_setting(db,"clearWatermark")?,
        "historyReplayAt":get_setting(db,"historyReplayAt")?,"sourceBinding":source_binding,
        "privacyMode":get_setting(db,"preferences")?.and_then(|value|value["privacyMode"].as_bool()).unwrap_or(false),
        "historyPaused":get_setting(db,"historyPaused")?,"historyCancelled":get_setting(db,"historyCancelled")?}).to_string().as_bytes());
    let existing: Option<(i64, i64, String)> = db
        .query_row(
            "SELECT id,generation,context FROM source_files WHERE path=?1 AND source_id=?2",
            params![path, source],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(err)?;
    let (id, mut generation, mut context) = if let Some((id, generation, context)) = existing {
        (
            id,
            generation,
            serde_json::from_str::<Context>(&context).unwrap_or_default(),
        )
    } else {
        db.execute("INSERT INTO source_files(path,fingerprint,source_id,channel_id,os_file_key) VALUES(?1,?2,?3,?4,?5)",
            params![path,fingerprint,source,format!("{source}:jsonl"),physical_key]).map_err(err)?;
        (db.last_insert_rowid(), 0, Context::default())
    };
    let mut progress = context
        .legacy
        .get(SNAPSHOT_CONTEXT_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value::<SnapshotProgress>(value).ok())
        .unwrap_or_default();
    if progress.fingerprint != fingerprint
        || progress.admission_key != admission_key
        || snapshot_cutoff_expanded(progress.admission_cutoff.as_deref(), cutoff)
        || progress.next_record > records.len()
    {
        generation = generation.checked_add(1).ok_or("快照读取代次已耗尽")?;
        context = Context::default();
        progress = SnapshotProgress {
            fingerprint: fingerprint.clone(),
            admission_key,
            admission_cutoff: cutoff.map(|value| value.to_rfc3339()),
            next_record: 0,
            history_gap: false,
        };
    }
    progress.admission_cutoff = cutoff.map(|value| value.to_rfc3339());
    context.notify_eligible = false;
    context.live_cursor = None;
    let live_time = chrono::DateTime::parse_from_rfc3339(live_start)
        .ok()
        .map(|value| value.with_timezone(&Utc));
    // header 即使不含计量也先恢复原生 session 上下文；不能从文件名补身份。
    if let Some(header) = records.first() {
        adapter.advance_local_context(
            &super::adapter::Observation {
                source_id: source.into(),
                channel_id: format!("{source}:jsonl"),
                event: header.clone(),
            },
            &mut context,
        );
    }
    while progress.next_record < records.len() {
        if cancel.load(Ordering::Relaxed)
            || started.elapsed() > std::time::Duration::from_millis(250)
        {
            break;
        }
        let index = progress.next_record;
        let observation = super::adapter::Observation {
            source_id: source.into(),
            channel_id: format!("{source}:jsonl"),
            event: records[index].clone(),
        };
        let native_time = observation
            .event
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        let before_cutoff = native_time
            .as_ref()
            .zip(cutoff)
            .is_some_and(|(time, cutoff)| time < cutoff);
        let old_history = !history_enabled
            && native_time
                .as_ref()
                .zip(live_time.as_ref())
                .is_some_and(|(time, live)| time < live);
        if before_cutoff || old_history {
            adapter.advance_local_context(&observation, &mut context);
            if old_history && !before_cutoff {
                progress.history_gap = true;
            }
        } else {
            let digest = hash(&serde_json::to_vec(&observation.event).map_err(err)?);
            let inserted=db.execute("INSERT OR IGNORE INTO ingested_records(file_id,generation,offset,kind,digest) VALUES(?1,?2,?3,?4,?5)",
                params![id,generation,index as i64,strv(&observation.event,"type"),digest]).map_err(err)?;
            report.new_responses += if inserted == 0 {
                adapter.ingest_replayed_local(db, &observation, path, &mut context, cutoff)?
            } else {
                adapter.ingest_local(db, &observation, path, &mut context, cutoff)?
            };
            if let Some(key) = adapter.fact_reference_with_context(&observation, &context) {
                db.execute("UPDATE ingested_records SET fact_id=?1 WHERE file_id=?2 AND generation=?3 AND offset=?4",params![key,id,generation,index as i64]).map_err(err)?;
            }
        }
        progress.next_record += 1;
        report.lines += 1;
    }
    let complete = progress.next_record == records.len();
    if cancel.load(Ordering::Relaxed) {
        return Err("整体 JSON 快照读取已取消，事务未提交".into());
    }
    context.caught_up = complete && !progress.history_gap;
    if !context.caught_up {
        report.pending_files += 1;
    }
    context.legacy.insert(
        SNAPSHOT_CONTEXT_KEY.into(),
        serde_json::to_value(&progress).map_err(err)?,
    );
    db.execute("UPDATE source_files SET fingerprint=?1,generation=?2,offset=?3,last_start=0,last_hash=?1,context=?4,os_file_key=?5 WHERE id=?6",
        params![fingerprint,generation,if complete {length as i64} else {0},serde_json::to_string(&context).map_err(err)?,physical_key,id]).map_err(err)?;
    report.files += 1;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn consume_cursor(
    db: &Connection,
    adapter: &dyn SourceAdapter,
    source: &str,
    path: &str,
    file: &mut File,
    file_id: i64,
    generation: i64,
    cursor: &mut Cursor,
    cutoff: Option<&chrono::DateTime<Utc>>,
    live: bool,
    live_start: &str,
    runtime_id: &str,
    cancel: &AtomicBool,
    started: &std::time::Instant,
    byte_budget: usize,
    report: &mut ScanReport,
    new_reader_issues: &mut usize,
) -> Result<usize> {
    cursor.context.notify_eligible = live;
    cursor.context.runtime_start = live_start.into();
    cursor.context.runtime_id = runtime_id.into();
    file.seek(SeekFrom::Start(
        cursor.context.discard_offset.max(cursor.offset),
    ))
    .map_err(err)?;
    let mut reader = BufReader::new(file);
    let mut bytes = 0;
    let file_started = std::time::Instant::now();
    loop {
        let line = limited_line(
            &mut reader,
            if cursor.context.discard_offset > 0 {
                0
            } else {
                4 * 1024 * 1024
            },
            cancel,
            Some(started),
        )
        .map_err(err)?;
        let read = line.total;
        if read == 0 {
            cursor.context.caught_up = true;
            break;
        }
        if !line.complete {
            report.pending_files += 1;
            if line.oversize || cursor.context.discard_offset > 0 {
                cursor.context.discard_offset =
                    cursor.context.discard_offset.max(cursor.offset) + read as u64;
                report_reader_issue(
                    db,
                    report,
                    new_reader_issues,
                    "record_too_large",
                    path,
                    &hash(format!("{file_id}:{generation}:{}", cursor.offset).as_bytes()),
                    "记录超过 4 MiB 安全上限，分批跳过并等待行结束；覆盖存在缺口",
                )?;
            }
            break;
        }
        let start = cursor.offset;
        let digest = line.digest;
        let oversize = line.oversize || cursor.context.discard_offset > 0;
        if oversize {
            report_reader_issue(
                db,
                report,
                new_reader_issues,
                "record_too_large",
                path,
                &digest,
                "完整行超过 4 MiB 安全上限，已跳过并标记覆盖缺口",
            )?;
        } else {
            match adapter.parse_line(&line.bytes) {
                Ok(event) => {
                    let observation = super::adapter::Observation {
                        source_id: source.into(),
                        channel_id: format!("{source}:jsonl"),
                        event,
                    };
                    cursor.context.record_position = Some((file_id, generation, start));
                    let bootstrap_old = cursor.bootstrap_end.is_some_and(|end| start < end)
                        && observation
                            .event
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .zip(chrono::DateTime::parse_from_rfc3339(&cursor.bootstrap_since).ok())
                            .is_some_and(|(time, since)| time < since);
                    if bootstrap_old {
                        adapter.prime_local_metadata(
                            db,
                            &observation,
                            path,
                            &mut cursor.context,
                        )?;
                        adapter.advance_local_context(&observation, &mut cursor.context);
                    } else {
                        let inserted=db.execute("INSERT OR IGNORE INTO ingested_records(file_id,generation,offset,kind,digest) VALUES(?1,?2,?3,?4,?5)",params![file_id,generation,start as i64,strv(&observation.event,"type"),digest]).map_err(err)?;
                        report.new_responses += if inserted == 0 {
                            adapter.ingest_replayed_local(
                                db,
                                &observation,
                                path,
                                &mut cursor.context,
                                cutoff,
                            )?
                        } else {
                            adapter.ingest_local(
                                db,
                                &observation,
                                path,
                                &mut cursor.context,
                                cutoff,
                            )?
                        };
                        if let Some(key) =
                            adapter.fact_reference_with_context(&observation, &cursor.context)
                        {
                            db.execute("UPDATE ingested_records SET fact_id=?1 WHERE file_id=?2 AND generation=?3 AND offset=?4",params![key,file_id,generation,start as i64]).map_err(err)?;
                        }
                    }
                    cursor.context.record_position = None;
                }
                Err(_) => {
                    report_reader_issue(
                        db,
                        report,
                        new_reader_issues,
                        "invalid_json",
                        path,
                        &digest,
                        "完整行不是有效 UTF-8 JSON；未保存原文，其余行继续采集",
                    )?;
                }
            }
        }
        cursor.offset = cursor.context.discard_offset.max(cursor.offset) + read as u64;
        cursor.context.discard_offset = 0;
        cursor.last_start = start;
        cursor.last_hash = if oversize { String::new() } else { digest };
        if cursor.bootstrap_end.is_some_and(|end| cursor.offset >= end) {
            cursor.bootstrap_end = None;
            cursor.bootstrap_since.clear();
        }
        report.lines += 1;
        bytes += read;
        if bytes >= 2 * 1024 * 1024
            || bytes >= byte_budget
            || cancel.load(Ordering::Relaxed)
            || file_started.elapsed() > std::time::Duration::from_millis(50)
            || started.elapsed() > std::time::Duration::from_millis(250)
        {
            report.pending_files += 1;
            break;
        }
    }
    Ok(bytes)
}
pub(super) fn scan(
    core: &mut Core,
    cancel: &AtomicBool,
    observation: Option<super::queries::budget::Scan>,
) -> Result<ScanReport> {
    let subscription_scan = core.begin_subscription_scan()?;
    let started = std::time::Instant::now();
    let mut rows = core.scan_source_bindings()?;
    let source_pos = get_setting(&core.db, "sourceRoundRobin")?
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    if !rows.is_empty() {
        let n = rows.len();
        rows.rotate_left(source_pos % n);
    }
    let mut combined = ScanReport::default();
    let source_count = rows.len();
    let mut attempted = 0;
    let mut committed_sources = std::collections::BTreeSet::new();
    for (source, root, client_kind) in rows {
        if cancel.load(Ordering::Relaxed)
            || started.elapsed() > std::time::Duration::from_millis(250)
        {
            break;
        }
        attempted += 1;
        match scan_source(core, &source, Path::new(&root), &client_kind, cancel) {
            Ok(r) => {
                // scan_source returns only after its fact + live cursor transaction commits.
                committed_sources.insert(source.clone());
                combined.files += r.files;
                combined.lines += r.lines;
                combined.new_responses += r.new_responses;
                combined.pending_files += r.pending_files;
                combined.issues += r.issues;
            }
            Err(_) => {
                issue(
                    &core.db,
                    "source_scan_failed",
                    &root,
                    &source,
                    "来源扫描失败，其他来源继续；下轮独立重试",
                )?;
                combined.issues += 1;
            }
        }
    }
    put_setting(
        &core.db,
        "sourceRoundRobin",
        &json!(if source_count == 0 {
            0
        } else {
            (source_pos + attempted.max(1)) % source_count
        }),
    )?;
    if !cancel.load(Ordering::Relaxed) {
        if combined.issues == 0 && attempted == source_count {
            put_setting(&core.db, "lastSuccessfulScan", &json!(now()))?;
        }
        put_setting(&core.db, "pendingFiles", &json!(combined.pending_files))?;
        super::queries::budget::finish(core, observation, &committed_sources, false)?;
        super::queries::apply_retention(core)?;
        core.finish_subscription_scan(subscription_scan, &committed_sources, false)?;
    } else {
        super::queries::budget::finish(core, observation, &committed_sources, true)?;
        core.finish_subscription_scan(subscription_scan, &committed_sources, true)?;
    }
    Ok(combined)
}
fn scan_source(
    core: &mut Core,
    source_id: &str,
    source_root: &Path,
    client_kind: &str,
    cancel: &AtomicBool,
) -> Result<ScanReport> {
    let adapter = super::adapter::adapter_for(client_kind).ok_or("来源适配器未注册")?;
    if !source_root.exists() {
        issue(
            &core.db,
            "missing_source",
            &source_root.to_string_lossy(),
            source_id,
            "工具日志目录不存在，保留已有账本",
        )?;
        return Err("missing source".into());
    }
    let config = super::adapter::SourceConfig {
        id: source_id.into(),
        root: source_root.to_path_buf(),
        environment: "windows-local".into(),
    };
    let guard = if client_kind == "codex" {
        None
    } else {
        Some(
            SourceRootGuard::new(source_root)
                .map_err(|_| "来源根目录不是稳定的普通本地目录".to_owned())?,
        )
    };
    let discovered = adapter.discover_with_cancel(&config, cancel)?;
    scan_discovered_source_guarded(core, source_id, adapter, discovered, cancel, guard.as_ref())
}
#[cfg(test)]
fn scan_discovered_source(
    core: &mut Core,
    source_id: &str,
    adapter: &dyn SourceAdapter,
    discovered: super::adapter::Discovery,
    cancel: &AtomicBool,
) -> Result<ScanReport> {
    scan_discovered_source_guarded(core, source_id, adapter, discovered, cancel, None)
}
fn scan_discovered_source_guarded(
    core: &mut Core,
    source_id: &str,
    adapter: &dyn SourceAdapter,
    mut discovered: super::adapter::Discovery,
    cancel: &AtomicBool,
    guard: Option<&SourceRootGuard>,
) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let mut files = std::mem::take(&mut discovered.files);
    let unsupported = discovered.unsupported;
    files.sort_by_cached_key(|p| {
        let modified = if cancel.load(Ordering::Relaxed) {
            discovered.cancelled = true;
            SystemTime::UNIX_EPOCH
        } else {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or_else(|_| {
                    discovered.errors.push(p.clone());
                    SystemTime::UNIX_EPOCH
                })
        };
        std::cmp::Reverse(modified)
    });
    discovered.errors.sort();
    discovered.errors.dedup();
    let discovery_errors = discovered.errors.len();
    let discovery_complete = discovery_errors == 0 && !discovered.cancelled;
    // Discovery/stat can be slower than the scheduler's entire 250 ms quantum.
    // Start a separate bounded read quantum only after that work, otherwise a
    // large source can finish discovery and then read zero bytes on every scan.
    let started = &std::time::Instant::now();
    // Real-time candidates retain priority; a persistent cursor gives every historical file a turn.
    let cursor_key = format!("fileRoundRobin:{source_id}");
    let cursor = get_setting(&core.db, &cursor_key)?
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let phase_key = format!("fileScanPhase:{source_id}");
    let phase = get_setting(&core.db, &phase_key)?
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_files = files.len();
    let discovered_paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let mut queue: Vec<(usize, PathBuf, bool)> = files
        .into_iter()
        .enumerate()
        .map(|(index, path)| (index, path, false))
        .collect();
    if total_files > 0 {
        queue.rotate_left(cursor % total_files);
    }
    if phase % 2 == 0 {
        if let Some(index) = queue.iter().position(|(index, _, _)| *index == 0) {
            let mut first = queue.remove(index);
            first.2 = true;
            queue.insert(0, first);
        }
    }
    let mut visited = 0usize;
    let mut next_cursor = cursor;
    let paused = get_setting(&core.db, "historyPaused")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let cancelled = get_setting(&core.db, "historyCancelled")?
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let history_enabled = !paused && !cancelled;
    let cutoff_time = ["importSince", "clearWatermark", "retentionWatermark"]
        .iter()
        .filter_map(|k| {
            get_setting(&core.db, k)
                .ok()
                .flatten()
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                })
                .map(|d| d.with_timezone(&Utc))
        })
        .max();
    let mut budget_bytes = 16 * 1024 * 1024usize;
    let registered_at: String = core
        .db
        .query_row(
            "SELECT created_at FROM source_instances WHERE id=?1",
            [source_id],
            |r| r.get(0),
        )
        .map_err(err)?;
    let replay_at = get_setting(&core.db, "historyReplayAt")?
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let live_start = [core.runtime_start.clone(), registered_at, replay_at]
        .into_iter()
        .max()
        .unwrap();
    // One bounded source batch is one atomic commit. Per-file commits caused an fsync storm
    // during history import and consumed the scheduling budget before older files were visited.
    let tx = core.db.unchecked_transaction().map_err(err)?;
    let previous_issue_row: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(rowid),0) FROM ingestion_issues",
            [],
            |row| row.get(0),
        )
        .map_err(err)?;
    let mut new_reader_issues = 0usize;
    for path in &discovered.errors {
        let path_text = path.to_string_lossy();
        report_reader_issue(
            &tx,
            &mut report,
            &mut new_reader_issues,
            "source_discovery_failed",
            &path_text,
            source_id,
            "部分目录或文件元数据暂不可读，可读文件继续采集；发现覆盖不完整，下轮重试",
        )?;
    }
    for (position, path, priority) in queue {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if started.elapsed() > std::time::Duration::from_millis(250) {
            break;
        }
        if !priority {
            next_cursor = position + 1;
        }
        visited += 1;
        if budget_bytes == 0 {
            report.pending_files += 1;
            continue;
        }
        let path_text = path.to_string_lossy().to_string();
        let opened = match guard {
            Some(guard) => guard.open(&path),
            None if adapter.client_kind() == "codex" => shared_open(&path),
            None => Err(source_path_error()),
        };
        let mut file = match opened {
            Ok(f) => f,
            Err(_) => {
                report_reader_issue(
                    &tx,
                    &mut report,
                    &mut new_reader_issues,
                    "source_locked",
                    &path_text,
                    "",
                    "暂时无法共享读取，将于下轮重试",
                )?;
                report.pending_files += 1;
                continue;
            }
        };
        let len = file.metadata().map_err(err)?.len();
        if adapter.is_json_snapshot(&path) {
            consume_snapshot(
                &tx,
                adapter,
                source_id,
                &path_text,
                &mut file,
                len,
                history_enabled,
                cutoff_time.as_ref(),
                &live_start,
                cancel,
                started,
                &mut budget_bytes,
                &mut report,
                &mut new_reader_issues,
            )?;
            if let Some(guard) = guard {
                guard
                    .validate_read(&file, &path)
                    .map_err(|_| "来源路径在快照读取期间改变，事务未提交".to_owned())?;
            }
            #[cfg(test)]
            if get_setting(&tx, "testFailBeforeCommit")?.and_then(|value| value.as_bool())
                == Some(true)
            {
                return Err("injected-before-commit".into());
            }
            continue;
        }
        let created_during_runtime = file
            .metadata()
            .and_then(|m| m.created())
            .ok()
            .map(|t| {
                let d: chrono::DateTime<Utc> = t.into();
                d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true) > live_start
            })
            .unwrap_or(false);
        let physical_key = file_key(&file);
        let mut prefix = [0u8; 4096];
        let n = file.read(&mut prefix).map_err(err)?;
        if n == 0 {
            continue;
        }
        let fingerprint = hash(&prefix[..n]);
        let existing:Option<(i64,i64,u64,u64,String,String)>=core.db.query_row("SELECT id,generation,offset,last_start,last_hash,context FROM source_files WHERE path=?1 AND source_id=?2",params![path_text,source_id],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,i64>(2)?.max(0) as u64,r.get::<_,i64>(3)?.max(0) as u64,r.get(4)?,r.get(5)?))).optional().map_err(err)?;
        if let Some((id, _, offset, start, anchor, _)) = &existing {
            let old_key: String = core
                .db
                .query_row(
                    "SELECT os_file_key FROM source_files WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(err)?;
            // v1 rows have no OS identity. Adopt it only when a committed, nonempty
            // anchor proves continuity; an empty key alone cannot identify a replaced file.
            let legacy_continuity = old_key.is_empty()
                && *offset > 0
                && *offset <= len
                && *start < *offset
                && !anchor.is_empty()
                && same_anchor(&mut file, *start, anchor);
            if !physical_key.is_empty()
                && ((!old_key.is_empty() && old_key != physical_key)
                    || (old_key.is_empty() && !legacy_continuity))
            {
                core.db
                    .execute(
                        "UPDATE source_files SET path=path||'#replaced-'||id WHERE id=?1",
                        [id],
                    )
                    .map_err(err)?;
            } else if legacy_continuity && !physical_key.is_empty() {
                tx.execute(
                    "UPDATE source_files SET os_file_key=?1 WHERE id=?2 AND os_file_key=''",
                    params![physical_key, id],
                )
                .map_err(err)?;
            }
        }
        let existing = if let Some(ref state) = existing {
            let stored: String = core
                .db
                .query_row(
                    "SELECT path FROM source_files WHERE id=?1",
                    [state.0],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if stored == path_text {
                existing
            } else {
                None
            }
        } else {
            None
        };
        if let Some((_, _, offset, start, anchor, context)) = &existing {
            let mut context: Context = serde_json::from_str(context).unwrap_or_default();
            if *offset == len
                && context.discard_offset == 0
                && same_anchor(&mut file, *start, anchor)
                && context
                    .live_cursor
                    .as_ref()
                    .map(|live| live.offset == len && live.context.discard_offset == 0)
                    .unwrap_or(true)
            {
                if !context.caught_up || context.live_cursor.is_none() {
                    context.caught_up = true;
                    if context.live_cursor.is_none() {
                        context.live_cursor = Some(Box::new(Cursor {
                            offset: *offset,
                            last_start: *start,
                            last_hash: anchor.clone(),
                            context: context.clone(),
                            ..Cursor::default()
                        }));
                    }
                    tx.execute(
                        "UPDATE source_files SET context=?1 WHERE path=?2",
                        params![serde_json::to_string(&context).map_err(err)?, path_text],
                    )
                    .map_err(err)?;
                }
                report.files += 1;
                continue;
            }
        }
        // Modification time only influences queue priority. It never excludes an unread file.
        let mut state = existing;
        if state.is_none() {
            // Preserve cursor when a known file moved into the archive; a live copy gets its own physical identity.
            let mut s=tx.prepare("SELECT id,path,generation,offset,last_start,last_hash,context FROM source_files WHERE os_file_key=?1 AND source_id=?2 AND os_file_key<>''").map_err(err)?;
            let candidates = s
                .query_map(params![physical_key, source_id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?.max(0) as u64,
                        r.get::<_, i64>(4)?.max(0) as u64,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            drop(s);
            for (id, old, generation, offset, start, anchor, context) in candidates {
                if !Path::new(&old).exists()
                    && offset <= len
                    && same_anchor(&mut file, start, &anchor)
                {
                    tx.execute(
                        "UPDATE source_files SET path=?1 WHERE id=?2",
                        params![path_text, id],
                    )
                    .map_err(err)?;
                    state = Some((id, generation, offset, start, anchor, context));
                    break;
                }
            }
        }
        if state.is_none() {
            tx.execute("INSERT INTO source_files(path,fingerprint,source_id,channel_id,os_file_key) VALUES(?1,?2,?3,?4,?5)",params![path_text,fingerprint,source_id,format!("{source_id}:jsonl"),physical_key]).map_err(err)?;
            state = Some((tx.last_insert_rowid(), 0, 0, 0, String::new(), "{}".into()));
        }
        let (id, mut generation, mut offset, mut last_start, mut last_hash, context) =
            state.unwrap();
        let mut ctx: Context = serde_json::from_str(&context).unwrap_or_default();
        if offset > len || !same_anchor(&mut file, last_start, &last_hash) {
            generation += 1;
            offset = 0;
            last_start = 0;
            last_hash.clear();
            ctx = Context::default();
            report_reader_issue(
                &tx,
                &mut report,
                &mut new_reader_issues,
                "file_replaced",
                &path_text,
                &fingerprint,
                "文件截断或替换，已建立新读取代次，事实身份继续去重",
            )?;
        }
        if ctx.discard_offset > len {
            ctx.discard_offset = 0;
        }
        let mut live = match ctx.live_cursor.take() {
            Some(cursor) => *cursor,
            None if ctx.caught_up => Cursor {
                offset,
                last_start,
                last_hash: last_hash.clone(),
                context: ctx.clone(),
                ..Cursor::default()
            },
            None => make_live_cursor(&mut file, len, &ctx, created_during_runtime, &live_start)?,
        };
        if live.offset > len
            || live.context.discard_offset > len
            || !same_anchor(&mut file, live.last_start, &live.last_hash)
        {
            // Both cursors share a physical generation. Rebuild the live view without
            // discarding or fast-forwarding the historical position.
            live = make_live_cursor(&mut file, len, &ctx, created_during_runtime, &live_start)?;
        }
        if live.context.owner.is_empty() {
            if let Some(end) = prefix[..n].iter().position(|b| *b == b'\n') {
                if let Ok(event) = adapter.parse_line(&prefix[..end]) {
                    adapter.prime_local_metadata(
                        &tx,
                        &super::adapter::Observation {
                            source_id: source_id.into(),
                            channel_id: format!("{source_id}:jsonl"),
                            event,
                        },
                        &path_text,
                        &mut live.context,
                    )?;
                }
            }
        }
        let shared_progress = history_enabled
            && live.bootstrap_end.is_none()
            && live.offset == offset
            && live.context.discard_offset == ctx.discard_offset
            && (created_during_runtime || ctx.caught_up);
        let pending_before = report.pending_files;
        // 新来源的整体 checkpoint 可能没有顶层时间；暂停历史时由 adapter 对每个原生消息
        // 应用 liveStart 下界。不能用新文件 created 时间把复制来的旧消息当作新用量。
        let foreign_live_cutoff = if adapter.client_kind() != "codex" && !history_enabled {
            cutoff_time
                .clone()
                .into_iter()
                .chain(
                    chrono::DateTime::parse_from_rfc3339(&live_start)
                        .ok()
                        .map(|value| value.with_timezone(&Utc)),
                )
                .max()
        } else {
            cutoff_time.clone()
        };
        let mut file_bytes = consume_cursor(
            &tx,
            adapter,
            source_id,
            &path_text,
            &mut file,
            id,
            generation,
            &mut live,
            foreign_live_cutoff.as_ref(),
            true,
            &live_start,
            &core.runtime_id,
            cancel,
            started,
            budget_bytes,
            &mut report,
            &mut new_reader_issues,
        )?;
        let mut historical = Cursor {
            offset,
            last_start,
            last_hash,
            context: ctx,
            ..Cursor::default()
        };
        if shared_progress {
            // No historical interval was skipped: one decode can commit both
            // positions, avoiding duplicate work for ordinary live files.
            historical = live.clone();
        } else if history_enabled
            && !cancel.load(Ordering::Relaxed)
            && started.elapsed() <= std::time::Duration::from_millis(250)
            && file_bytes < budget_bytes
        {
            file_bytes += consume_cursor(
                &tx,
                adapter,
                source_id,
                &path_text,
                &mut file,
                id,
                generation,
                &mut historical,
                cutoff_time.as_ref(),
                false,
                &live_start,
                &core.runtime_id,
                cancel,
                started,
                budget_bytes - file_bytes,
                &mut report,
                &mut new_reader_issues,
            )?;
        } else if historical.offset < len {
            report.pending_files += 1;
        }
        // A file can have both a paused history gap and an incomplete live tail.
        // Public pendingFiles counts files, rather than internal cursor work.
        report.pending_files = pending_before + usize::from(report.pending_files > pending_before);
        historical.context.live_cursor = Some(Box::new(live));
        historical.context.caught_up =
            historical.offset >= len && historical.context.discard_offset == 0;
        if let Some(guard) = guard {
            guard
                .validate_read(&file, &path)
                .map_err(|_| "来源路径在 JSONL 读取期间改变，事务未提交".to_owned())?;
        }
        tx.execute("UPDATE source_files SET fingerprint=?1,generation=?2,offset=?3,last_start=?4,last_hash=?5,context=?6 WHERE id=?7",params![fingerprint,generation,historical.offset as i64,historical.last_start as i64,historical.last_hash,serde_json::to_string(&historical.context).map_err(err)?,id]).map_err(err)?;
        #[cfg(test)]
        if get_setting(&tx, "testFailBeforeCommit")?.and_then(|v| v.as_bool()) == Some(true) {
            return Err("injected-before-commit".into());
        }
        // Fact writes and committed byte cursor are deliberately one SQLite transaction.
        budget_bytes = budget_bytes.saturating_sub(file_bytes);
        report.files += 1;
    }
    report.pending_files += total_files.saturating_sub(visited);
    let new_issues: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ingestion_issues WHERE rowid>?1",
            [previous_issue_row],
            |row| row.get(0),
        )
        .map_err(err)?;
    // Reader observations are already reported, including repeated diagnoses.
    // Add only the new adapter diagnoses so mixed old/new problems stay visible
    // without counting newly inserted reader diagnoses a second time.
    let new_adapter_issues = usize::try_from(new_issues)
        .map_err(err)?
        .checked_sub(new_reader_issues)
        .ok_or("本批诊断计数不一致，事务已取消")?;
    report.issues = report
        .issues
        .checked_add(new_adapter_issues)
        .ok_or("本批诊断计数超出整数范围")?;
    put_setting(&tx, &cursor_key, &json!(next_cursor))?;
    put_setting(&tx, &phase_key, &json!(phase + 1))?;
    put_setting(
        &tx,
        &format!("lastSourceCheck:{source_id}"),
        &json!({"checkedAt":now(),"pendingFiles":report.pending_files,"issues":report.issues,"files":report.files,"unsupportedFiles":unsupported,"discoveryComplete":discovery_complete,"discoveryErrors":discovery_errors,"cancelled":discovered.cancelled || cancel.load(Ordering::Relaxed)}),
    )?;
    if report.issues == 0 && discovery_complete && !cancel.load(Ordering::Relaxed) {
        put_setting(
            &tx,
            &format!("lastSourceSuccess:{source_id}"),
            &json!(now()),
        )?;
    }
    let mut statement = tx
        .prepare("SELECT path,context FROM source_files WHERE source_id=?1")
        .map_err(err)?;
    let known = statement
        .query_map([source_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?
        .into_iter()
        .map(|(path, context)| {
            (
                path,
                serde_json::from_str::<Context>(&context)
                    .map(|c| c.caught_up)
                    .unwrap_or(false),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    drop(statement);
    let pending = discovered_paths
        .iter()
        .filter(|path| known.get(*path) != Some(&true))
        .count();
    // An interrupted/partial listing cannot prove that omitted history is done.
    if discovery_complete && !cancel.load(Ordering::Relaxed) {
        put_setting(&tx, &format!("historyPending:{source_id}"), &json!(pending))?;
    }
    if let Some(guard) = guard {
        guard
            .revalidate()
            .map_err(|_| "来源根目录在读取期间改变，事务未提交".to_owned())?;
    }
    tx.commit().map_err(err)?;
    Ok(report)
}

#[cfg(test)]
#[path = "reader_reliability_tests.rs"]
mod reliability_tests;

#[cfg(test)]
#[path = "foreign_source_tests.rs"]
mod foreign_source_tests;

#[cfg(test)]
#[path = "snapshot_admission_tests.rs"]
mod snapshot_admission_tests;
