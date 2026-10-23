//! 本机数据型 VRM 1.0 素材库。只写宿主数据目录，不读取或修改账本。
//! 这是资源有界的保守子集检查，不宣称实现完整 glTF/VRM 验证器。
use crate::local_pet_skin::{
    guard_directory_chain, guard_storage_directory_chain, guarded_file, read_file_bounded,
    validate_absolute_local, write_new,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;
pub const MAX_MODEL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const MAX_TEXTURE_BYTES: u64 = 96 * 1024 * 1024;
const MAX_ACCESSOR_BYTES: usize = 128 * 1024 * 1024;
const MAX_MODELS: usize = 8;
const EXTENSIONS: &[&str] = &[
    "VRMC_vrm",
    "VRMC_springBone",
    "VRMC_materials_mtoon",
    "VRMC_node_constraint",
    "KHR_materials_unlit",
];
const REQUIRED_BONES: &[&str] = &[
    "hips",
    "spine",
    "head",
    "leftUpperLeg",
    "leftLowerLeg",
    "leftFoot",
    "rightUpperLeg",
    "rightLowerLeg",
    "rightFoot",
    "leftUpperArm",
    "leftLowerArm",
    "leftHand",
    "rightUpperArm",
    "rightLowerArm",
    "rightHand",
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VrmSkinBudget {
    pub nodes: usize,
    pub joints: usize,
    pub triangles: u64,
    /// 全部内嵌图像的基础 RGBA8 算术预算，包含缩略图；不是实测 GPU 占用。
    pub decoded_texture_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VrmSkinModel {
    pub model_id: String,
    pub name: String,
    pub authors: Vec<String>,
    pub bytes: u64,
    pub meta: Value,
    pub budget: VrmSkinBudget,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VrmSkinProblem {
    pub model_id: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VrmSkinState {
    pub selected_model_id: Option<String>,
    pub models: Vec<VrmSkinModel>,
    pub problems: Vec<VrmSkinProblem>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    schema_version: u32,
    selected_model_id: Option<String>,
}

/// 由桌面宿主持有，并由上层 Mutex 串行调用修改方法。根路径不能来自渲染器。
pub struct VrmSkinStore {
    root: PathBuf,
}

impl VrmSkinStore {
    pub fn new(app_data: &Path) -> Result<Self> {
        validate_absolute_local(app_data)?;
        let _data_guards = guard_directory_chain(app_data)?;
        let root = app_data.join("vrm-pet-skins-v1");
        if !root.exists() {
            fs::create_dir(&root).map_err(io_error)?;
        }
        let _root_guards = guard_storage_directory_chain(&root, &root)?;
        Ok(Self { root })
    }

    pub fn state(&self) -> Result<VrmSkinState> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        let (models, mut problems, _) = self.scan()?;
        let selected = match self.read_selection() {
            Ok(selected) => selected,
            Err(error) => {
                problems.push(VrmSkinProblem {
                    model_id: None,
                    message: format!("VRM选择记录不可用，已回退内置形象：{error}"),
                });
                None
            }
        };
        let selected_model_id = selected.filter(|id| {
            if models.iter().any(|model| &model.model_id == id) {
                true
            } else {
                problems.push(VrmSkinProblem {
                    model_id: Some(id.clone()),
                    message: "已选VRM不可用，已回退内置形象；请先切换后再移除".into(),
                });
                false
            }
        });
        if problems.len() > 32 {
            problems.truncate(31);
            problems.push(VrmSkinProblem {
                model_id: None,
                message: "VRM诊断数量较多，其余诊断已省略；请清理未知条目后重新读取".into(),
            });
        }
        Ok(VrmSkinState {
            selected_model_id,
            models,
            problems,
        })
    }

    pub fn import_file(&mut self, source: &Path) -> Result<VrmSkinModel> {
        validate_absolute_local(source)?;
        if source.starts_with(&self.root) {
            return Err("导入源必须位于VRM素材库之外".into());
        }
        let _store_guards = guard_storage_directory_chain(&self.root, &self.root)?;
        let _source_guards = guard_directory_chain(source.parent().ok_or("VRM文件路径无效")?)?;
        let mut original = guarded_file(source)?;
        let raw = read_file_bounded(&mut original, MAX_MODEL_BYTES)?;
        let model = inspect_model(&raw, true)?;
        let destination = self.model_path(&model.model_id)?;
        if destination.exists() {
            return self
                .load_model(&model.model_id, true)
                .map(|(_, model)| model);
        }
        if self.scan()?.2 >= MAX_MODELS {
            return Err("本地VRM最多保留8个，请先移除一个未使用的模型".into());
        }
        let temporary = self
            .root
            .join(format!(".import-{}.vrm", uuid::Uuid::new_v4()));
        write_new(&temporary, &raw)?;
        if destination.exists() {
            let _ = fs::remove_file(&temporary);
            return Err("VRM素材已存在，未覆盖现有文件".into());
        }
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        // 导入不自动选中；只有用户显式确认选择后才改变角色。
        Ok(model)
    }

    pub fn select(&mut self, model_id: Option<&str>) -> Result<VrmSkinState> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        if let Some(id) = model_id {
            self.load_model(id, true)?;
        }
        self.write_selection(model_id)?;
        self.state()
    }

    pub fn remove(&mut self, model_id: &str) -> Result<VrmSkinState> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        self.model_path(model_id)?;
        // 即便当前模型损坏，也要求显式切换，不能由删除偷偷变更选择。
        if self.read_selection()?.as_deref() == Some(model_id) {
            return Err("请先切换到内置或其它形象，再移除当前VRM".into());
        }
        self.remove_inactive(model_id)
    }

    /// 仅供宿主已核验独立角色选择authority、确认该ID未在使用时调用。
    /// 库内旧选择记录损坏不应阻止恢复；此入口不修写该旧记录，也不接受路径。
    pub(crate) fn remove_inactive(&mut self, model_id: &str) -> Result<VrmSkinState> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        let path = self.model_path(model_id)?;
        let file = guarded_file(&path)?;
        drop(file);
        fs::remove_file(&path).map_err(io_error)?;
        self.state()
    }

    pub fn read_model(&self, model_id: &str) -> Result<Vec<u8>> {
        self.load_model(model_id, false).map(|(raw, _)| raw)
    }

    fn model_path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err("非法VRM模型标识".into());
        }
        Ok(self.root.join(format!("{id}.vrm")))
    }

    fn load_model(&self, id: &str, decode: bool) -> Result<(Vec<u8>, VrmSkinModel)> {
        let _guards = guard_storage_directory_chain(&self.root, &self.root)?;
        let raw = read_file_bounded(&mut guarded_file(&self.model_path(id)?)?, MAX_MODEL_BYTES)?;
        if digest(&raw) != id {
            return Err("VRM原始文件SHA-256不符，未加载被改动的模型".into());
        }
        let model = inspect_model(&raw, decode)?;
        Ok((raw, model))
    }

    fn scan(&self) -> Result<(Vec<VrmSkinModel>, Vec<VrmSkinProblem>, usize)> {
        let mut ids = BTreeSet::new();
        let mut problems = Vec::new();
        let mut truncated = false;
        for (index, entry) in fs::read_dir(&self.root).map_err(io_error)?.enumerate() {
            if index >= 128 {
                truncated = true;
                problems.push(VrmSkinProblem {
                    model_id: None,
                    message: "VRM素材目录条目超限，已停止枚举".into(),
                });
                break;
            }
            let entry = entry.map_err(io_error)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "VRM素材目录名称无效")?;
            if let Some(id) = name.strip_suffix(".vrm").filter(|id| valid_id(id)) {
                ids.insert(id.to_owned());
            } else if name != "selection.json"
                && !name.starts_with(".import-")
                && !name.starts_with(".selection-")
            {
                problems.push(VrmSkinProblem {
                    model_id: None,
                    message: "VRM素材库含未知条目，未读取该条目".into(),
                });
            }
        }
        let occupied = if truncated { MAX_MODELS } else { ids.len() };
        let mut models = Vec::new();
        for id in ids {
            if models.len() >= MAX_MODELS {
                problems.push(VrmSkinProblem {
                    model_id: Some(id),
                    message: "VRM数量超限，该模型未加载".into(),
                });
                continue;
            }
            match self.load_model(&id, false) {
                Ok((_, model)) => models.push(model),
                Err(error) => problems.push(VrmSkinProblem {
                    model_id: Some(id),
                    message: error,
                }),
            }
        }
        Ok((models, problems, occupied))
    }

    fn read_selection(&self) -> Result<Option<String>> {
        let path = self.root.join("selection.json");
        if !path.exists() {
            return Ok(None);
        }
        let raw = read_file_bounded(&mut guarded_file(&path)?, 1024)?;
        let selection: Selection =
            serde_json::from_slice(&raw).map_err(|_| "VRM选择记录格式无效")?;
        if selection.schema_version != 1
            || selection
                .selected_model_id
                .as_ref()
                .is_some_and(|id| !valid_id(id))
        {
            return Err("VRM选择记录版本或标识无效".into());
        }
        Ok(selection.selected_model_id)
    }

    fn write_selection(&self, id: Option<&str>) -> Result<()> {
        let raw = serde_json::to_vec(&Selection {
            schema_version: 1,
            selected_model_id: id.map(str::to_owned),
        })
        .map_err(|_| "VRM选择记录无法编码")?;
        let destination = self.root.join("selection.json");
        if destination.exists() {
            drop(guarded_file(&destination)?);
        }
        let temporary = self
            .root
            .join(format!(".selection-{}.json", uuid::Uuid::new_v4()));
        write_new(&temporary, &raw)?;
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn digest(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn io_error(error: std::io::Error) -> String {
    format!("VRM本地文件操作失败：{}", error.kind())
}
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or_else(|| "VRM包含无效对象".into())
}
fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "VRM包含无效数组".into())
}
fn optional_array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    match value.get(key) {
        None => Ok(&[]),
        Some(value) => array(value),
    }
}
fn unsigned(value: &Value) -> Result<usize> {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| "VRM索引或数量必须是非负整数".into())
}
fn default_unsigned(value: &Value, key: &str, default: usize) -> Result<usize> {
    value.get(key).map_or(Ok(default), unsigned)
}
fn index(value: &Value, length: usize) -> Result<usize> {
    let n = unsigned(value)?;
    if n >= length {
        Err("VRM资源索引越界".into())
    } else {
        Ok(n)
    }
}
fn finite(value: &Value) -> Result<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (*v as f32).is_finite())
        .ok_or_else(|| "VRM数值必须是有限浮点数".into())
}
fn vector(value: &Value, length: usize) -> Result<()> {
    let values = array(value)?;
    if values.len() != length {
        return Err("VRM向量长度无效".into());
    }
    for v in values {
        finite(v)?;
    }
    Ok(())
}
fn optional_vector(value: &Value, key: &str, length: usize) -> Result<()> {
    if let Some(v) = value.get(key) {
        vector(v, length)?;
    }
    Ok(())
}
fn bounded(value: &Value, lower: f64, upper: f64) -> Result<()> {
    let n = finite(value)?;
    if n < lower || n > upper {
        Err("VRM数值不在允许范围内".into())
    } else {
        Ok(())
    }
}
fn valid_text(text: &str, maximum: usize) -> bool {
    !text.is_empty() && text.chars().count() <= maximum && !text.chars().any(|ch| ch.is_control() || matches!(ch, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}

/// 不解释 extras，不加载任何外部资源，也不给自定义脚本留下执行入口。
fn validate_json_tree(value: &Value, declared_extensions: &HashSet<&str>) -> Result<()> {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                if key.eq_ignore_ascii_case("uri")
                    || key.eq_ignore_ascii_case("script")
                    || key.eq_ignore_ascii_case("scripts")
                {
                    return Err("VRM不能包含URI资源或脚本字段".into());
                }
                if key == "extensions" {
                    for name in object(child)?.keys() {
                        if !EXTENSIONS.contains(&name.as_str())
                            || !declared_extensions.contains(name.as_str())
                        {
                            return Err(format!("VRM包含尚未支持或未声明的扩展：{name}"));
                        }
                    }
                }
                validate_json_tree(child, declared_extensions)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_json_tree(child, declared_extensions)?;
            }
        }
        Value::Number(_) => {
            finite(value)?;
        }
        _ => {}
    }
    Ok(())
}

