//! Local data-only PNG directory packages. No ledger/settings/account access.
//! Wire into the desktop feature only; PNG decoding uses its existing image-png dependency.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

pub const MAX_MANIFEST_BYTES: u64 = 128 * 1024;
pub const MAX_FRAME_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_INSTALLED_SKINS: usize = 8;
const FRAME_WIDTH: u32 = 1024;
const FRAME_HEIGHT: u32 = 1536;
const RGBA_BYTES: usize = FRAME_WIDTH as usize * FRAME_HEIGHT as usize * 4;
const CLIPS: [&str; 13] = [
    "idle",
    "active",
    "complete",
    "alert",
    "sleep",
    "shy",
    "affection",
    "stretch",
    "wink",
    "happy",
    "bow",
    "hug-tail",
    "walk",
];
const REQUIRED_CLIPS: [&str; 5] = ["idle", "active", "complete", "alert", "sleep"];
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPetSkinManifestV1 {
    pub schema_version: u32,
    pub renderer_kind: String,
    pub name: String,
    pub author: String,
    pub source: String,
    pub license: String,
    pub attribution: String,
    pub bounds_alpha_threshold: u8,
    pub static_pose: String,
    pub frames: Vec<LocalPetSkinFrame>,
    pub clips: Vec<LocalPetSkinClip>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPetSkinFrame {
    pub id: String,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha256: String,
    pub bounds: [u32; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPetSkinClip {
    pub id: String,
    pub fallback: String,
    pub canvas: [u32; 2],
    #[serde(rename = "sharedTransform")]
    pub shared_transform: LocalPetSkinTransform,
    #[serde(rename = "loop")]
    pub loops: bool,
    #[serde(rename = "loopRange", default, skip_serializing_if = "Option::is_none")]
    pub loop_range: Option<LocalPetSkinLoopRange>,
    pub sequence: Vec<LocalPetSkinStep>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPetSkinTransform {
    pub scale: f64,
    pub anchor: [i32; 2],
    pub translate: [i32; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPetSkinLoopRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalPetSkinStep {
    pub frame: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPetSkinPackage {
    /// SHA-256 of the exact manifest bytes, never a package-supplied identity.
    pub skin_id: String,
    pub manifest: LocalPetSkinManifestV1,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSkinState {
    pub selected_skin_id: Option<String>,
    pub skins: Vec<LocalPetSkinPackage>,
    pub problems: Vec<LocalPetSkinProblem>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPetSkinProblem {
    /// Only an opaque package ID is exposed for recovery; never a source path.
    pub skin_id: Option<String>,
    pub message: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    schema_version: u32,
    selected_skin_id: Option<String>,
}

/// One desktop-owned store; serialize mutating calls with the application's Mutex.
/// Callers supply the application's data directory, never a renderer-supplied store path.
pub struct LocalPetSkinStore {
    root: PathBuf,
}

impl LocalPetSkinStore {
    pub fn new(application_data_directory: &Path) -> Result<Self> {
        validate_absolute_local(application_data_directory)?;
        let _data_guards = guard_directory_chain(application_data_directory)?;
        let root = application_data_directory.join("local-pet-skins-v1");
        if !root.exists() {
            fs::create_dir(&root).map_err(io_error)?;
        }
        let _root_guards = guard_storage_directory_chain(&root, &root)?;
        Ok(Self { root })
    }

    pub fn state(&self) -> Result<PetSkinState> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        let (skins, mut problems, _) = self.scan_packages()?;
        let selected = match self.read_selection() {
            Ok(selected) => selected,
            Err(error) => {
                problems.push(LocalPetSkinProblem {
                    skin_id: None,
                    message: format!("选择记录不可用，已回退内置皮肤：{error}"),
                });
                None
            }
        };
        // A removed selected package falls back to the built-in. Persisted
        // selection is not rewritten by a read, and ledger facts are never touched.
        let selected_skin_id = selected.filter(|id| {
            if skins.iter().any(|skin| &skin.skin_id == id) {
                true
            } else {
                problems.push(LocalPetSkinProblem {
                    skin_id: Some(id.clone()),
                    message: "已选皮肤不可用，已回退内置皮肤".into(),
                });
                false
            }
        });
        normalize_problems(&mut problems);
        Ok(PetSkinState {
            selected_skin_id,
            skins,
            problems,
        })
    }

    pub fn import_directory(&mut self, source: &Path) -> Result<LocalPetSkinPackage> {
        validate_absolute_local(source)?;
        if source.starts_with(&self.root) {
            return Err("导入源必须在应用皮肤存储目录之外".into());
        }
        let _store_guards = guard_storage_directory_chain(&self.root, &self.root)?;
        let _source_guards = guard_directory_chain(source)?;
        let mut original_manifest = guarded_file(&source.join("manifest.json"))?;
        let raw = read_file_bounded(&mut original_manifest, MAX_MANIFEST_BYTES)?;
        let package = parse_package(&raw)?;
        let _frame_directory_guards = guard_directory_chain(&source.join("frames"))?;
        verify_roster(source, &package.manifest)?;
        // Hold every original file read-only/share-read through validation and copy.
        // The byte stream that is checked is the byte stream written to isolation.
        let mut originals = Vec::with_capacity(package.manifest.frames.len());
        for frame in &package.manifest.frames {
            originals.push(guarded_file(&source.join(&frame.path))?);
        }
        let destination = self.root.join(&package.skin_id);
        let already_installed = destination.exists();
        if !already_installed && self.scan_packages()?.2 >= MAX_INSTALLED_SKINS {
            return Err("本地皮肤最多保留8个，请先移除一个未使用的皮肤".into());
        }
        let stage = self.root.join(format!(".import-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&stage).map_err(io_error)?;
        fs::create_dir(stage.join("frames")).map_err(io_error)?;
        let mut created = Vec::new();
        let copied = (|| -> Result<()> {
            for (frame, original) in package.manifest.frames.iter().zip(originals.iter_mut()) {
                let bytes = read_file_bounded(original, MAX_FRAME_BYTES)?;
                validate_frame(&bytes, frame, true)?;
                let path = stage.join(&frame.path);
                created.push(path.clone());
                write_new(&path, &bytes)?;
            }
            created.push(stage.join("manifest.json"));
            write_new(&stage.join("manifest.json"), &raw)?;
            verify_roster(&stage, &package.manifest)?;
            Ok(())
        })();
        if let Err(error) = copied {
            cleanup_stage(&self.root, &stage, &created);
            return Err(error);
        }
        if already_installed {
            // An identical manifest cannot silently repair or replace an installed
            // package. Validate that copy; a corrupt existing copy is an error.
            let existing = (|| -> Result<LocalPetSkinPackage> {
                let existing = self.read_package(&package.skin_id)?;
                for frame in &existing.manifest.frames {
                    let _ = self.read_frame(&existing.skin_id, &frame.id)?;
                }
                Ok(existing)
            })();
            cleanup_stage(&self.root, &stage, &created);
            return existing;
        }
        // Fresh destination only. A second owner winning this identity is rejected;
        // there is no replace/remove-existing operation and no automatic selection.
        if destination.exists() {
            cleanup_stage(&self.root, &stage, &created);
            return Err("本地皮肤目录已存在，未覆盖任何已安装素材".into());
        }
        if let Err(error) = fs::rename(&stage, &destination) {
            cleanup_stage(&self.root, &stage, &created);
            return Err(io_error(error));
        }
        Ok(package)
    }

    pub fn select(&mut self, skin_id: Option<&str>) -> Result<PetSkinState> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        if let Some(id) = skin_id {
            let package = self.read_package(id)?;
            // A corrupt local copy must not replace the current selection.
            for frame in &package.manifest.frames {
                let _ = self.read_frame(id, &frame.id)?;
            }
        }
        self.write_selection(skin_id)?;
        self.state()
    }

    fn write_selection(&self, skin_id: Option<&str>) -> Result<()> {
        let selection = Selection {
            schema_version: 1,
            selected_skin_id: skin_id.map(str::to_owned),
        };
        let raw = serde_json::to_vec(&selection).map_err(json_error)?;
        let path = self.root.join("selection.json");
        if path.exists() {
            // This is only a type/reparse check. Explicitly release this share-read
            // handle before Windows MoveFileExW replaces the existing selection.
            let existing = guarded_file(&path)?;
            drop(existing);
        }
        let temporary = self
            .root
            .join(format!(".selection-{}.json", uuid::Uuid::new_v4()));
        write_new(&temporary, &raw)?;
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        Ok(())
    }

    /// Uninstall only an inactive opaque package directory. Preflight the whole
    /// bounded flat tree, then delete ordinary files without recursive traversal.
    /// Even a damaged manifest can be removed; selected healthy skins require an
    /// explicit switch first, so removal cannot silently change the shown role.
    pub fn remove(&mut self, skin_id: &str) -> Result<PetSkinState> {
        let _store_guards = guard_storage_directory_chain(&self.root, &self.root)?;
        if !valid_skin_id(skin_id) {
            return Err("非法本地皮肤标识".into());
        }
        if self.state()?.selected_skin_id.as_deref() == Some(skin_id) {
            return Err("请先切换到内置或其它皮肤，再移除此皮肤".into());
        }
        let path = self.root.join(skin_id);
        let package_guards = guard_storage_directory_chain(&path, &self.root)?;
        let frames = path.join("frames");
        let mut files = Vec::new();
        let mut has_frames = false;
        for entry in fs::read_dir(&path).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
            if metadata_is_reparse(&metadata) {
                return Err("不移除包含reparse point的皮肤目录".into());
            }
            if metadata.is_dir() && entry.file_name() == "frames" {
                has_frames = true;
            } else if metadata.is_file() {
                files.push(entry.path());
            } else {
                return Err("皮肤目录包含未知子目录，未删除任何文件".into());
            }
            if files.len() > 66 {
                return Err("皮肤移除预检文件数量超限".into());
            }
        }
        let frame_guards = if has_frames {
            Some(guard_storage_directory_chain(&frames, &self.root)?)
        } else {
            None
        };
        if has_frames {
            for entry in fs::read_dir(&frames).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
                if !metadata.is_file() || metadata_is_reparse(&metadata) {
                    return Err("皮肤帧包含链接或子目录，未删除任何文件".into());
                }
                files.push(entry.path());
                if files.len() > 130 {
                    return Err("皮肤移除预检文件数量超限".into());
                }
            }
        }
        // A broken selected package is already visually on the built-in fallback;
        // clear that stale persisted reference only after the removal preflight.
        if self.read_selection().ok().flatten().as_deref() == Some(skin_id) {
            self.write_selection(None)?;
        }
        for file in files {
            fs::remove_file(file).map_err(io_error)?;
        }
        drop(frame_guards);
        if has_frames {
            fs::remove_dir(&frames).map_err(io_error)?;
        }
        drop(package_guards);
        fs::remove_dir(&path).map_err(io_error)?;
        self.state()
    }

    /// Opaque installed skin identity + simple frame identity only. No arbitrary
    /// path, URL, source directory or caller-controlled data root enters this API.
    pub fn read_frame(&self, skin_id: &str, frame_id: &str) -> Result<Vec<u8>> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        if !valid_frame_id(frame_id) {
            return Err("非法皮肤帧标识".into());
        }
        let package = self.read_package(skin_id)?;
        let frame = package
            .manifest
            .frames
            .iter()
            .find(|frame| frame.id == frame_id)
            .ok_or_else(|| "皮肤帧不在已安装清单中".to_string())?;
        let _package_guards =
            guard_storage_directory_chain(&self.root.join(skin_id).join("frames"), &self.root)?;
        let bytes = read_file_bounded(
            &mut guarded_file(&self.root.join(skin_id).join(&frame.path))?,
            MAX_FRAME_BYTES,
        )?;
        validate_frame(&bytes, frame, false)?;
        Ok(bytes)
    }

    fn scan_packages(&self) -> Result<(Vec<LocalPetSkinPackage>, Vec<LocalPetSkinProblem>, usize)> {
        let mut ids = Vec::new();
        let mut problems = Vec::new();
        let mut entries = 0;
        for entry in fs::read_dir(&self.root).map_err(io_error)? {
            entries += 1;
            if entries > 128 {
                problems.push(LocalPetSkinProblem {
                    skin_id: None,
                    message: "本地皮肤存储目录条目超限；已停止枚举".into(),
                });
                break;
            }
            let entry = entry.map_err(io_error)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "本地皮肤目录名称无效".to_string())?;
            if valid_skin_id(&name) {
                ids.push(name);
            } else if name != "selection.json"
                && !name.starts_with(".import-")
                && !name.starts_with(".selection-")
            {
                problems.push(LocalPetSkinProblem {
                    skin_id: None,
                    message: "本地皮肤存储包含未知条目；该条目未加载".into(),
                });
            }
        }
        ids.sort();
        let occupied = if entries > 128 {
            MAX_INSTALLED_SKINS
        } else {
            ids.len()
        };
        let mut skins = Vec::new();
        for id in ids {
            match self.read_package(&id) {
                Ok(package) if skins.len() < MAX_INSTALLED_SKINS => skins.push(package),
                Ok(_) => problems.push(LocalPetSkinProblem {
                    skin_id: Some(id),
                    message: "皮肤数量超限；请移除此未启用包".into(),
                }),
                Err(error) => problems.push(LocalPetSkinProblem {
                    skin_id: Some(id),
                    message: format!("皮肤包不可用：{error}"),
                }),
            }
        }
        Ok((skins, problems, occupied))
    }

    fn read_package(&self, skin_id: &str) -> Result<LocalPetSkinPackage> {
        if !valid_skin_id(skin_id) {
            return Err("非法本地皮肤标识".into());
        }
        let path = self.root.join(skin_id);
        let _guards = guard_storage_directory_chain(&path, &self.root)?;
        let raw = read_file_bounded(
            &mut guarded_file(&path.join("manifest.json"))?,
            MAX_MANIFEST_BYTES,
        )?;
        if digest(&raw) != skin_id {
            return Err("本地皮肤清单完整性不符".into());
        }
        let package = parse_package(&raw)?;
        let _frame_guards = guard_storage_directory_chain(&path.join("frames"), &self.root)?;
        verify_roster(&path, &package.manifest)?;
        Ok(package)
    }

    fn read_selection(&self) -> Result<Option<String>> {
        let path = self.root.join("selection.json");
        if !path.exists() {
            return Ok(None);
        }
        let raw = read_file_bounded(&mut guarded_file(&path)?, 1024)?;
        let selection: Selection = serde_json::from_slice(&raw).map_err(json_error)?;
        if selection.schema_version != 1
            || selection
                .selected_skin_id
                .as_ref()
                .is_some_and(|id| !valid_skin_id(id))
        {
            return Err("本地皮肤选择记录无效".into());
        }
        Ok(selection.selected_skin_id)
    }
}

fn parse_package(raw: &[u8]) -> Result<LocalPetSkinPackage> {
    if raw.is_empty() || raw.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("皮肤清单超过128 KiB或为空".into());
    }
    let manifest: LocalPetSkinManifestV1 = serde_json::from_slice(raw).map_err(json_error)?;
    validate_manifest(&manifest)?;
    Ok(LocalPetSkinPackage {
        skin_id: digest(raw),
        manifest,
    })
}

fn validate_manifest(manifest: &LocalPetSkinManifestV1) -> Result<()> {
    if manifest.schema_version != 1
        || manifest.renderer_kind != "local-png-clips"
        || manifest.static_pose != "idle"
        || manifest.bounds_alpha_threshold != 127
    {
        return Err("不支持的本地PNG皮肤版本、渲染器或静态回退".into());
    }
    for (text, maximum) in [
        (&manifest.name, 128),
        (&manifest.author, 256),
        (&manifest.source, 1024),
        (&manifest.license, 256),
        (&manifest.attribution, 2048),
    ] {
        if !valid_text(text, maximum) {
            return Err("皮肤名称、作者、来源、许可或署名文本缺失/无效".into());
        }
    }
    if manifest.frames.is_empty()
        || manifest.frames.len() > 64
        || manifest.clips.len() < 5
        || manifest.clips.len() > 16
    {
        return Err("皮肤帧或片段数量超限".into());
    }
    let mut frames = HashSet::new();
    let mut total = 0u64;
    for frame in &manifest.frames {
        let [left, top, right, bottom] = frame.bounds;
        if !valid_frame_id(&frame.id)
            || !frames.insert(frame.id.as_str())
            || frame.path != format!("frames/{}.png", frame.id)
            || frame.width != FRAME_WIDTH
            || frame.height != FRAME_HEIGHT
            || frame.bytes < 45
            || frame.bytes > MAX_FRAME_BYTES
            || !valid_skin_id(&frame.sha256)
            || left >= right
            || top >= bottom
            || right > FRAME_WIDTH
            || bottom > FRAME_HEIGHT
        {
            return Err("皮肤帧标识、路径、尺寸、摘要或范围无效".into());
        }
        total = total
            .checked_add(frame.bytes)
            .ok_or_else(|| "皮肤素材大小溢出".to_string())?;
    }
    if total > MAX_PACKAGE_BYTES {
        return Err("皮肤素材超过128 MiB预算".into());
    }
    let mut ids = HashSet::new();
    let mut used = HashSet::new();
    for clip in &manifest.clips {
        if !CLIPS.contains(&clip.id.as_str())
            || !ids.insert(clip.id.as_str())
            || !frames.contains(clip.fallback.as_str())
            || clip.canvas != [FRAME_WIDTH, FRAME_HEIGHT]
            || clip.shared_transform.scale != 1.0
            || clip.shared_transform.anchor != [512, 1490]
            || clip.shared_transform.translate != [0, 0]
            || clip.sequence.is_empty()
            || clip.sequence.len() > 64
        {
            return Err("皮肤片段标识、静态回退、布局或序列无效".into());
        }
        let mut distinct = HashSet::from([clip.fallback.as_str()]);
        used.insert(clip.fallback.as_str());
        let mut milliseconds = 0u64;
        for step in &clip.sequence {
            if !frames.contains(step.frame.as_str())
                || !(16..=10_000).contains(&step.duration_ms)
                || step
                    .segment
                    .as_ref()
                    .is_some_and(|segment| !valid_text(segment, 64))
            {
                return Err("皮肤帧时长、阶段或引用无效".into());
            }
            milliseconds += u64::from(step.duration_ms);
            distinct.insert(step.frame.as_str());
            used.insert(step.frame.as_str());
        }
        if distinct.len() > 12 || milliseconds > 60_000 {
            return Err("单片段超过十二帧或60秒预算".into());
        }
        if let Some(range) = &clip.loop_range {
            if !clip.loops || range.start >= range.end || range.end > clip.sequence.len() {
                return Err("皮肤循环主体范围无效".into());
            }
        }
        // Completion is always finite; artwork cannot keep a success celebration
        // authoritative indefinitely. Walk is cosmetic data only, never a gait capability.
        if clip.id == "complete" && clip.loops {
            return Err("完成动作必须有限播放".into());
        }
        if clip.id == "idle" && !clip.loops {
            return Err("idle必须提供可重复主体".into());
        }
    }
    if REQUIRED_CLIPS.iter().any(|id| !ids.contains(id)) || used.len() != frames.len() {
        return Err("皮肤缺少五类基本动作或包含未引用帧".into());
    }
    Ok(())
}

fn validate_frame(raw: &[u8], frame: &LocalPetSkinFrame, decode: bool) -> Result<()> {
    if raw.len() as u64 != frame.bytes || digest(raw) != frame.sha256 {
        return Err("皮肤PNG长度或SHA-256不符".into());
    }
    inspect_png(raw)?;
    if decode {
        let image =
            tauri::image::Image::from_bytes(raw).map_err(|_| "皮肤PNG无法完整解码".to_string())?;
        if image.width() != FRAME_WIDTH
            || image.height() != FRAME_HEIGHT
            || image.rgba().len() != RGBA_BYTES
        {
            return Err("皮肤PNG实际解码尺寸无效".into());
        }
        let rgba = image.rgba();
        for pixel in [
            0,
            FRAME_WIDTH as usize - 1,
            (FRAME_HEIGHT as usize - 1) * FRAME_WIDTH as usize,
            FRAME_WIDTH as usize * FRAME_HEIGHT as usize - 1,
        ] {
            if rgba[pixel * 4 + 3] > 1 {
                return Err("皮肤PNG四角必须透明".into());
            }
        }
        let mut bounds = [FRAME_WIDTH, FRAME_HEIGHT, 0, 0];
        for (index, pixel) in rgba.chunks_exact(4).enumerate() {
            if pixel[3] > 127 {
                let x = index as u32 % FRAME_WIDTH;
                let y = index as u32 / FRAME_WIDTH;
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x + 1);
                bounds[3] = bounds[3].max(y + 1);
            }
        }
        if bounds != frame.bounds {
            return Err("皮肤PNG实际alpha>127范围与清单不符或全透明".into());
        }
    }
    Ok(())
}

/// Reject compressed metadata/APNG and check all chunk CRCs before the existing
/// desktop decoder allocates its bounded 1024x1536 RGBA image. No source rewriting.
fn inspect_png(raw: &[u8]) -> Result<()> {
    if raw.len() < 45 || raw.len() as u64 > MAX_FRAME_BYTES || &raw[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("皮肤PNG标记或大小无效".into());
    }
    let mut offset = 8usize;
    let mut header = false;
    let mut data = false;
    let mut data_closed = false;
    while offset + 12 <= raw.len() {
        let length = u32::from_be_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .filter(|n| *n <= raw.len())
            .ok_or_else(|| "皮肤PNG数据截断".to_string())?;
        let kind = &raw[offset + 4..offset + 8];
        let body = &raw[offset + 8..end - 4];
        let checksum = u32::from_be_bytes(raw[end - 4..end].try_into().unwrap());
        if crc32(&raw[offset + 4..end - 4]) != checksum {
            return Err("皮肤PNG块CRC不符".into());
        }
        if !header && kind != b"IHDR" {
            return Err("皮肤PNG必须先有IHDR".into());
        }
        match kind {
            b"IHDR" => {
                if header
                    || offset != 8
                    || length != 13
                    || u32::from_be_bytes(body[..4].try_into().unwrap()) != FRAME_WIDTH
                    || u32::from_be_bytes(body[4..8].try_into().unwrap()) != FRAME_HEIGHT
                    || body[8] != 8
                    || body[9] != 6
                    || body[10] != 0
                    || body[11] != 0
                    || body[12] > 1
                {
                    return Err("皮肤PNG必须是1024x1536的8位RGBA".into());
                }
                header = true;
            }
            b"IDAT" => {
                if data_closed {
                    return Err("皮肤PNG数据块不连续".into());
                }
                data = true;
            }
            b"IEND" => {
                if length != 0 || !data || end != raw.len() {
                    return Err("皮肤PNG结尾无效".into());
                }
                return Ok(());
            }
            b"sRGB" if length == 1 && body[0] <= 3 && !data => {}
            b"gAMA" if length == 4 && !data => {}
            b"cHRM" if length == 32 && !data => {}
            b"pHYs" if length == 9 && body[8] <= 1 && !data => {}
            // Existing AI-assisted originals carry opaque C2PA provenance. It is
            // retained byte-for-byte, CRC-checked and never interpreted/executed.
            b"caBX" if length <= 64 * 1024 && !data => {}
            _ => return Err("皮肤PNG包含不支持的动画或附加数据块".into()),
        }
        if data && kind != b"IDAT" {
            data_closed = true;
        }
        offset = end;
    }
    Err("皮肤PNG缺少完整IEND".into())
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                0xedb88320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}
fn crc32(raw: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc_table();
    let mut value = !0u32;
    for byte in raw {
        value = TABLE[((value ^ u32::from(*byte)) & 255) as usize] ^ (value >> 8);
    }
    !value
}

fn valid_text(text: &str, maximum: usize) -> bool {
    !text.is_empty() && text == text.trim() && text.len() <= maximum && !text.chars().any(|c| c.is_control()
        || matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}
fn normalize_problems(problems: &mut Vec<LocalPetSkinProblem>) {
    if problems.len() > 32 {
        problems.truncate(31);
        problems.push(LocalPetSkinProblem {
            skin_id: None,
            message: "另有皮肤存储问题未逐项列出".into(),
        });
    }
    for problem in problems {
        let mut safe = String::new();
        for character in problem.message.chars() {
            if character.is_control()
                || matches!(character, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                continue;
            }
            if safe.len() + character.len_utf8() > 2048 {
                break;
            }
            safe.push(character);
        }
        problem.message = safe.trim().to_owned();
        if problem.message.is_empty() {
            problem.message = "本地皮肤不可用".into();
        }
    }
}
fn valid_skin_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_frame_id(id: &str) -> bool {
    let reserved = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'-' || b == b'_')
        && !reserved.contains(&id)
}
fn digest(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn io_error(error: std::io::Error) -> String {
    format!("本地皮肤文件操作失败：{error}")
}
fn json_error(error: serde_json::Error) -> String {
    format!("本地皮肤清单格式无效：{error}")
}

fn validate_absolute_local(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("皮肤目录必须是绝对本地路径".into());
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        {
            return Err("皮肤目录不接受网络或设备路径".into());
        }
    }
    Ok(())
}
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
fn guarded_open(path: &Path, directory: bool, owned_directory_write_share: bool) -> Result<File> {
    if metadata_is_reparse(&fs::symlink_metadata(path).map_err(io_error)?) {
        return Err("皮肤路径不接受链接或reparse point".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            // Windows child rename needs write sharing on its parent. Allow it
            // only inside the host-owned store; untrusted source directories and
            // all files deny writes. Every handle still denies delete sharing.
            .share_mode(if directory && owned_directory_write_share {
                3
            } else {
                1
            })
            .custom_flags(0x00200000 | if directory { 0x02000000 } else { 0 });
    }
    let file = options.open(path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if metadata_is_reparse(&metadata)
        || directory != metadata.is_dir()
        || (!directory && !metadata.is_file())
    {
        return Err("皮肤路径类型或reparse point无效".into());
    }
    Ok(file)
}
fn guarded_file(path: &Path) -> Result<File> {
    guarded_open(path, false, false)
}
fn guard_directory_chain(path: &Path) -> Result<Vec<File>> {
    guard_directory_chain_mode(path, None)
}
fn guard_storage_directory_chain(path: &Path, store: &Path) -> Result<Vec<File>> {
    if !path.starts_with(store) {
        return Err("皮肤存储目录守卫越界".into());
    }
    guard_directory_chain_mode(path, Some(store))
}
fn guard_directory_chain_mode(path: &Path, store: Option<&Path>) -> Result<Vec<File>> {
    validate_absolute_local(path)?;
    let mut ancestors: Vec<&Path> = path.ancestors().collect();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|ancestor| {
            guarded_open(
                ancestor,
                true,
                store.is_some_and(|root| ancestor.starts_with(root)),
            )
        })
        .collect()
}
fn read_file_bounded(file: &mut File, limit: u64) -> Result<Vec<u8>> {
    let length = file.metadata().map_err(io_error)?.len();
    if length == 0 || length > limit {
        return Err("皮肤文件大小超限或为空".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut raw = Vec::with_capacity(length as usize);
    file.take(limit + 1)
        .read_to_end(&mut raw)
        .map_err(io_error)?;
    if raw.len() as u64 != length {
        return Err("皮肤文件在读取时发生变化".into());
    }
    Ok(raw)
}
fn verify_roster(directory: &Path, manifest: &LocalPetSkinManifestV1) -> Result<()> {
    let expected: BTreeSet<String> = manifest
        .frames
        .iter()
        .map(|f| format!("{}.png", f.id))
        .collect();
    let mut root_names = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "皮肤目录含无效文件名".to_string())?;
        if !root_names.insert(name) || root_names.len() > 2 {
            return Err("皮肤目录只允许manifest.json和frames".into());
        }
        if metadata_is_reparse(&fs::symlink_metadata(entry.path()).map_err(io_error)?) {
            return Err("皮肤包包含链接".into());
        }
    }
    if root_names != BTreeSet::from(["manifest.json".to_string(), "frames".to_string()]) {
        return Err("皮肤包根目录不完整".into());
    }
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory.join("frames")).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if !metadata.is_file() || metadata_is_reparse(&metadata) {
            return Err("皮肤frames只能包含普通PNG文件".into());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "皮肤帧文件名无效".to_string())?;
        if !actual.insert(name) || actual.len() > 64 {
            return Err("皮肤帧文件数量超限".into());
        }
    }
    if actual != expected {
        return Err("皮肤目录存在缺少、额外或重定向帧".into());
    }
    Ok(())
}
fn write_new(path: &Path, raw: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(raw)
        .and_then(|_| file.sync_all())
        .map_err(io_error)
}
fn cleanup_stage(store: &Path, stage: &Path, created: &[PathBuf]) {
    // Only this nonce's explicitly created files, followed by non-recursive empty
    // directory removal. Never follow a reparse point or recursively delete a path.
    if stage.parent() != Some(store)
        || !stage
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".import-"))
    {
        return;
    }
    let Ok(_guards) = guard_storage_directory_chain(&stage.join("frames"), store) else {
        return;
    };
    for file in created {
        if file.parent() == Some(stage) || file.parent() == Some(stage.join("frames").as_path()) {
            let _ = fs::remove_file(file);
        }
    }
    drop(_guards);
    let _ = fs::remove_dir(stage.join("frames"));
    let _ = fs::remove_dir(stage);
}

#[cfg(test)]
#[path = "local_pet_skin_tests.rs"]
mod tests;
