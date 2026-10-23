use super::*;
use serde_json::json;

#[test]
fn launcher_management_payloads_reject_paths_lists_nulls_and_invalid_types() {
    for value in [
        json!({"launcherId":"known", "label":null}),
        json!({"launcherId":"known", "label":7}),
        json!({"launcherId":"known", "workAppEnabled":null}),
        json!({"launcherId":"known", "workAppEnabled":"true"}),
        json!({"launcherId":"known", "label":"应用", "executable":"C:\\other.exe"}),
        json!({"launcherId":"known", "entries":[]}),
        json!({"label":"应用"}),
    ] {
        assert!(serde_json::from_value::<LauncherUpdateOptions>(value).is_err());
    }
    for value in [
        json!({"launcherId":"known", "direction":"first"}),
        json!({"launcherId":"known", "direction":null}),
        json!({"launcherId":"known", "direction":1}),
        json!({"launcherId":"known", "direction":"up", "entries":[]}),
        json!({"launcherId":"known", "direction":"down", "executable":"C:\\other.exe"}),
    ] {
        assert!(serde_json::from_value::<LauncherMoveOptions>(value).is_err());
    }
    let update: LauncherUpdateOptions = serde_json::from_value(json!({
        "launcherId":"known", "workAppEnabled":false,
    })).unwrap();
    assert!(update.label.is_none());
    assert_eq!(update.work_app_enabled, Some(false));
    assert!(matches!(serde_json::from_value::<LauncherMoveOptions>(json!({
        "launcherId":"known", "direction":"up",
    })).unwrap().direction, LauncherMoveDirection::Up));
}