fn glb_parts(raw: &[u8]) -> Result<(Value, &[u8])> {
    if raw.len() < 28
        || raw.len() as u64 > MAX_MODEL_BYTES
        || &raw[..4] != b"glTF"
        || le_u32(raw, 4)? != 2
        || le_u32(raw, 8)? as usize != raw.len()
    {
        return Err("只支持完整的GLB v2 VRM文件（最大64MiB）".into());
    }
    let json_length = le_u32(raw, 12)? as usize;
    let json_end = 20usize
        .checked_add(json_length)
        .filter(|n| *n <= raw.len())
        .ok_or("VRM JSON块截断")?;
    if json_length == 0
        || json_length > MAX_JSON_BYTES
        || json_length % 4 != 0
        || le_u32(raw, 16)? != 0x4e4f534a
    {
        return Err("VRM必须先有不超过4MiB的JSON块".into());
    }
    let document: Value =
        serde_json::from_slice(&raw[20..json_end]).map_err(|_| "VRM JSON格式无效或嵌套过深")?;
    let bin_length = le_u32(raw, json_end)? as usize;
    if le_u32(raw, json_end + 4)? != 0x004e4942
        || bin_length == 0
        || bin_length % 4 != 0
        || json_end
            .checked_add(8)
            .and_then(|n| n.checked_add(bin_length))
            != Some(raw.len())
    {
        return Err("VRM只支持单JSON块和单内嵌BIN块".into());
    }
    Ok((document, &raw[json_end + 8..]))
}

