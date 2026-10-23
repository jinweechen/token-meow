//! 来源登记按实际日志入口比较；路径别名不创建第二个账本身份。
use super::*;

#[derive(Clone)]
pub(super) struct RegisteredSource {
    pub id: String,
    pub root: String,
    pub client_kind: String,
    pub enabled: bool,
    root_key: Option<PathKey>,
    entry_key: Option<PathKey>,
}

pub(super) fn registered(db: &Connection) -> Result<Vec<RegisteredSource>> {
    let mut statement = db
        .prepare("SELECT id,root,client_kind,enabled FROM source_instances ORDER BY created_at,id")
        .map_err(err)?;
    let rows = statement
        .query_map([], |row| {
            Ok(RegisteredSource {
                id: row.get(0)?,
                root: row.get(1)?,
                client_kind: row.get(2)?,
                enabled: row.get(3)?,
                root_key: None,
                entry_key: None,
            })
        })
        .map_err(err)?;
    let mut rows = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    let mut keys = std::collections::BTreeMap::<PathBuf, Option<PathKey>>::new();
    for row in &mut rows {
        let root = Path::new(&row.root);
        row.root_key = keys
            .entry(root.to_path_buf())
            .or_insert_with(|| path_key(root).ok())
            .clone();
        let entry = log_entry(root, &row.client_kind);
        row.entry_key = keys
            .entry(entry.clone())
            .or_insert_with(|| path_key(&entry).ok())
            .clone();
    }
    Ok(rows)
}

fn log_entry(root: &Path, client_kind: &str) -> PathBuf {
    let child = match client_kind {
        "claude" => Some("projects"),
        "gemini" => Some("tmp"),
        _ => None,
    };
    match child {
        Some(child)
            if !root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case(child)) =>
        {
            root.join(child)
        }
        _ => root.to_path_buf(),
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PathKey {
    directory: String,
    tail: Vec<std::ffi::OsString>,
}

fn directory_identity(path: &Path) -> Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let handle = fs::OpenOptions::new()
            .read(true)
            .access_mode(0x80)
            .share_mode(7)
            .custom_flags(0x0200_0000)
            .open(path)
            .map_err(err)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
            return Err("来源目录的原生身份无法读取。".into());
        }
        return Ok(format!(
            "win:{}:{}:{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).map_err(err)?;
        Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(path.to_string_lossy().into_owned())
    }
}

fn path_key(path: &Path) -> Result<PathKey> {
    // 尚未创建 projects/tmp 时也比较最近真实父目录，避免必须先运行 CLI 才能登记。
    let resolved = resolved_path(path)?;
    let mut existing = resolved.as_path();
    let mut tail = Vec::new();
    while !existing.exists() {
        tail.push(
            existing
                .file_name()
                .ok_or("来源目录没有可解析的父目录。")?
                .to_os_string(),
        );
        existing = existing.parent().ok_or("来源目录没有可解析的父目录。")?;
    }
    tail.reverse();
    // 使用卷/文件 ID，不对完整路径强制转小写；Windows 大小写敏感目录中的 Foo/foo 仍是独立身份。
    Ok(PathKey {
        directory: directory_identity(existing)?,
        tail,
    })
}

fn same_entry(left: &RegisteredSource, right: &RegisteredSource) -> bool {
    // 同一登记目录不能作为不同工具再次登记；实际入口还覆盖 .claude/projects 等别名。
    left.root == right.root
        || left
            .root_key
            .as_ref()
            .zip(right.root_key.as_ref())
            .is_some_and(|(a, b)| a == b)
        || left
            .entry_key
            .as_ref()
            .zip(right.entry_key.as_ref())
            .is_some_and(|(a, b)| a == b)
}

pub(super) fn matching_source<'a>(
    rows: &'a [RegisteredSource],
    root: &Path,
    client_kind: &str,
    except_id: Option<&str>,
) -> Result<Option<&'a RegisteredSource>> {
    let root_key = path_key(root).ok();
    let entry = log_entry(root, client_kind);
    let entry_key = if entry == root {
        root_key.clone()
    } else {
        path_key(&entry).ok()
    };
    let candidate = RegisteredSource {
        id: String::new(),
        root: root.to_string_lossy().into_owned(),
        client_kind: client_kind.into(),
        enabled: true,
        root_key,
        entry_key,
    };
    Ok(matching_registered(rows, &candidate, except_id))
}

fn matching_registered<'a>(
    rows: &'a [RegisteredSource],
    candidate: &RegisteredSource,
    except_id: Option<&str>,
) -> Option<&'a RegisteredSource> {
    // 本次登记/扫描已采样每个入口的原生身份，来源两两比较只读内存键。
    // 无法读取的其它目录不阻塞独立目录；原始根相等仍保持旧版的跨工具约束。
    rows.iter()
        .find(|row| except_id != Some(row.id.as_str()) && same_entry(row, candidate))
}

