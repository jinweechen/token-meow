//! Conservative window-region policy. All time values come from the native
//! monotonic clock; renderer timestamps and geometry are never authoritative.
use serde::{Deserialize, Serialize};

pub const MAX_GRID_WIDTH: u32 = 128;
pub const MAX_GRID_HEIGHT: u32 = 192;
pub const MAX_MASK_BYTES: usize = 3072;
pub const MAX_UI_RECTS: usize = 16;
pub const MAX_REGION_RECTS: usize = 4096;
pub const MASK_LEASE_MS: u64 = 1000;
pub const MASK_RENEW_MS: u64 = 400;
pub const DEFAULT_TEMPORARY_MS: u64 = 15_000;
pub const MAX_TEMPORARY_MS: u64 = 30_000;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const EDGE_PADDING: u32 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
}

impl Viewport {
    pub fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= 8192
            && self.height <= 8192
            && (48..=768).contains(&self.dpi)
    }
}

/// Physical client coordinates. Right and bottom are exclusive.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameMask {
    pub owner: u64,
    pub revision: u64,
    pub frame_key: String,
    pub viewport: Viewport,
    pub grid_width: u32,
    pub grid_height: u32,
    /// Row-major, least significant bit first. A set cell contains any alpha > 0.
    pub bits: Vec<u8>,
    pub ui_rects: Vec<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    StaleOwner,
    StaleRevision,
    InvalidFrameKey,
    GeometryMismatch,
    InvalidMask,
    TooComplex,
    AlreadyPublished,
    Expired,
    Exhausted,
}

#[derive(Debug)]
struct Frame {
    revision: u64,
    key: String,
    published: bool,
}

#[derive(Debug)]
struct Mask {
    revision: u64,
    deadline: u64,
    rects: Vec<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region<'a> {
    FullWindow,
    Rects(&'a [Rect]),
}

#[derive(Debug)]
pub struct Plan<'a> {
    pub viewport: Option<Viewport>,
    pub generation: u64,
    pub region: Region<'a>,
    pub ignore_cursor_events: bool,
    pub temporary_remaining_ms: u64,
}

/// Call only from the pet owner. A renderer registration replaces all older
/// owners. Begin-frame must be applied/acknowledged before displaying that frame.
#[derive(Debug, Default)]
pub struct PetPointerPolicy {
    owner: u64,
    owner_active: bool,
    last_revision: u64,
    frame: Option<Frame>,
    mask: Option<Mask>,
    viewport: Option<Viewport>,
    generation: u64,
    alpha_enabled: bool,
    click_through: bool,
    visible: bool,
    temporary_deadline: Option<u64>,
}