fn le_u32(raw: &[u8], start: usize) -> Result<u32> {
    let bytes = raw
        .get(start..start.checked_add(4).ok_or("VRM数据长度溢出")?)
        .ok_or("VRM数据截断")?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

#[derive(Clone)]
struct BufferView {
    start: usize,
    length: usize,
    stride: Option<usize>,
}
#[derive(Clone)]
struct SparseAccessor {
    count: usize,
    indices_start: usize,
    indices_component: usize,
    indices_size: usize,
    values_start: usize,
}
#[derive(Clone)]
struct Accessor {
    component: usize,
    count: usize,
    kind: String,
    normalized: bool,
    element_size: usize,
    component_offsets: Vec<usize>,
    stride: usize,
    base: Option<usize>,
    sparse: Option<SparseAccessor>,
}
impl Accessor {
    fn offset(&self, bin: &[u8], item: usize, component: usize) -> Result<Option<usize>> {
        // 直接在已校验的稀疏索引字节中二分，不按声明count额外分配索引数组。
        if let Some(sparse) = &self.sparse {
            let (mut left, mut right) = (0, sparse.count);
            while left < right {
                let middle = left + (right - left) / 2;
                let value = read_unsigned(
                    bin,
                    sparse.indices_start + middle * sparse.indices_size,
                    sparse.indices_component,
                )?;
                if value < item {
                    left = middle + 1;
                } else if value > item {
                    right = middle;
                } else {
                    return Ok(Some(
                        sparse.values_start
                            + middle * self.element_size
                            + self.component_offsets[component],
                    ));
                }
            }
        }
        Ok(self
            .base
            .map(|start| start + item * self.stride + self.component_offsets[component]))
    }
    fn unsigned_at(&self, bin: &[u8], item: usize, component: usize) -> Result<usize> {
        match self.offset(bin, item, component)? {
            None => Ok(0),
            Some(offset) => read_unsigned(bin, offset, self.component),
        }
    }
}

fn read_unsigned(bin: &[u8], offset: usize, component: usize) -> Result<usize> {
    match component {
        5121 => bin
            .get(offset)
            .map(|v| *v as usize)
            .ok_or_else(|| "VRM整数数据截断".into()),
        5123 => {
            let value = bin
                .get(offset..offset.checked_add(2).ok_or("VRM数据溢出")?)
                .ok_or("VRM整数数据截断")?;
            Ok(u16::from_le_bytes(value.try_into().unwrap()) as usize)
        }
        5125 => Ok(le_u32(bin, offset)? as usize),
        _ => Err("VRM索引必须使用无符号整数".into()),
    }
}

fn bounded_region(
    view: &BufferView,
    offset: usize,
    count: usize,
    stride: usize,
    element: usize,
    alignment: usize,
) -> Result<usize> {
    let occupied = count
        .checked_sub(1)
        .and_then(|n| n.checked_mul(stride))
        .and_then(|n| n.checked_add(element))
        .ok_or("VRM访问器长度溢出")?;
    if offset
        .checked_add(occupied)
        .is_none_or(|end| end > view.length)
        || offset % alignment != 0
        || (view.start + offset) % alignment != 0
    {
        return Err("VRM访问器超出bufferView或未对齐".into());
    }
    Ok(view.start + offset)
}

fn validate_accessors(document: &Value, bin: &[u8], views: &[BufferView]) -> Result<Vec<Accessor>> {
    let raw_accessors = array(&document["accessors"])?;
    if raw_accessors.is_empty() || raw_accessors.len() > 4096 {
        return Err("VRM访问器数量超限或为空".into());
    }
    let mut total = 0usize;
    let mut result = Vec::new();
    for raw in raw_accessors {
        object(raw)?;
        let component = unsigned(&raw["componentType"])?;
        let size = match component {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err("VRM访问器分量类型尚未支持".into()),
        };
        let kind = raw["type"].as_str().ok_or("VRM访问器缺少type")?;
        let (rows, columns) = match kind {
            "SCALAR" => (1, 1),
            "VEC2" => (2, 1),
            "VEC3" => (3, 1),
            "VEC4" => (4, 1),
            "MAT2" => (2, 2),
            "MAT3" => (3, 3),
            "MAT4" => (4, 4),
            _ => return Err("VRM访问器形状尚未支持".into()),
        };
        let column_bytes = if columns > 1 {
            (rows * size + 3) / 4 * 4
        } else {
            rows * size
        };
        let element_size = column_bytes * columns;
        let component_offsets = (0..columns)
            .flat_map(|column| (0..rows).map(move |row| column * column_bytes + row * size))
            .collect::<Vec<_>>();
        let count = unsigned(&raw["count"])?;
        if count == 0 {
            return Err("VRM访问器count必须大于0".into());
        }
        total = count
            .checked_mul(element_size)
            .and_then(|bytes| total.checked_add(bytes))
            .filter(|bytes| *bytes <= MAX_ACCESSOR_BYTES)
            .ok_or("VRM展开访问器超过128MiB预算")?;
        let normalized = match raw.get("normalized") {
            None => false,
            Some(value) => value.as_bool().ok_or("VRM normalized必须是布尔值")?,
        };
        if normalized && matches!(component, 5125 | 5126) {
            return Err("VRM浮点或uint32访问器不能normalized".into());
        }
        let offset = default_unsigned(raw, "byteOffset", 0)?;
        let mut stride = element_size;
        let base = if let Some(view_id) = raw.get("bufferView") {
            let view = &views[index(view_id, views.len())?];
            stride = view.stride.unwrap_or(element_size);
            if stride < element_size || stride % size != 0 || offset % size != 0 {
                return Err("VRM访问器stride或偏移无效".into());
            }
            Some(bounded_region(
                view,
                offset,
                count,
                stride,
                element_size,
                if columns > 1 { 4 } else { size },
            )?)
        } else {
            if offset != 0 {
                return Err("无bufferView的访问器不能有偏移".into());
            }
            None
        };
        let sparse = if let Some(sparse) = raw.get("sparse") {
            object(sparse)?;
            let sparse_count = unsigned(&sparse["count"])?;
            if sparse_count == 0 || sparse_count > count {
                return Err("VRM sparse数量越界".into());
            }
            let indices = &sparse["indices"];
            object(indices)?;
            let index_component = unsigned(&indices["componentType"])?;
            let index_size = match index_component {
                5121 => 1,
                5123 => 2,
                5125 => 4,
                _ => return Err("VRM sparse索引类型无效".into()),
            };
            let view = &views[index(&indices["bufferView"], views.len())?];
            if view.stride.is_some() {
                return Err("VRM sparse不接受交错bufferView".into());
            }
            let start = bounded_region(
                view,
                default_unsigned(indices, "byteOffset", 0)?,
                sparse_count,
                index_size,
                index_size,
                index_size,
            )?;
            let mut previous = None;
            for position in 0..sparse_count {
                let value = read_unsigned(bin, start + position * index_size, index_component)?;
                if value >= count || previous.is_some_and(|previous| previous >= value) {
                    return Err("VRM sparse索引须严格递增且位于访问器内".into());
                }
                previous = Some(value);
            }
            let indices_start = start;
            let values = &sparse["values"];
            object(values)?;
            let view = &views[index(&values["bufferView"], views.len())?];
            if view.stride.is_some() {
                return Err("VRM sparse值不接受交错bufferView".into());
            }
            let start = bounded_region(
                view,
                default_unsigned(values, "byteOffset", 0)?,
                sparse_count,
                element_size,
                element_size,
                if columns > 1 { 4 } else { size },
            )?;
            Some(SparseAccessor {
                count: sparse_count,
                indices_start,
                indices_component: index_component,
                indices_size: index_size,
                values_start: start,
            })
        } else {
            None
        };
        for key in ["min", "max"] {
            if let Some(value) = raw.get(key) {
                vector(value, rows * columns)?;
            }
        }
        // 同时检查会被sparse覆盖的基础值；不向WebView传入任何NaN/Infinity。
        if component == 5126 {
            for (start, length, item_stride) in
                base.map(|start| (start, count, stride)).into_iter().chain(
                    sparse
                        .as_ref()
                        .map(|sparse| (sparse.values_start, sparse.count, element_size)),
                )
            {
                for item in 0..length {
                    for component_offset in &component_offsets {
                        let bits = le_u32(bin, start + item * item_stride + component_offset)?;
                        if !f32::from_bits(bits).is_finite() {
                            return Err("VRM BIN访问器包含非有限浮点数".into());
                        }
                    }
                }
            }
        }
        result.push(Accessor {
            component,
            count,
            kind: kind.into(),
            normalized,
            element_size,
            component_offsets,
            stride,
            base,
            sparse,
        });
    }
    Ok(result)
}

fn inspect_model(raw: &[u8], decode_images: bool) -> Result<VrmSkinModel> {
    let (document, bin) = glb_parts(raw)?;
    object(&document)?;
    if document["asset"]["version"] != "2.0"
        || document["asset"]
            .get("minVersion")
            .is_some_and(|v| v != "2.0")
    {
        return Err("VRM只支持glTF 2.0".into());
    }
    let mut declared = HashSet::new();
    for extension in optional_array(&document, "extensionsUsed")? {
        let name = extension.as_str().ok_or("VRM扩展名称无效")?;
        if !EXTENSIONS.contains(&name) || !declared.insert(name) {
            return Err(format!("VRM扩展尚未支持或重复声明：{name}"));
        }
    }
    for extension in optional_array(&document, "extensionsRequired")? {
        if !extension
            .as_str()
            .is_some_and(|name| EXTENSIONS.contains(&name) && declared.contains(name))
        {
            return Err("VRM包含尚未支持的必要扩展".into());
        }
    }
    validate_json_tree(&document, &declared)?;
    if !optional_array(&document, "animations")?.is_empty()
        || !optional_array(&document, "cameras")?.is_empty()
    {
        return Err("此版VRM暂不接受内嵌动画或相机".into());
    }
    let buffers = array(&document["buffers"])?;
    if buffers.len() != 1 {
        return Err("VRM必须只有一个内嵌buffer".into());
    }
    let buffer_length = unsigned(&buffers[0]["byteLength"])?;
    if buffer_length == 0
        || buffer_length > bin.len()
        || bin.len() - buffer_length > 3
        || bin[buffer_length..].iter().any(|byte| *byte != 0)
    {
        return Err("VRM BIN长度或填充无效".into());
    }
    let raw_views = array(&document["bufferViews"])?;
    if raw_views.is_empty() || raw_views.len() > 8192 {
        return Err("VRM bufferView数量超限或为空".into());
    }
    let mut views = Vec::new();
    for raw_view in raw_views {
        object(raw_view)?;
        if unsigned(&raw_view["buffer"])? != 0 {
            return Err("VRM bufferView引用了外部buffer".into());
        }
        let start = default_unsigned(raw_view, "byteOffset", 0)?;
        let length = unsigned(&raw_view["byteLength"])?;
        if length == 0
            || start
                .checked_add(length)
                .is_none_or(|end| end > buffer_length)
        {
            return Err("VRM bufferView超出BIN范围".into());
        }
        let stride = raw_view.get("byteStride").map(unsigned).transpose()?;
        if stride.is_some_and(|n| !(4..=252).contains(&n) || n % 4 != 0) {
            return Err("VRM bufferView交错步长无效".into());
        }
        if raw_view
            .get("target")
            .is_some_and(|v| !matches!(v.as_u64(), Some(34962 | 34963)))
            || (raw_view["target"] == 34963 && stride.is_some())
        {
            return Err("VRM bufferView target或索引步长无效".into());
        }
        views.push(BufferView {
            start,
            length,
            stride,
        });
    }
    let accessors = validate_accessors(&document, bin, &views)?;
    let nodes = array(&document["nodes"])?;
    if nodes.is_empty() || nodes.len() > 512 {
        return Err("VRM节点数量须为1至512".into());
    }
    let meshes = array(&document["meshes"])?;
    let skins = optional_array(&document, "skins")?;
    let materials = optional_array(&document, "materials")?;
    if meshes.is_empty() || meshes.len() > 256 || skins.len() > 128 || materials.len() > 128 {
        return Err("VRM网格、骨架或材质数量超限".into());
    }
    let (parents, mesh_instances) = validate_nodes(&document, nodes, meshes.len(), skins.len())?;
    let (mesh_triangles, morph_counts) = validate_meshes(meshes, &accessors, bin, materials.len())?;
    let triangles = mesh_triangles.iter().zip(mesh_instances.iter()).try_fold(
        0u64,
        |sum, (triangles, instances)| {
            triangles
                .checked_mul((*instances).max(1) as u64)
                .and_then(|n| sum.checked_add(n))
                .filter(|n| *n <= 100_000)
                .ok_or("VRM三角形预算超过100000（包含重复网格实例）")
        },
    )?;
    let joints = validate_skins(skins, nodes, meshes, &accessors, bin)?;
    let images = optional_array(&document, "images")?;
    let textures = optional_array(&document, "textures")?;
    let samplers = optional_array(&document, "samplers")?;
    if images.len() > 128 || textures.len() > 128 || samplers.len() > 128 {
        return Err("VRM图像、纹理或采样器数量超限".into());
    }
    let mut decoded_texture_bytes = 0u64;
    for image in images {
        object(image)?;
        if image["mimeType"] != "image/png" {
            return Err("此版VRM只支持内嵌PNG纹理".into());
        }
        let view = &views[index(&image["bufferView"], views.len())?];
        if view.stride.is_some() {
            return Err("VRM图像bufferView不能有byteStride".into());
        }
        let encoded = &bin[view.start..view.start + view.length];
        let (width, height) = inspect_png(encoded)?;
        decoded_texture_bytes = decoded_texture_bytes
            .checked_add(u64::from(width) * u64::from(height) * 4)
            .filter(|n| *n <= MAX_TEXTURE_BYTES)
            .ok_or("VRM全部图像（含缩略图）超过96MiB基础RGBA预算")?;
        if decode_images {
            let decoded =
                tauri::image::Image::from_bytes(encoded).map_err(|_| "VRM内嵌PNG无法完整解码")?;
            if decoded.width() != width
                || decoded.height() != height
                || decoded.rgba().len() as u64 != u64::from(width) * u64::from(height) * 4
            {
                return Err("VRM PNG实际解码尺寸与头部不符".into());
            }
        }
    }
    for sampler in samplers {
        object(sampler)?;
        for key in ["wrapS", "wrapT"] {
            if sampler
                .get(key)
                .is_some_and(|v| !matches!(v.as_u64(), Some(33071 | 33648 | 10497)))
            {
                return Err("VRM纹理wrap枚举无效".into());
            }
        }
        if sampler
            .get("magFilter")
            .is_some_and(|v| !matches!(v.as_u64(), Some(9728 | 9729)))
            || sampler.get("minFilter").is_some_and(|v| {
                !matches!(v.as_u64(), Some(9728 | 9729 | 9984 | 9985 | 9986 | 9987))
            })
        {
            return Err("VRM纹理filter枚举无效".into());
        }
    }
    for texture in textures {
        object(texture)?;
        index(&texture["source"], images.len())?;
        if let Some(sampler) = texture.get("sampler") {
            index(sampler, samplers.len())?;
        }
    }
    for material in materials {
        validate_material(material, textures.len())?;
    }
    let vrm = &document["extensions"]["VRMC_vrm"];
    if vrm["specVersion"] != "1.0" {
        return Err("只支持VRM 1.0，暂不支持VRM 0.x或预览格式".into());
    }
    let meta = &vrm["meta"];
    let (name, authors) = validate_meta(meta, images.len())?;
    validate_vrm(vrm, nodes, meshes, &morph_counts, materials.len(), &parents)?;
    if let Some(spring) = document["extensions"].get("VRMC_springBone") {
        validate_spring(spring, nodes.len())?;
    }
    validate_constraints(nodes)?;
    Ok(VrmSkinModel {
        model_id: digest(raw),
        name,
        authors,
        bytes: raw.len() as u64,
        meta: meta.clone(),
        budget: VrmSkinBudget {
            nodes: nodes.len(),
            joints,
            triangles,
            decoded_texture_bytes,
        },
    })
}

fn visit(index: usize, children: &[Vec<usize>], marks: &mut [u8]) -> Result<()> {
    if marks[index] == 1 {
        return Err("VRM节点或约束包含循环".into());
    }
    if marks[index] == 2 {
        return Ok(());
    }
    marks[index] = 1;
    for child in &children[index] {
        visit(*child, children, marks)?;
    }
    marks[index] = 2;
    Ok(())
}

fn validate_nodes(
    document: &Value,
    nodes: &[Value],
    mesh_count: usize,
    skin_count: usize,
) -> Result<(Vec<Option<usize>>, Vec<usize>)> {
    let mut parents = vec![None; nodes.len()];
    let mut children = vec![Vec::new(); nodes.len()];
    let mut instances = vec![0usize; mesh_count];
    for (node_id, node) in nodes.iter().enumerate() {
        object(node)?;
        if node.get("camera").is_some() {
            return Err("此版VRM节点不能绑定相机".into());
        }
        if let Some(mesh) = node.get("mesh") {
            instances[index(mesh, mesh_count)?] += 1;
        }
        if let Some(skin) = node.get("skin") {
            index(skin, skin_count)?;
            if node.get("mesh").is_none() {
                return Err("VRM蒙皮节点缺少网格".into());
            }
        }
        if node.get("matrix").is_some()
            && ["translation", "rotation", "scale"]
                .iter()
                .any(|key| node.get(key).is_some())
        {
            return Err("VRM节点不能同时使用matrix和TRS".into());
        }
        optional_vector(node, "matrix", 16)?;
        optional_vector(node, "translation", 3)?;
        optional_vector(node, "rotation", 4)?;
        optional_vector(node, "scale", 3)?;
        if let Some(rotation) = node.get("rotation") {
            let norm = array(rotation)?
                .iter()
                .map(|v| finite(v).map(|n| n * n))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .sum::<f64>();
            if !(0.99..=1.01).contains(&norm) {
                return Err("VRM节点旋转不是单位四元数".into());
            }
        }
        for child in optional_array(node, "children")? {
            let child = index(child, nodes.len())?;
            if child == node_id || parents[child].replace(node_id).is_some() {
                return Err("VRM节点存在循环、重复子节点或多个父节点".into());
            }
            children[node_id].push(child);
        }
    }
    let mut marks = vec![0u8; nodes.len()];
    for id in 0..nodes.len() {
        visit(id, &children, &mut marks)?;
    }
    let scenes = array(&document["scenes"])?;
    if scenes.is_empty() || scenes.len() > 16 {
        return Err("VRM场景数量无效".into());
    }
    if let Some(scene) = document.get("scene") {
        index(scene, scenes.len())?;
    }
    for scene in scenes {
        object(scene)?;
        let roots = array(&scene["nodes"])?;
        let mut seen = HashSet::new();
        for root in roots {
            let id = index(root, nodes.len())?;
            if parents[id].is_some() || !seen.insert(id) {
                return Err("VRM场景根节点重复或另有父节点".into());
            }
        }
    }
    Ok((parents, instances))
}

fn validate_attribute(name: &str, accessor: &Accessor) -> Result<()> {
    let float = accessor.component == 5126 && !accessor.normalized;
    let normalized_unsigned = matches!(accessor.component, 5121 | 5123) && accessor.normalized;
    let valid = match name {
        "POSITION" | "NORMAL" => accessor.kind == "VEC3" && float,
        "TANGENT" => accessor.kind == "VEC4" && float,
        "TEXCOORD_0" | "TEXCOORD_1" => accessor.kind == "VEC2" && (float || normalized_unsigned),
        "COLOR_0" => {
            matches!(accessor.kind.as_str(), "VEC3" | "VEC4") && (float || normalized_unsigned)
        }
        "JOINTS_0" => {
            accessor.kind == "VEC4"
                && matches!(accessor.component, 5121 | 5123)
                && !accessor.normalized
        }
        "WEIGHTS_0" => accessor.kind == "VEC4" && (float || normalized_unsigned),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("VRM顶点属性类型尚未支持或无效：{name}"))
    }
}

