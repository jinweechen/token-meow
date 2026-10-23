//! 单一角色选择记录独立于两类素材库；素材目录故障不阻断内置角色恢复。
use super::*;
use crate::{local_pet_skin::{LocalPetSkinProblem, PetSkinState}, pet_skin_selection::{self, ActiveSkin}, vrm_skin::{VrmSkinProblem, VrmSkinState}};

fn png_problem(state: &mut PetSkinState, message: String) {
    state.problems.truncate(31);
    state.problems.push(LocalPetSkinProblem { skin_id: None, message });
}
fn vrm_problem(state: &mut VrmSkinState, message: String) {
    state.problems.truncate(31);
    state.problems.push(VrmSkinProblem { model_id: None, message });
}
// 调用者已取得 skin_selection；返回的选择只服从宿主根目录的权威记录。
pub(super) fn png_state(shared: &Shared) -> Result<PetSkinState, String> {
    let mut state = match shared.skins.lock().map_err(|_| lock_error())?.as_ref() {
        Ok(store) => store.state().unwrap_or_else(|error| PetSkinState { selected_skin_id: None, skins: vec![], problems: vec![LocalPetSkinProblem { skin_id: None, message: error }] }),
        Err(error) => PetSkinState { selected_skin_id: None, skins: vec![], problems: vec![LocalPetSkinProblem { skin_id: None, message: error.clone() }] },
    };
    let choice = pet_skin_selection::read(&shared.data_dir);
    match choice.active {
        None => {}, // 升级兼容：没有新记录时，仅沿用旧 PNG 选择。
        Some(ActiveSkin::Png(id)) => {
            state.selected_skin_id = state.skins.iter().any(|skin| skin.skin_id == id).then_some(id);
            if state.selected_skin_id.is_none() { png_problem(&mut state, "所选 PNG 皮肤不可用，已回退内置角色。".into()); }
        }
        Some(_) => state.selected_skin_id = None,
    }
    if let Some(problem) = choice.problem { png_problem(&mut state, problem); }
    Ok(state)
}
pub(super) fn vrm_state(shared: &Shared) -> Result<VrmSkinState, String> {
    let mut state = match shared.vrm_skins.lock().map_err(|_| lock_error())?.as_ref() {
        Ok(store) => store.state().unwrap_or_else(|error| VrmSkinState { selected_model_id: None, models: vec![], problems: vec![VrmSkinProblem { model_id: None, message: error }] }),
        Err(error) => VrmSkinState { selected_model_id: None, models: vec![], problems: vec![VrmSkinProblem { model_id: None, message: error.clone() }] },
    };
    let choice = pet_skin_selection::read(&shared.data_dir);
    match choice.active {
        Some(ActiveSkin::Vrm(id)) => {
            state.selected_model_id = state.models.iter().any(|model| model.model_id == id).then_some(id);
            if state.selected_model_id.is_none() { vrm_problem(&mut state, "所选 VRM 角色不可用，已回退内置角色。".into()); }
        }
        _ => state.selected_model_id = None,
    }
    if let Some(problem) = choice.problem { vrm_problem(&mut state, problem); }
    Ok(state)
}
fn clear_png(shared: &Shared) {
    if let Ok(mut owner) = shared.skins.lock() { if let Ok(store) = owner.as_mut() { let _ = store.select(None); } }
}
fn clear_vrm(shared: &Shared) {
    if let Ok(mut owner) = shared.vrm_skins.lock() { if let Ok(store) = owner.as_mut() { let _ = store.select(None); } }
}
pub(super) fn select_png(shared: &Shared, id: Option<&str>) -> Result<PetSkinState, String> {
    let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
    if let Some(id) = id {
        let mut owner = shared.skins.lock().map_err(|_| lock_error())?;
        let store = owner.as_mut().map_err(|error| error.clone())?;
        let previous = store.state()?.selected_skin_id;
        if pet_skin_selection::read(&shared.data_dir).active.is_none() {
            let prior = previous.as_ref().map(|id| ActiveSkin::Png(id.clone())).unwrap_or(ActiveSkin::Builtin);
            pet_skin_selection::write(&shared.data_dir, &prior)?;
        }
        store.select(Some(id))?;
        if let Err(error) = pet_skin_selection::write(&shared.data_dir, &ActiveSkin::Png(id.to_owned())) {
            let restored = store.select(previous.as_deref());
            return Err(if restored.is_ok() { error } else { format!("{error}；还原库内选择失败，请重新选择角色。") });
        }
        drop(owner);
        clear_vrm(shared);
    } else {
        // 恢复内置只要求宿主选择记录可写，不要求两类素材目录健康。
        pet_skin_selection::write(&shared.data_dir, &ActiveSkin::Builtin)?;
        clear_png(shared); clear_vrm(shared);
    }
    png_state(shared)
}
pub(super) fn select_vrm(shared: &Shared, id: Option<&str>) -> Result<VrmSkinState, String> {
    let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
    if let Some(id) = id {
        let mut owner = shared.vrm_skins.lock().map_err(|_| lock_error())?;
        let store = owner.as_mut().map_err(|error| error.clone())?;
        let previous = store.state()?.selected_model_id;
        store.select(Some(id))?;
        if let Err(error) = pet_skin_selection::write(&shared.data_dir, &ActiveSkin::Vrm(id.to_owned())) {
            let restored = store.select(previous.as_deref());
            return Err(if restored.is_ok() { error } else { format!("{error}；还原库内选择失败，请重新选择角色。") });
        }
        drop(owner);
        clear_png(shared);
    } else {
        pet_skin_selection::write(&shared.data_dir, &ActiveSkin::Builtin)?;
        clear_png(shared); clear_vrm(shared);
    }
    vrm_state(shared)
}
pub(super) fn remove_png(shared: &Shared, id: &str) -> Result<PetSkinState, String> {
    let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
    extra_pets::require_unused_skin(shared, id, false)?;
    let choice = pet_skin_selection::read(&shared.data_dir);
    if matches!(&choice.active, Some(ActiveSkin::Png(selected)) if selected == id) { return Err("请先切换角色，再移除当前 PNG 皮肤。".into()); }
    {
        let mut owner = shared.skins.lock().map_err(|_| lock_error())?;
        let store = owner.as_mut().map_err(|error| error.clone())?;
        if choice.active.is_some() { store.remove_inactive(id)?; } else { store.remove(id)?; }
    }
    png_state(shared)
}
pub(super) fn remove_vrm(shared: &Shared, id: &str) -> Result<VrmSkinState, String> {
    let _selection = shared.skin_selection.lock().map_err(|_| lock_error())?;
    extra_pets::require_unused_skin(shared, id, true)?;
    let choice = pet_skin_selection::read(&shared.data_dir);
    if matches!(&choice.active, Some(ActiveSkin::Vrm(selected)) if selected == id) { return Err("请先切换角色，再移除当前 VRM 角色。".into()); }
    {
        let mut owner = shared.vrm_skins.lock().map_err(|_| lock_error())?;
        let store = owner.as_mut().map_err(|error| error.clone())?;
        store.remove_inactive(id)?;
    }
    vrm_state(shared)
}