impl PetPointerPolicy {
    fn invalidate_mask(&mut self) {
        self.mask = None;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn register_renderer(&mut self) -> Result<u64, PolicyError> {
        if self.owner == MAX_SAFE_INTEGER {
            return Err(PolicyError::Exhausted);
        }
        self.owner += 1;
        self.owner_active = true;
        self.last_revision = 0;
        self.reset_context();
        Ok(self.owner)
    }

    pub fn release_renderer(&mut self, owner: u64) -> Result<(), PolicyError> {
        self.check_owner(owner)?;
        self.owner_active = false;
        self.reset_context();
        Ok(())
    }

    /// Source/reset/close invalidates both the renderer frame and temporary mode.
    /// The caller also registers a new owner when replacing the renderer itself.
    pub fn reset_context(&mut self) {
        self.frame = None;
        self.temporary_deadline = None;
        self.invalidate_mask();
    }

    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            self.reset_context();
        }
    }

    /// Supply geometry read from the HWND, never from a renderer command.
    pub fn set_viewport(&mut self, viewport: Option<Viewport>) {
        let viewport = viewport.filter(|value| value.valid());
        if self.viewport != viewport {
            self.viewport = viewport;
            self.frame = None;
            self.invalidate_mask();
        }
    }

    pub fn set_preferences(&mut self, alpha_enabled: bool, click_through: bool) {
        if self.alpha_enabled != alpha_enabled {
            self.alpha_enabled = alpha_enabled;
            self.frame = None;
            self.invalidate_mask();
        }
        // Temporary mode restores this current preference, not an old snapshot.
        self.click_through = click_through;
    }

    pub fn begin_frame(
        &mut self,
        owner: u64,
        revision: u64,
        frame_key: String,
    ) -> Result<(), PolicyError> {
        self.check_owner(owner)?;
        if revision <= self.last_revision || revision > MAX_SAFE_INTEGER {
            return Err(PolicyError::StaleRevision);
        }
        // Once a newer frame is announced, even an invalid payload must release
        // the old shape. Old/stale messages above cannot release a newer shape.
        self.last_revision = revision;
        self.frame = None;
        self.invalidate_mask();
        if frame_key.is_empty() || frame_key.len() > 256 || frame_key.chars().any(char::is_control)
        {
            return Err(PolicyError::InvalidFrameKey);
        }
        self.frame = Some(Frame {
            revision,
            key: frame_key,
            published: false,
        });
        Ok(())
    }

    pub fn publish_mask(&mut self, payload: FrameMask, now_ms: u64) -> Result<(), PolicyError> {
        self.check_frame(payload.owner, payload.revision, Some(&payload.frame_key))?;
        let frame = self.frame.as_mut().ok_or(PolicyError::StaleRevision)?;
        if frame.published {
            return Err(PolicyError::AlreadyPublished);
        }
        // A revision grants one publication, not a repeating lease. Rebuilding
        // after expiry/layout/error requires a new begin+rectangle ACK first.
        frame.published = true;
        self.invalidate_mask();
        if !self.visible || !self.alpha_enabled || self.viewport != Some(payload.viewport) {
            return Err(PolicyError::GeometryMismatch);
        }
        let rects = mask_rects(&payload)?;
        self.mask = Some(Mask {
            revision: payload.revision,
            deadline: now_ms.saturating_add(MASK_LEASE_MS),
            rects,
        });
        Ok(())
    }

    pub fn renew(&mut self, owner: u64, revision: u64, now_ms: u64) -> Result<(), PolicyError> {
        self.check_frame(owner, revision, None)?;
        match self.mask.as_mut() {
            Some(mask) if mask.revision == revision && now_ms < mask.deadline => {
                mask.deadline = now_ms.saturating_add(MASK_LEASE_MS);
                Ok(())
            }
            _ => {
                self.invalidate_mask();
                Err(PolicyError::Expired)
            }
        }
    }

    pub fn begin_temporary(&mut self, now_ms: u64, requested_ms: Option<u64>) {
        if self.visible {
            let duration = requested_ms
                .unwrap_or(DEFAULT_TEMPORARY_MS)
                .clamp(1, MAX_TEMPORARY_MS);
            self.temporary_deadline = Some(now_ms.saturating_add(duration));
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn cancel_temporary(&mut self) {
        if self.temporary_deadline.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Call on the existing native monotonic 50 ms tick, before any early return.
    /// This method performs no focus, foreground, persistence or OS operations.
    pub fn plan(&mut self, now_ms: u64) -> Plan<'_> {
        if self
            .mask
            .as_ref()
            .is_some_and(|mask| now_ms >= mask.deadline)
        {
            self.invalidate_mask();
        }
        if self
            .temporary_deadline
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.cancel_temporary();
        }
        let remaining = self
            .temporary_deadline
            .map_or(0, |deadline| deadline.saturating_sub(now_ms));
        let region = if remaining == 0 && self.visible && self.alpha_enabled {
            self.mask
                .as_ref()
                .map_or(Region::FullWindow, |mask| Region::Rects(&mask.rects))
        } else {
            Region::FullWindow
        };
        Plan {
            viewport: self.viewport,
            generation: self.generation,
            region,
            ignore_cursor_events: self.click_through && remaining == 0,
            temporary_remaining_ms: remaining,
        }
    }

    fn check_owner(&self, owner: u64) -> Result<(), PolicyError> {
        if !self.owner_active || owner == 0 || owner != self.owner {
            Err(PolicyError::StaleOwner)
        } else {
            Ok(())
        }
    }

    fn check_frame(&self, owner: u64, revision: u64, key: Option<&str>) -> Result<(), PolicyError> {
        self.check_owner(owner)?;
        if self.frame.as_ref().is_some_and(|frame| {
            frame.revision == revision && key.is_none_or(|key| frame.key == key)
        }) {
            Ok(())
        } else {
            Err(PolicyError::StaleRevision)
        }
    }
}