fn validate_meshes(
    meshes: &[Value],
    accessors: &[Accessor],
    bin: &[u8],
    materials: usize,
) -> Result<(Vec<u64>, Vec<usize>)> {
    let mut primitive_count = 0usize;
    let mut triangles = Vec::new();
    let mut morph_counts = Vec::new();
    for mesh in meshes {
        object(mesh)?;
        let primitives = array(&mesh["primitives"])?;
        if primitives.is_empty() {
            return Err("VRM网格没有primitive".into());
        }
        primitive_count += primitives.len();
        if primitive_count > 256 {
            return Err("VRM primitive数量超过256".into());
        }
        let mut mesh_triangles = 0u64;
        let mut morph_count = None;
        for primitive in primitives {
            object(primitive)?;
            if default_unsigned(primitive, "mode", 4)? != 4 {
                return Err("此版VRM只支持TRIANGLES网格".into());
            }
            let attributes = object(&primitive["attributes"])?;
            let position = &accessors[index(
                attributes.get("POSITION").ok_or("VRM网格缺少POSITION")?,
                accessors.len(),
            )?];
            validate_attribute("POSITION", position)?;
            for (name, value) in attributes {
                let accessor = &accessors[index(value, accessors.len())?];
                validate_attribute(name, accessor)?;
                if accessor.count != position.count {
                    return Err("VRM同一primitive顶点属性数量不一致".into());
                }
            }
            if attributes.contains_key("JOINTS_0") != attributes.contains_key("WEIGHTS_0") {
                return Err("VRM蒙皮顶点属性缺少JOINTS或WEIGHTS".into());
            }
            let count = if let Some(value) = primitive.get("indices") {
                let accessor = &accessors[index(value, accessors.len())?];
                if accessor.kind != "SCALAR"
                    || !matches!(accessor.component, 5121 | 5123 | 5125)
                    || accessor.normalized
                {
                    return Err("VRM三角形索引访问器无效".into());
                }
                if accessor.count > 300_000 {
                    return Err("VRM索引数量超过三角形预算".into());
                }
                for item in 0..accessor.count {
                    if accessor.unsigned_at(bin, item, 0)? >= position.count {
                        return Err("VRM三角形索引超出顶点数量".into());
                    }
                }
                accessor.count
            } else {
                position.count
            };
            if count == 0 || count % 3 != 0 {
                return Err("VRM三角形索引数量不是3的倍数".into());
            }
            mesh_triangles = mesh_triangles
                .checked_add((count / 3) as u64)
                .filter(|n| *n <= 100_000)
                .ok_or("VRM三角形预算超过100000")?;
            if let Some(material) = primitive.get("material") {
                index(material, materials)?;
            }
            let targets = optional_array(primitive, "targets")?;
            if targets.len() > 64 || morph_count.is_some_and(|expected| expected != targets.len()) {
                return Err("VRM morph targets超过64或同网格数量不一致".into());
            }
            morph_count = Some(targets.len());
            for target in targets {
                let fields = object(target)?;
                if fields.is_empty() {
                    return Err("VRM morph target为空".into());
                }
                for (name, value) in fields {
                    let accessor = &accessors[index(value, accessors.len())?];
                    if !matches!(name.as_str(), "POSITION" | "NORMAL" | "TANGENT")
                        || accessor.component != 5126
                        || accessor.normalized
                        || accessor.kind != "VEC3"
                        || accessor.count != position.count
                    {
                        return Err("VRM morph访问器形状或索引无效".into());
                    }
                }
            }
        }
        let morph_count = morph_count.unwrap_or(0);
        if let Some(weights) = mesh.get("weights") {
            vector(weights, morph_count)?;
        }
        triangles.push(mesh_triangles);
        morph_counts.push(morph_count);
    }
    Ok((triangles, morph_counts))
}

