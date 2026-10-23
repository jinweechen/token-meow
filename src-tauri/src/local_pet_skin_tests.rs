use super::*;
use serde_json::{json, Value};

fn declaration() -> Value {
    let sequence = json!([{"frame":"front","durationMs":160,"segment":"body"}]);
    let clips: Vec<Value> = REQUIRED_CLIPS
        .iter()
        .map(|id| {
            let loops = matches!(*id, "idle" | "active" | "alert");
            let mut clip = json!({"id":id,"fallback":"front","canvas":[1024,1536],
            "sharedTransform":{"scale":1,"anchor":[512,1490],"translate":[0,0]},
            "loop":loops,"sequence":sequence});
            if loops {
                clip["loopRange"] = json!({"start":0,"end":1});
            }
            clip
        })
        .collect();
    json!({"schema_version":1,"renderer_kind":"local-png-clips","name":"本地测试皮肤",
        "author":"Fixture author","source":"Project-owned test fixture",
        "license":"LicenseRef-Project-Owned","attribution":"Test only",
        "bounds_alpha_threshold":127,"static_pose":"idle",
        "frames":[{"id":"front","path":"frames/front.png","width":1024,"height":1536,
            "bytes":45,"sha256":"0".repeat(64),"bounds":[93,11,937,1484]}],"clips":clips})
}

fn parse(value: &Value) -> Result<LocalPetSkinPackage> {
    parse_package(&serde_json::to_vec(value).unwrap())
}

fn source_fixture(directory: &Path) -> Vec<u8> {
    let image = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/pets/nekomi/walk-v10-complete/model-reference/master-front-v2.png");
    let bytes = fs::read(image).unwrap();
    let mut manifest = declaration();
    manifest["frames"][0]["bytes"] = json!(bytes.len());
    manifest["frames"][0]["sha256"] = json!(digest(&bytes));
    fs::create_dir_all(directory.join("frames")).unwrap();
    fs::write(directory.join("frames/front.png"), &bytes).unwrap();
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    bytes
}

#[test]
fn rejects_forged_capability_unknown_duplicate_and_escaped_paths() {
    assert!(parse(&declaration()).is_ok());
    let mut gait = declaration();
    gait["nativeGait"] = json!(true);
    assert!(parse(&gait).is_err());
    for path in [
        "../front.png",
        "frames/../front.png",
        "C:/front.png",
        "https://host/front.png",
        "frames\\front.png",
    ] {
        let mut value = declaration();
        value["frames"][0]["path"] = json!(path);
        assert!(parse(&value).is_err(), "{path}");
    }
    let mut reserved = declaration();
    reserved["frames"][0]["id"] = json!("con");
    assert!(parse(&reserved).is_err());
    let mut duplicate = declaration();
    let frame = duplicate["frames"][0].clone();
    duplicate["frames"].as_array_mut().unwrap().push(frame);
    assert!(parse(&duplicate).is_err());
    let raw = serde_json::to_string(&declaration()).unwrap().replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    assert!(parse_package(raw.as_bytes()).is_err());
}

#[test]
fn enforces_playback_dimensions_metadata_and_slot_budgets() {
    for (key, value) in [
        ("name", json!("bad\u{202e}name")),
        ("license", json!("")),
        ("author", json!("x".repeat(257))),
    ] {
        let mut manifest = declaration();
        manifest[key] = value;
        assert!(parse(&manifest).is_err());
    }
    let mut width = declaration();
    width["frames"][0]["width"] = json!(100_000);
    assert!(parse(&width).is_err());
    let mut oversize = declaration();
    oversize["frames"][0]["bytes"] = json!(MAX_FRAME_BYTES + 1);
    assert!(parse(&oversize).is_err());
    for duration in [0, 15, 10_001, 60_000] {
        let mut manifest = declaration();
        manifest["clips"][0]["sequence"][0]["durationMs"] = json!(duration);
        assert!(parse(&manifest).is_err());
    }
    let mut range = declaration();
    range["clips"][0]["loopRange"]["end"] = json!(2);
    assert!(parse(&range).is_err());
    let mut finite = declaration();
    finite["clips"][2]["loopRange"] = json!({"start":0,"end":1});
    assert!(parse(&finite).is_err());
    let mut missing = declaration();
    missing["clips"].as_array_mut().unwrap().remove(3);
    assert!(parse(&missing).is_err());
    let mut slots = declaration();
    let mut references = Vec::new();
    for index in 0..13 {
        let id = format!("f{index}");
        slots["frames"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"path":format!("frames/f{index}.png"),
            "width":1024,"height":1536,"bytes":45,"sha256":"0".repeat(64),"bounds":[1,1,100,100]}));
        references.push(json!({"frame":id,"durationMs":160}));
    }
    slots["clips"][0]["sequence"] = json!(references);
    assert!(parse(&slots).is_err());
}

