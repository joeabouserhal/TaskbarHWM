//! Translucent acrylic for the flyout.
//!
//! The documented DWM system backdrop (DWMSBT_TRANSIENTWINDOW) renders as a flat opaque fill for an active
//! flyout window on current Windows 11 builds, so there is no see-through. The acrylic blur-behind composition
//! effect (the one Start and the taskbar flyouts use) gives real blur with an adjustable tint, so we use that
//! and keep the system backdrop only as a fallback if the call isn't available.
use std::ffi::c_void;

use windows::core::{s, w, BOOL};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Controls::MARGINS;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    /// Tint, 0xAABBGGRR.
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionAttribute {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

type SetWca = unsafe extern "system" fn(HWND, *mut CompositionAttribute) -> BOOL;

const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

fn acrylic_blur_behind(hwnd: HWND, dark: bool, tint: u32) -> bool {
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return false };
        let Some(p) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return false };
        let set: SetWca = std::mem::transmute(p);
        let (r, g, b) = if dark { (0x2Cu32, 0x2Cu32, 0x2Cu32) } else { (0xF3u32, 0xF3u32, 0xF3u32) };
        let mut policy = AccentPolicy {
            state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
            flags: 0,
            gradient: (tint.min(255) << 24) | (b << 16) | (g << 8) | r,
            animation: 0,
        };
        let mut data = CompositionAttribute {
            attribute: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut c_void,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set(hwnd, &mut data).as_bool()
    }
}

/// Applies the flyout's material, corners and theme. Returns true if the translucent acrylic was used.
/// `tint` is the tint opacity 0..=255 (lower = more see-through). Safe to call again to change it live.
pub fn apply(hwnd: HWND, dark: bool, tint: u32) -> bool {
    unsafe {
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const c_void, 4);
        let dark_mode: i32 = dark as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_mode as *const _ as *const c_void, 4);
        if acrylic_blur_behind(hwnd, dark, tint) {
            return true;
        }
        // Fallback: the system backdrop (flat, but still native-looking).
        let backdrop = DWMSBT_TRANSIENTWINDOW;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const c_void, 4);
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        false
    }
}