fn validate_skins(
    skins: &[Value],
    nodes: &[Value],
    meshes: &[Value],
    accessors: &[Accessor],
    bin: &[u8],
) -> Result<usize> {
    let mut unique = HashSet::new();
    let mut lengths = Vec::new();
    for skin in skins {
        object(skin)?;
        let joints = array(&skin["joints"])?;
        if joints.is_empty() || joints.len() > 200 {
            return Err("VRM单个skin的joint数量须为1至200".into());
        }
        let mut seen = HashSet::new();
        for value in joints {
            let id = index(value, nodes.len())?;
            if !seen.insert(id) {
                return Err("VRM skin含重复joint".into());
            }
            unique.insert(id);
        }
        if unique.len() > 200 {
            return Err("VRM全部skin的唯一joint数量超过200".into());
        }
        if let Some(skeleton) = skin.get("skeleton") {
            index(skeleton, nodes.len())?;
        }
        if let Some(value) = skin.get("inverseBindMatrices") {
            let accessor = &accessors[index(value, accessors.len())?];
            if accessor.kind != "MAT4"
                || accessor.component != 5126
                || accessor.normalized
                || accessor.count < joints.len()
            {
                return Err("VRM逆绑定矩阵访问器无效".into());
            }
        }
        lengths.push(joints.len());
    }
    for node in nodes {
        let Some(skin) = node.get("skin") else {
            continue;
        };
        let count = lengths[index(skin, lengths.len())?];
        let mesh = &meshes[index(&node["mesh"], meshes.len())?];
        for primitive in array(&mesh["primitives"])? {
            let attributes = object(&primitive["attributes"])?;
            let accessor = &accessors[index(
                attributes
                    .get("JOINTS_0")
                    .ok_or("VRM蒙皮网格缺少JOINTS_0")?,
                accessors.len(),
            )?];
            for item in 0..accessor.count {
                for component in 0..4 {
                    if accessor.unsigned_at(bin, item, component)? >= count {
                        return Err("VRM顶点joint索引超出skin范围".into());
                    }
                }
            }
        }
    }
    Ok(unique.len())
}