fn current_entry_cursor_ids(
    db: &Connection,
    rows: &[RegisteredSource],
) -> Result<std::collections::BTreeSet<String>> {
    let entries: std::collections::BTreeMap<_, _> = rows
        .iter()
        .filter_map(|row| row.entry_key.as_ref().map(|key| (row.id.as_str(), key)))
        .collect();
    let mut result = std::collections::BTreeSet::new();
    let mut parents = std::collections::BTreeMap::<PathBuf, Option<PathKey>>::new();
    let mut statement = db
        .prepare("SELECT source_id,path FROM source_files")
        .map_err(err)?;
    let cursors = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(err)?;
    for cursor in cursors {
        let (id, path) = cursor.map_err(err)?;
        if result.contains(&id) {
            continue;
        }
        let Some(entry) = entries.get(id.as_str()) else {
            continue;
        };
        // 只沿用当前日志入口之下的游标；曾经位于其它目录的历史游标不授予新入口归属。
        // 各文件通常共享父目录，缓存其原生身份，避免每个游标重复打开整条目录链。
        for parent in Path::new(&path).ancestors().skip(1) {
            if parent.as_os_str().is_empty() {
                continue;
            }
            let key = parents
                .entry(parent.to_path_buf())
                .or_insert_with(|| path_key(parent).ok());
            if key.as_ref() == Some(*entry) {
                result.insert(id);
                break;
            }
        }
    }
    Ok(result)
}

pub(super) fn ensure_enable_keeps_cursor_owner(
    db: &Connection,
    rows: &[RegisteredSource],
    id: &str,
) -> Result<()> {
    let current = rows.iter().find(|row| row.id == id).ok_or("未知来源实例")?;
    let with_cursors = current_entry_cursor_ids(db, rows)?;
    // registered 已按 created_at,id 排序；有多个旧身份时与扫描采用同一稳定顺序。
    let owners: Vec<_> = rows
        .iter()
        .filter(|row| with_cursors.contains(&row.id))
        .cloned()
        .collect();
    if let Some(owner) = matching_registered(&owners, current, None) {
        if owner.id != current.id {
            return Err(format!("此日志入口的旧游标属于来源 {}；请停用重复项后启用原来源，或更改为独立目录。历史记录不会自动合并。", owner.id));
        }
    }
    Ok(())
}

impl Core {
    /// 历史登记也受入口互斥约束；不合并或删除其原身份和历史事实。
    pub(super) fn scan_source_bindings(&self) -> Result<Vec<(String, String, String)>> {
        let mut rows = registered(&self.db)?;
        let with_cursors = current_entry_cursor_ids(&self.db, &rows)?;
        // 旧父子入口可能已由较晚登记的来源拥有唯一文件游标；沿用该身份，避免继续 UNIQUE 冲突。
        // sort_by_key 是稳定排序，同样有/无游标时仍沿用 created_at,id 的登记顺序。
        rows.sort_by_key(|row| !with_cursors.contains(&row.id));
        let mut owners = Vec::<RegisteredSource>::new();
        let mut admitted = Vec::new();
        for row in rows
            .into_iter()
            .filter(|row| adapter::adapter_for(&row.client_kind).is_some())
        {
            // 已停用来源若拥有唯一游标，也保留该入口归属；后来项不能复用它写另一身份。
            if !row.enabled && !with_cursors.contains(&row.id) {
                continue;
            }
            if let Some(owner) = matching_registered(&owners, &row, None) {
                if !row.enabled {
                    continue;
                }
                let message = if owner.enabled {
                    format!("来源日志入口与已登记来源 {} 重复，已暂停此来源采集以防双计；历史记录未合并，请核对后更改目录或停用重复来源。", owner.id)
                } else {
                    format!("此日志入口的旧游标属于已停用来源 {}，已暂停当前来源采集；请先停用重复项再启用旧来源，或更改为独立目录。历史记录未合并，请核对已有记录是否重复。", owner.id)
                };
                issue(
                    &self.db,
                    "source_path_duplicate",
                    &row.root,
                    &hash(format!("{}|{}", row.id, owner.id).as_bytes()),
                    &message,
                )?;
                put_setting(
                    &self.db,
                    &format!("lastSourceCheck:{}", row.id),
                    &json!({
                        "checkedAt":now(),"pendingFiles":0,"issues":1,"files":0,
                        "unsupportedFiles":0,"discoveryComplete":false,"discoveryErrors":0,"cancelled":false,
                        "blockedReason":"source_path_duplicate","duplicateOf":owner.id,"error":message,
                    }),
                )?;
            } else {
                if row.enabled {
                    admitted.push((row.id.clone(), row.root.clone(), row.client_kind.clone()));
                }
                owners.push(row);
            }
        }
        Ok(admitted)
    }
}

#[cfg(test)]
#[path = "source_path_tests.rs"]
mod tests;
