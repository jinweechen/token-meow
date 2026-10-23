//! 仅在当前进程保留的辅助桌宠登记；窗口、采集器和主桌宠权限均由宿主管理。
use crate::pet_skin_selection::ActiveSkin;
use serde::{Deserialize, Serialize, Serializer};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const UNKNOWN_INSTANCE: &str = "辅助桌宠已关闭或实例已变化，请刷新后重试。";
const INVALID_LEASE: &str = "辅助桌宠渲染许可已失效，请重新登记当前窗口。";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PetIdentity {
    pub pet_id: String,
    pub instance_epoch: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum WireSkin {
    Builtin {},
    Png { id: String },
    Vrm { id: String },
}

fn valid_skin(choice: &ActiveSkin) -> bool {
    match choice {
        ActiveSkin::Builtin => true,
        ActiveSkin::Png(id) | ActiveSkin::Vrm(id) => {
            id.len() == 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }
    }
}
fn serialize_skin<S: Serializer>(choice: &ActiveSkin, serializer: S) -> Result<S::Ok, S::Error> {
    match choice {
        ActiveSkin::Builtin => WireSkin::Builtin {},
        ActiveSkin::Png(id) => WireSkin::Png { id: id.clone() },
        ActiveSkin::Vrm(id) => WireSkin::Vrm { id: id.clone() },
    }
    .serialize(serializer)
}
fn deserialize_skin<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ActiveSkin, D::Error> {
    let skin = match WireSkin::deserialize(deserializer)? {
        WireSkin::Builtin {} => ActiveSkin::Builtin,
        WireSkin::Png { id } => ActiveSkin::Png(id),
        WireSkin::Vrm { id } => ActiveSkin::Vrm(id),
    };
    if valid_skin(&skin) {
        Ok(skin)
    } else {
        Err(serde::de::Error::custom(
            "角色标识必须是本地 SHA-256 摘要。",
        ))
    }
}
fn present_skin<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ActiveSkin>, D::Error> {
    deserialize_skin(deserializer).map(Some)
}
fn present_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
fn serialize_generation<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PetConfig {
    #[serde(deserialize_with = "deserialize_skin")]
    pub skin_choice: ActiveSkin,
    pub visible: bool,
    pub scale: f64,
}
impl Default for PetConfig {
    fn default() -> Self {
        Self {
            skin_choice: ActiveSkin::Builtin,
            visible: true,
            scale: 1.0,
        }
    }
}
impl PetConfig {
    fn validate(&self) -> Result<(), String> {
        if !valid_skin(&self.skin_choice) {
            return Err("辅助桌宠的角色标识无效。".into());
        }
        if !self.scale.is_finite() || !(0.5..=1.5).contains(&self.scale) {
            return Err("辅助桌宠缩放须为 0.5 至 1.5。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PetConfigPatch {
    #[serde(default, deserialize_with = "present_skin")]
    pub skin_choice: Option<ActiveSkin>,
    #[serde(default, deserialize_with = "present_option")]
    pub visible: Option<bool>,
    #[serde(default, deserialize_with = "present_option")]
    pub scale: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowSize {
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlacementBounds {
    pub work_area: WorkArea,
    pub window_size: WindowSize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PetDescription {
    pub pet_id: String,
    pub instance_epoch: String,
    pub label: String,
    #[serde(serialize_with = "serialize_skin")]
    pub skin_choice: ActiveSkin,
    pub visible: bool,
    pub scale: f64,
    pub position: Option<PhysicalPoint>,
}
impl PetDescription {
    pub(crate) fn identity(&self) -> PetIdentity {
        PetIdentity {
            pet_id: self.pet_id.clone(),
            instance_epoch: self.instance_epoch.clone(),
        }
    }
}

/// 只有 native reserve 能构造；异步创建回调持有 clone，也只能取消自己的生命周期。
#[derive(Clone)]
pub(crate) struct Reservation {
    pub identity: PetIdentity,
    pub window_label: String,
    pub generation: u64,
    pub token: String,
    cancelled: Arc<AtomicBool>,
}
impl Reservation {
    pub(crate) fn is_live(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
    }
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn same_owner(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.window_label == other.window_label
            && self.generation == other.generation
            && self.token == other.token
            && Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

/// 辅助视图专用许可。它不含主桌宠移动、透明命中、弹卡或通知确认权限。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererLease {
    #[serde(flatten)]
    pub identity: PetIdentity,
    pub renderer_epoch: String,
    pub token: String,
    #[serde(serialize_with = "serialize_generation")]
    pub generation: u64,
    #[serde(skip)]
    instance_cancelled: Arc<AtomicBool>,
    #[serde(skip)]
    cancelled: Arc<AtomicBool>,
}
impl RendererLease {
    pub(crate) fn is_live(&self) -> bool {
        !self.instance_cancelled.load(Ordering::Acquire) && !self.cancelled.load(Ordering::Acquire)
    }
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn same_owner(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.renderer_epoch == other.renderer_epoch
            && self.token == other.token
            && self.generation == other.generation
            && Arc::ptr_eq(&self.cancelled, &other.cancelled)
            && Arc::ptr_eq(&self.instance_cancelled, &other.instance_cancelled)
    }
}

struct Pending {
    reservation: Reservation,
    config: PetConfig,
}
struct Entry {
    description: PetDescription,
    reservation: Reservation,
    renderer: Option<RendererLease>,
}

/// 首期只允许一个辅助窗口；主 pet 不登记在这里，不能由该注册表增删或授权。
#[derive(Default)]
pub(crate) struct Registry {
    generation: u64,
    pending: Option<Pending>,
    entry: Option<Entry>,
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.clear();
    }
}

impl Registry {
    fn next_generation(&mut self) -> Result<u64, String> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("辅助桌宠代次已耗尽，请重新启动应用。")?;
        Ok(self.generation)
    }
    fn prune_cancelled(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| !pending.reservation.is_live())
        {
            self.pending = None;
        }
        if self
            .entry
            .as_ref()
            .is_some_and(|entry| !entry.reservation.is_live())
        {
            if let Some(entry) = self.entry.take() {
                if let Some(renderer) = entry.renderer {
                    renderer.cancel();
                }
            }
        }
    }
    pub(crate) fn reserve(&mut self, config: PetConfig) -> Result<Reservation, String> {
        config.validate()?;
        self.prune_cancelled();
        if self.pending.is_some() || self.entry.is_some() {
            return Err("目前最多保留一个辅助桌宠，请先关闭已有窗口。".into());
        }
        let generation = self.next_generation()?;
        let pet_id = uuid::Uuid::new_v4().to_string();
        let reservation = Reservation {
            window_label: format!("extra-pet-{pet_id}"),
            identity: PetIdentity {
                pet_id,
                instance_epoch: uuid::Uuid::new_v4().to_string(),
            },
            generation,
            token: uuid::Uuid::new_v4().to_string(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        self.pending = Some(Pending {
            reservation: reservation.clone(),
            config,
        });
        Ok(reservation)
    }
    pub(crate) fn abort(&mut self, reservation: &Reservation) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.reservation.same_owner(reservation))
        {
            if let Some(pending) = self.pending.take() {
                pending.reservation.cancel();
            }
            true
        } else {
            false
        }
    }
    pub(crate) fn commit(
        &mut self,
        reservation: &Reservation,
        actual_window_label: &str,
    ) -> Result<PetDescription, String> {
        if !self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.reservation.same_owner(reservation))
        {
            return Err(UNKNOWN_INSTANCE.into());
        }
        let pending = self.pending.take().ok_or(UNKNOWN_INSTANCE)?;
        if !pending.reservation.is_live() || actual_window_label != pending.reservation.window_label
        {
            pending.reservation.cancel();
            return Err("辅助窗口创建未完成或实际窗口标签不匹配，已取消登记。".into());
        }
        let description = PetDescription {
            pet_id: pending.reservation.identity.pet_id.clone(),
            instance_epoch: pending.reservation.identity.instance_epoch.clone(),
            label: actual_window_label.to_owned(),
            skin_choice: pending.config.skin_choice,
            visible: pending.config.visible,
            scale: pending.config.scale,
            position: None,
        };
        self.entry = Some(Entry {
            description: description.clone(),
            reservation: pending.reservation,
            renderer: None,
        });
        Ok(description)
    }
    pub(crate) fn get_by_label(&self, actual_window_label: &str) -> Option<PetDescription> {
        self.entry
            .as_ref()
            .filter(|entry| {
                entry.reservation.is_live() && entry.description.label == actual_window_label
            })
            .map(|entry| entry.description.clone())
    }
    pub(crate) fn get(&self, identity: &PetIdentity) -> Option<PetDescription> {
        self.entry
            .as_ref()
            .filter(|entry| {
                entry.reservation.is_live() && entry.description.identity() == *identity
            })
            .map(|entry| entry.description.clone())
    }
    pub(crate) fn list(&self) -> Vec<PetDescription> {
        self.entry
            .as_ref()
            .filter(|entry| entry.reservation.is_live())
            .map(|entry| vec![entry.description.clone()])
            .unwrap_or_default()
    }
    fn entry_mut(&mut self, identity: &PetIdentity) -> Result<&mut Entry, String> {
        self.entry
            .as_mut()
            .filter(|entry| {
                entry.reservation.is_live() && entry.description.identity() == *identity
            })
            .ok_or_else(|| UNKNOWN_INSTANCE.into())
    }
    pub(crate) fn configure(
        &mut self,
        identity: &PetIdentity,
        patch: PetConfigPatch,
    ) -> Result<PetDescription, String> {
        if patch.skin_choice.is_none() && patch.visible.is_none() && patch.scale.is_none() {
            return Err("请至少修改一项辅助桌宠设置。".into());
        }
        let entry = self.entry_mut(identity)?;
        let config = PetConfig {
            skin_choice: patch
                .skin_choice
                .unwrap_or_else(|| entry.description.skin_choice.clone()),
            visible: patch.visible.unwrap_or(entry.description.visible),
            scale: patch.scale.unwrap_or(entry.description.scale),
        };
        config.validate()?;
        if config.skin_choice != entry.description.skin_choice
            || config.visible != entry.description.visible
            || config.scale != entry.description.scale
        {
            if let Some(renderer) = entry.renderer.take() {
                renderer.cancel();
            }
            entry.description.skin_choice = config.skin_choice;
            entry.description.visible = config.visible;
            entry.description.scale = config.scale;
        }
        Ok(entry.description.clone())
    }
    pub(crate) fn set_position(
        &mut self,
        identity: &PetIdentity,
        requested: PhysicalPoint,
        bounds: PlacementBounds,
    ) -> Result<PetDescription, String> {
        let point = clamp_position(requested, bounds)?;
        let entry = self.entry_mut(identity)?;
        entry.description.position = Some(point);
        Ok(entry.description.clone())
    }
    pub(crate) fn remove(&mut self, identity: &PetIdentity) -> Result<PetDescription, String> {
        if !self
            .entry
            .as_ref()
            .is_some_and(|entry| entry.description.identity() == *identity)
        {
            return Err(UNKNOWN_INSTANCE.into());
        }
        let entry = self.entry.take().ok_or(UNKNOWN_INSTANCE)?;
        entry.reservation.cancel();
        if let Some(renderer) = entry.renderer {
            renderer.cancel();
        }
        Ok(entry.description)
    }
    pub(crate) fn revoke_window(
        &mut self,
        actual_window_label: &str,
        instance_epoch: &str,
    ) -> Option<PetDescription> {
        let identity = self
            .entry
            .as_ref()
            .filter(|entry| {
                entry.description.label == actual_window_label
                    && entry.description.instance_epoch == instance_epoch
            })?
            .description
            .identity();
        self.remove(&identity).ok()
    }
    pub(crate) fn clear(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.reservation.cancel();
        }
        if let Some(entry) = self.entry.take() {
            entry.reservation.cancel();
            if let Some(renderer) = entry.renderer {
                renderer.cancel();
            }
        }
    }
    pub(crate) fn register_renderer(
        &mut self,
        actual_window_label: &str,
        identity: &PetIdentity,
        renderer_epoch: &str,
    ) -> Result<RendererLease, String> {
        if !uuid::Uuid::parse_str(renderer_epoch)
            .is_ok_and(|epoch| epoch.to_string() == renderer_epoch)
        {
            return Err("辅助视图渲染代次无效。".into());
        }
        let entry = self.entry_mut(identity)?;
        if entry.description.label != actual_window_label || !entry.description.visible {
            return Err(INVALID_LEASE.into());
        }
        if let Some(previous) = entry.renderer.take() {
            previous.cancel();
        }
        let generation = self.next_generation()?;
        let entry = self.entry_mut(identity)?;
        let lease = RendererLease {
            identity: identity.clone(),
            renderer_epoch: renderer_epoch.to_owned(),
            token: uuid::Uuid::new_v4().to_string(),
            generation,
            instance_cancelled: entry.reservation.cancelled.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        entry.renderer = Some(lease.clone());
        Ok(lease)
    }

    /// 系统暂停立即撤销当前视图许可，恢复后必须重新登记，不能续用旧回调。
    pub(crate) fn revoke_renderers(&mut self) {
        if let Some(entry) = self.entry.as_mut() {
            if let Some(lease) = entry.renderer.take() { lease.cancel(); }
        }
    }
    pub(crate) fn validate_renderer(
        &self,
        actual_window_label: &str,
        lease: &RendererLease,
    ) -> bool {
        self.entry.as_ref().is_some_and(|entry| {
            entry.reservation.is_live()
                && entry.description.visible
                && entry.description.label == actual_window_label
                && entry.description.identity() == lease.identity
                && lease.is_live()
                && entry
                    .renderer
                    .as_ref()
                    .is_some_and(|current| current.same_owner(lease))
        })
    }

    /// IPC 先用真实调用窗口标签和前端提交的 opaque token 查当前许可，再把 clone 交给原生任务。
    pub(crate) fn renderer_for_request(
        &self,
        actual_window_label: &str,
        identity: &PetIdentity,
        renderer_epoch: &str,
        token: &str,
    ) -> Option<RendererLease> {
        let lease = self.entry.as_ref()?.renderer.as_ref()?;
        (lease.identity == *identity
            && lease.renderer_epoch == renderer_epoch
            && lease.token == token
            && self.validate_renderer(actual_window_label, lease))
        .then(|| lease.clone())
    }
}

fn clamp_position(
    requested: PhysicalPoint,
    bounds: PlacementBounds,
) -> Result<PhysicalPoint, String> {
    let area = bounds.work_area;
    let size = bounds.window_size;
    if area.width == 0
        || area.height == 0
        || size.width == 0
        || size.height == 0
        || size.width > area.width
        || size.height > area.height
        || i64::from(area.x) + i64::from(area.width) > i64::from(i32::MAX) + 1
        || i64::from(area.y) + i64::from(area.height) > i64::from(i32::MAX) + 1
    {
        return Err("辅助桌宠尺寸或工作区无效，请重新获取实际显示器边界。".into());
    }
    let right = i64::from(area.x) + i64::from(area.width - size.width);
    let bottom = i64::from(area.y) + i64::from(area.height - size.height);
    Ok(PhysicalPoint {
        x: i64::from(requested.x).clamp(i64::from(area.x), right) as i32,
        y: i64::from(requested.y).clamp(i64::from(area.y), bottom) as i32,
    })
}

#[cfg(test)]
#[path = "pet_registry_tests.rs"]
mod tests;