fn texture_info(value: &Value, textures: usize) -> Result<()> {
    object(value)?;
    index(&value["index"], textures)?;
    if default_unsigned(value, "texCoord", 0)? > 1 {
        return Err("此版VRM仅支持两组纹理坐标".into());
    }
    Ok(())
}

fn validate_material(material: &Value, textures: usize) -> Result<()> {
    object(material)?;
    if material
        .get("alphaMode")
        .is_some_and(|value| !matches!(value.as_str(), Some("OPAQUE" | "MASK" | "BLEND")))
    {
        return Err("VRM alphaMode无效".into());
    }
    if let Some(value) = material.get("doubleSided") {
        if !value.is_boolean() {
            return Err("VRM doubleSided须为布尔值".into());
        }
    }
    optional_vector(material, "emissiveFactor", 3)?;
    for key in ["normalTexture", "occlusionTexture", "emissiveTexture"] {
        if let Some(value) = material.get(key) {
            texture_info(value, textures)?;
        }
    }
    if let Some(pbr) = material.get("pbrMetallicRoughness") {
        object(pbr)?;
        optional_vector(pbr, "baseColorFactor", 4)?;
        for key in ["baseColorTexture", "metallicRoughnessTexture"] {
            if let Some(value) = pbr.get(key) {
                texture_info(value, textures)?;
            }
        }
        for key in ["metallicFactor", "roughnessFactor"] {
            if let Some(value) = pbr.get(key) {
                bounded(value, 0.0, 1.0)?;
            }
        }
    }
    if let Some(mtoon) = material["extensions"].get("VRMC_materials_mtoon") {
        if mtoon["specVersion"] != "1.0" {
            return Err("VRM MToon版本尚未支持".into());
        }
        for (key, value) in object(mtoon)? {
            if key.ends_with("Texture") {
                texture_info(value, textures)?;
            }
        }
        for key in [
            "shadeColorFactor",
            "matcapFactor",
            "parametricRimColorFactor",
            "outlineColorFactor",
        ] {
            optional_vector(mtoon, key, 3)?;
        }
        if mtoon.get("outlineWidthMode").is_some_and(|v| {
            !matches!(
                v.as_str(),
                Some("none" | "worldCoordinates" | "screenCoordinates")
            )
        }) {
            return Err("VRM MToon outlineWidthMode无效".into());
        }
    }
    Ok(())
}

