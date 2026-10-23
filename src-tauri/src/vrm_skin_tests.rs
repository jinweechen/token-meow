use super::*;
use serde_json::json;

fn chunk(kind: &[u8; 4], body: &[u8], png: &mut Vec<u8>) {
    png.extend_from_slice(&(body.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(body);
    let mut checksum = kind.to_vec();
    checksum.extend_from_slice(body);
    png.extend_from_slice(&crc32(&checksum).to_be_bytes());
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = width.to_be_bytes().to_vec();
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &header, &mut png);
    // 一个像素的未压缩zlib块；大尺寸头仅供头部预算拒绝场景使用。
    let pixels = [0u8, 255, 0, 0, 255];
    let mut zlib = vec![0x78, 0x01, 0x01, 5, 0, 0xfa, 0xff];
    zlib.extend_from_slice(&pixels);
    let (mut a, mut b) = (1u32, 0u32);
    for byte in pixels {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    chunk(b"IDAT", &zlib, &mut png);
    chunk(b"IEND", &[], &mut png);
    png
}

fn append_view(document: &mut Value, bin: &mut Vec<u8>, bytes: &[u8]) -> usize {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let start = bin.len();
    bin.extend_from_slice(bytes);
    let views = document["bufferViews"].as_array_mut().unwrap();
    let id = views.len();
    views.push(json!({"buffer":0,"byteOffset":start,"byteLength":bytes.len()}));
    id
}

fn fixture() -> (Value, Vec<u8>) {
    let mut nodes = (0..15).map(|_| json!({})).collect::<Vec<_>>();
    for (parent, children) in [
        (0, vec![1, 3, 6]),
        (1, vec![2, 9, 12]),
        (3, vec![4]),
        (4, vec![5]),
        (6, vec![7]),
        (7, vec![8]),
        (9, vec![10]),
        (10, vec![11]),
        (12, vec![13]),
        (13, vec![14]),
    ] {
        nodes[parent]["children"] = json!(children);
    }
    nodes.push(json!({"mesh":0}));
    let bones = REQUIRED_BONES
        .iter()
        .enumerate()
        .map(|(id, bone)| ((*bone).to_owned(), json!({"node":id})))
        .collect::<Map<_, _>>();
    let mut document = json!({
        "asset":{"version":"2.0"},"buffers":[{"byteLength":0}],"bufferViews":[],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}, {"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}],
        "nodes":nodes,"meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1,"material":0}]}],
        "scenes":[{"nodes":[0,15]}],"scene":0,
        "images":[{"mimeType":"image/png","bufferView":2}],"textures":[{"source":0}],
        "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],
        "extensionsUsed":["VRMC_vrm"],"extensions":{"VRMC_vrm":{"specVersion":"1.0","humanoid":{"humanBones":bones},
            "meta":{"name":"最小VRM样本","authors":["本地测试"],"licenseUrl":"https://vrm.dev/licenses/1.0/","avatarPermission":"everyone","commercialUsage":"corporation","creditNotation":"required","allowRedistribution":false,"modification":"prohibited","thumbnailImage":0,"copyrightInformation":"测试版权","thirdPartyLicenses":"测试保留文字"}}}
    });
    let mut bin = Vec::new();
    let positions = [0f32, 0., 0., 1., 0., 0., 0., 1., 0.]
        .iter()
        .flat_map(|n| n.to_le_bytes())
        .collect::<Vec<_>>();
    append_view(&mut document, &mut bin, &positions);
    append_view(&mut document, &mut bin, &[0, 0, 1, 0, 2, 0]);
    append_view(&mut document, &mut bin, &png(1, 1));
    (document, bin)
}

fn glb(document: &Value, bin: &[u8]) -> Vec<u8> {
    let mut document = document.clone();
    document["buffers"][0]["byteLength"] = json!(bin.len());
    let mut json = serde_json::to_vec(&document).unwrap();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let mut raw = b"glTF".to_vec();
    raw.extend_from_slice(&2u32.to_le_bytes());
    raw.extend_from_slice(&((28 + json.len() + bin.len()) as u32).to_le_bytes());
    raw.extend_from_slice(&(json.len() as u32).to_le_bytes());
    raw.extend_from_slice(&0x4e4f534au32.to_le_bytes());
    raw.extend_from_slice(&json);
    raw.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    raw.extend_from_slice(&0x004e4942u32.to_le_bytes());
    raw.extend_from_slice(&bin);
    raw
}

fn source(root: &Path, name: &str, document: &Value, bin: &[u8]) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, glb(document, bin)).unwrap();
    path
}

