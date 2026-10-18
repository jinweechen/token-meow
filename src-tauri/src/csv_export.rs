//! Save already-rendered CSV bytes without opening the selected file for writing.
//! Path checks reject static aliases/reparse points; they are not a security
//! boundary against a hostile process racing filesystem namespace changes.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

fn failure(context: &str, error: impl std::fmt::Display) -> String {
    format!("导出失败：{context}：{error}")
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("导出失败：路径为空。".into());
    }
    // Refuse ambiguous traversal rather than resolving `..` across a link.
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("导出失败：路径不能包含上级目录跳转。".into());
    }
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| failure("无法解析路径", e))
    }
}

fn check_directory_chain(directory: &Path) -> Result<(), String> {
    for ancestor in directory.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata =
            fs::symlink_metadata(ancestor).map_err(|e| failure("无法检查保存位置", e))?;
        if is_reparse(&metadata) || !metadata.is_dir() {
            return Err("导出失败：保存位置不能经过符号链接或重解析点。".into());
        }
    }
    Ok(())
}

// Registered roots may currently be offline/missing. Resolve their nearest
// existing ancestor and retain the missing suffix, rather than dropping them.
fn resolved_root(root: &Path) -> Result<PathBuf, String> {
    let absolute = absolute_path(root)?;
    let mut existing = absolute.as_path();
    let mut tail = Vec::new();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                tail.push(
                    existing
                        .file_name()
                        .ok_or("导出失败：保护目录无有效父路径。")?
                        .to_owned(),
                );
                existing = existing
                    .parent()
                    .ok_or("导出失败：保护目录无有效父路径。")?;
            }
            Err(e) => return Err(failure("无法检查保护目录", e)),
        }
    }
    // A registered root itself may be a legitimate directory alias. Follow it
    // read-only so the real root is protected too. Destination links are refused.
    if !fs::metadata(existing)
        .map_err(|e| failure("无法检查保护目录", e))?
        .is_dir()
    {
        return Err("导出失败：保护路径不是有效目录。".into());
    }
    let mut resolved = fs::canonicalize(existing).map_err(|e| failure("无法解析保护目录", e))?;
    for part in tail.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn path_key(path: &Path) -> Result<String, String> {
    let key = path
        .to_str()
        .ok_or("导出失败：路径编码无效。")?
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_owned();
    #[cfg(windows)]
    {
        Ok(key.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        Ok(key)
    }
}

fn within(path: &Path, root: &Path) -> Result<bool, String> {
    let path = path_key(path)?;
    let root = path_key(root)?;
    Ok(path == root || path.starts_with(&format!("{root}/")))
}

fn checked_destination(path: &Path, protected_roots: &[PathBuf]) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err("导出失败：请选择完整的 CSV 保存路径。".into());
    }
    absolute_path(path)?;
    let name = path
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or("导出失败：文件名无效。")?;
    if path
        .extension()
        .and_then(|part| part.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("csv"))
        || name.contains([':', '\0'])
        || name.ends_with(['.', ' '])
    {
        return Err("导出失败：保存文件必须使用 .csv 扩展名。".into());
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if !matches!(path.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)
                | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)))
        {
            return Err("导出失败：不能保存到设备路径。".into());
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            return Err("导出失败：不能使用设备文件名。".into());
        }
    }
    let parent = path.parent().ok_or("导出失败：保存路径没有父目录。")?;
    check_directory_chain(parent)?;
    let destination = fs::canonicalize(parent)
        .map_err(|e| failure("无法解析保存目录", e))?
        .join(name);
    let existing = match fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            if is_reparse(&metadata) || !metadata.is_file() {
                return Err("导出失败：目标必须是普通 CSV 文件，不能是链接或目录。".into());
            }
            Some(fs::canonicalize(&destination).map_err(|e| failure("无法解析目标文件", e))?)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(failure("无法检查目标文件", e)),
    };
    if protected_roots.is_empty() {
        return Err("导出失败：无法确定来源和宠物数据保护目录。".into());
    }
    for root in protected_roots {
        let root = resolved_root(root)?;
        if within(&destination, &root)?
            || existing
                .as_ref()
                .map(|path| within(path, &root))
                .transpose()?
                .unwrap_or(false)
        {
            return Err(
                "导出失败：不能保存到已登记来源目录或宠物数据目录中，请选择其他位置。".into(),
            );
        }
    }
    Ok(destination)
}

trait SaveIo {
    fn write(&self, file: &mut File, bytes: &[u8]) -> io::Result<()>;
    fn sync(&self, file: &mut File) -> io::Result<()>;
    fn replace(&self, temporary: &Path, destination: &Path) -> io::Result<()>;
}

struct Disk;
impl SaveIo for Disk {
    fn write(&self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
        file.write_all(bytes)
    }
    fn sync(&self, file: &mut File) -> io::Result<()> {
        file.flush().and_then(|_| file.sync_all())
    }
    fn replace(&self, temporary: &Path, destination: &Path) -> io::Result<()> {
        fs::rename(temporary, destination)
    }
}