#[test]
fn png_structure_rejects_crc_truncation_apng_and_trailing_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let bytes = source_fixture(temporary.path());
    inspect_png(&bytes).unwrap();
    let mut crc = bytes.clone();
    crc[29] ^= 1;
    assert!(inspect_png(&crc).is_err());
    assert!(inspect_png(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(inspect_png(&trailing).is_err());
    let mut apng = Vec::new();
    apng.extend_from_slice(&bytes[..33]);
    apng.extend_from_slice(&8u32.to_be_bytes());
    let begin = apng.len();
    apng.extend_from_slice(b"acTL");
    apng.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
    let checksum = crc32(&apng[begin..]);
    apng.extend_from_slice(&checksum.to_be_bytes());
    apng.extend_from_slice(&bytes[33..]);
    assert!(inspect_png(&apng).is_err());
}

#[test]
fn import_is_isolated_and_selection_is_explicit_and_reversible() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    let original = source_fixture(&source);
    let data = temporary.path().join("data");
    fs::create_dir(&data).unwrap();
    let mut store = LocalPetSkinStore::new(&data).unwrap();
    let package = store.import_directory(&source).unwrap();
    assert!(valid_skin_id(&package.skin_id));
    assert_eq!(store.state().unwrap().selected_skin_id, None);
    assert_eq!(store.state().unwrap().skins.len(), 1);
    fs::write(source.join("frames/front.png"), b"changed external source").unwrap();
    assert_eq!(
        store.read_frame(&package.skin_id, "front").unwrap(),
        original
    );
    assert_eq!(
        store
            .select(Some(&package.skin_id))
            .unwrap()
            .selected_skin_id
            .as_deref(),
        Some(package.skin_id.as_str())
    );
    assert!(store.read_frame(&package.skin_id, "../front").is_err());
    assert!(store.read_frame(&"f".repeat(64), "front").is_err());
    assert!(store.import_directory(&source).is_err());
    assert_eq!(
        store.state().unwrap().selected_skin_id.as_deref(),
        Some(package.skin_id.as_str())
    );
    assert!(store.remove(&package.skin_id).is_err());
    assert_eq!(store.select(None).unwrap().selected_skin_id, None);
    assert!(store.remove(&package.skin_id).unwrap().skins.is_empty());
    assert!(!data.join("ledger.sqlite3").exists());
}

#[test]
fn host_confirmed_inactive_skin_can_be_removed_without_rewriting_a_broken_old_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    source_fixture(&source);
    let data = temporary.path().join("data");
    fs::create_dir(&data).unwrap();
    let mut store = LocalPetSkinStore::new(&data).unwrap();
    let package = store.import_directory(&source).unwrap();
    let selection = data.join("local-pet-skins-v1/selection.json");
    fs::write(&selection, b"broken selection").unwrap();
    let state = store.remove_inactive(&package.skin_id).unwrap();
    assert!(state.skins.is_empty());
    assert!(!state.problems.is_empty());
    assert_eq!(fs::read(selection).unwrap(), b"broken selection");
    assert!(store.remove_inactive("../foreign").is_err());
}

#[test]
fn rejects_extra_files_and_detects_installed_frame_corruption_before_select() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    source_fixture(&source);
    let data = temporary.path().join("data");
    fs::create_dir(&data).unwrap();
    let mut store = LocalPetSkinStore::new(&data).unwrap();
    fs::write(source.join("payload.js"), b"throw new Error()").unwrap();
    assert!(store.import_directory(&source).is_err());
    fs::remove_file(source.join("payload.js")).unwrap();
    let package = store.import_directory(&source).unwrap();
    fs::write(
        data.join("local-pet-skins-v1")
            .join(&package.skin_id)
            .join("frames/front.png"),
        b"corrupt",
    )
    .unwrap();
    assert!(store.read_frame(&package.skin_id, "front").is_err());
    assert!(store.select(Some(&package.skin_id)).is_err());
    assert_eq!(store.state().unwrap().selected_skin_id, None);
}

#[cfg(windows)]
#[test]
fn directory_guards_allow_child_install_but_block_ancestor_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let store = temporary.path().join("store");
    fs::create_dir(&store).unwrap();
    let stage = store.join(".import-owned");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("manifest.json"), b"private fixture").unwrap();
    let installed = store.join("installed");
    // Untrusted directory chains keep the old deny-write/delete policy.
    let untrusted_held = guard_directory_chain(&store).unwrap();
    assert!(fs::rename(&stage, &installed).is_err());
    drop(untrusted_held);
    let held = guard_storage_directory_chain(&store, &store).unwrap();
    assert!(guard_storage_directory_chain(temporary.path(), &store).is_err());
    fs::rename(&stage, &installed).unwrap();
    assert!(fs::rename(&store, temporary.path().join("replaced-store")).is_err());
    fs::remove_file(installed.join("manifest.json")).unwrap();
    fs::remove_dir(&installed).unwrap();
    assert!(fs::remove_dir(&store).is_err());
    drop(held);
    fs::rename(&store, temporary.path().join("released-store")).unwrap();
}