#[test]
fn accepts_bounded_embedded_model_and_preserves_license_without_granting_permissions() {
    let (document, bin) = fixture();
    let raw = glb(&document, &bin);
    let model = inspect_model(&raw, true).unwrap();
    assert_eq!(model.model_id, digest(&raw));
    assert_eq!(model.bytes, raw.len() as u64);
    assert_eq!(model.meta, document["extensions"]["VRMC_vrm"]["meta"]);
    assert_eq!(model.meta["allowRedistribution"], false);
    assert_eq!(model.meta["modification"], "prohibited");
    assert_eq!(
        (
            model.budget.nodes,
            model.budget.joints,
            model.budget.triangles,
            model.budget.decoded_texture_bytes
        ),
        (16, 0, 1, 4)
    );
    let view = serde_json::to_value(model).unwrap();
    assert!(view.get("modelId").is_some());
    assert_eq!(view["budget"]["decodedTextureBytes"], 4);
}

#[test]
fn rejects_uri_script_compressed_or_unknown_extensions_and_old_vrm() {
    let (document, bin) = fixture();
    for (path, value) in [
        (
            "/buffers/0/uri",
            json!("data:application/octet-stream;base64,AAAA"),
        ),
        ("/images/0/uri", json!("https://remote/image.png")),
        ("/extensions/VRMC_vrm/specVersion", json!("0.0")),
        (
            "/extensions/VRMC_vrm/meta/licenseUrl",
            json!("https://not-the-vrm-license/"),
        ),
    ] {
        let mut changed = document.clone();
        if let Some(slot) = changed.pointer_mut(path) {
            *slot = value;
        } else {
            let split = path.rfind('/').unwrap();
            changed
                .pointer_mut(&path[..split])
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(path[split + 1..].to_owned(), value);
        }
        assert!(
            inspect_model(&glb(&changed, &bin), false).is_err(),
            "拒绝{path}"
        );
    }
    for extension in [
        "KHR_draco_mesh_compression",
        "EXT_meshopt_compression",
        "KHR_texture_basisu",
        "unrecognized_extension",
    ] {
        let mut changed = document.clone();
        changed["extensionsUsed"]
            .as_array_mut()
            .unwrap()
            .push(json!(extension));
        changed["extensionsRequired"] = json!([extension]);
        assert!(inspect_model(&glb(&changed, &bin), false).is_err());
    }
    let mut changed = document;
    changed["extras"] = json!({"scripts":["never execute"]});
    assert!(inspect_model(&glb(&changed, &bin), false).is_err());
}

