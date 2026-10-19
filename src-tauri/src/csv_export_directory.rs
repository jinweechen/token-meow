use std::{fs, path::PathBuf};

pub(super) fn first_available(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<PathBuf, String> {
    candidates
        .into_iter()
        .find(|path| path.is_absolute() && path.is_dir() && fs::read_dir(path).is_ok())
        .ok_or_else(|| "导出失败：没有可用的保存目录，请检查文档、下载或用户目录后重试。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "token-meow-export-directory-{}-{stamp}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            for name in ["Documents", "Downloads"] {
                let path = self.0.join(name);
                if path.is_dir() {
                    let _ = fs::remove_dir(path);
                } else if path.is_file() {
                    let _ = fs::remove_file(path);
                }
            }
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[test]
    fn missing_desktop_falls_back_without_creating_profile_folders() {
        let fixture = Fixture::new();
        let desktop = fixture.0.join("Desktop");
        let documents = fixture.0.join("Documents");
        let downloads = fixture.0.join("Downloads");
        let selected = first_available([
            documents.clone(),
            downloads.clone(),
            desktop.clone(),
            fixture.0.clone(),
        ])
        .unwrap();
        assert_eq!(selected, fixture.0);
        assert!(!desktop.exists() && !documents.exists() && !downloads.exists());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 0);
    }

    #[test]
    fn skips_files_and_missing_paths_and_prefers_first_browsable_directory() {
        let fixture = Fixture::new();
        let documents = fixture.0.join("Documents");
        let downloads = fixture.0.join("Downloads");
        fs::write(&documents, b"existing-file").unwrap();
        fs::create_dir(&downloads).unwrap();
        assert_eq!(
            first_available([
                PathBuf::from("relative"),
                documents.clone(),
                fixture.0.join("Desktop"),
                downloads.clone(),
                fixture.0.clone()
            ])
            .unwrap(),
            downloads
        );
        assert_eq!(fs::read(&documents).unwrap(), b"existing-file");
        assert_eq!(fs::read_dir(&downloads).unwrap().count(), 0);
        assert!(first_available([documents, fixture.0.join("Desktop")]).is_err());
    }
}
