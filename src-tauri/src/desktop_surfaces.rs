//! 只读的前台普通窗口几何快照；不枚举后台窗口，不读取标题、进程路径或内容。
//! 这里不拥有任何窗口，也不执行移动。Snapshot 只在短时复核后为上层提供平台事实。
//! 本文件为基于 Win32 API 的原创薄适配，没有复制第三方桌宠源码或素材。

use std::{
    collections::hash_map::RandomState,
    fmt,
    hash::BuildHasher,
    sync::OnceLock,
    time::{Duration, Instant},
};

const OBSERVATION_TTL: Duration = Duration::from_millis(500);
const MAX_RECT_SIDE: i64 = 32_768;
const MAX_RECT_AREA: i64 = 256 * 1024 * 1024;
const MAX_COORDINATE: i64 = 1_000_000;
const MIN_SURFACE_WIDTH: i64 = 64;
const MIN_SURFACE_HEIGHT: i64 = 48;

/// 物理虚拟屏幕坐标，right/bottom 为开区间。负坐标可用于主屏左侧或上方的显示器。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    fn width(self) -> i64 {
        i64::from(self.right) - i64::from(self.left)
    }
    fn height(self) -> i64 {
        i64::from(self.bottom) - i64::from(self.top)
    }

    fn valid(self) -> bool {
        let width = self.width();
        let height = self.height();
        width > 0
            && height > 0
            && width <= MAX_RECT_SIDE
            && height <= MAX_RECT_SIDE
            && width * height <= MAX_RECT_AREA
            && [self.left, self.top, self.right, self.bottom]
                .into_iter()
                .all(|value| i64::from(value).abs() <= MAX_COORDINATE)
    }

    fn intersection(self, other: Self) -> Option<Self> {
        if !self.valid() || !other.valid() {
            return None;
        }
        let clipped = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        clipped.valid().then_some(clipped)
    }
}

/// rect 经过工作区裁剪，四个布尔值只指向原窗口的真实边。
/// 例如窗口顶部在工作区上方，裁出的 rect.top 不能被当成该窗口的顶边平台。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisibleEdges {
    pub rect: Rect,
    pub top: bool,
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
}

fn visible_edges(raw: Rect, work_area: Rect) -> Option<VisibleEdges> {
    if raw.width() < MIN_SURFACE_WIDTH || raw.height() < MIN_SURFACE_HEIGHT {
        return None;
    }
    let rect = raw.intersection(work_area)?;
    if rect.width() < MIN_SURFACE_WIDTH || rect.height() < MIN_SURFACE_HEIGHT {
        return None;
    }
    let edges = VisibleEdges {
        rect,
        top: raw.top == rect.top,
        left: raw.left == rect.left,
        right: raw.right == rect.right,
        bottom: raw.bottom == rect.bottom,
    };
    (edges.top || edges.left || edges.right || edges.bottom).then_some(edges)
}

/// 只能作同进程观测关联，不能代替 TTL 或原生重新读取。不得从此值推断 HWND。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ObservationSignature(u64);

impl ObservationSignature {
    pub(super) fn opaque_id(self) -> u64 {
        self.0
    }
}

// HWND/PID 只留在此模块。哈希碰撞也不构成许可：revalidate 总会比较完整 identity。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct NativeIdentity {
    handle: usize,
    process_id: u32,
    rect: Rect,
}

#[derive(Clone)]
pub(super) struct Snapshot {
    identity: NativeIdentity,
    work_area: Rect,
    edges: VisibleEdges,
    signature: ObservationSignature,
    observed_at: Instant,
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 日志只提供 opaque 关联和允许公开的几何，不泄露原生句柄或进程标识。
        formatter
            .debug_struct("ForegroundSurfaceSnapshot")
            .field("signature", &self.signature)
            .field("edges", &self.edges)
            .field("observed_at", &self.observed_at)
            .finish_non_exhaustive()
    }
}

impl Snapshot {
    fn new(
        identity: NativeIdentity,
        work_area: Rect,
        observed_at: Instant,
        now: Instant,
    ) -> Option<Self> {
        if identity.handle == 0 || identity.process_id == 0 {
            return None;
        }
        let edges = visible_edges(identity.rect, work_area)?;
        static SIGNATURE_HASHER: OnceLock<RandomState> = OnceLock::new();
        let opaque = SIGNATURE_HASHER
            .get_or_init(RandomState::new)
            .hash_one((identity, work_area));
        let snapshot = Self {
            identity,
            work_area,
            edges,
            signature: ObservationSignature(opaque),
            observed_at,
        };
        snapshot.fresh_at(now).then_some(snapshot)
    }