#[test]
fn rejects_invalid_glb_layout_truncation_and_unknown_license_enums() {
    let (document, bin) = fixture();
    let raw = glb(&document, &bin);
    for length in [0, 8, 19, raw.len() - 1] {
        assert!(inspect_model(&raw[..length], false).is_err());
    }
    let mut changed = raw.clone();
    changed[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert!(inspect_model(&changed, false).is_err());
    let mut changed = raw;
    changed.extend_from_slice(&[0, 0, 0, 0]);
    let len = changed.len() as u32;
    changed[8..12].copy_from_slice(&len.to_le_bytes());
    assert!(inspect_model(&changed, false).is_err());
    for key in [
        "avatarPermission",
        "commercialUsage",
        "creditNotation",
        "modification",
    ] {
        let mut changed = document.clone();
        changed["extensions"]["VRMC_vrm"]["meta"][key] = json!("future-not-automatically-allowed");
        assert!(inspect_model(&glb(&changed, &bin), false).is_err());
    }
    let mut changed = document;
    changed["extensions"]["VRMC_vrm"]["meta"]["allowRedistribution"] = json!("true");
    assert!(inspect_model(&glb(&changed, &bin), false).is_err());
}

#[test]
fn rejects_cycles_multiple_parents_and_resource_indices_outside_bounds() {
    let (document, bin) = fixture();
    for (path, value) in [
        ("/nodes/0/children", json!([0])),
        ("/nodes/2/children", json!([0])),
        ("/nodes/2/children", json!([3])),
        ("/nodes/15/mesh", json!(999)),
        ("/meshes/0/primitives/0/attributes/POSITION", json!(999)),
        ("/meshes/0/primitives/0/material", json!(999)),
        ("/textures/0/source", json!(999)),
        ("/extensions/VRMC_vrm/meta/thumbnailImage", json!(999)),
        (
            "/extensions/VRMC_vrm/humanoid/humanBones/head/node",
            json!(999),
        ),
    ] {
        let mut changed = document.clone();
        let split = path.rfind('/').unwrap();
        if let Some(slot) = changed.pointer_mut(path) {
            *slot = value;
        } else {
            changed
                .pointer_mut(&path[..split])
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(path[split + 1..].into(), value);
        }
        assert!(
            inspect_model(&glb(&changed, &bin), false).is_err(),
            "拒绝{path}"
        );
    }
    let mut changed = bin;
    changed[36..38].copy_from_slice(&99u16.to_le_bytes());
    assert!(inspect_model(&glb(&document, &changed), false).is_err());
}

fn sparse_fixture() -> (Value, Vec<u8>, usize, usize) {
    let (mut document, mut bin) = fixture();
    let indices = append_view(&mut document, &mut bin, &[1, 2]);
    let values = [1f32, 0., 0., 0., 1., 0.]
        .iter()
        .flat_map(|n| n.to_le_bytes())
        .collect::<Vec<_>>();
    let values = append_view(&mut document, &mut bin, &values);
    document["accessors"][0]
        .as_object_mut()
        .unwrap()
        .remove("bufferView");
    document["accessors"][0]["sparse"] = json!({"count":2,"indices":{"bufferView":indices,"componentType":5121},"values":{"bufferView":values}});
    (document, bin, indices, values)
}

#[test]
fn accepts_sparse_base_zero_and_rejects_range_order_alignment_and_nonfinite_values() {
    let (document, bin, indices, values) = sparse_fixture();
    assert!(inspect_model(&glb(&document, &bin), true).is_ok());
    let start = document["bufferViews"][indices]["byteOffset"]
        .as_u64()
        .unwrap() as usize;
    for invalid in [[2u8, 1u8], [1, 3], [1, 1]] {
        let mut changed = bin.clone();
        changed[start..start + 2].copy_from_slice(&invalid);
        assert!(inspect_model(&glb(&document, &changed), false).is_err());
    }
    let mut changed = document.clone();
    changed["accessors"][0]["sparse"]["count"] = json!(4);
    assert!(inspect_model(&glb(&changed, &bin), false).is_err());
    let mut changed = document.clone();
    changed["accessors"][0]["sparse"]["values"]["byteOffset"] = json!(1);
    assert!(inspect_model(&glb(&changed, &bin), false).is_err());
    let start = document["bufferViews"][values]["byteOffset"]
        .as_u64()
        .unwrap() as usize;
    let mut changed = bin;
    changed[start..start + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(inspect_model(&glb(&document, &changed), false).is_err());
}

#[test]
fn rejects_nonfinite_base_values_and_expanded_sparse_allocation_bombs() {
    let (mut document, mut bin) = fixture();
    bin[..4].copy_from_slice(&f32::INFINITY.to_le_bytes());
    assert!(inspect_model(&glb(&document, &bin), false).is_err());
    let (sparse, bin, _, _) = sparse_fixture();
    document = sparse;
    document["accessors"][0]["count"] = json!(u32::MAX);
    assert!(inspect_model(&glb(&document, &bin), false)
        .unwrap_err()
        .contains("128MiB"));
}

#[test]
fn image_budget_includes_unreferenced_thumbnail_and_png_crc_precedes_decode() {
    let (mut document, mut bin) = fixture();
    let view = append_view(&mut document, &mut bin, &png(4096, 4096));
    document["images"] = json!([{"mimeType":"image/png","bufferView":view},{"mimeType":"image/png","bufferView":view}]);
    document["extensions"]["VRMC_vrm"]["meta"]["thumbnailImage"] = json!(1);
    assert!(inspect_model(&glb(&document, &bin), false)
        .unwrap_err()
        .contains("96MiB"));
    let (document, mut bin) = fixture();
    let view = &document["bufferViews"][2];
    let start = view["byteOffset"].as_u64().unwrap() as usize;
    bin[start + 29] ^= 1;
    assert!(inspect_model(&glb(&document, &bin), true)
        .unwrap_err()
        .contains("CRC"));
    assert!(inspect_png(&png(4097, 1)).is_err());
}

#[test]
fn import_select_restart_and_remove_do_not_change_sources_or_other_host_data() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("app");
    fs::create_dir(&app).unwrap();
    let sentinel = app.join("ledger-marker.txt");
    fs::write(&sentinel, b"not a VRM store").unwrap();
    let (document, bin) = fixture();
    let path = source(root.path(), "sample.vrm", &document, &bin);
    let raw = fs::read(&path).unwrap();
    let mut store = VrmSkinStore::new(&app).unwrap();
    let model = store.import_file(&path).unwrap();
    assert!(store.state().unwrap().selected_model_id.is_none());
    assert_eq!(store.state().unwrap().models.len(), 1);
    store.select(Some(&model.model_id)).unwrap();
    drop(store);
    let mut store = VrmSkinStore::new(&app).unwrap();
    assert_eq!(
        store.state().unwrap().selected_model_id.as_deref(),
        Some(model.model_id.as_str())
    );
    assert_eq!(store.read_model(&model.model_id).unwrap(), raw);
    assert!(store.remove(&model.model_id).is_err());
    store.select(None).unwrap();
    store.remove(&model.model_id).unwrap();
    assert!(store.state().unwrap().models.is_empty());
    assert_eq!(fs::read(&path).unwrap(), raw);
    assert_eq!(fs::read(sentinel).unwrap(), b"not a VRM store");
    assert!(store.read_model("../ledger-marker.txt").is_err());
}

#[test]
fn corrupt_model_and_failed_selection_preserve_previous_selection_bytes() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("app");
    fs::create_dir(&app).unwrap();
    let mut store = VrmSkinStore::new(&app).unwrap();
    let (mut document, bin) = fixture();
    let first = store
        .import_file(&source(root.path(), "first.vrm", &document, &bin))
        .unwrap();
    store.select(Some(&first.model_id)).unwrap();
    document["extensions"]["VRMC_vrm"]["meta"]["name"] = json!("第二个模型");
    let second = store
        .import_file(&source(root.path(), "second.vrm", &document, &bin))
        .unwrap();
    let selection = fs::read(store.root.join("selection.json")).unwrap();
    let path = store.model_path(&second.model_id).unwrap();
    let mut raw = fs::read(&path).unwrap();
    raw[20] ^= 1;
    fs::write(&path, &raw).unwrap();
    assert!(store.select(Some(&second.model_id)).is_err());
    assert_eq!(
        fs::read(store.root.join("selection.json")).unwrap(),
        selection
    );
    let state = store.state().unwrap();
    assert_eq!(
        state.selected_model_id.as_deref(),
        Some(first.model_id.as_str())
    );
    assert!(state
        .problems
        .iter()
        .any(|problem| problem.model_id.as_deref() == Some(second.model_id.as_str())));
    assert!(store.read_model(&second.model_id).is_err());
    store.remove(&second.model_id).unwrap();
    let path = store.model_path(&first.model_id).unwrap();
    fs::write(path, b"broken selected model").unwrap();
    assert!(store.state().unwrap().selected_model_id.is_none());
    assert!(store.remove(&first.model_id).is_err());
    store.select(None).unwrap();
    store.remove(&first.model_id).unwrap();
}

