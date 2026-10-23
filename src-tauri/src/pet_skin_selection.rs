//! 独立于 PNG/VRM 素材库的宿主角色选择记录，不读取或改写账本。
use crate::local_pet_skin::{
    guard_storage_directory_chain, guarded_file, read_file_bounded, write_new,
};
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::Path};

const RECORD_NAME: &str = "pet-renderer-selection-v1.json";
const MAX_RECORD_BYTES: u64 = 1_024;
const READ_PROBLEM: &str = "角色选择记录不可用，已回退内置角色；请重新选择。";
const WRITE_PROBLEM: &str = "无法保存角色选择记录，原选择仍保留。";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActiveSkin {
    Builtin,
    Png(String),
    Vrm(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkinChoice {
    /// None 仅代表宿主权威记录尚不存在，由宿主兼容旧 PNG 选择。
    pub active: Option<ActiveSkin>,
    pub problem: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    active: StoredSkin,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum StoredSkin {
    Builtin {},
    Png { id: String },
    Vrm { id: String },
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl StoredSkin {
    fn from_active(active: &ActiveSkin) -> Result<Self, String> {
        match active {
            ActiveSkin::Builtin => Ok(Self::Builtin {}),
            ActiveSkin::Png(id) if valid_id(id) => Ok(Self::Png { id: id.clone() }),
            ActiveSkin::Vrm(id) if valid_id(id) => Ok(Self::Vrm { id: id.clone() }),
            _ => Err("角色标识无效，未改变当前选择。".into()),
        }
    }
    fn into_active(self) -> Result<ActiveSkin, ()> {
        match self {
            Self::Builtin {} => Ok(ActiveSkin::Builtin),
            Self::Png { id } if valid_id(&id) => Ok(ActiveSkin::Png(id)),
            Self::Vrm { id } if valid_id(&id) => Ok(ActiveSkin::Vrm(id)),
            _ => Err(()),
        }
    }
}

fn read_inner(app_data: &Path) -> Result<Option<ActiveSkin>, ()> {
    let _guards = guard_storage_directory_chain(app_data, app_data).map_err(|_| ())?;
    let record = app_data.join(RECORD_NAME);
    // exists() 会把悬空链接当作缺失；只有固定位置确实不存在才允许旧 PNG 兼容。
    match fs::symlink_metadata(&record) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    }
    let raw = read_file_bounded(
        &mut guarded_file(&record).map_err(|_| ())?,
        MAX_RECORD_BYTES,
    )
    .map_err(|_| ())?;
    let record: Record = serde_json::from_slice(&raw).map_err(|_| ())?;
    if record.schema_version != 1 {
        return Err(());
    }
    record.active.into_active().map(Some)
}

/// 文件格式、类型、权限和守卫失败均关闭旧选择，错误文字不包含本机路径或原文。
pub(crate) fn read(app_data: &Path) -> SkinChoice {
    match read_inner(app_data) {
        Ok(active) => SkinChoice {
            active,
            problem: None,
        },
        Err(()) => SkinChoice {
            active: Some(ActiveSkin::Builtin),
            problem: Some(READ_PROBLEM.into()),
        },
    }
}

/// 宿主在 skin_selection 锁内验证目标素材后调用；此处不访问任一素材库。
pub(crate) fn write(app_data: &Path, active: &ActiveSkin) -> Result<(), String> {
    let active = StoredSkin::from_active(active)?;
    let raw = serde_json::to_vec(&Record {
        schema_version: 1,
        active,
    })
    .map_err(|_| WRITE_PROBLEM.to_owned())?;
    if raw.is_empty() || raw.len() as u64 > MAX_RECORD_BYTES {
        return Err(WRITE_PROBLEM.into());
    }
    let _guards =
        guard_storage_directory_chain(app_data, app_data).map_err(|_| WRITE_PROBLEM.to_owned())?;
    let destination = app_data.join(RECORD_NAME);
    match fs::symlink_metadata(&destination) {
        // 先验证原记录类型并释放只读句柄，Windows 原子替换不能留着禁止删除共享的旧句柄。
        Ok(_) => drop(guarded_file(&destination).map_err(|_| WRITE_PROBLEM.to_owned())?),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return Err(WRITE_PROBLEM.into()),
    }
    let temporary = app_data.join(format!(
        ".pet-renderer-selection-{}.pending",
        uuid::Uuid::new_v4()
    ));
    // write_new 用 create_new 排除覆盖并执行 sync_all；创建失败时不删除归属不明的同名文件。
    write_new(&temporary, &raw).map_err(|_| WRITE_PROBLEM.to_owned())?;
    if fs::rename(&temporary, &destination).is_err() {
        // 只有 write_new 已确认本次创建的临时文件才清理，不递归删除任何目录。
        let _ = fs::remove_file(&temporary);
        return Err(WRITE_PROBLEM.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "pet_skin_selection_tests.rs"]
mod tests;