fn mask_rects(payload: &FrameMask) -> Result<Vec<Rect>, PolicyError> {
    let (width, height) = (payload.grid_width, payload.grid_height);
    if !payload.viewport.valid()
        || width == 0
        || height == 0
        || width > MAX_GRID_WIDTH
        || height > MAX_GRID_HEIGHT
        || width > payload.viewport.width
        || height > payload.viewport.height
        || payload.ui_rects.len() > MAX_UI_RECTS
    {
        return Err(PolicyError::InvalidMask);
    }
    let count = (width * height) as usize;
    if payload.bits.len() != count.div_ceil(8)
        || payload.bits.len() > MAX_MASK_BYTES
        || (count % 8 != 0 && payload.bits[count / 8] >> (count % 8) != 0)
    {
        return Err(PolicyError::InvalidMask);
    }
    let on = |x: u32, y: u32| {
        let index = (y * width + x) as usize;
        payload.bits[index / 8] & (1 << (index % 8)) != 0
    };
    let viewport = payload.viewport;
    let mut rects = Vec::new();
    for y in 0..height {
        let mut x = 0;
        while x < width {
            if !on(x, y) {
                x += 1;
                continue;
            }
            let left = x;
            while x < width && on(x, y) {
                x += 1;
            }
            rects.push(padded(
                Rect {
                    left: left * viewport.width / width,
                    top: y * viewport.height / height,
                    right: (x * viewport.width).div_ceil(width),
                    bottom: ((y + 1) * viewport.height).div_ceil(height),
                },
                viewport,
            ));
            if rects.len() > MAX_REGION_RECTS {
                return Err(PolicyError::TooComplex);
            }
        }
    }
    for rect in &payload.ui_rects {
        if rect.left >= rect.right
            || rect.top >= rect.bottom
            || rect.right > viewport.width
            || rect.bottom > viewport.height
        {
            return Err(PolicyError::InvalidMask);
        }
        rects.push(padded(*rect, viewport));
    }
    if rects.is_empty() {
        return Err(PolicyError::InvalidMask);
    }
    if rects.len() > MAX_REGION_RECTS {
        return Err(PolicyError::TooComplex);
    }
    Ok(rects)
}

