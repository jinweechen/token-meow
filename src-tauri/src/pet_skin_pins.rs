//! 跨后台验证和 UI 提交的短期素材占用；不持有库锁、不写磁盘。
use crate::pet_skin_selection::ActiveSkin;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Default)]
struct State {
    closed: bool,
    entries: HashMap<String, Entry>,
}
struct Entry {
    skins: Vec<ActiveSkin>,
    cancelled: Arc<AtomicBool>,
}
#[derive(Clone, Default)]
pub(super) struct SkinPins(Arc<Mutex<State>>);
struct Lease {
    owner: SkinPins,
    id: String,
    cancelled: Arc<AtomicBool>,
}
/// 后台操作持有 owner；离开作用域即取消尚未完成的 UI 回调。
pub(super) struct SkinPin(Arc<Lease>);
/// 排队的 UI 回调保留素材占用，直到自身退出，防止超时回滚时素材被删。
#[derive(Clone)]
pub(super) struct SkinPinTicket(Arc<Lease>);
impl SkinPins {
    pub(super) fn pin(&self, skins: Vec<ActiveSkin>) -> Result<SkinPin, String> {
        let mut state = self.0.lock().map_err(|_| "角色占用状态不可用。")?;
        if state.closed {
            return Err("应用正在退出，角色操作已取消。".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancelled = Arc::new(AtomicBool::new(false));
        state.entries.insert(
            id.clone(),
            Entry {
                skins,
                cancelled: cancelled.clone(),
            },
        );
        Ok(SkinPin(Arc::new(Lease {
            owner: self.clone(),
            id,
            cancelled,
        })))
    }
    pub(super) fn uses(&self, id: &str, vrm: bool) -> Result<bool, String> {
        let state = self.0.lock().map_err(|_| "角色占用状态不可用。")?;
        Ok(state.entries.values().any(|entry| {
            entry.skins.iter().any(|skin| {
                matches!(skin,
            ActiveSkin::Vrm(value) if vrm && value == id)
                    || matches!(skin,
            ActiveSkin::Png(value) if !vrm && value == id)
            })
        }))
    }
    pub(super) fn close(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.closed = true;
            // 仍在执行的 UI 回调保留 pin，直至回滚/退出；禁止新操作。
            for entry in state.entries.values() {
                entry.cancelled.store(true, Ordering::Release);
            }
        }
    }
}
impl SkinPin {
    pub(super) fn ticket(&self) -> SkinPinTicket {
        SkinPinTicket(self.0.clone())
    }
}
impl Drop for SkinPin {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::Release);
    }
}
impl SkinPinTicket {
    pub(super) fn is_live(&self) -> bool {
        !self.0.cancelled.load(Ordering::Acquire)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.0.lock() {
            state.entries.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const NEW: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    #[test]
    fn queued_create_blocks_deletion_after_background_validation_without_holding_selection() {
        let selection = Arc::new(Mutex::new(()));
        let pins = SkinPins::default();
        let operation = {
            let _selection = selection.lock().unwrap();
            pins.pin(vec![ActiveSkin::Png(OLD.into())]).unwrap()
        };
        let queued = operation.ticket();
        let worker_pins = pins.clone();
        let worker_selection = selection.clone();
        let blocked = std::thread::spawn(move || {
            let _selection = worker_selection.lock().unwrap();
            worker_pins.uses(OLD, false).unwrap()
        })
        .join()
        .unwrap();
        assert!(blocked);
        assert!(queued.is_live());
        drop(queued);
        drop(operation);
        assert!(!pins.uses(OLD, false).unwrap());
    }
    #[test]
    fn timed_out_owner_cancels_callback_but_keeps_rollback_skins_until_callback_exits() {
        let pins = SkinPins::default();
        let operation = pins
            .pin(vec![
                ActiveSkin::Png(OLD.into()),
                ActiveSkin::Vrm(NEW.into()),
            ])
            .unwrap();
        let callback = operation.ticket();
        drop(operation);
        assert!(!callback.is_live());
        assert!(pins.uses(OLD, false).unwrap());
        assert!(pins.uses(NEW, true).unwrap());
        assert!(!pins.uses(OLD, true).unwrap());
        assert!(!pins.uses(NEW, false).unwrap());
        drop(callback);
        assert!(!pins.uses(OLD, false).unwrap());
        assert!(!pins.uses(NEW, true).unwrap());
    }
    #[test]
    fn configuration_pin_protects_both_target_and_previous_skin_for_failed_native_rollback() {
        let pins = SkinPins::default();
        let operation = pins
            .pin(vec![
                ActiveSkin::Vrm(OLD.into()),
                ActiveSkin::Png(NEW.into()),
            ])
            .unwrap();
        let callback = operation.ticket();
        assert!(pins.uses(OLD, true).unwrap());
        assert!(pins.uses(NEW, false).unwrap());
        drop(callback);
        assert!(pins.uses(OLD, true).unwrap());
        drop(operation);
        assert!(!pins.uses(OLD, true).unwrap());
        assert!(!pins.uses(NEW, false).unwrap());
    }
    #[test]
    fn older_callback_drop_cannot_remove_a_newer_operation_pin_for_same_skin() {
        let pins = SkinPins::default();
        let old = pins.pin(vec![ActiveSkin::Png(OLD.into())]).unwrap();
        let old_callback = old.ticket();
        let current = pins.pin(vec![ActiveSkin::Png(OLD.into())]).unwrap();
        drop(old);
        drop(old_callback);
        assert!(pins.uses(OLD, false).unwrap());
        assert!(current.ticket().is_live());
        drop(current);
        assert!(!pins.uses(OLD, false).unwrap());
    }
    #[test]
    fn shutdown_cancels_callbacks_disallows_new_pins_and_releases_after_all_owners_exit() {
        let pins = SkinPins::default();
        let operation = pins.pin(vec![ActiveSkin::Png(OLD.into())]).unwrap();
        let callback = operation.ticket();
        pins.close();
        assert!(!callback.is_live());
        assert!(pins.pin(vec![ActiveSkin::Builtin]).is_err());
        drop(operation);
        assert!(pins.uses(OLD, false).unwrap());
        drop(callback);
        assert!(pins.0.lock().unwrap().entries.is_empty());
    }
}