#[test]
fn rejects_ninth_import_and_keeps_exact_duplicates_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("app");
    fs::create_dir(&app).unwrap();
    let mut store = VrmSkinStore::new(&app).unwrap();
    let (mut document, bin) = fixture();
    let mut last = None;
    for n in 0..8 {
        document["extensions"]["VRMC_vrm"]["meta"]["name"] = json!(format!("模型{n}"));
        let path = source(root.path(), "next.vrm", &document, &bin);
        last = Some((path.clone(), store.import_file(&path).unwrap()));
    }
    let (path, model) = last.unwrap();
    assert_eq!(store.import_file(&path).unwrap().model_id, model.model_id);
    assert_eq!(store.state().unwrap().models.len(), 8);
    document["extensions"]["VRMC_vrm"]["meta"]["name"] = json!("第九个");
    let path = source(root.path(), "ninth.vrm", &document, &bin);
    assert!(store.import_file(&path).is_err());
    assert_eq!(store.state().unwrap().models.len(), 8);
    assert!(!fs::read_dir(&store.root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".import-")));
}

#[test]
fn invalid_selection_reports_problem_and_explicit_fallback_repairs_atomically() {
    let root = tempfile::tempdir().unwrap();
    let mut store = VrmSkinStore::new(root.path()).unwrap();
    let selection = store.root.join("selection.json");
    fs::write(&selection, b"not-json").unwrap();
    let state = store.state().unwrap();
    assert!(state.selected_model_id.is_none());
    assert!(!state.problems.is_empty());
    assert_eq!(fs::read(&selection).unwrap(), b"not-json");
    store.select(None).unwrap();
    assert!(store.state().unwrap().problems.is_empty());
}