    pub(super) fn rect(&self) -> Rect {
        self.edges.rect
    }
    pub(super) fn edges(&self) -> VisibleEdges {
        self.edges
    }
    pub(super) fn signature(&self) -> ObservationSignature {
        self.signature
    }
    pub(super) fn observed_at(&self) -> Instant {
        self.observed_at
    }
    pub(super) fn age_at(&self, now: Instant) -> Option<Duration> {
        now.checked_duration_since(self.observed_at)
    }

    fn fresh_at(&self, now: Instant) -> bool {
        self.age_at(now).is_some_and(|age| age < OBSERVATION_TTL)
    }

    fn revalidate_with(
        &self,
        work_area: Rect,
        before: Instant,
        capture: impl FnOnce() -> Option<NativeIdentity>,
        finished_at: impl FnOnce() -> Instant,
    ) -> bool {
        if work_area != self.work_area || !work_area.valid() || !self.fresh_at(before) {
            return false;
        }
        let Some(current) = capture() else {
            return false;
        };
        current == self.identity
            && visible_edges(current.rect, work_area) == Some(self.edges)
            && self.fresh_at(finished_at())
    }

    /// 紧邻实际 apply 再读一次前台窗口、PID、可见性、DWM 和原始矩形。
    /// 此函数不锁住外部程序；调用者仍须同步消费结果，不可 await 后再执行移动。
    pub(super) fn revalidate(&self, work_area: Rect) -> bool {
        self.revalidate_with(work_area, Instant::now(), native::foreground, Instant::now)
    }
}

/// 不缓存读取结果；任何失败都返回 None，不继承先前成功的平台。
pub(super) fn sample(work_area: Rect) -> Option<Snapshot> {
    if !work_area.valid() {
        return None;
    }
    let observed_at = Instant::now();
    Snapshot::new(
        native::foreground()?,
        work_area,
        observed_at,
        Instant::now(),
    )
}

#[cfg(windows)]
mod native {
    use super::{NativeIdentity, Rect};
    use std::{ffi::c_void, mem::size_of};
    use windows_sys::Win32::{
        Foundation::{GetLastError, SetLastError, HWND, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
        System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::{
            GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowLongW,
            GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, GWL_EXSTYLE,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    };

    fn process_id(hwnd: HWND) -> Option<u32> {
        let mut process_id = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
        (thread != 0 && process_id != 0).then_some(process_id)
    }

    fn ordinary_visible(hwnd: HWND) -> bool {
        unsafe {
            if hwnd.is_null()
                || hwnd == GetShellWindow()
                || hwnd == GetDesktopWindow()
                || IsWindowVisible(hwnd) == 0
                || IsIconic(hwnd) != 0
                || IsZoomed(hwnd) != 0
            {
                return false;
            }
            SetLastError(0);
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            if (style == 0 && GetLastError() != 0)
                || style as u32 & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0
            {
                return false;
            }
            // 仅识别系统 shell 类；不取标题，也不保存或发布应用自定义类名。
            let mut class = [0u16; 64];
            let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            if length <= 0 {
                return false;
            }
            let shell = [
                "Progman",
                "WorkerW",
                "Shell_TrayWnd",
                "Shell_SecondaryTrayWnd",
            ];
            if shell.iter().any(|name| {
                name.encode_utf16()
                    .eq(class[..length as usize].iter().copied())
            }) {
                return false;
            }
            let mut cloaked = 0u32;
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED as u32,
                &mut cloaked as *mut _ as *mut c_void,
                size_of::<u32>() as u32,
            ) >= 0
                && cloaked == 0
        }
    }

    fn frame(hwnd: HWND) -> Option<Rect> {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        let result = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                &mut rect as *mut _ as *mut c_void,
                size_of::<RECT>() as u32,
            )
        };
        (result >= 0).then_some(Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }

    pub(super) fn foreground() -> Option<NativeIdentity> {
        let hwnd = unsafe { GetForegroundWindow() };
        let pid = process_id(hwnd)?;
        if pid == unsafe { GetCurrentProcessId() } || !ordinary_visible(hwnd) {
            return None;
        }
        let rect = frame(hwnd)?;
        // 读取期间窗口切换、关闭或移动时，不把多次调用拼成一份成功观测。
        if unsafe { GetForegroundWindow() } != hwnd
            || process_id(hwnd) != Some(pid)
            || !ordinary_visible(hwnd)
            || frame(hwnd) != Some(rect)
        {
            return None;
        }
        Some(NativeIdentity {
            handle: hwnd as usize,
            process_id: pid,
            rect,
        })
    }
}

#[cfg(not(windows))]
mod native {
    pub(super) fn foreground() -> Option<super::NativeIdentity> {
        None
    }
}

#[cfg(test)]
#[path = "desktop_surfaces_tests.rs"]
mod tests;
