use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn unreadable_directory_does_not_block_readable_sibling_discovery() {
    let temp = tempfile::tempdir().unwrap();
    let sessions = temp.path().join("sessions");
    let blocked = sessions.join("blocked");
    let readable = sessions.join("readable");
    fs::create_dir_all(&blocked).unwrap();
    fs::create_dir(&readable).unwrap();
    let expected = readable.join("one.jsonl");
    fs::write(&expected, b"{}\n").unwrap();
    fs::write(blocked.join("omitted.jsonl"), b"{}\n").unwrap();
    let mut found = Discovery::default();
    discover_codex_with(
        &sessions,
        &mut found,
        true,
        &AtomicBool::new(false),
        &|path| {
            if path == blocked {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected denied directory",
                ))
            } else {
                fs::read_dir(path)
            }
        },
    );
    assert_eq!(found.files, vec![expected]);
    assert_eq!(found.errors, vec![blocked]);
    assert!(!found.cancelled);
}

#[test]
fn directory_discovery_observes_cancellation_between_entries() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("never-read.jsonl"), b"{}\n").unwrap();
    let cancel = AtomicBool::new(false);
    let mut found = Discovery::default();
    discover_codex_with(temp.path(), &mut found, true, &cancel, &|path| {
        let entries = fs::read_dir(path)?;
        cancel.store(true, Ordering::Relaxed);
        Ok(entries)
    });
    assert!(found.cancelled);
    assert!(found.files.is_empty());
    assert!(found.errors.is_empty());
}

#[test]
fn already_cancelled_discovery_never_opens_a_directory() {
    let mut found = Discovery::default();
    discover_codex_with(
        Path::new("does-not-exist"),
        &mut found,
        true,
        &AtomicBool::new(true),
        &|_| panic!("a cancelled discovery must not open directories"),
    );
    assert!(found.cancelled);
    assert!(found.errors.is_empty());
}

#[test]
fn missing_optional_sessions_directories_are_not_coverage_errors() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("direct.jsonl");
    fs::write(&path, b"{}\n").unwrap();
    let config = SourceConfig {
        id: "test".into(),
        root: temp.path().to_path_buf(),
        environment: "fixture".into(),
    };
    let found = CodexAdapter.discover(&config).unwrap();
    assert_eq!(found.files, vec![path]);
    assert!(found.errors.is_empty());
    assert!(!found.cancelled);
}

#[cfg(unix)]
#[test]
fn selected_sessions_symlink_is_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    let external = temp.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("excluded.jsonl"), b"{}\n").unwrap();
    let sessions = temp.path().join("sessions");
    std::os::unix::fs::symlink(&external, &sessions).unwrap();
    let config = SourceConfig {
        id: "test".into(),
        root: sessions,
        environment: "fixture".into(),
    };
    let found = CodexAdapter.discover(&config).unwrap();
    assert!(found.files.is_empty());
    assert!(found.errors.is_empty());
}