fn validate_meta(meta: &Value, images: usize) -> Result<(String, Vec<String>)> {
    object(meta)?;
    let name = meta["name"]
        .as_str()
        .filter(|name| valid_text(name, 128))
        .ok_or("VRM缺少有效名称")?
        .to_owned();
    let authors = array(&meta["authors"])?;
    if authors.is_empty() || authors.len() > 16 {
        return Err("VRM作者须为1至16项".into());
    }
    let authors = authors
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| valid_text(value, 128))
                .map(str::to_owned)
                .ok_or_else(|| "VRM作者字段无效".into())
        })
        .collect::<Result<Vec<_>>>()?;
    if meta["licenseUrl"] != "https://vrm.dev/licenses/1.0/" {
        return Err("VRM必须保留明确的VRM Public License 1.0许可".into());
    }
    for (key, allowed) in [
        (
            "avatarPermission",
            &["onlyAuthor", "onlySeparatelyLicensedPerson", "everyone"][..],
        ),
        (
            "commercialUsage",
            &["personalNonProfit", "personalProfit", "corporation"][..],
        ),
        ("creditNotation", &["required", "unnecessary"][..]),
        (
            "modification",
            &[
                "prohibited",
                "allowModification",
                "allowModificationRedistribution",
            ][..],
        ),
    ] {
        if meta
            .get(key)
            .is_some_and(|value| !value.as_str().is_some_and(|value| allowed.contains(&value)))
        {
            return Err(format!("VRM许可枚举未知：{key}"));
        }
    }
    for key in [
        "allowExcessivelyViolentUsage",
        "allowExcessivelySexualUsage",
        "allowPoliticalOrReligiousUsage",
        "allowAntisocialOrHateUsage",
        "allowRedistribution",
    ] {
        if meta.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err(format!("VRM许可字段必须是布尔值：{key}"));
        }
    }
    if let Some(thumbnail) = meta.get("thumbnailImage") {
        index(thumbnail, images)?;
    }
    for key in [
        "copyrightInformation",
        "contactInformation",
        "thirdPartyLicenses",
        "version",
    ] {
        if meta.get(key).is_some_and(|value| !value.is_string()) {
            return Err("VRM许可文字字段无效".into());
        }
    }
    if let Some(url) = meta.get("otherLicenseUrl") {
        let url = url
            .as_str()
            .filter(|url| {
                valid_text(url, 2048) && (url.starts_with("https://") || url.starts_with("http://"))
            })
            .ok_or("VRM附加许可URL无效")?;
        let _ = url; // 仅保存和显示，不访问此URL。
    }
    if let Some(references) = meta.get("references") {
        for reference in array(references)? {
            if !reference.is_string() {
                return Err("VRM许可引用字段无效".into());
            }
        }
    }
    Ok((name, authors))
}

fn is_descendant(mut node: usize, ancestor: usize, parents: &[Option<usize>]) -> bool {
    while let Some(parent) = parents[node] {
        if parent == ancestor {
            return true;
        }
        node = parent;
    }
    false
}