struct OwnedTemporary {
    path: PathBuf,
    file: Option<File>,
    committed: bool,
}
impl Drop for OwnedTemporary {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn save_with(
    path: &Path,
    bytes: &[u8],
    protected: &[PathBuf],
    io: &impl SaveIo,
) -> Result<(), String> {
    let destination = checked_destination(path, protected)?;
    let temporary = destination
        .parent()
        .ok_or("导出失败：保存目录无效。")?
        .join(format!(".token-meow-export-{}.tmp", uuid::Uuid::new_v4()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| failure("无法创建临时文件", e))?;
    // This guard exists only after create_new succeeds. It never removes a
    // pre-existing file, the selected destination, or source/data files.
    let mut owned = OwnedTemporary {
        path: temporary,
        file: Some(file),
        committed: false,
    };
    let file = owned.file.as_mut().ok_or("导出失败：临时文件未就绪。")?;
    io.write(file, bytes)
        .map_err(|e| failure("写入临时文件失败，原文件未替换", e))?;
    io.sync(file)
        .map_err(|e| failure("同步临时文件失败，原文件未替换", e))?;
    drop(owned.file.take()); // Windows rename requires our writing handle closed.
    let rechecked = checked_destination(path, protected)?;
    if path_key(&rechecked)? != path_key(&destination)? {
        return Err("导出失败：保存目录已变化，原文件未替换。".into());
    }
    io.replace(&owned.path, &destination)
        .map_err(|e| failure("替换 CSV 失败，原文件仍保留", e))?;
    owned.committed = true;
    Ok(())
}

pub(super) fn save(path: &Path, bytes: &[u8], protected: &[PathBuf]) -> Result<(), String> {
    save_with(path, bytes, protected, &Disk)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _temp: tempfile::TempDir,
        output: PathBuf,
        protected: Vec<PathBuf>,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let output = temp.path().join("exports");
            let protected: Vec<_> = ["source-primary", "source-disabled", "pet-data"]
                .iter()
                .map(|name| temp.path().join(name))
                .collect();
            fs::create_dir(&output).unwrap();
            for root in &protected {
                fs::create_dir(root).unwrap();
                fs::write(root.join("keep.csv"), b"protected-csv").unwrap();
                fs::write(root.join("source.jsonl"), b"source-log").unwrap();
                fs::write(root.join("ledger.db"), b"data-db").unwrap();
            }
            Self {
                _temp: temp,
                output,
                protected,
            }
        }
        fn assert_protected(&self) {
            for root in &self.protected {
                assert_eq!(fs::read(root.join("keep.csv")).unwrap(), b"protected-csv");
                assert_eq!(fs::read(root.join("source.jsonl")).unwrap(), b"source-log");
                assert_eq!(fs::read(root.join("ledger.db")).unwrap(), b"data-db");
                assert_eq!(fs::read_dir(root).unwrap().count(), 3);
            }
        }
        fn names(&self) -> Vec<std::ffi::OsString> {
            let mut names: Vec<_> = fs::read_dir(&self.output)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            names.sort();
            names
        }
    }

