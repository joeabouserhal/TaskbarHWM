//! Connected displays, and the taskbar that sits on each.
//!
//! Windows gives the main display's taskbar the class `Shell_TrayWnd` and every other display's taskbar the class
//! `Shell_SecondaryTrayWnd` (those exist only while "Show my taskbar on all displays" is on). A display is
//! identified by its device name (`\\.\DISPLAY2`), which stays the same while the monitor stays plugged into the same
//! output, so it can be saved in the settings.
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW};

/// The entry in the setting that means "every display, including ones plugged in later".
pub const ALL: &str = "*";

#[derive(Clone, Debug)]
pub struct Display {
    /// Device name, e.g. `\\.\DISPLAY2`.
    pub key: String,
    /// What the settings page calls it: "Display 2 · 1920×1080", with "· main" for the main one. Numbered in list
    /// order (main first, then left to right), not by Windows' internal device numbers.
    pub label: String,
    pub primary: bool,
    left: i32,
    size: (i32, i32),
}

fn describe(h: HMONITOR) -> Option<Display> {
    unsafe {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return None;
        }
        let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
        let key = String::from_utf16_lossy(&mi.szDevice[..len]);
        let r = mi.monitorInfo.rcMonitor;
        let primary = mi.monitorInfo.dwFlags & 1 != 0; // MONITORINFOF_PRIMARY
        Some(Display { key, label: String::new(), primary, left: r.left, size: (r.right - r.left, r.bottom - r.top) })
    }
}

unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    if let Some(d) = describe(h) {
        (*(lp.0 as *mut Vec<Display>)).push(d);
    }
    BOOL(1)
}

/// Every connected display: the main one first, then the rest from left to right.
pub fn list() -> Vec<Display> {
    let mut out: Vec<Display> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut out as *mut Vec<Display> as isize));
    }
    out.sort_by_key(|d| (!d.primary, d.left));
    for (i, d) in out.iter_mut().enumerate() {
        d.label = format!("Display {} \u{00B7} {}\u{00D7}{}{}", i + 1, d.size.0, d.size.1, if d.primary { " \u{00B7} main" } else { "" });
    }
    out
}

/// The displays the widget is shown on: those named in `chosen` ("*" for all of them), or the main display when the
/// choice is empty or names nothing that is connected.
pub fn selected<'a>(chosen: &[String], all: &'a [Display]) -> Vec<&'a Display> {
    if chosen.iter().any(|k| k == ALL) {
        return all.iter().collect();
    }
    let picked: Vec<&Display> = all.iter().filter(|d| chosen.contains(&d.key)).collect();
    if !picked.is_empty() {
        return picked;
    }
    all.iter().filter(|d| d.primary).chain(all.first()).take(1).collect()
}

/// Every taskbar window, with the key of the display it is on.
pub fn trays() -> Vec<(HWND, String)> {
    let mut out = Vec::new();
    let mut add = |h: HWND| {
        let key = unsafe { describe(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST)) }.map(|d| d.key);
        if let Some(key) = key {
            out.push((h, key));
        }
    };
    unsafe {
        if let Ok(h) = FindWindowW(w!("Shell_TrayWnd"), None) {
            add(h);
        }
        let mut prev: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), PCWSTR::null()) {
            if h.is_invalid() {
                break;
            }
            add(h);
            prev = Some(h);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(key: &str, primary: bool, left: i32) -> Display {
        Display { key: key.to_string(), label: key.to_string(), primary, left, size: (0, 0) }
    }

    fn keys(v: Vec<&Display>) -> Vec<String> {
        v.into_iter().map(|d| d.key.clone()).collect()
    }

    #[test]
    fn nothing_chosen_means_the_main_display() {
        let all = vec![d("A", true, 0), d("B", false, 2560)];
        assert_eq!(keys(selected(&[], &all)), ["A"]);
    }

    #[test]
    fn star_means_every_display() {
        let all = vec![d("A", true, 0), d("B", false, 2560)];
        assert_eq!(keys(selected(&["*".to_string()], &all)), ["A", "B"]);
    }

    #[test]
    fn a_choice_picks_just_those_displays() {
        let all = vec![d("A", true, 0), d("B", false, 2560)];
        assert_eq!(keys(selected(&["B".to_string()], &all)), ["B"]);
    }

    #[test]
    fn a_choice_of_unplugged_displays_falls_back_to_main() {
        let all = vec![d("A", true, 0)];
        assert_eq!(keys(selected(&["Gone".to_string()], &all)), ["A"]);
    }
}