fn validate_vrm(
    vrm: &Value,
    nodes: &[Value],
    meshes: &[Value],
    morph_counts: &[usize],
    material_count: usize,
    parents: &[Option<usize>],
) -> Result<()> {
    let bones = object(&vrm["humanoid"]["humanBones"])?;
    let mut bone_nodes = HashSet::new();
    for bone in REQUIRED_BONES {
        if !bones.contains_key(*bone) {
            return Err(format!("VRM缺少必要人形骨骼：{bone}"));
        }
    }
    for bone in bones.values() {
        let node = index(&bone["node"], nodes.len())?;
        if !bone_nodes.insert(node) {
            return Err("VRM人形骨骼重复使用同一节点".into());
        }
    }
    for chain in [
        &["hips", "spine", "head"][..],
        &["hips", "leftUpperLeg", "leftLowerLeg", "leftFoot"][..],
        &["hips", "rightUpperLeg", "rightLowerLeg", "rightFoot"][..],
        &["spine", "leftUpperArm", "leftLowerArm", "leftHand"][..],
        &["spine", "rightUpperArm", "rightLowerArm", "rightHand"][..],
    ] {
        for pair in chain.windows(2) {
            let parent = index(&bones[pair[0]]["node"], nodes.len())?;
            let child = index(&bones[pair[1]]["node"], nodes.len())?;
            if !is_descendant(child, parent, parents) {
                return Err("VRM必要人形骨骼层次无效".into());
            }
        }
    }
    for node in nodes {
        if let Some(weights) = node.get("weights") {
            let mesh = index(&node["mesh"], meshes.len())?;
            vector(weights, morph_counts[mesh])?;
        }
    }
    if let Some(first_person) = vrm.get("firstPerson") {
        object(first_person)?;
        for annotation in optional_array(first_person, "meshAnnotations")? {
            let node = index(&annotation["node"], nodes.len())?;
            if nodes[node].get("mesh").is_none()
                || !matches!(
                    annotation["type"].as_str(),
                    Some("auto" | "both" | "thirdPersonOnly" | "firstPersonOnly")
                )
            {
                return Err("VRM第一人称网格标注无效".into());
            }
        }
    }
    if let Some(look_at) = vrm.get("lookAt") {
        object(look_at)?;
        if !matches!(look_at["type"].as_str(), Some("bone" | "expression")) {
            return Err("VRM lookAt类型未知".into());
        }
        optional_vector(look_at, "offsetFromHeadBone", 3)?;
        for key in [
            "rangeMapHorizontalInner",
            "rangeMapHorizontalOuter",
            "rangeMapVerticalDown",
            "rangeMapVerticalUp",
        ] {
            if let Some(range) = look_at.get(key) {
                object(range)?;
                for key in ["inputMaxValue", "outputScale"] {
                    if let Some(value) = range.get(key) {
                        bounded(value, 0.0, f32::MAX as f64)?;
                    }
                }
            }
        }
    }
    if let Some(expressions) = vrm.get("expressions") {
        object(expressions)?;
        let mut count = 0usize;
        for key in ["preset", "custom"] {
            if let Some(group) = expressions.get(key) {
                for (name, expression) in object(group)? {
                    count += 1;
                    if count > 128 || !valid_text(name, 128) {
                        return Err("VRM表情数量或名称无效".into());
                    }
                    object(expression)?;
                    if expression
                        .get("isBinary")
                        .is_some_and(|value| !value.is_boolean())
                    {
                        return Err("VRM表情isBinary无效".into());
                    }
                    for key in ["overrideBlink", "overrideLookAt", "overrideMouth"] {
                        if expression.get(key).is_some_and(|value| {
                            !matches!(value.as_str(), Some("none" | "block" | "blend"))
                        }) {
                            return Err("VRM表情覆盖枚举未知".into());
                        }
                    }
                    for bind in optional_array(expression, "morphTargetBinds")? {
                        let node = index(&bind["node"], nodes.len())?;
                        let mesh = index(&nodes[node]["mesh"], meshes.len())?;
                        index(&bind["index"], morph_counts[mesh])?;
                        bounded(&bind["weight"], 0.0, 1.0)?;
                    }
                    for bind in optional_array(expression, "materialColorBinds")? {
                        index(&bind["material"], material_count)?;
                        if !matches!(
                            bind["type"].as_str(),
                            Some(
                                "color"
                                    | "emissionColor"
                                    | "shadeColor"
                                    | "matcapColor"
                                    | "rimColor"
                                    | "outlineColor"
                            )
                        ) {
                            return Err("VRM表情材质颜色类型未知".into());
                        }
                        vector(&bind["targetValue"], 4)?;
                    }
                    for bind in optional_array(expression, "textureTransformBinds")? {
                        index(&bind["material"], material_count)?;
                        vector(&bind["scale"], 2)?;
                        vector(&bind["offset"], 2)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_spring(spring: &Value, nodes: usize) -> Result<()> {
    if spring["specVersion"] != "1.0" {
        return Err("VRM springBone版本尚未支持".into());
    }
    let colliders = optional_array(spring, "colliders")?;
    let groups = optional_array(spring, "colliderGroups")?;
    let springs = optional_array(spring, "springs")?;
    if colliders.len() > 128 || groups.len() > 128 || springs.len() > 128 {
        return Err("VRM弹簧或碰撞器数量超过保守支持范围".into());
    }
    for collider in colliders {
        index(&collider["node"], nodes)?;
        let shape = object(&collider["shape"])?;
        if shape.len() != 1 {
            return Err("VRM碰撞器必须指定一种形状".into());
        }
        let (kind, value) = shape.iter().next().ok_or("VRM碰撞形状为空")?;
        if !matches!(kind.as_str(), "sphere" | "capsule") {
            return Err("VRM碰撞形状未知".into());
        }
        object(value)?;
        optional_vector(value, "offset", 3)?;
        if let Some(radius) = value.get("radius") {
            bounded(radius, 0.0, f32::MAX as f64)?;
        }
        if kind == "capsule" {
            vector(&value["tail"], 3)?;
        }
    }
    for group in groups {
        for value in array(&group["colliders"])? {
            index(value, colliders.len())?;
        }
    }
    let mut joint_count = 0usize;
    for spring in springs {
        object(spring)?;
        if let Some(center) = spring.get("center") {
            index(center, nodes)?;
        }
        for value in optional_array(spring, "colliderGroups")? {
            index(value, groups.len())?;
        }
        let joints = array(&spring["joints"])?;
        joint_count += joints.len();
        if joints.is_empty() || joint_count > 1024 {
            return Err("VRM弹簧joint数量超过保守支持范围".into());
        }
        let mut seen = HashSet::new();
        for joint in joints {
            let node = index(&joint["node"], nodes)?;
            if !seen.insert(node) {
                return Err("VRM单个弹簧含重复joint".into());
            }
            for key in ["hitRadius", "stiffness", "gravityPower"] {
                if let Some(value) = joint.get(key) {
                    bounded(value, 0.0, f32::MAX as f64)?;
                }
            }
            if let Some(value) = joint.get("dragForce") {
                bounded(value, 0.0, 1.0)?;
            }
            optional_vector(joint, "gravityDir", 3)?;
        }
    }
    Ok(())
}

fn validate_constraints(nodes: &[Value]) -> Result<()> {
    let mut dependencies = vec![Vec::new(); nodes.len()];
    for (id, node) in nodes.iter().enumerate() {
        let Some(extension) = node["extensions"].get("VRMC_node_constraint") else {
            continue;
        };
        if extension["specVersion"] != "1.0" {
            return Err("VRM节点约束版本尚未支持".into());
        }
        let constraints = object(&extension["constraint"])?;
        if constraints.len() != 1 {
            return Err("VRM节点必须指定一种约束".into());
        }
        let (kind, value) = constraints.iter().next().ok_or("VRM节点约束为空")?;
        object(value)?;
        match kind.as_str() {
            "roll" => {
                if !matches!(value["rollAxis"].as_str(), Some("X" | "Y" | "Z")) {
                    return Err("VRM rollAxis枚举无效".into());
                }
            }
            "aim" => {
                if !matches!(
                    value["aimAxis"].as_str(),
                    Some(
                        "PositiveX"
                            | "NegativeX"
                            | "PositiveY"
                            | "NegativeY"
                            | "PositiveZ"
                            | "NegativeZ"
                    )
                ) {
                    return Err("VRM aimAxis枚举无效".into());
                }
            }
            "rotation" => {}
            _ => return Err("VRM节点约束类型未知".into()),
        }
        let source = index(&value["source"], nodes.len())?;
        if source == id {
            return Err("VRM节点不能约束到自身".into());
        }
        if let Some(weight) = value.get("weight") {
            bounded(weight, 0.0, 1.0)?;
        }
        dependencies[id].push(source);
    }
    let mut marks = vec![0; nodes.len()];
    for node in 0..nodes.len() {
        visit(node, &dependencies, &mut marks)?;
    }
    Ok(())
}

/// 在调用既有PNG解码器前，先限制尺寸并拒绝压缩元数据、APNG和未知块。
fn inspect_png(raw: &[u8]) -> Result<(u32, u32)> {
    if raw.len() < 45 || &raw[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("VRM图像不是完整PNG".into());
    }
    let mut offset = 8usize;
    let mut size = None;
    let mut color = 0u8;
    let mut palette = false;
    let mut transparency = false;
    let mut data = false;
    let mut data_closed = false;
    while offset.checked_add(12).is_some_and(|end| end <= raw.len()) {
        let length = u32::from_be_bytes(raw[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .filter(|n| *n <= raw.len())
            .ok_or("VRM PNG块截断")?;
        let kind = &raw[offset + 4..offset + 8];
        let body = &raw[offset + 8..end - 4];
        if crc32(&raw[offset + 4..end - 4])
            != u32::from_be_bytes(raw[end - 4..end].try_into().unwrap())
        {
            return Err("VRM PNG块CRC不符".into());
        }
        if size.is_none() && kind != b"IHDR" {
            return Err("VRM PNG必须先有IHDR".into());
        }
        match kind {
            b"IHDR" => {
                if size.is_some() || offset != 8 || length != 13 {
                    return Err("VRM PNG头部无效".into());
                }
                let width = u32::from_be_bytes(body[..4].try_into().unwrap());
                let height = u32::from_be_bytes(body[4..8].try_into().unwrap());
                if width == 0
                    || height == 0
                    || width > 4096
                    || height > 4096
                    || body[8] != 8
                    || !matches!(body[9], 0 | 2 | 3 | 4 | 6)
                    || body[10] != 0
                    || body[11] != 0
                    || body[12] > 1
                {
                    return Err("VRM PNG须为单边不超过4096的8位图像".into());
                }
                color = body[9];
                size = Some((width, height));
            }
            b"PLTE"
                if !palette
                    && !data
                    && !matches!(color, 0 | 4)
                    && (3..=768).contains(&length)
                    && length % 3 == 0 =>
            {
                palette = true;
            }
            b"tRNS"
                if !transparency
                    && !data
                    && match color {
                        0 => length == 2,
                        2 => length == 6,
                        3 => palette && length <= 256,
                        _ => false,
                    } =>
            {
                transparency = true;
            }
            b"IDAT" => {
                if data_closed || (color == 3 && !palette) {
                    return Err("VRM PNG数据块顺序无效".into());
                }
                data = true;
            }
            b"IEND" => {
                if length != 0 || !data || end != raw.len() {
                    return Err("VRM PNG结尾无效".into());
                }
                return size.ok_or_else(|| "VRM PNG缺少尺寸".into());
            }
            b"sRGB" if !data && length == 1 && body[0] <= 3 => {}
            b"gAMA" if !data && length == 4 => {}
            b"cHRM" if !data && length == 32 => {}
            b"pHYs" if !data && length == 9 && body[8] <= 1 => {}
            _ => return Err("VRM PNG包含尚未支持的附加块或动画".into()),
        }
        if data && kind != b"IDAT" {
            data_closed = true;
        }
        offset = end;
    }
    Err("VRM PNG缺少IEND".into())
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut n = 0;
    while n < 256 {
        let mut crc = n as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                0xedb88320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[n] = crc;
        n += 1;
    }
    table
}
fn crc32(raw: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc_table();
    let mut crc = u32::MAX;
    for byte in raw {
        crc = TABLE[((crc ^ u32::from(*byte)) & 255) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
#[path = "vrm_skin_tests.rs"]
mod tests;