    #[test]
    fn success_preserves_rendered_bytes_and_replaces_existing_csv() {
        let fixture = Fixture::new();
        let path = fixture.output.join("ledger.CSV");
        fs::write(&path, b"old-csv").unwrap();
        let bytes = "\u{feff}source_id,thread_id,response_id,event_time,project,requested_model,input_tokens,output_tokens,total_tokens,cache_read,cache_write,reasoning_output,metric_semantics,quality\r\n\"s\",\"t\",\"r\",\"now\",\"'=SUM(1) 猫\",\"m\",\"1\",\"2\",\"3\",\"0\",\"\",\"\",\"v1\",\"exact\"\r\n".as_bytes();
        save(&path, bytes, &fixture.protected).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            fixture.names(),
            vec![std::ffi::OsString::from("ledger.CSV")]
        );
        fixture.assert_protected();
    }

    #[test]
    fn empty_relative_and_non_csv_paths_are_rejected_before_writing() {
        let fixture = Fixture::new();
        for path in [
            PathBuf::new(),
            PathBuf::from("relative.csv"),
            fixture.output.join("ledger"),
            fixture.output.join("ledger.csv.exe"),
            fixture.output.join("ledger.csv "),
            fixture.output.join("ledger.csv:stream.csv"),
        ] {
            assert!(save(&path, b"new", &fixture.protected).is_err(), "{path:?}");
        }
        assert!(fixture.names().is_empty());
        fixture.assert_protected();
    }

    #[test]
    fn every_registered_source_and_data_root_are_protected() {
        let fixture = Fixture::new();
        for root in &fixture.protected {
            assert!(save(&root.join("keep.csv"), b"new", &fixture.protected).is_err());
            assert!(save(&root.join("new.csv"), b"new", &fixture.protected).is_err());
        }
        fixture.assert_protected();
    }

    #[test]
    fn canonical_parent_and_prefix_boundary_do_not_allow_protected_writes() {
        let fixture = Fixture::new();
        let source = fs::canonicalize(&fixture.protected[0]).unwrap();
        assert!(save(&source.join("new.csv"), b"new", &fixture.protected).is_err());
        let traversal = fixture
            .output
            .join("..")
            .join("source-primary")
            .join("new.csv");
        assert!(save(&traversal, b"new", &fixture.protected).is_err());
        let sibling = fixture._temp.path().join("source-primary-other");
        fs::create_dir(&sibling).unwrap();
        save(&sibling.join("allowed.csv"), b"new", &fixture.protected).unwrap();
        fixture.assert_protected();
    }

    #[test]
    fn directory_and_unresolvable_protection_are_rejected() {
        let fixture = Fixture::new();
        let directory = fixture.output.join("directory.csv");
        fs::create_dir(&directory).unwrap();
        assert!(save(&directory, b"new", &fixture.protected).is_err());
        assert!(save(&fixture.output.join("new.csv"), b"new", &[]).is_err());
        let protected_file = fixture.output.join("not-a-directory");
        fs::write(&protected_file, b"keep").unwrap();
        assert!(save(&fixture.output.join("new.csv"), b"new", &[protected_file]).is_err());
        fixture.assert_protected();
    }

    #[test]
    fn missing_registered_root_keeps_its_resolved_suffix() {
        let fixture = Fixture::new();
        let missing = fixture._temp.path().join("offline-source").join("nested");
        let resolved = resolved_root(&missing).unwrap();
        assert_eq!(
            resolved,
            fs::canonicalize(fixture._temp.path())
                .unwrap()
                .join("offline-source")
                .join("nested")
        );
        assert!(within(&resolved.join("new.csv"), &resolved).unwrap());
        let mut protected = fixture.protected.clone();
        protected.push(missing);
        save(&fixture.output.join("allowed.csv"), b"safe", &protected).unwrap();
        fixture.assert_protected();
    }

    enum Fault {
        Write,
        Sync,
        Rename,
    }
    impl SaveIo for Fault {
        fn write(&self, file: &mut File, bytes: &[u8]) -> io::Result<()> {
            if matches!(self, Self::Write) {
                file.write_all(&bytes[..bytes.len().min(3)])?;
                return Err(io::Error::other("injected write failure"));
            }
            Disk.write(file, bytes)
        }
        fn sync(&self, file: &mut File) -> io::Result<()> {
            if matches!(self, Self::Sync) {
                return Err(io::Error::other("injected sync failure"));
            }
            Disk.sync(file)
        }
        fn replace(&self, temporary: &Path, destination: &Path) -> io::Result<()> {
            if matches!(self, Self::Rename) {
                return Err(io::Error::other("injected rename failure"));
            }
            Disk.replace(temporary, destination)
        }
    }

    #[test]
    fn write_sync_and_rename_failure_leave_old_csv_and_foreign_temp_unchanged() {
        let fixture = Fixture::new();
        let path = fixture.output.join("ledger.csv");
        fs::write(&path, b"old-csv").unwrap();
        let foreign = fixture.output.join(".token-meow-export-other.tmp");
        fs::write(&foreign, b"foreign-temp").unwrap();
        let names = fixture.names();
        for fault in [Fault::Write, Fault::Sync, Fault::Rename] {
            assert!(save_with(&path, b"replacement-csv", &fixture.protected, &fault).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"old-csv");
            assert_eq!(fs::read(&foreign).unwrap(), b"foreign-temp");
            assert_eq!(fixture.names(), names);
            fixture.assert_protected();
        }
        let absent = fixture.output.join("new.csv");
        assert!(save_with(&absent, b"new", &fixture.protected, &Fault::Write).is_err());
        assert!(!absent.exists());
        assert_eq!(fixture.names(), names);
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_alias_and_device_names_are_rejected() {
        let fixture = Fixture::new();
        let alias = fixture._temp.path().join("SOURCE-PRIMARY").join("new.csv");
        assert!(save(&alias, b"new", &fixture.protected).is_err());
        for name in ["CON.csv", "NUL.csv", "COM1.csv", "LPT9.csv"] {
            assert!(save(&fixture.output.join(name), b"new", &fixture.protected).is_err());
        }
        assert!(fixture.names().is_empty());
        fixture.assert_protected();
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn symlink_destination_is_rejected_and_registered_alias_protects_real_root() {
        #[cfg(unix)]
        use std::os::unix::fs::{symlink, symlink as symlink_dir};
        #[cfg(windows)]
        use std::os::windows::fs::{symlink_dir, symlink_file as symlink};
        let fixture = Fixture::new();
        let alias = fixture.output.join("alias");
        symlink_dir(&fixture.protected[0], &alias)
            .expect("symlink fixture needs OS symlink permission; do not treat a fixture failure as a path-policy pass");
        assert!(save(&alias.join("new.csv"), b"new", &fixture.protected).is_err());
        let file_alias = fixture.output.join("alias.csv");
        symlink(fixture.protected[0].join("keep.csv"), &file_alias).unwrap();
        assert!(save(&file_alias, b"new", &fixture.protected).is_err());
        assert!(save(
            &fixture.protected[0].join("keep.csv"),
            b"new",
            &[alias.clone()]
        )
        .is_err());
        save(&fixture.output.join("new.csv"), b"new", &[alias]).unwrap();
        fixture.assert_protected();
    }
}