#[test]
fn unknown_store_entries_keep_problem_projection_bounded_and_recoverable() {
    let root = tempfile::tempdir().unwrap();
    let store = VrmSkinStore::new(root.path()).unwrap();
    for index in 0..48 {
        fs::write(
            store.root.join(format!("unknown-{index}.txt")),
            b"not loaded",
        )
        .unwrap();
    }
    let state = store.state().unwrap();
    assert!(state.models.is_empty());
    assert_eq!(state.problems.len(), 32);
    assert!(state.problems.last().unwrap().message.contains("省略"));
}

#[test]
fn host_confirmed_inactive_removal_preserves_broken_legacy_selection_for_recovery() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("app");
    fs::create_dir(&app).unwrap();
    let mut store = VrmSkinStore::new(&app).unwrap();
    let (document, bin) = fixture();
    let model = store
        .import_file(&source(root.path(), "sample.vrm", &document, &bin))
        .unwrap();
    let path = store.model_path(&model.model_id).unwrap();
    let selection = store.root.join("selection.json");
    let broken = b"broken legacy selection, retained exactly";
    fs::write(&selection, broken).unwrap();
    assert!(store.remove(&model.model_id).is_err());
    assert!(path.exists());
    let state = store.remove_inactive(&model.model_id).unwrap();
    assert!(!path.exists());
    assert!(state.models.is_empty());
    assert!(state.selected_model_id.is_none());
    assert!(state
        .problems
        .iter()
        .any(|problem| problem.model_id.is_none() && problem.message.contains("选择记录")));
    assert_eq!(fs::read(selection).unwrap(), broken);
    assert!(store.remove_inactive("../selection.json").is_err());
    assert!(store.remove_inactive("selection.json").is_err());
}

