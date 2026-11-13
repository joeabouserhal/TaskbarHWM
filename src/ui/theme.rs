//! Fluent design tokens. Light/dark follows the Windows system theme.
use windows::core::w;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD};

use super::gfx::color;

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub text: D2D1_COLOR_F,
    pub text_secondary: D2D1_COLOR_F,
    pub hover_fill: D2D1_COLOR_F,
    pub pressed_fill: D2D1_COLOR_F,
    pub card_fill: D2D1_COLOR_F,
    pub card_stroke: D2D1_COLOR_F,
    pub track: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    /// Status colors: good / caution / bad.
    pub ok: D2D1_COLOR_F,
    pub warn: D2D1_COLOR_F,
    pub bad: D2D1_COLOR_F,
    /// Taskbar row-label badge: tinted chip and the text on it.
    pub badge_fill: D2D1_COLOR_F,
    pub badge_text: D2D1_COLOR_F,
}

/// Taskbar and system flyouts follow the "System" theme mode (SystemUsesLightTheme).
fn system_is_light() -> bool {
    let mut v: u32 = 0;
    let mut len = 4u32;
    unsafe {
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut len),
        );
        r.is_ok() && v == 1
    }
}

/// WinUI's accent fill: "Light 2" shade of the user's accent in dark mode, "Dark 1" in light mode.
/// The Explorer\Accent\AccentPalette value holds 8 RGBA entries: Light3, Light2, Light1, Base, Dark1, Dark2, Dark3, ...
fn palette_accent(dark: bool) -> Option<D2D1_COLOR_F> {
    let mut buf = [0u8; 32];
    let mut len = 32u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        )
        .ok()
        .ok()?;
    }
    if len < 32 {
        return None;
    }
    let i = if dark { 1 } else { 4 } * 4;
    Some(color(buf[i], buf[i + 1], buf[i + 2], 1.0))
}

fn accent(dark: bool) -> D2D1_COLOR_F {
    if let Some(c) = palette_accent(dark) {
        return c;
    }
    let (mut argb, mut opaque) = (0u32, Default::default());
    let ok = unsafe { DwmGetColorizationColor(&mut argb, &mut opaque).is_ok() };
    if ok {
        color((argb >> 16) as u8, (argb >> 8) as u8, argb as u8, 1.0)
    } else {
        color(0, 120, 212, 1.0) // Windows default blue
    }
}

fn with_alpha(mut c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    c.a = a;
    c
}

impl Theme {
    pub fn current() -> Theme {
        if system_is_light() {
            Theme {
                dark: false,
                text: color(0, 0, 0, 0.9),
                text_secondary: color(0, 0, 0, 0.6),
                hover_fill: color(0, 0, 0, 0.06),
                pressed_fill: color(0, 0, 0, 0.04),
                card_fill: color(255, 255, 255, 0.55),
                card_stroke: color(0, 0, 0, 0.06),
                track: color(0, 0, 0, 0.12),
                accent: accent(false),
                ok: color(15, 123, 15, 1.0),
                warn: color(157, 93, 0, 1.0),
                bad: color(196, 43, 28, 1.0),
                badge_fill: with_alpha(accent(false), 0.22),
                badge_text: color(0, 0, 0, 0.9),
            }
        } else {
            Theme {
                dark: true,
                text: color(255, 255, 255, 1.0),
                text_secondary: color(255, 255, 255, 0.78),
                hover_fill: color(255, 255, 255, 0.09),
                pressed_fill: color(255, 255, 255, 0.06),
                card_fill: color(255, 255, 255, 0.05),
                card_stroke: color(0, 0, 0, 0.10),
                track: color(255, 255, 255, 0.16),
                accent: accent(true),
                ok: color(108, 203, 95, 1.0),
                warn: color(252, 194, 0, 1.0),
                bad: color(255, 153, 164, 1.0),
                badge_fill: with_alpha(accent(true), 0.65),
                badge_text: color(255, 255, 255, 1.0),
            }
        }
    }
}