#[cfg(windows)]
#[test]
fn source_read_handle_blocks_write_and_delete_until_import_scope_ends() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("held.png");
    fs::write(&file, b"private fixture").unwrap();
    let held = guarded_file(&file).unwrap();
    assert!(OpenOptions::new().write(true).open(&file).is_err());
    assert!(fs::remove_file(&file).is_err());
    drop(held);
    fs::remove_file(&file).unwrap();
}

#[test]
fn damaged_manifest_and_selection_do_not_block_builtin_or_package_removal() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    source_fixture(&source);
    let data = temporary.path().join("data");
    fs::create_dir(&data).unwrap();
    let mut store = LocalPetSkinStore::new(&data).unwrap();
    let package = store.import_directory(&source).unwrap();
    store.select(Some(&package.skin_id)).unwrap();
    let root = data.join("local-pet-skins-v1");
    fs::write(root.join(&package.skin_id).join("manifest.json"), b"{}").unwrap();
    let broken = store.state().unwrap();
    assert_eq!(broken.selected_skin_id, None);
    assert!(broken.skins.is_empty());
    assert!(broken
        .problems
        .iter()
        .any(|p| p.skin_id.as_deref() == Some(package.skin_id.as_str())));
    assert_eq!(store.select(None).unwrap().selected_skin_id, None);
    fs::write(root.join("selection.json"), b"not json").unwrap();
    let broken_selection = store.state().unwrap();
    assert_eq!(broken_selection.selected_skin_id, None);
    assert!(broken_selection
        .problems
        .iter()
        .any(|p| p.skin_id.is_none()));
    store.select(None).unwrap();
    let recovered = store.remove(&package.skin_id).unwrap();
    assert!(recovered.skins.is_empty() && recovered.problems.is_empty());
    assert!(!root.join(package.skin_id).exists());
}

#[test]
fn full_store_reclaims_one_damaged_slot_without_replacing_other_packages() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    source_fixture(&source);
    let data = temporary.path().join("data");
    fs::create_dir(&data).unwrap();
    let mut store = LocalPetSkinStore::new(&data).unwrap();
    let root = data.join("local-pet-skins-v1");
    for index in 0..8 {
        let path = root.join(format!("{index:064x}"));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("manifest.json"), b"{}").unwrap();
    }
    assert_eq!(store.state().unwrap().problems.len(), 8);
    assert!(store.import_directory(&source).is_err());
    store.remove(&"0".repeat(64)).unwrap();
    let package = store.import_directory(&source).unwrap();
    assert_eq!(store.state().unwrap().skins[0].skin_id, package.skin_id);
    for index in 1..8 {
        assert_eq!(
            fs::read(root.join(format!("{index:064x}")).join("manifest.json")).unwrap(),
            b"{}"
        );
    }
    assert!(store.remove("../ledger").is_err());
    assert!(data.is_dir());
}

#[cfg(windows)]
#[test]
fn selection_replacement_requires_external_read_guard_to_be_released() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    source_fixture(&source);
    let data = temporary.path().join("data");
    fs::create_dir(&data).unwrap();
    let mut store = LocalPetSkinStore::new(&data).unwrap();
    let package = store.import_directory(&source).unwrap();
    store.select(Some(&package.skin_id)).unwrap();
    let root = data.join("local-pet-skins-v1");
    let held = guarded_file(&root.join("selection.json")).unwrap();
    assert!(store.select(None).is_err());
    assert_eq!(
        store.state().unwrap().selected_skin_id.as_deref(),
        Some(package.skin_id.as_str())
    );
    drop(held);
    assert_eq!(store.select(None).unwrap().selected_skin_id, None);
    assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".selection-")));
}

#[test]
fn diagnostics_fit_frontend_text_and_count_contract_even_for_hostile_errors() {
    let mut problems: Vec<LocalPetSkinProblem> = (0..40)
        .map(|_| LocalPetSkinProblem {
            skin_id: None,
            message: format!(" \n\u{202e}{}\t ", "诊".repeat(1000)),
        })
        .collect();
    normalize_problems(&mut problems);
    assert_eq!(problems.len(), 32);
    for problem in problems {
        assert!(!problem.message.is_empty() && problem.message.len() <= 2048);
        assert_eq!(problem.message, problem.message.trim());
        assert!(!problem
            .message
            .chars()
            .any(|c| matches!(c, '\n' | '\t' | '\u{202e}')));
    }
}
