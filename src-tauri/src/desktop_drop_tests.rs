use super::*;
use serde_json::json;

#[test]
fn ipc_accepts_only_opaque_ids() {
    for value in [
        json!({"batchId":"b", "paths":["C:\\secret.txt"]}),
        json!({"batchId":null}),
        json!({"batchId":[]}),
        json!({}),
    ] {
        assert!(serde_json::from_value::<DropBatchOptions>(value).is_err());
    }
    for value in [
        json!({"batchId":"b", "launcherId":"known", "executable":"C:\\other.exe"}),
        json!({"batchId":"b", "launcherId":"known", "files":[]}),
        json!({"batchId":"b", "launcherId":null}),
        json!({"launcherId":"known"}),
    ] {
        assert!(serde_json::from_value::<DropOpenOptions>(value).is_err());
    }
    let batch: DropBatchOptions = serde_json::from_value(json!({"batchId":"b"})).unwrap();
    assert_eq!(batch.batch_id, "b");
    let open: DropOpenOptions =
        serde_json::from_value(json!({"batchId":"b", "launcherId":"known"})).unwrap();
    assert_eq!(
        (open.batch_id.as_str(), open.launcher_id.as_str()),
        ("b", "known")
    );
}

#[test]
fn file_labels_strip_controls_and_direction_overrides_with_a_character_limit() {
    assert_eq!(visible_file_name("正常🐈.txt"), "正常🐈.txt");
    assert_eq!(visible_file_name("x\u{202e}exe.txt\n"), "x�exe.txt�");
    for control in [
        '\0', '\t', '\u{0085}', '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}',
        '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        assert_eq!(visible_file_name(&format!("a{control}b")), "a�b");
    }
    assert_eq!(visible_file_name(&"🐈".repeat(130)).chars().count(), 120);
}

#[test]
fn command_context_removes_private_values_without_clearing_normal_user_configuration() {
    let mut command = Command::new("unused-test-command");
    command
        .env("CODEX_HOME", "must-not-inherit")
        .env("HTTPS_PROXY", "keep-normal-user-setting");
    remove_private_environment(&mut command);
    let overrides: std::collections::HashMap<_, _> = command.get_envs().collect();
    for key in PRIVATE_KEYS {
        assert_eq!(overrides.get(OsStr::new(key)), Some(&None));
    }
    assert_eq!(
        overrides.get(OsStr::new("HTTPS_PROXY")),
        Some(&Some(OsStr::new("keep-normal-user-setting")))
    );
}

#[cfg(windows)]
mod native {
    use super::*;

    fn file(root: &Path, name: &str, content: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn capture(owner: &mut DropOwner, paths: Vec<PathBuf>, now: Instant) -> DropSummary {
        owner.capture(paths, "scope-a", now).unwrap()
    }

    #[test]
    fn executable_drop_is_only_a_single_use_registration_candidate() {
        let root = tempfile::tempdir().unwrap();
        let exe = file(root.path(), "应用.EXE", b"not executed");
        let mut owner = DropOwner::default();
        let now = Instant::now();
        let summary = capture(&mut owner, vec![exe.clone()], now);
        assert_eq!(summary.kind, DropKind::RegisterExecutable);
        assert_eq!(summary.file_names, ["应用.EXE"]);
        assert_eq!(summary.expires_in_ms, 60_000);
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains(&root.path().to_string_lossy().to_string()));
        assert_eq!(
            owner
                .take_registration(&summary.batch_id, "scope-a", now)
                .unwrap(),
            exe.canonicalize().unwrap()
        );
        assert!(owner
            .take_registration(&summary.batch_id, "scope-a", now)
            .is_err());
        assert!(owner.current("scope-a", now).is_none());
    }

