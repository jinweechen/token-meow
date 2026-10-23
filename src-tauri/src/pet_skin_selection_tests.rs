use super::*;
use serde_json::json;

fn id(character: char) -> String {
    character.to_string().repeat(64)
}

fn assert_closed(root: &Path) {
    let choice = read(root);
    assert_eq!(choice.active, Some(ActiveSkin::Builtin));
    assert_eq!(choice.problem.as_deref(), Some(READ_PROBLEM));
    assert!(!choice
        .problem
        .unwrap()
        .contains(&root.to_string_lossy().to_string()));
}

#[test]
fn missing_authority_is_legacy_and_never_reads_old_model_or_png_records() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        read(root.path()),
        SkinChoice {
            active: None,
            problem: None
        }
    );
    for directory in ["local-pet-skins-v1", "vrm-pet-skins-v1"] {
        let directory = root.path().join(directory);
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("selection.json"),
            b"untrusted old records are not authority inputs",
        )
        .unwrap();
    }
    assert_eq!(
        read(root.path()),
        SkinChoice {
            active: None,
            problem: None
        }
    );
    assert!(!root.path().join(RECORD_NAME).exists());
}

#[test]
fn png_vrm_and_builtin_replace_one_authority_record_and_survive_reopening() {
    let root = tempfile::tempdir().unwrap();
    for active in [
        ActiveSkin::Png(id('a')),
        ActiveSkin::Vrm(id('b')),
        ActiveSkin::Builtin,
    ] {
        write(root.path(), &active).unwrap();
        let raw = fs::read(root.path().join(RECORD_NAME)).unwrap();
        assert!(raw.len() <= MAX_RECORD_BYTES as usize);
        let reopened = Path::new(root.path().as_os_str()).to_owned();
        assert_eq!(
            read(&reopened),
            SkinChoice {
                active: Some(active),
                problem: None
            }
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn invalid_ids_do_not_overwrite_existing_authority_or_create_staging_files() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), &ActiveSkin::Png(id('a'))).unwrap();
    let previous = fs::read(root.path().join(RECORD_NAME)).unwrap();
    for bad in [
        String::new(),
        "A".repeat(64),
        "g".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
        "../other".into(),
        r"C:\private\model.vrm".into(),
        format!("{}\0", "a".repeat(63)),
    ] {
        for value in [ActiveSkin::Png(bad.clone()), ActiveSkin::Vrm(bad)] {
            assert!(write(root.path(), &value).is_err());
            assert_eq!(fs::read(root.path().join(RECORD_NAME)).unwrap(), previous);
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }
}

#[test]
fn record_schema_unknown_fields_types_versions_and_invalid_ids_fail_closed_without_rewriting() {
    let root = tempfile::tempdir().unwrap();
    let invalid = [
        json!({}),
        json!(null),
        json!([]),
        json!({"schemaVersion":1,"active":null}),
        json!({"schemaVersion":"1","active":{"kind":"builtin"}}),
        json!({"schemaVersion":0,"active":{"kind":"builtin"}}),
        json!({"schemaVersion":2,"active":{"kind":"builtin"}}),
        json!({"schemaVersion":1,"active":{"kind":"Builtin"}}),
        json!({"schemaVersion":1,"active":{"kind":"unknown"}}),
        json!({"schemaVersion":1,"active":{"kind":"builtin","id":id('a')}}),
        json!({"schemaVersion":1,"active":{"kind":"builtin","path":"C:/private/model"}}),
        json!({"schemaVersion":1,"active":{"kind":"png"}}),
        json!({"schemaVersion":1,"active":{"kind":"png","id":null}}),
        json!({"schemaVersion":1,"active":{"kind":"png","id":7}}),
        json!({"schemaVersion":1,"active":{"kind":"png","id":id('a'),"file":"other"}}),
        json!({"schemaVersion":1,"active":{"kind":"vrm","id":"../model"}}),
        json!({"schemaVersion":1,"active":{"kind":"vrm","id":"A".repeat(64)}}),
        json!({"schemaVersion":1,"active":{"kind":"builtin"},"selectedSkinId":id('a')}),
    ];
    for value in invalid {
        let raw = serde_json::to_vec(&value).unwrap();
        fs::write(root.path().join(RECORD_NAME), &raw).unwrap();
        assert_closed(root.path());
        assert_eq!(fs::read(root.path().join(RECORD_NAME)).unwrap(), raw);
    }
}

#[test]
fn empty_oversized_invalid_utf8_duplicate_fields_and_trailing_json_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    for raw in [
        vec![],
        vec![b' '; 1025],
        vec![0xff, 0xfe],
        b"not json C:/private/path".to_vec(),
        br#"{"schemaVersion":1,"schemaVersion":1,"active":{"kind":"builtin"}}"#.to_vec(),
        br#"{"schemaVersion":1,"active":{"kind":"png","id":"a","id":"b"}}"#.to_vec(),
        br#"{"schemaVersion":1,"active":{"kind":"builtin"}} []"#.to_vec(),
    ] {
        fs::write(root.path().join(RECORD_NAME), &raw).unwrap();
        assert_closed(root.path());
        assert_eq!(fs::read(root.path().join(RECORD_NAME)).unwrap(), raw);
    }
}