fn padded(rect: Rect, viewport: Viewport) -> Rect {
    Rect {
        left: rect.left.saturating_sub(EDGE_PADDING),
        top: rect.top.saturating_sub(EDGE_PADDING),
        right: rect.right.saturating_add(EDGE_PADDING).min(viewport.width),
        bottom: rect
            .bottom
            .saturating_add(EDGE_PADDING)
            .min(viewport.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> (PetPointerPolicy, u64) {
        let mut policy = PetPointerPolicy::default();
        policy.set_visible(true);
        policy.set_preferences(true, false);
        policy.set_viewport(Some(Viewport {
            width: 100,
            height: 100,
            dpi: 96,
        }));
        let owner = policy.register_renderer().unwrap();
        policy.begin_frame(owner, 1, "frame-one".into()).unwrap();
        (policy, owner)
    }
    fn payload(owner: u64) -> FrameMask {
        FrameMask {
            owner,
            revision: 1,
            frame_key: "frame-one".into(),
            viewport: Viewport {
                width: 100,
                height: 100,
                dpi: 96,
            },
            grid_width: 2,
            grid_height: 2,
            bits: vec![1],
            ui_rects: vec![Rect {
                left: 90,
                top: 90,
                right: 100,
                bottom: 100,
            }],
        }
    }
    #[test]
    fn conservative_cells_and_ui_are_padded_and_bounded() {
        let (mut policy, owner) = ready();
        policy.publish_mask(payload(owner), 0).unwrap();
        assert_eq!(
            policy.plan(0).region,
            Region::Rects(&[
                Rect {
                    left: 0,
                    top: 0,
                    right: 52,
                    bottom: 52
                },
                Rect {
                    left: 88,
                    top: 88,
                    right: 100,
                    bottom: 100
                }
            ])
        );
    }
    #[test]
    fn begin_layout_change_and_deadline_restore_full_window() {
        let (mut policy, owner) = ready();
        policy.publish_mask(payload(owner), 0).unwrap();
        policy.renew(owner, 1, 400).unwrap();
        assert!(matches!(policy.plan(1399).region, Region::Rects(_)));
        assert_eq!(policy.plan(1400).region, Region::FullWindow);
        assert_eq!(policy.renew(owner, 1, 1401), Err(PolicyError::Expired));
        policy.begin_frame(owner, 2, "frame-two".into()).unwrap();
        assert_eq!(policy.plan(1401).region, Region::FullWindow);
        policy.set_viewport(Some(Viewport {
            width: 101,
            height: 100,
            dpi: 96,
        }));
        assert_eq!(
            policy.publish_mask(payload(owner), 1500),
            Err(PolicyError::StaleRevision)
        );
    }
    #[test]
    fn stale_owner_or_revision_cannot_erase_a_newer_mask() {
        let (mut policy, owner) = ready();
        policy.publish_mask(payload(owner), 0).unwrap();
        assert_eq!(
            policy.begin_frame(owner, 1, "old".into()),
            Err(PolicyError::StaleRevision)
        );
        assert_eq!(
            policy.publish_mask(payload(owner + 1), 0),
            Err(PolicyError::StaleOwner)
        );
        assert!(matches!(policy.plan(0).region, Region::Rects(_)));
        let next = policy.register_renderer().unwrap();
        assert_ne!(owner, next);
        assert_eq!(policy.plan(0).region, Region::FullWindow);
        policy.release_renderer(next).unwrap();
        assert_eq!(
            policy.begin_frame(next, 1, "late".into()),
            Err(PolicyError::StaleOwner)
        );
    }
    #[test]
    fn malformed_current_mask_and_geometry_never_reuse_old_shape() {
        let (mut policy, owner) = ready();
        policy.publish_mask(payload(owner), 0).unwrap();
        policy.begin_frame(owner, 2, "frame-two".into()).unwrap();
        let mut bad = payload(owner);
        bad.revision = 2;
        bad.frame_key = "frame-two".into();
        bad.bits = vec![0x80];
        assert_eq!(policy.publish_mask(bad, 1), Err(PolicyError::InvalidMask));
        assert_eq!(policy.plan(1).region, Region::FullWindow);
        policy.begin_frame(owner, 3, "frame-three".into()).unwrap();
        let mut bad = payload(owner);
        bad.revision = 3;
        bad.frame_key = "frame-three".into();
        bad.viewport.dpi = 144;
        assert_eq!(
            policy.publish_mask(bad, 1),
            Err(PolicyError::GeometryMismatch)
        );
    }
    #[test]
    fn publication_is_one_shot_and_expired_revisions_cannot_be_revived() {
        let (mut policy, owner) = ready();
        policy.publish_mask(payload(owner), 0).unwrap();
        assert_eq!(
            policy.publish_mask(payload(owner), 400),
            Err(PolicyError::AlreadyPublished)
        );
        assert!(matches!(policy.plan(999).region, Region::Rects(_)));
        assert_eq!(policy.plan(1000).region, Region::FullWindow);
        assert_eq!(
            policy.publish_mask(payload(owner), 1001),
            Err(PolicyError::AlreadyPublished)
        );
        assert_eq!(policy.plan(1001).region, Region::FullWindow);
        policy.begin_frame(owner, 2, "frame-two".into()).unwrap();
        let mut next = payload(owner);
        next.revision = 2;
        next.frame_key = "frame-two".into();
        policy.publish_mask(next, 1002).unwrap();
        assert!(matches!(policy.plan(1002).region, Region::Rects(_)));
    }
    #[test]
    fn temporary_restores_current_preference_and_hidden_cancels() {
        let (mut policy, _) = ready();
        policy.set_preferences(true, true);
        policy.begin_temporary(100, None);
        assert!(!policy.plan(100).ignore_cursor_events);
        assert_eq!(policy.plan(100).region, Region::FullWindow);
        policy.set_preferences(true, false);
        assert!(!policy.plan(15_100).ignore_cursor_events);
        policy.set_preferences(true, true);
        policy.begin_temporary(20_000, Some(u64::MAX));
        assert_eq!(policy.plan(20_000).temporary_remaining_ms, 30_000);
        policy.set_visible(false);
        assert!(policy.plan(20_001).ignore_cursor_events);
        assert_eq!(policy.plan(20_001).temporary_remaining_ms, 0);
    }
    #[test]
    fn complex_or_empty_regions_fall_back_without_truncation() {
        let mut value = payload(1);
        value.bits = vec![0];
        value.ui_rects.clear();
        assert_eq!(mask_rects(&value), Err(PolicyError::InvalidMask));
        value.grid_width = 100;
        value.grid_height = 100;
        value.bits = vec![0x55; 1250];
        assert_eq!(mask_rects(&value), Err(PolicyError::TooComplex));
    }
}
