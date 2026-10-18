//! Thin Windows HRGN adapter. The caller supplies the already-authorized pet
//! HWND and runs these functions on its existing GUI thread. No window search,
//! focus changes, timers, global hooks, or cursor polling happen here.
#[cfg(windows)]
use crate::pet_pointer_policy::{Plan, Region, Viewport, MAX_REGION_RECTS};
#[cfg(windows)]
use std::{io, ptr};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, HRGN, RGN_OR,
    },
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{GetClientRect, GetWindowRect},
    },
};

#[cfg(windows)]
pub fn read_viewport(hwnd: HWND) -> io::Result<Viewport> {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    if hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
        return Err(io::Error::other("cannot read pet client rectangle"));
    }
    let width = rect
        .right
        .checked_sub(rect.left)
        .and_then(|value| u32::try_from(value).ok());
    let height = rect
        .bottom
        .checked_sub(rect.top)
        .and_then(|value| u32::try_from(value).ok());
    let viewport = Viewport {
        width: width.unwrap_or(0),
        height: height.unwrap_or(0),
        dpi: unsafe { GetDpiForWindow(hwnd) },
    };
    if !viewport.valid() {
        return Err(io::Error::other("unsupported pet client geometry"));
    }
    Ok(viewport)
}

/// Restore the normal rectangular window. SetWindowRgn takes ownership only on
/// success; a null region requires no GDI object to be transferred or deleted.
#[cfg(windows)]
pub fn clear(hwnd: HWND) -> io::Result<()> {
    if hwnd.is_null() || unsafe { SetWindowRgn(hwnd, ptr::null_mut(), 1) } == 0 {
        Err(io::Error::other("cannot restore pet window rectangle"))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
struct OwnedRegion(HRGN);

#[cfg(windows)]
impl OwnedRegion {
    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> io::Result<Self> {
        let region = unsafe { CreateRectRgn(left, top, right, bottom) };
        if region.is_null() {
            Err(io::Error::other("cannot allocate pet window region"))
        } else {
            Ok(Self(region))
        }
    }
}

#[cfg(windows)]
impl Drop for OwnedRegion {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

/// Geometry is re-read before every application. Cache successful generations
/// in the owner if desired, but still read/update native geometry each tick.
/// Any shape failure attempts to restore a full rectangle and returns an error.
#[cfg(windows)]
pub fn apply(hwnd: HWND, plan: &Plan<'_>) -> io::Result<()> {
    if matches!(plan.region, Region::FullWindow) {
        return clear(hwnd);
    }
    match apply_shape(hwnd, plan) {
        Ok(()) => Ok(()),
        Err(error) => match clear(hwnd) {
            Ok(()) => Err(error),
            Err(clear_error) => Err(io::Error::other(format!(
                "{error}; rectangle recovery failed: {clear_error}"
            ))),
        },
    }
}

#[cfg(windows)]
fn apply_shape(hwnd: HWND, plan: &Plan<'_>) -> io::Result<()> {
    let viewport = read_viewport(hwnd)?;
    if plan.viewport != Some(viewport) {
        return Err(io::Error::other(
            "pet viewport changed before region application",
        ));
    }
    let Region::Rects(rects) = plan.region else {
        return clear(hwnd);
    };
    if rects.is_empty() || rects.len() > MAX_REGION_RECTS {
        return Err(io::Error::other("invalid pet region rectangle count"));
    }
    // SetWindowRgn uses WINDOW coordinates, whereas renderer masks use CLIENT
    // coordinates. Resolve both origins on this GUI thread under its DPI context.
    let mut client_origin = POINT { x: 0, y: 0 };
    let mut window: RECT = unsafe { std::mem::zeroed() };
    if unsafe { ClientToScreen(hwnd, &mut client_origin) } == 0
        || unsafe { GetWindowRect(hwnd, &mut window) } == 0
    {
        return Err(io::Error::other("cannot map pet client origin"));
    }
    let dx = client_origin
        .x
        .checked_sub(window.left)
        .ok_or_else(|| io::Error::other("pet x overflow"))?;
    let dy = client_origin
        .y
        .checked_sub(window.top)
        .ok_or_else(|| io::Error::other("pet y overflow"))?;
    let translate = |value: u32, offset: i32| -> io::Result<i32> {
        i32::try_from(value)
            .ok()
            .and_then(|value| value.checked_add(offset))
            .ok_or_else(|| io::Error::other("pet region coordinate overflow"))
    };
    let mut combined = OwnedRegion::rect(0, 0, 0, 0)?;
    for rect in rects {
        if rect.left >= rect.right
            || rect.top >= rect.bottom
            || rect.right > viewport.width
            || rect.bottom > viewport.height
        {
            return Err(io::Error::other(
                "pet region lies outside exact client geometry",
            ));
        }
        let part = OwnedRegion::rect(
            translate(rect.left, dx)?,
            translate(rect.top, dy)?,
            translate(rect.right, dx)?,
            translate(rect.bottom, dy)?,
        )?;
        if unsafe { CombineRgn(combined.0, combined.0, part.0, RGN_OR) } == 0 {
            return Err(io::Error::other("cannot combine pet window region"));
        }
    }
    if unsafe { SetWindowRgn(hwnd, combined.0, 1) } == 0 {
        return Err(io::Error::other("cannot apply pet window region"));
    }
    // Windows owns this HRGN after success. OwnedRegion must not delete it.
    combined.0 = ptr::null_mut();
    Ok(())
}