#[test]
fn broken_target_library_paths_do_not_block_authority_or_builtin_recovery() {
    let root = tempfile::tempdir().unwrap();
    let png = root.path().join("local-pet-skins-v1");
    let vrm = root.path().join("vrm-pet-skins-v1");
    fs::write(&png, b"a file blocks PNG directory initialization").unwrap();
    fs::write(&vrm, b"a file blocks VRM directory initialization").unwrap();
    for active in [
        ActiveSkin::Png(id('a')),
        ActiveSkin::Vrm(id('b')),
        ActiveSkin::Builtin,
    ] {
        write(root.path(), &active).unwrap();
        assert_eq!(read(root.path()).active, Some(active));
    }
    assert_eq!(
        fs::read(&png).unwrap(),
        b"a file blocks PNG directory initialization"
    );
    assert_eq!(
        fs::read(&vrm).unwrap(),
        b"a file blocks VRM directory initialization"
    );
}

#[test]
fn missing_root_relative_root_and_directory_in_place_of_record_are_not_legacy() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    assert_closed(&missing);
    assert!(write(&missing, &ActiveSkin::Builtin).is_err());
    let relative = Path::new("relative-app-data-not-opened");
    assert_closed(relative);
    assert!(write(relative, &ActiveSkin::Builtin).is_err());
    fs::create_dir(root.path().join(RECORD_NAME)).unwrap();
    fs::write(root.path().join(RECORD_NAME).join("keep.txt"), b"keep").unwrap();
    assert_closed(root.path());
    assert!(write(root.path(), &ActiveSkin::Builtin).is_err());
    assert_eq!(
        fs::read(root.path().join(RECORD_NAME).join("keep.txt")).unwrap(),
        b"keep"
    );
}

#[test]
fn failed_read_never_leaks_a_record_value_or_path_and_explicit_builtin_can_repair_bad_json() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join(RECORD_NAME),
        b"C:/highly-private-path must never enter a diagnostic",
    )
    .unwrap();
    assert_closed(root.path());
    write(root.path(), &ActiveSkin::Builtin).unwrap();
    assert_eq!(
        read(root.path()),
        SkinChoice {
            active: Some(ActiveSkin::Builtin),
            problem: None
        }
    );
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{os::windows::fs::OpenOptionsExt, process::Command};

    fn junction(link: &Path, target: &Path) {
        let result = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(result.status.success(), "创建测试 junction 失败");
    }

    #[test]
    fn root_or_record_junctions_cannot_redirect_authority_reads_or_writes() {
        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        fs::create_dir(&actual).unwrap();
        write(&actual, &ActiveSkin::Png(id('a'))).unwrap();
        let previous = fs::read(actual.join(RECORD_NAME)).unwrap();
        let alias = root.path().join("alias");
        junction(&alias, &actual);
        assert_closed(&alias);
        assert!(write(&alias, &ActiveSkin::Builtin).is_err());
        assert_eq!(fs::read(actual.join(RECORD_NAME)).unwrap(), previous);
        fs::remove_dir(&alias).unwrap();
        let other_root = root.path().join("other");
        fs::create_dir(&other_root).unwrap();
        junction(&other_root.join(RECORD_NAME), &actual);
        assert_closed(&other_root);
        assert!(write(&other_root, &ActiveSkin::Builtin).is_err());
        assert_eq!(fs::read(actual.join(RECORD_NAME)).unwrap(), previous);
        fs::remove_dir(other_root.join(RECORD_NAME)).unwrap();
    }

    #[test]
    fn replacement_sharing_failure_keeps_previous_record_and_removes_only_own_pending_file() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &ActiveSkin::Png(id('a'))).unwrap();
        let record = root.path().join(RECORD_NAME);
        let previous = fs::read(&record).unwrap();
        let keep = root
            .path()
            .join(".pet-renderer-selection-unrelated.pending");
        fs::write(&keep, b"not created by this call").unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&record)
            .unwrap();
        assert!(write(root.path(), &ActiveSkin::Vrm(id('b'))).is_err());
        assert_eq!(fs::read(&record).unwrap(), previous);
        assert_eq!(fs::read(&keep).unwrap(), b"not created by this call");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
        drop(held);
        write(root.path(), &ActiveSkin::Builtin).unwrap();
        assert_eq!(read(root.path()).active, Some(ActiveSkin::Builtin));
    }
}

#[cfg(unix)]
#[test]
fn directory_file_and_dangling_symlinks_fail_closed_without_touching_their_targets() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let actual = root.path().join("actual");
    fs::create_dir(&actual).unwrap();
    write(&actual, &ActiveSkin::Vrm(id('b'))).unwrap();
    let previous = fs::read(actual.join(RECORD_NAME)).unwrap();
    let alias = root.path().join("alias");
    symlink(&actual, &alias).unwrap();
    assert_closed(&alias);
    assert!(write(&alias, &ActiveSkin::Builtin).is_err());
    let other = root.path().join("other");
    fs::create_dir(&other).unwrap();
    let link = other.join(RECORD_NAME);
    symlink(actual.join(RECORD_NAME), &link).unwrap();
    assert_closed(&other);
    assert!(write(&other, &ActiveSkin::Builtin).is_err());
    fs::remove_file(&link).unwrap();
    symlink(root.path().join("missing"), &link).unwrap();
    assert_closed(&other);
    assert!(write(&other, &ActiveSkin::Builtin).is_err());
    assert_eq!(fs::read(actual.join(RECORD_NAME)).unwrap(), previous);
}