    #[test]
    fn files_are_single_use_and_executable_cannot_be_opened_instead_of_registered() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let exe = file(root.path(), "app.exe", b"not executed");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![document.clone()], now);
        assert_eq!(summary.kind, DropKind::OpenFiles);
        let selection = owner.take_files(&summary.batch_id, "scope-a", now).unwrap();
        assert_eq!(selection.files[0].path, document.canonicalize().unwrap());
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        let summary = capture(&mut owner, vec![exe], now);
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        assert!(owner
            .take_registration(&summary.batch_id, "scope-a", now)
            .is_err());
        let summary = capture(&mut owner, vec![document], now);
        assert!(owner
            .take_registration(&summary.batch_id, "scope-a", now)
            .is_err());
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
    }

    #[test]
    fn limit_mixed_and_duplicate_attempts_revoke_previous_batch() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let exe = file(root.path(), "app.exe", b"not executed");
        let second_exe = file(root.path(), "other.exe", b"not executed");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        for paths in [
            vec![],
            vec![document.clone(); 9],
            vec![document.clone(), document.clone()],
            vec![exe.clone(), document.clone()],
            vec![exe, second_exe],
        ] {
            let previous = capture(&mut owner, vec![document.clone()], now);
            assert!(owner.capture(paths, "scope-a", now).is_err());
            assert!(owner
                .take_files(&previous.batch_id, "scope-a", now)
                .is_err());
            assert!(owner.current("scope-a", now).is_none());
        }
        let paths = (0..8)
            .map(|index| file(root.path(), &format!("{index}.txt"), b"ok"))
            .collect();
        let summary = capture(&mut owner, paths, now);
        assert_eq!(summary.file_names.len(), 8);
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_ok());
    }

    #[test]
    fn expiry_scope_change_clear_and_clock_rollback_invalidate_tokens() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![document.clone()], now);
        assert_eq!(
            owner
                .current("scope-a", now + Duration::from_millis(59_999))
                .unwrap()
                .expires_in_ms,
            1
        );
        assert!(owner
            .take_files(&summary.batch_id, "scope-a", now + DROP_TTL)
            .is_err());
        let summary = capture(&mut owner, vec![document.clone()], now);
        assert!(owner.current("scope-b", now).is_none());
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        let summary = capture(&mut owner, vec![document.clone()], now);
        owner.clear();
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        let later = now + Duration::from_secs(1);
        let summary = capture(&mut owner, vec![document], later);
        assert!(owner.current("scope-a", now).is_none());
        assert!(owner
            .take_files(&summary.batch_id, "scope-a", later)
            .is_err());
    }

    #[test]
    fn stale_cancel_and_unknown_token_do_not_clear_the_new_batch() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let old = capture(&mut owner, vec![document.clone()], now);
        let new = capture(&mut owner, vec![document], now);
        assert_ne!(old.batch_id, new.batch_id);
        assert!(!owner.cancel(&old.batch_id));
        assert!(owner.take_files(&old.batch_id, "scope-a", now).is_err());
        assert_eq!(
            owner.current("scope-a", now).unwrap().batch_id,
            new.batch_id
        );
        assert!(owner.cancel(&new.batch_id));
        assert!(owner.current("scope-a", now).is_none());
    }

    #[test]
    fn changed_replaced_or_missing_files_fail_after_consuming_token() {
        let root = tempfile::tempdir().unwrap();
        let path = file(root.path(), "document.txt", b"first");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![path.clone()], now);
        fs::write(&path, b"changed length").unwrap();
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        assert!(owner.current("scope-a", now).is_none());
        let summary = capture(&mut owner, vec![path.clone()], now);
        fs::rename(&path, root.path().join("retained-old.txt")).unwrap();
        fs::write(&path, b"changed length").unwrap();
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        assert!(owner.current("scope-a", now).is_none());
        let summary = capture(&mut owner, vec![path.clone()], now);
        fs::remove_file(&path).unwrap();
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        assert!(owner.current("scope-a", now).is_none());
    }

    #[test]
    fn changed_file_after_consumption_cannot_be_used_to_prepare_launch() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"first");
        let target = file(root.path(), "app.exe", b"not executed");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![document.clone()], now);
        let selection = owner.take_files(&summary.batch_id, "scope-a", now).unwrap();
        fs::rename(&document, root.path().join("retained-old.txt")).unwrap();
        fs::write(&document, b"first").unwrap();
        assert!(prepare_file_command(&target, &selection).is_err());
    }

    #[test]
    fn issued_but_unlaunched_selection_is_revoked_by_cancel_clear_scope_expiry_replacement_or_stop()
    {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let target = file(root.path(), "app.exe", b"not executed");
        let now = Instant::now();
        for revoke in 0..7 {
            let mut owner = DropOwner::default();
            let summary = capture(&mut owner, vec![document.clone()], now);
            let selection = owner.take_files(&summary.batch_id, "scope-a", now).unwrap();
            // 同域只读查询看到已取走的批次时，不额外撤销正在执行的显式选择。
            assert!(owner.current("scope-a", now).is_none());
            assert!(prepare_file_command(&target, &selection).is_ok());
            match revoke {
                0 => assert!(owner.cancel(&summary.batch_id)),
                1 => owner.clear(),
                2 => {
                    assert!(owner.current("scope-b", now).is_none());
                }
                3 => {
                    assert!(owner.current("scope-a", now + DROP_TTL).is_none());
                }
                4 => {
                    capture(&mut owner, vec![document.clone()], now);
                }
                5 => {
                    assert!(owner.capture(vec![], "scope-a", now).is_err());
                }
                6 => drop(owner),
                _ => unreachable!(),
            }
            assert!(
                prepare_file_command(&target, &selection).is_err(),
                "revoke case {revoke}"
            );
        }
    }

    #[test]
    fn issued_selection_enforces_its_own_deadline_even_without_an_owner_poll() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let target = file(root.path(), "app.exe", b"not executed");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![document], now);
        let mut selection = owner.take_files(&summary.batch_id, "scope-a", now).unwrap();
        selection.lease.created_at = now.checked_sub(DROP_TTL).unwrap();
        assert!(prepare_file_command(&target, &selection).is_err());
    }

    #[test]
    fn last_write_change_without_length_change_invalidates_original_identity() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"same length");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![document.clone()], now);
        fs::OpenOptions::new()
            .write(true)
            .open(&document)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(100_000))
            .unwrap();
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        assert!(owner.current("scope-a", now).is_none());
    }

    fn junction(link: &Path, target: &Path) {
        // 仅创建测试夹具；产品的文件转交从不经过命令解释器。
        let result = Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(result.status.success(), "无法创建 junction 测试夹具");
    }

    #[test]
    fn junction_ancestor_is_rejected_on_capture_and_if_inserted_before_consumption() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("original");
        fs::create_dir(&parent).unwrap();
        let document = file(&parent, "document.txt", b"document");
        let alias = root.path().join("alias");
        junction(&alias, &parent);
        let now = Instant::now();
        let mut owner = DropOwner::default();
        assert!(owner
            .capture(vec![alias.join("document.txt")], "scope-a", now)
            .is_err());
        fs::remove_dir(&alias).unwrap();
        let summary = capture(&mut owner, vec![document], now);
        let moved = root.path().join("retained-original");
        fs::rename(&parent, &moved).unwrap();
        junction(&parent, &moved);
        assert!(owner.take_files(&summary.batch_id, "scope-a", now).is_err());
        assert!(owner.current("scope-a", now).is_none());
        fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn directories_missing_relative_network_device_traversal_and_ads_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let document = file(root.path(), "document.txt", b"document");
        let mut owner = DropOwner::default();
        let now = Instant::now();
        for path in [
            root.path().to_owned(),
            root.path().join("missing.txt"),
            PathBuf::from("document.txt"),
            PathBuf::from(r"\\server\share\document.txt"),
            PathBuf::from(r"\\?\UNC\server\share\document.txt"),
            PathBuf::from(r"\\.\C:\document.txt"),
            root.path().join("sub").join("..").join("document.txt"),
            PathBuf::from(format!("{}:hidden", document.display())),
        ] {
            assert!(
                owner.capture(vec![path.clone()], "scope-a", now).is_err(),
                "rejected {path:?}"
            );
            assert!(owner.current("scope-a", now).is_none());
        }
    }

    #[test]
    fn command_uses_registered_executable_and_separate_absolute_arguments_without_shell() {
        let root = tempfile::tempdir().unwrap();
        let first = file(root.path(), "first file & $dollar.txt", b"one");
        let second = file(root.path(), "二号;file.txt", b"two");
        let target = file(root.path(), "registered app.exe", b"not executed");
        let now = Instant::now();
        let mut owner = DropOwner::default();
        let summary = capture(&mut owner, vec![first.clone(), second.clone()], now);
        let selection = owner.take_files(&summary.batch_id, "scope-a", now).unwrap();
        let command = prepare_file_command(&target, &selection).unwrap();
        assert_eq!(
            command.get_program(),
            target.canonicalize().unwrap().as_os_str()
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                first.canonicalize().unwrap(),
                second.canonicalize().unwrap()
            ]
            .iter()
            .map(|path| path.as_os_str())
            .collect::<Vec<_>>()
        );
        assert_eq!(
            command.get_current_dir(),
            target.canonicalize().unwrap().parent()
        );
        let invalid_target = root.path().join("not-an-app.txt");
        fs::write(&invalid_target, b"not executed").unwrap();
        assert!(prepare_file_command(&invalid_target, &selection).is_err());
    }
}