#[cfg(windows)]
#[test]
fn host_confirmed_inactive_removal_rejects_junction_without_touching_target() {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("app");
    let target = root.path().join("retained-target");
    fs::create_dir(&app).unwrap();
    fs::create_dir(&target).unwrap();
    let marker = target.join("keep.txt");
    fs::write(&marker, b"unchanged target").unwrap();
    let mut store = VrmSkinStore::new(&app).unwrap();
    let id = "a".repeat(64);
    let link = store.model_path(&id).unwrap();
    fs::create_dir(&link).unwrap();
    // 在私有临时目录中创建mount-point reparse；不需要符号链接权限或外部进程。
    let printable = target
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    let substitute = format!(r"\??\{printable}")
        .encode_utf16()
        .collect::<Vec<_>>();
    let printable = printable.encode_utf16().collect::<Vec<_>>();
    let mut paths = substitute.clone();
    paths.push(0);
    paths.extend_from_slice(&printable);
    paths.push(0);
    let mut reparse = 0xa0000003u32.to_le_bytes().to_vec();
    reparse.extend_from_slice(&((8 + paths.len() * 2) as u16).to_le_bytes());
    reparse.extend_from_slice(&0u16.to_le_bytes());
    reparse.extend_from_slice(&0u16.to_le_bytes());
    reparse.extend_from_slice(&((substitute.len() * 2) as u16).to_le_bytes());
    reparse.extend_from_slice(&(((substitute.len() + 1) * 2) as u16).to_le_bytes());
    reparse.extend_from_slice(&((printable.len() * 2) as u16).to_le_bytes());
    for code in paths {
        reparse.extend_from_slice(&code.to_le_bytes());
    }
    let handle = fs::OpenOptions::new()
        .write(true)
        .share_mode(7)
        .custom_flags(0x02000000 | 0x00200000)
        .open(&link)
        .unwrap();
    let mut returned = 0u32;
    let result = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            0x000900a4,
            reparse.as_ptr().cast(),
            reparse.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        result,
        0,
        "创建临时junction失败：{}",
        std::io::Error::last_os_error()
    );
    drop(handle);
    assert!(store.remove_inactive(&id).is_err());
    assert_eq!(fs::read(&marker).unwrap(), b"unchanged target");
    fs::remove_dir(&link).unwrap();
}

/// 只有显式传入已授权的官方fixture路径才运行；缺少环境变量必须失败。
#[test]
#[ignore = "需要显式TOKEN_MEOW_VRM_AUDIT_MODEL，仅供官方模型隔离验收"]
fn official_fixture_import_select_restart_read_and_remove() {
    let path = PathBuf::from(
        std::env::var_os("TOKEN_MEOW_VRM_AUDIT_MODEL")
            .expect("必须显式提供TOKEN_MEOW_VRM_AUDIT_MODEL，不能跳过或读取默认个人目录"),
    );
    let raw = fs::read(&path).expect("读取显式授权的官方VRM样本");
    let expected = digest(&raw);
    let app = tempfile::tempdir().unwrap();
    let mut store = VrmSkinStore::new(app.path()).unwrap();
    let model = store.import_file(&path).unwrap();
    assert_eq!(model.model_id, expected);
    assert!(store.state().unwrap().selected_model_id.is_none());
    eprintln!("VRM_AUDIT_MODEL {}", serde_json::to_string(&model).unwrap());
    store.select(Some(&model.model_id)).unwrap();
    drop(store);
    let mut store = VrmSkinStore::new(app.path()).unwrap();
    assert_eq!(
        store.state().unwrap().selected_model_id.as_deref(),
        Some(model.model_id.as_str())
    );
    let stored = store.read_model(&model.model_id).unwrap();
    assert_eq!(stored, raw);
    assert_eq!(digest(&stored), expected);
    assert!(store.remove(&model.model_id).is_err());
    store.select(None).unwrap();
    store.remove(&model.model_id).unwrap();
    assert!(store.state().unwrap().models.is_empty());
}