#[test]
fn launcher_management_label_uses_unicode_character_limit_and_rejects_hidden_controls() {
    assert_eq!(normalized_launcher_label("  常用编辑器  ").unwrap(), "常用编辑器");
    assert_eq!(normalized_launcher_label(&format!(" {} ", "🐈".repeat(80))).unwrap(), "🐈".repeat(80));
    assert!(normalized_launcher_label(&"🐈".repeat(81)).is_err());
    assert!(normalized_launcher_label("").is_err());
    assert!(normalized_launcher_label(" \u{3000} ").is_err());
    for character in ['\0', '\t', '\n', '\u{007f}', '\u{0085}', '\u{009f}',
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}',
        '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'] {
        assert!(normalized_launcher_label(&format!("{character}应用")).is_err(), "{character:?}");
        assert!(normalized_launcher_label(&format!("应用{character}")).is_err(), "{character:?}");
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn default_registration_removes_direction_controls_and_survives_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("查看\u{202e}txt.exe");
        fs::write(&path, b"registration only").unwrap();
        let mut owner = Companion::new(temp.path());
        let entry = owner.register(path).unwrap();
        assert_eq!(entry.label, "查看\u{fffd}txt");
        assert_eq!(normalized_launcher_label(&entry.label).unwrap(), entry.label);
        assert_eq!(Companion::new(temp.path()).list().unwrap()[0].label, entry.label);
    }

    fn registered(root: &Path) -> (Companion, Vec<LauncherSummary>, Vec<PathBuf>) {
        let mut owner = Companion::new(root);
        let mut summaries = Vec::new();
        let mut targets = Vec::new();
        for name in ["first.exe", "second.exe", "third.exe"] {
            let executable = root.join(name);
            fs::write(&executable, "仅用于登记测试，不会启动".as_bytes()).unwrap();
            let summary = owner.register(executable).unwrap();
            targets.push(owner.target(&summary.id).unwrap());
            summaries.push(summary);
        }
        (owner, summaries, targets)
    }

    fn update(id: &str, label: Option<&str>, enabled: Option<bool>) -> LauncherUpdateOptions {
        LauncherUpdateOptions {
            launcher_id: id.to_owned(),
            label: label.map(str::to_owned),
            work_app_enabled: enabled,
        }
    }

    fn movement(id: &str, direction: LauncherMoveDirection) -> LauncherMoveOptions {
        LauncherMoveOptions { launcher_id: id.to_owned(), direction }
    }

    fn ids(owner: &Companion) -> Vec<String> {
        owner.list().unwrap().into_iter().map(|entry| entry.id).collect()
    }

    fn memory(owner: &Companion) -> Value {
        serde_json::to_value(&owner.entries).unwrap()
    }

    fn assert_no_pending(root: &Path) {
        assert!(fs::read_dir(root).unwrap().all(|entry| !entry.unwrap()
            .file_name().to_string_lossy().contains("pending-")));
    }

    #[test]
    fn launcher_management_updates_restart_with_same_identity_target_and_opaque_summary() {
        let temp = tempfile::tempdir().unwrap();
        let (mut owner, entries, targets) = registered(temp.path());
        let id = &entries[0].id;
        owner.app_sample_at = Some(42);
        owner.active_app = Some(ActiveApp { id: id.clone(), label: entries[0].label.clone() });
        let changed = owner.update(update(id, Some("  常用编辑器  "), Some(false))).unwrap();
        assert_eq!(changed.id, *id);
        assert_eq!(changed.label, "常用编辑器");
        assert!(!changed.work_app_enabled);
        assert_eq!(owner.target(id).unwrap(), targets[0]);
        assert!(owner.app_sample_at.is_none());
        assert!(owner.active_app.is_none());
        let summary = serde_json::to_value(&changed).unwrap();
        assert_eq!(summary, json!({"id":id, "label":"常用编辑器", "workAppEnabled":false}));
        let reopened = Companion::new(temp.path());
        assert_eq!(serde_json::to_value(reopened.list().unwrap()).unwrap(),
            serde_json::to_value(owner.list().unwrap()).unwrap());
        assert_eq!(reopened.target(id).unwrap(), targets[0]);

        let enabled = owner.update(update(id, None, Some(true))).unwrap();
        assert_eq!(enabled.label, "常用编辑器");
        assert!(enabled.work_app_enabled);
        let unicode_label = "🐈".repeat(80);
        let renamed = owner.update(update(id, Some(&unicode_label), None)).unwrap();
        assert!(renamed.work_app_enabled);
        assert_eq!(renamed.label, unicode_label);
        let reopened = Companion::new(temp.path());
        assert_eq!(reopened.list().unwrap()[0].label, unicode_label);
        assert!(reopened.list().unwrap()[0].work_app_enabled);
        for (entry, target) in entries.iter().zip(&targets) {
            assert_eq!(reopened.target(&entry.id).unwrap(), *target);
            assert_eq!(fs::read(target).unwrap(), "仅用于登记测试，不会启动".as_bytes());
        }
        assert_no_pending(temp.path());
    }

    #[test]
    fn launcher_management_moves_use_latest_order_and_restart_preserves_every_target() {
        let temp = tempfile::tempdir().unwrap();
        let (mut owner, entries, targets) = registered(temp.path());
        owner.move_launcher(movement(&entries[2].id, LauncherMoveDirection::Up)).unwrap();
        assert_eq!(ids(&owner), vec![entries[0].id.clone(), entries[2].id.clone(), entries[1].id.clone()]);
        owner.move_launcher(movement(&entries[2].id, LauncherMoveDirection::Up)).unwrap();
        assert_eq!(ids(&owner), vec![entries[2].id.clone(), entries[0].id.clone(), entries[1].id.clone()]);
        owner.move_launcher(movement(&entries[2].id, LauncherMoveDirection::Down)).unwrap();
        let reopened = Companion::new(temp.path());
        assert_eq!(ids(&reopened), vec![entries[0].id.clone(), entries[2].id.clone(), entries[1].id.clone()]);
        for (entry, target) in entries.iter().zip(&targets) {
            assert_eq!(reopened.target(&entry.id).unwrap(), *target);
            assert_eq!(reopened.list().unwrap().iter().find(|item| item.id == entry.id).unwrap().label, entry.label);
        }
        assert_no_pending(temp.path());
    }

    #[test]
    fn launcher_management_invalid_updates_and_unknown_ids_keep_disk_memory_and_sampling() {
        let temp = tempfile::tempdir().unwrap();
        let (mut owner, entries, _) = registered(temp.path());
        let saved = fs::read(&owner.registry_path).unwrap();
        let original = memory(&owner);
        owner.app_sample_at = Some(42);
        owner.active_app = Some(ActiveApp { id: entries[0].id.clone(), label: entries[0].label.clone() });
        assert!(owner.update(update(&entries[0].id, None, None)).is_err());
        for label in ["".to_owned(), " \u{3000} ".to_owned(), "应用\n".to_owned(),
            "应用\u{202e}".to_owned(), "🐈".repeat(81)] {
            assert!(owner.update(update(&entries[0].id, Some(&label), Some(false))).is_err());
        }
        assert!(owner.update(update("unknown-id", Some("名称"), Some(false))).is_err());
        assert!(owner.move_launcher(movement("unknown-id", LauncherMoveDirection::Up)).is_err());
        assert_eq!(fs::read(&owner.registry_path).unwrap(), saved);
        assert_eq!(memory(&owner), original);
        assert_eq!(owner.app_sample_at, Some(42));
        assert_eq!(owner.active_app.as_ref().unwrap().id, entries[0].id);
        assert_no_pending(temp.path());
    }

    #[test]
    fn launcher_management_failed_replace_keeps_existing_file_memory_and_sampling() {
        let temp = tempfile::tempdir().unwrap();
        let (mut owner, entries, _) = registered(temp.path());
        let saved = fs::read(&owner.registry_path).unwrap();
        let original = memory(&owner);
        owner.app_sample_at = Some(42);
        owner.active_app = Some(ActiveApp { id: entries[0].id.clone(), label: entries[0].label.clone() });
        // 保留读写共享但不授予删除共享，使已有文件的原子替换确定失败。
        let registry_lock = OpenOptions::new().read(true).share_mode(0x1 | 0x2)
            .open(&owner.registry_path).unwrap();
        assert!(owner.update(update(&entries[0].id, Some("替换失败"), Some(false))).is_err());
        assert!(owner.move_launcher(movement(&entries[0].id, LauncherMoveDirection::Down)).is_err());
        assert_eq!(fs::read(&owner.registry_path).unwrap(), saved);
        assert_eq!(memory(&owner), original);
        assert_eq!(owner.app_sample_at, Some(42));
        assert_eq!(owner.active_app.as_ref().unwrap().label, entries[0].label);
        assert_no_pending(temp.path());
        drop(registry_lock);
        assert_eq!(memory(&Companion::new(temp.path())), original);
    }

    #[test]
    fn launcher_management_boundary_moves_and_identical_update_are_no_ops() {
        let temp = tempfile::tempdir().unwrap();
        let (mut owner, entries, _) = registered(temp.path());
        let saved = fs::read(&owner.registry_path).unwrap();
        let original = memory(&owner);
        owner.app_sample_at = Some(42);
        let registry_lock = OpenOptions::new().read(true).share_mode(0x1 | 0x2)
            .open(&owner.registry_path).unwrap();
        owner.move_launcher(movement(&entries[0].id, LauncherMoveDirection::Up)).unwrap();
        owner.move_launcher(movement(&entries[2].id, LauncherMoveDirection::Down)).unwrap();
        owner.update(update(&entries[0].id, Some(&entries[0].label), Some(true))).unwrap();
        assert_eq!(fs::read(&owner.registry_path).unwrap(), saved);
        assert_eq!(memory(&owner), original);
        assert_eq!(owner.app_sample_at, Some(42));
        assert_no_pending(temp.path());
        drop(registry_lock);
    }

    #[test]
    fn launcher_management_corrupt_registry_rejects_mutations_and_keeps_original_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("companion-launchers.json");
        let original = b"{ invalid registry";
        fs::write(&path, original).unwrap();
        let mut owner = Companion::new(temp.path());
        assert!(owner.update(update("unknown-id", Some("应用"), Some(false))).is_err());
        assert!(owner.move_launcher(movement("unknown-id", LauncherMoveDirection::Down)).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
        assert!(!owner.state().capabilities.launchers);
        assert_no_pending(temp.path());
    }
}
