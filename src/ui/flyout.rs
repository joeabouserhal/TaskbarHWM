//! The flyout that opens above the taskbar widget: acrylic backdrop, a dashboard page with one card per
//! hardware section, and a settings page where the user picks what to show and where.
//!
//! Drawing is immediate-mode: each frame the page layout runs against a `Painter`, which both draws the
//! controls and records their clickable regions. A dry run (no device context) measures the content first.
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Shell::IsUserAnAdmin;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

use super::gfx::color;
use super::gpu_surface::{GpuGfx, GpuSurface};
use super::theme::Theme;
use super::widget::WM_CONFIG;
use crate::config::{self, Config};
use crate::metrics::{self, Card, Section, METRICS};
use crate::sensors::cpu_pawnio::Unavailable;
use crate::sensors::disk::DiskStats;
use crate::sensors::Snapshot;
use crate::shell::autostart;
use crate::shell::setup::{self, SetupState};

// Layout, in DIPs.
const WIDTH: f32 = 340.0;
const SETTINGS_WIDTH: f32 = 864.0;
const PAD: f32 = 12.0;
const CARD_PAD: f32 = 14.0;
const HEADER_H: f32 = 40.0;
const BODY_TOP: f32 = PAD + HEADER_H;
const CARD_TITLE_H: f32 = 24.0;
const ROW_H: f32 = 24.0;
const SET_ROW_H: f32 = 34.0;
const BAR_EXTRA: f32 = 8.0;
const NOTE_H: f32 = 32.0;
const CARD_GAP: f32 = 8.0;
const CARD_RADIUS: f32 = 8.0;
const GAP_ABOVE_TASKBAR: f32 = 12.0;
const SCREEN_MARGIN: f32 = 16.0;
const SLIDE: f32 = 20.0;
const ANIM: Duration = Duration::from_millis(220);
const TIMER_ANIM: usize = 1;
const WHEEL_STEP: f32 = 48.0;
const CHIP_H: f32 = 28.0;
const CHIP_ROW_H: f32 = 42.0;

pub struct OpenParams<'a> {
    /// Left edge of the widget, in screen pixels.
    pub anchor_x: i32,
    /// Top edge of the taskbar, in screen pixels.
    pub taskbar_top: i32,
    pub scale: f32,
    pub widget: HWND,
    pub snap: &'a Snapshot,
    pub theme: Theme,
    pub dwrite: &'a IDWriteFactory,
    /// Open straight on the settings page (first-run welcome).
    pub settings: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Settings,
}

/// The dashboard is a compact column; settings use a wider two-column grid.
fn page_width(p: Page) -> f32 {
    match p {
        Page::Dashboard => WIDTH,
        Page::Settings => SETTINGS_WIDTH,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Flyout,
    Pinned,
}

/// Which strip of reorderable chips: the widget's rows (CPU/GPU/RAM) or its columns (usage/clock/...).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Rows,
    Cols,
}

/// A chip being dragged. `x` is the mouse position (DIPs), `grab` the mouse offset inside the chip.
#[derive(Clone, Copy)]
struct Drag {
    list: ListKind,
    from: usize,
    grab: f32,
    x: f32,
}

#[derive(Clone, Copy)]
enum Action {
    /// Pressing a chip starts a drag (handled on mouse-down, not on click).
    Chip(ListKind, usize),
    OpenSettings,
    Back,
    Toggle(&'static str, Target),
    Fahrenheit(bool),
    Interval(u32),
    Tint(u32),
    /// Widget text size +/- 1.
    FontStep(i32),
    /// Taskbar row labels: "icon", "text" or "both".
    Labels(&'static str),
    /// Taskbar row labels in a badge, or plain.
    Badge(bool),
    /// Toggle disk `index` (into the live disk list) for the flyout or the taskbar.
    DiskToggle(usize, Target),
    /// How disk space is shown: "used_total", "percent" or "free".
    DiskStyle(&'static str),
    /// "combined" (one Disks section) or "split" (a section per disk).
    DiskLayout(&'static str),
    /// Show network speeds as "bytes" or "bits" per second.
    NetUnit(&'static str),
    /// Network on the taskbar: "combined" (one row) or "split" (download and upload as separate rows).
    NetLayout(&'static str),
    /// Toggle network adapter `index` (into the live adapter list) in or out of the speed totals.
    NetAdapter(usize),
    /// Toggle graphics card `index` (into the live GPU list) for the flyout or the taskbar.
    GpuToggle(usize, Target),
    /// Widget column spacing +/- 1.
    GapStep(i32),
    Autostart(bool),
    InstallPawn,
}

struct Hit {
    r: D2D_RECT_F,
    a: Action,
}

struct Fmts {
    title: IDWriteTextFormat,
    card_title: IDWriteTextFormat,
    label: IDWriteTextFormat,
    value: IDWriteTextFormat,
    /// The unit after a value ("GHz", "%"): smaller than the value, same right edge.
    unit: IDWriteTextFormat,
    subtitle: IDWriteTextFormat,
    small: IDWriteTextFormat,
    seg: IDWriteTextFormat,
    icon: IDWriteTextFormat,
    note: IDWriteTextFormat,
    status: IDWriteTextFormat,
    value_c: IDWriteTextFormat,
    /// Left-aligned label that ends in an ellipsis when it doesn't fit (long volume names).
    row_label: IDWriteTextFormat,
}

// Field order matters: the surface must drop before the device that created it.
struct Fly {
    hwnd: HWND,
    widget: HWND,
    surface: GpuSurface,
    _gfx: GpuGfx,
    fmts: Fmts,
    cpu_name: String,
    snap: Snapshot,
    theme: Theme,
    scale: f32,
    x: i32,
    final_y: i32,
    taskbar_top: i32,
    max_h_dip: f32,
    opened: Instant,
    page: Page,
    scroll: f32,
    max_scroll: f32,
    mouse: Option<(f32, f32)>,
    tracking: bool,
    hits: Vec<Hit>,
    elevated: bool,
    autostart: bool,
    autostart_msg: Option<String>,
    pawn_msg: Option<String>,
    dwrite: IDWriteFactory,
    drag: Option<Drag>,
    /// Where the dragged chip would land, as painted last (index in the final order).
    drag_target: Option<usize>,
}

thread_local! {
    static FLY: RefCell<Option<Fly>> = const { RefCell::new(None) };
    static LAST_CLOSED: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub fn is_open() -> bool {
    FLY.with(|f| f.try_borrow().map(|b| b.is_some()).unwrap_or(true))
}

/// Opens the flyout, or closes it if it's already open.
pub fn toggle(p: OpenParams) {
    if is_open() {
        close(false);
        return;
    }
    // The click that just closed the flyout (by taking focus away) must not reopen it.
    if LAST_CLOSED.with(|c| c.get()).is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
        return;
    }
    if let Err(e) = open(p) {
        eprintln!("flyout failed to open: {e}");
        close(false);
    }
}

/// Opens the flyout on its settings page, switching an already open flyout over to it.
pub fn show_settings(mut p: OpenParams) {
    close(false);
    p.settings = true;
    if let Err(e) = open(p) {
        eprintln!("flyout failed to open: {e}");
        close(false);
    }
}

pub fn update(snap: &Snapshot) {
    with_fly(|f| {
        f.snap = snap.clone();
        f.render();
    });
}

fn with_fly(f: impl FnOnce(&mut Fly)) {
    FLY.with(|cell| {
        if let Ok(mut g) = cell.try_borrow_mut() {
            if let Some(fly) = g.as_mut() {
                f(fly);
            }
        }
    });
}

pub fn close(by_deactivate: bool) {
    let fly = FLY.with(|f| f.try_borrow_mut().ok().and_then(|mut b| b.take()));
    if let Some(f) = fly {
        let hwnd = f.hwnd;
        drop(f); // release the swapchain before the window goes away
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        if by_deactivate {
            LAST_CLOSED.with(|c| c.set(Some(Instant::now())));
        }
        // Give the GPU/composition memory back to the OS.
        crate::sensors::trim_working_set();
    }
}

fn make_fmt(dw: &IDWriteFactory, family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<IDWriteTextFormat> {
    unsafe {
        let f = dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(f)
    }
}

fn make_fmts(dw: &IDWriteFactory) -> Result<Fmts> {
    let text = w!("Segoe UI Variable Text");
    let fm = Fmts {
        title: make_fmt(dw, w!("Segoe UI Variable Display"), 18.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        card_title: make_fmt(dw, text, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        label: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        value: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        unit: make_fmt(dw, text, 10.5, DWRITE_FONT_WEIGHT_NORMAL)?,
        subtitle: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        small: make_fmt(dw, text, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        seg: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        icon: make_fmt(dw, w!("Segoe Fluent Icons"), 16.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        note: make_fmt(dw, text, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?,
        status: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        value_c: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        row_label: make_fmt(dw, text, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
    };
    unsafe {
        fm.value.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        fm.unit.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        fm.subtitle.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        fm.subtitle.SetTrimming(
            &DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 },
            None,
        )?;
        let ellipsis = dw.CreateEllipsisTrimmingSign(&fm.row_label)?;
        fm.row_label.SetTrimming(
            &DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 },
            &ellipsis,
        )?;
        fm.note.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
        fm.note.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        for f in [&fm.small, &fm.seg, &fm.icon, &fm.value_c] {
            f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        }
    }
    Ok(fm)
}

fn open(p: OpenParams) -> Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("TaskbarHWM.Flyout");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc); // fails harmlessly if already registered

        let start_page = if p.settings { Page::Settings } else { Page::Dashboard };
        let w_px = (page_width(start_page) * p.scale).ceil() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
            class,
            w!("TaskbarHWM Flyout"),
            WS_POPUP,
            0,
            0,
            w_px,
            200,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;

        // Translucent acrylic, rounded corners, theme-matched frame.
        super::backdrop::apply(hwnd, p.theme.dark, config::get().tint);

        let gfx = GpuGfx::new()?;
        let surface = GpuSurface::new(&gfx, hwnd, w_px as u32, 200, 96.0 * p.scale)?;
        let gap = GAP_ABOVE_TASKBAR * p.scale;
        let mut fly = Fly {
            hwnd,
            widget: p.widget,
            surface,
            _gfx: gfx,
            fmts: make_fmts(p.dwrite)?,
            cpu_name: crate::sensors::cpu::cpu_name(),
            snap: p.snap.clone(),
            theme: p.theme,
            scale: p.scale,
            x: p.anchor_x,
            final_y: p.taskbar_top - 200,
            taskbar_top: p.taskbar_top,
            max_h_dip: (p.taskbar_top as f32 - gap - SCREEN_MARGIN * p.scale) / p.scale,
            opened: Instant::now(),
            page: start_page,
            scroll: 0.0,
            max_scroll: 0.0,
            mouse: None,
            tracking: false,
            hits: Vec::new(),
            elevated: IsUserAnAdmin().as_bool(),
            autostart: autostart::is_enabled(),
            autostart_msg: None,
            pawn_msg: None,
            dwrite: p.dwrite.clone(),
            drag: None,
            drag_target: None,
        };
        fly.render(); // sizes the window to its content
        fly.apply_pos();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetForegroundWindow(hwnd);
        SetTimer(Some(hwnd), TIMER_ANIM, 16, None);
        FLY.with(|f| *f.borrow_mut() = Some(fly));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Painter: draws controls and records hit regions.
// ---------------------------------------------------------------------------------------------

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

struct Painter<'a> {
    dc: Option<&'a ID2D1DeviceContext>,
    brush: Option<ID2D1SolidColorBrush>,
    t: Theme,
    fm: &'a Fmts,
    mouse: Option<(f32, f32)>,
    scroll: f32,
    body_bottom: f32,
    width: f32,
    in_body: bool,
    /// Extra height added to every settings row, so a column of cards can be stretched to end flush with its neighbours.
    row_extra: f32,
    hits: Vec<Hit>,
    dw: &'a IDWriteFactory,
    drag: Option<Drag>,
    drag_target: Option<usize>,
}

impl<'a> Painter<'a> {
    /// Height of one settings row.
    fn rh(&self) -> f32 {
        SET_ROW_H + self.row_extra
    }

    fn new(dc: Option<&'a ID2D1DeviceContext>, t: Theme, fm: &'a Fmts, dw: &'a IDWriteFactory, drag: Option<Drag>, mouse: Option<(f32, f32)>, scroll: f32, body_bottom: f32, width: f32) -> Self {
        let brush = dc.and_then(|d| unsafe { d.CreateSolidColorBrush(&t.text, None).ok() });
        Painter { dc, brush, t, fm, mouse, scroll, body_bottom, width, in_body: false, row_extra: 0.0, hits: Vec::new(), dw, drag, drag_target: None }
    }

    /// Content coordinates -> window coordinates.
    fn map(&self, r: D2D_RECT_F) -> D2D_RECT_F {
        if self.in_body {
            let dy = BODY_TOP - self.scroll;
            rect(r.left, r.top + dy, r.right, r.bottom + dy)
        } else {
            r
        }
    }

    fn visible(&self, r: &D2D_RECT_F) -> bool {
        !self.in_body || (r.bottom > BODY_TOP && r.top < self.body_bottom)
    }

    fn hovered(&self, r: D2D_RECT_F) -> bool {
        let Some((mx, my)) = self.mouse else { return false };
        if self.in_body && (my < BODY_TOP || my > self.body_bottom) {
            return false;
        }
        mx >= r.left && mx < r.right && my >= r.top && my < r.bottom
    }

    fn begin_body(&mut self) {
        self.in_body = true;
        if let Some(dc) = self.dc {
            unsafe { dc.PushAxisAlignedClip(&rect(0.0, BODY_TOP, self.width, self.body_bottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        }
    }

    fn end_body(&mut self) {
        if let Some(dc) = self.dc {
            unsafe { dc.PopAxisAlignedClip() };
        }
        self.in_body = false;
    }

    fn set(&self, c: &D2D1_COLOR_F) -> Option<(&ID2D1DeviceContext, &ID2D1SolidColorBrush)> {
        let (dc, b) = (self.dc?, self.brush.as_ref()?);
        unsafe { b.SetColor(c) };
        Some((dc, b))
    }

    fn fill(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F) {
        let r = self.map(r);
        if !self.visible(&r) {
            return;
        }
        if let Some((dc, b)) = self.set(&c) {
            unsafe { dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, b) };
        }
    }

    fn stroke(&self, r: D2D_RECT_F, radius: f32, c: D2D1_COLOR_F) {
        let r = self.map(r);
        if !self.visible(&r) {
            return;
        }
        if let Some((dc, b)) = self.set(&c) {
            unsafe { dc.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, b, 1.0, None) };
        }
    }

    fn circle(&self, cx: f32, cy: f32, radius: f32, c: D2D1_COLOR_F) {
        let m = self.map(rect(cx, cy, cx, cy));
        if !self.visible(&rect(m.left - radius, m.top - radius, m.left + radius, m.top + radius)) {
            return;
        }
        if let Some((dc, b)) = self.set(&c) {
            unsafe {
                dc.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: m.left, Y: m.top }, radiusX: radius, radiusY: radius }, b);
            }
        }
    }

    fn text(&self, s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        let r = self.map(r);
        if !self.visible(&r) {
            return;
        }
        if let Some((dc, b)) = self.set(&c) {
            let wide: Vec<u16> = s.encode_utf16().collect();
            unsafe { dc.DrawText(&wide, f, &r, b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL) };
        }
    }

    /// Width and baseline (from the top of a `h`-tall box, text centered in it) of `s` drawn with `f`.
    fn measure_text(&self, s: &str, f: &IDWriteTextFormat, h: f32) -> (f32, f32) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(l) = self.dw.CreateTextLayout(&wide, f, 2000.0, h) else { return (0.0, 0.0) };
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            let mut lm = [DWRITE_LINE_METRICS::default(); 1];
            let mut n = 0u32;
            let base = if l.GetLineMetrics(Some(&mut lm), &mut n).is_ok() && n > 0 { (h - lm[0].height) / 2.0 + lm[0].baseline } else { 0.0 };
            (m.widthIncludingTrailingWhitespace, base)
        }
    }

    /// A value right-aligned in `r`, with its unit ("GHz", "%", "GB") smaller and muted, on the number's baseline.
    fn value(&self, s: &str, r: D2D_RECT_F) {
        let fm = self.fm;
        let (num, unit) = metrics::split_unit(s);
        let Some((unit, spaced)) = unit else {
            self.text(s, &fm.value, r, self.t.text);
            return;
        };
        let h = r.bottom - r.top;
        let (unit_w, _) = self.measure_text(unit, &fm.unit, h);
        let dy = self.measure_text("0", &fm.value, h).1 - self.measure_text("0", &fm.unit, h).1;
        let gap = if spaced { 2.5 } else { 0.5 };
        self.text(num, &fm.value, D2D_RECT_F { right: r.right - unit_w - gap, ..r }, self.t.text);
        let (a, b) = (self.t.text, self.t.text_secondary);
        let muted = D2D1_COLOR_F { r: a.r + (b.r - a.r) * 0.7, g: a.g + (b.g - a.g) * 0.7, b: a.b + (b.b - a.b) * 0.7, a: a.a + (b.a - a.a) * 0.7 };
        self.text(unit, &fm.unit, D2D_RECT_F { top: r.top + dy, bottom: r.bottom + dy, ..r }, muted);
    }

    fn hit(&mut self, r: D2D_RECT_F, a: Action) {
        let mut m = self.map(r);
        if self.in_body {
            m.top = m.top.max(BODY_TOP);
            m.bottom = m.bottom.min(self.body_bottom);
        }
        if m.bottom > m.top {
            self.hits.push(Hit { r: m, a });
        }
    }

    fn icon(&self, s: Section, r: D2D_RECT_F, c: D2D1_COLOR_F) {
        let r = self.map(r);
        if !self.visible(&r) {
            return;
        }
        if let Some((dc, b)) = self.set(&c) {
            super::icons::draw(dc, b, s, r.left, r.top, r.right - r.left);
        }
    }

    /// Width of `s` in `f`, in DIPs.
    fn text_width(&self, s: &str, f: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(l) = self.dw.CreateTextLayout(&wide, f, 1000.0, 40.0) else { return 40.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            m.width.ceil()
        }
    }

    /// A horizontal strip of draggable chips. While one is dragged, the others close ranks around a slot at the
    /// spot where it would land, and `drag_target` records that index.
    fn chip_strip(&mut self, x0: f32, y: f32, list: ListKind, items: &[(&'static str, &'static str, bool, Option<Section>)]) {
        const GAP: f32 = 6.0;
        let widths: Vec<f32> =
            items.iter().map(|(_, label, _, icon)| self.text_width(label, &self.fm.seg) + 24.0 + if icon.is_some() { 22.0 } else { 0.0 }).collect();
        let drag = self.drag.filter(|d| d.list == list && d.from < items.len());
        let mut order: Vec<usize> = (0..items.len()).collect();
        if let Some(d) = drag {
            let center = d.x - d.grab + widths[d.from] / 2.0;
            let mut others: Vec<usize> = order.iter().copied().filter(|&i| i != d.from).collect();
            let (mut cx, mut target) = (x0, others.len());
            for (pos, &i) in others.iter().enumerate() {
                if center < cx + widths[i] / 2.0 {
                    target = pos;
                    break;
                }
                cx += widths[i] + GAP;
            }
            others.insert(target, d.from);
            order = others;
            self.drag_target = Some(target);
        }
        let mut x = x0;
        for &i in &order {
            let r = rect(x, y, x + widths[i], y + CHIP_H);
            let (_, label, active, icon) = items[i];
            if drag.map(|d| d.from) == Some(i) {
                // Empty slot where the dragged chip will land.
                let mut c = self.t.text_secondary;
                c.a *= 0.5;
                self.stroke(r, 6.0, c);
            } else {
                self.hit(r, Action::Chip(list, i));
                let fill = if self.hovered(self.map(r)) && drag.is_none() { self.t.hover_fill } else { self.t.track };
                self.fill(r, 6.0, fill);
                self.chip_content(r, label, icon, if active { self.t.text } else { self.t.text_secondary });
            }
            x += widths[i] + GAP;
        }
        if let Some(d) = drag {
            let end = x - GAP;
            let left = (d.x - d.grab).clamp(x0, (end - widths[d.from]).max(x0));
            let r = rect(left, y - 2.0, left + widths[d.from], y + CHIP_H + 2.0);
            self.fill(r, 6.0, self.t.accent);
            self.chip_content(r, items[d.from].1, items[d.from].3, color(255, 255, 255, 1.0));
        }
    }

    /// A chip's optional icon plus its label.
    fn chip_content(&self, r: D2D_RECT_F, label: &str, icon: Option<Section>, c: D2D1_COLOR_F) {
        match icon {
            Some(sec) => {
                let cy = (r.top + r.bottom) / 2.0;
                self.icon(sec, rect(r.left + 11.0, cy - 8.0, r.left + 27.0, cy + 8.0), c);
                self.text(label, &self.fm.seg, rect(r.left + 28.0, r.top, r.right - 8.0, r.bottom), c);
            }
            None => self.text(label, &self.fm.seg, r, c),
        }
    }

    // ---- controls ----

    fn card(&self, x: f32, y: f32, w: f32, h: f32) {
        let r = rect(x, y, x + w, y + h);
        self.fill(r, CARD_RADIUS, self.t.card_fill);
        self.stroke(r, CARD_RADIUS, self.t.card_stroke);
    }

    fn icon_button(&mut self, x: f32, y: f32, glyph: &str, a: Action) {
        let r = rect(x, y, x + 32.0, y + 32.0);
        self.hit(r, a);
        if self.hovered(self.map(r)) {
            self.fill(r, 4.0, self.t.hover_fill);
        }
        self.text(glyph, &self.fm.icon, r, self.t.text);
    }

    /// Accent push button, right-aligned at `right`.
    fn button(&mut self, right: f32, y: f32, w: f32, label: &str, a: Action) {
        let r = rect(right - w, y, right, y + 28.0);
        self.hit(r, a);
        let mut c = self.t.accent;
        if self.hovered(self.map(r)) {
            c.a = 0.88;
        }
        self.fill(r, 4.0, c);
        self.text(label, &self.fm.seg, r, color(255, 255, 255, 1.0));
    }

    /// WinUI-style toggle switch, 40x20.
    fn toggle(&mut self, x: f32, y: f32, on: bool, a: Action) {
        let r = rect(x, y, x + 40.0, y + 20.0);
        self.hit(rect(x - 2.0, y - 4.0, x + 42.0, y + 24.0), a);
        let hov = self.hovered(self.map(r));
        let cy = y + 10.0;
        if on {
            let mut c = self.t.accent;
            if hov {
                c.a = 0.88;
            }
            self.fill(r, 10.0, c);
            self.circle(x + 30.0, cy, if hov { 7.0 } else { 6.0 }, color(255, 255, 255, 1.0));
        } else {
            if hov {
                self.fill(r, 10.0, self.t.hover_fill);
            }
            self.stroke(rect(x + 0.5, y + 0.5, x + 39.5, y + 19.5), 9.5, self.t.text_secondary);
            self.circle(x + 10.0, cy, if hov { 6.0 } else { 5.0 }, self.t.text_secondary);
        }
    }

    /// "-  value  +" spin control, right-aligned at `right`. A button is `None` when its limit is reached.
    fn stepper(&mut self, right: f32, y: f32, value: &str, minus: Option<Action>, plus: Option<Action>) {
        const BW: f32 = 28.0;
        const VW: f32 = 40.0;
        let x_plus = right - BW;
        let x_minus = x_plus - VW - BW;
        for (x, glyph, action) in [(x_minus, "\u{2212}", minus), (x_plus, "+", plus)] {
            let r = rect(x, y, x + BW, y + 28.0);
            match action {
                Some(a) => {
                    self.hit(r, a);
                    let fill = if self.hovered(self.map(r)) { self.t.hover_fill } else { self.t.track };
                    self.fill(r, 4.0, fill);
                    self.text(glyph, &self.fm.seg, r, self.t.text);
                }
                None => {
                    self.fill(r, 4.0, self.t.track);
                    let mut dim = self.t.text_secondary;
                    dim.a *= 0.4;
                    self.text(glyph, &self.fm.seg, r, dim);
                }
            }
        }
        self.text(value, &self.fm.value_c, rect(x_minus + BW, y, x_plus, y + 28.0), self.t.text);
    }

    /// Segmented selector, right-aligned at `right`.
    fn segmented(&mut self, right: f32, y: f32, each: f32, opts: &[(&str, bool, Action)]) {
        let x0 = right - each * opts.len() as f32;
        self.fill(rect(x0, y, right, y + 28.0), 6.0, self.t.track);
        for (i, (label, selected, a)) in opts.iter().enumerate() {
            let r = rect(x0 + i as f32 * each + 2.0, y + 2.0, x0 + (i + 1) as f32 * each - 2.0, y + 26.0);
            self.hit(r, *a);
            if *selected {
                self.fill(r, 4.0, self.t.accent);
                self.text(label, &self.fm.seg, r, color(255, 255, 255, 1.0));
            } else {
                if self.hovered(self.map(r)) {
                    self.fill(r, 4.0, self.t.hover_fill);
                }
                self.text(label, &self.fm.seg, r, self.t.text);
            }
        }
    }
}

fn header(p: &mut Painter, page: Page) {
    let fm = p.fm;
    match page {
        Page::Dashboard => {
            p.text("System", &fm.title, rect(PAD + 6.0, PAD, p.width - PAD - 40.0, PAD + HEADER_H), p.t.text);
            p.icon_button(p.width - PAD - 36.0, PAD + 4.0, "\u{E713}", Action::OpenSettings);
        }
        Page::Settings => {
            p.icon_button(PAD + 2.0, PAD + 4.0, "\u{E72B}", Action::Back);
            p.text("Settings", &fm.title, rect(PAD + 44.0, PAD, p.width - PAD, PAD + HEADER_H), p.t.text);
        }
    }
}

fn card_height(c: &Card) -> f32 {
    let rows: f32 = c.rows.iter().map(|r| ROW_H + if r.frac.is_some() { BAR_EXTRA } else { 0.0 }).sum();
    let note = if c.note.is_some() { NOTE_H + 4.0 } else { 0.0 };
    10.0 + CARD_TITLE_H + 2.0 + rows + note + 10.0
}

/// Dashboard page body. Returns the content height.
fn dashboard_body(p: &mut Painter, cards: &[Card]) -> f32 {
    let fm = p.fm;
    let mut y = 0.0;
    for c in cards {
        let h = card_height(c);
        p.card(PAD, y, p.width - 2.0 * PAD, h);
        let (x0, x1) = (PAD + CARD_PAD, p.width - PAD - CARD_PAD);
        let ty = y + 10.0;
        p.icon(c.section, rect(x0, ty + 4.0, x0 + 16.0, ty + 20.0), p.t.text);
        p.text(&c.title, &fm.card_title, rect(x0 + 24.0, ty, x0 + 90.0, ty + CARD_TITLE_H), p.t.text);
        if !c.subtitle.is_empty() {
            p.text(&c.subtitle, &fm.subtitle, rect(x0 + 62.0, ty, x1, ty + CARD_TITLE_H), p.t.text_secondary);
        }
        let mut ry = ty + CARD_TITLE_H + 2.0;
        for r in &c.rows {
            p.text(&r.label, &fm.row_label, rect(x0, ry, x1 - 92.0, ry + ROW_H), p.t.text_secondary);
            p.value(&r.value, rect(x0 + 100.0, ry, x1, ry + ROW_H));
            ry += ROW_H;
            if let Some(f) = r.frac {
                p.fill(rect(x0, ry, x1, ry + 4.0), 2.0, p.t.track);
                if f > 0.0 {
                    p.fill(rect(x0, ry, x0 + ((x1 - x0) * f).max(4.0), ry + 4.0), 2.0, if r.hot { p.t.bad } else { p.t.accent });
                }
                ry += BAR_EXTRA;
            }
        }
        if let Some(n) = c.note {
            let r = rect(x0, ry + 2.0, x1, ry + 2.0 + NOTE_H);
            p.hit(r, Action::OpenSettings);
            if p.hovered(p.map(r)) {
                p.fill(r, 4.0, p.t.hover_fill);
            }
            p.text(n, &fm.note, r, p.t.text_secondary);
        }
        y += h + CARD_GAP;
    }
    if cards.is_empty() {
        p.text(
            "No metrics are enabled. Open settings to choose some.",
            &fm.label,
            rect(PAD + 6.0, 0.0, p.width - PAD, 40.0),
            p.t.text_secondary,
        );
        y = 40.0 + CARD_GAP;
    }
    y - CARD_GAP + PAD
}

/// A card of per-metric toggles for one hardware section. Returns the card height.
fn metrics_card(p: &mut Painter, x: f32, y: f32, w: f32, title: &str, section: Section, cfg: &Config) -> f32 {
    metrics_card_with(p, x, y, w, title, section, cfg, 0).0
}

/// [`metrics_card`] with room for `extra_rows` more settings rows under the toggles. Returns the card height and
/// the y where those extra rows start.
fn metrics_card_with(p: &mut Painter, x: f32, y: f32, w: f32, title: &str, section: Section, cfg: &Config, extra_rows: usize) -> (f32, f32) {
    let fm = p.fm;
    let rh = p.rh();
    let defs: Vec<_> = METRICS.iter().filter(|m| m.section == section).collect();
    let h = 10.0 + CARD_TITLE_H + 2.0 + (defs.len() + extra_rows) as f32 * rh + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let pin_x = x1 - 40.0;
    let fly_x = pin_x - 24.0 - 40.0;
    let ty = y + 10.0;
    p.icon(section, rect(x0, ty + 4.0, x0 + 16.0, ty + 20.0), p.t.text);
    p.text(title, &fm.card_title, rect(x0 + 24.0, ty, x0 + 110.0, ty + CARD_TITLE_H), p.t.text);
    p.text("Flyout", &fm.small, rect(fly_x - 14.0, ty, fly_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    p.text("Taskbar", &fm.small, rect(pin_x - 14.0, ty, pin_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    let mut ry = ty + CARD_TITLE_H + 2.0;
    for d in defs {
        p.text(d.name, &fm.label, rect(x0, ry, x0 + 120.0, ry + rh), p.t.text);
        let toggle_y = ry + (rh - 20.0) / 2.0;
        p.toggle(fly_x, toggle_y, cfg.flyout.iter().any(|i| i == d.id), Action::Toggle(d.id, Target::Flyout));
        p.toggle(pin_x, toggle_y, cfg.pinned.iter().any(|i| i == d.id), Action::Toggle(d.id, Target::Pinned));
        ry += rh;
    }
    (h, ry)
}

/// Network speeds: which to show, and whether they read in bytes or bits per second.
fn network_card(p: &mut Painter, x: f32, y: f32, w: f32, cfg: &Config) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let (h, mut ry) = metrics_card_with(p, x, y, w, "Network", Section::Net, cfg, 2);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    p.text("Speed unit", &fm.label, rect(x0, ry, x0 + 100.0, ry + rh), p.t.text);
    let u = cfg.net_unit.as_str();
    p.segmented(x1, ry + (rh - 28.0) / 2.0, 54.0, &[("MB/s", u == "bytes", Action::NetUnit("bytes")), ("Mbps", u == "bits", Action::NetUnit("bits"))]);
    ry += rh;

    p.text("Taskbar layout", &fm.label, rect(x0, ry, x0 + 110.0, ry + rh), p.t.text);
    let lay = cfg.net_layout.as_str();
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        64.0,
        &[("Combined", lay == "combined", Action::NetLayout("combined")), ("Split", lay == "split", Action::NetLayout("split"))],
    );
    h
}

/// Which network adapters make up the speeds. Only offered when there are several.
fn adapters_card(p: &mut Painter, x: f32, y: f32, w: f32, view: &View) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let h = 10.0 + CARD_TITLE_H + 2.0 + view.adapters.len() as f32 * rh + 30.0 + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let pin_x = x1 - 40.0;
    let ty = y + 10.0;
    p.icon(Section::Net, rect(x0, ty + 4.0, x0 + 16.0, ty + 20.0), p.t.text);
    p.text("Network adapters", &fm.card_title, rect(x0 + 24.0, ty, x0 + 150.0, ty + CARD_TITLE_H), p.t.text);
    p.text("Counted", &fm.small, rect(pin_x - 14.0, ty, pin_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    let mut ry = ty + CARD_TITLE_H + 2.0;
    for (i, a) in view.adapters.iter().enumerate() {
        let (label, tone) = if a.connected { (a.name.clone(), p.t.text) } else { (format!("{} (not connected)", a.name), p.t.text_secondary) };
        p.text(&label, &fm.row_label, rect(x0, ry, pin_x - 10.0, ry + rh), tone);
        p.toggle(pin_x, ry + (rh - 20.0) / 2.0, a.included, Action::NetAdapter(i));
        ry += rh;
    }
    p.text("The speeds are the total of the adapters that are on.", &fm.note, rect(x0, ry + 2.0, x1, ry + 30.0), p.t.text_secondary);
    h
}

/// General options: units, update speed, autostart. Returns the card height.
fn general_card(p: &mut Painter, x: f32, y: f32, w: f32, cfg: &Config, view: &View) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let rows = 3.0 * rh + if view.autostart_msg.is_some() { 30.0 } else { 0.0 };
    let h = 10.0 + CARD_TITLE_H + 2.0 + rows + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let ty = y + 10.0;
    p.text("General", &fm.card_title, rect(x0, ty, x0 + 120.0, ty + CARD_TITLE_H), p.t.text);
    let mut ry = ty + CARD_TITLE_H + 2.0;

    p.text("Temperature unit", &fm.label, rect(x0, ry, x0 + 110.0, ry + rh), p.t.text);
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        44.0,
        &[("\u{00B0}C", !cfg.fahrenheit, Action::Fahrenheit(false)), ("\u{00B0}F", cfg.fahrenheit, Action::Fahrenheit(true))],
    );
    ry += rh;

    p.text("Update speed", &fm.label, rect(x0, ry, x0 + 80.0, ry + rh), p.t.text);
    let cur = cfg.interval_ms;
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        38.0,
        &[
            ("0.5s", cur == 500, Action::Interval(500)),
            ("1s", cur == 1000, Action::Interval(1000)),
            ("2s", cur == 2000, Action::Interval(2000)),
            ("5s", cur == 5000, Action::Interval(5000)),
        ],
    );
    ry += rh;

    p.text("Start with Windows", &fm.label, rect(x0, ry, x0 + 150.0, ry + rh), p.t.text);
    p.toggle(x1 - 40.0, ry + (rh - 20.0) / 2.0, view.autostart, Action::Autostart(!view.autostart));
    ry += rh;
    if let Some(m) = &view.autostart_msg {
        p.text(m, &fm.note, rect(x0, ry, x1, ry + 30.0), p.t.text_secondary);
    }
    h
}

/// How things look: taskbar text size, spacing between values, flyout transparency. Returns the card height.
fn customization_card(p: &mut Painter, x: f32, y: f32, w: f32, cfg: &Config) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let h = 10.0 + CARD_TITLE_H + 2.0 + 3.0 * rh + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let ty = y + 10.0;
    p.text("Customization", &fm.card_title, rect(x0, ty, x0 + 160.0, ty + CARD_TITLE_H), p.t.text);
    let mut ry = ty + CARD_TITLE_H + 2.0;

    // What identifies each row on the taskbar.
    p.text("Labels", &fm.label, rect(x0, ry, x0 + 50.0, ry + rh), p.t.text);
    let style = cfg.label_style.as_str();
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        66.0,
        &[
            ("Icon", style == "icon", Action::Labels("icon")),
            ("Label", style == "text", Action::Labels("text")),
            ("Icon + label", style == "both", Action::Labels("both")),
        ],
    );
    ry += rh;

    p.text("Label badge", &fm.label, rect(x0, ry, x0 + 120.0, ry + rh), p.t.text);
    p.toggle(x1 - 40.0, ry + (rh - 20.0) / 2.0, cfg.label_badge, Action::Badge(!cfg.label_badge));
    ry += rh;

    // Lower tint opacity = more see-through. Buckets so a hand-edited value still highlights the nearest one.
    p.text("Transparency", &fm.label, rect(x0, ry, x0 + 80.0, ry + rh), p.t.text);
    let bucket = if cfg.tint <= 120 { 0 } else if cfg.tint <= 180 { 1 } else { 2 };
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        54.0,
        &[("Clear", bucket == 0, Action::Tint(96)), ("Normal", bucket == 1, Action::Tint(150)), ("Frosted", bucket == 2, Action::Tint(205))],
    );
    h
}

/// Which graphics cards appear in the flyout and on the taskbar. Only offered when there are several.
fn gpus_card(p: &mut Painter, x: f32, y: f32, w: f32, view: &View) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let h = 10.0 + CARD_TITLE_H + 2.0 + view.gpus.len() as f32 * rh + 30.0 + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let pin_x = x1 - 40.0;
    let fly_x = pin_x - 24.0 - 40.0;
    let ty = y + 10.0;
    p.icon(Section::Gpu, rect(x0, ty + 4.0, x0 + 16.0, ty + 20.0), p.t.text);
    p.text("Graphics cards", &p.fm.card_title, rect(x0 + 24.0, ty, x0 + 130.0, ty + CARD_TITLE_H), p.t.text);
    p.text("Flyout", &fm.small, rect(fly_x - 14.0, ty, fly_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    p.text("Taskbar", &fm.small, rect(pin_x - 14.0, ty, pin_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    let mut ry = ty + CARD_TITLE_H + 2.0;
    for (i, g) in view.gpus.iter().enumerate() {
        p.text(&g.name, &fm.row_label, rect(x0, ry, fly_x - 10.0, ry + rh), p.t.text);
        let toggle_y = ry + (rh - 20.0) / 2.0;
        p.toggle(fly_x, toggle_y, !g.hidden_in_flyout, Action::GpuToggle(i, Target::Flyout));
        p.toggle(pin_x, toggle_y, g.on_taskbar, Action::GpuToggle(i, Target::Pinned));
        ry += rh;
    }
    p.text("With several cards on the taskbar they are labelled GPU1, GPU2 and so on.", &fm.note, rect(x0, ry + 2.0, x1, ry + 30.0), p.t.text_secondary);
    h
}

/// Connected local disks: which to show in the flyout and on the taskbar, and how to show their space.
fn disks_card(p: &mut Painter, x: f32, y: f32, w: f32, cfg: &Config, view: &View) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let rows = view.disks.len().max(1) as f32 + 2.0; // a row per disk (or a hint) plus the "Layout" and "Show" rows
    let h = 10.0 + CARD_TITLE_H + 2.0 + rows * rh + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let pin_x = x1 - 40.0;
    let fly_x = pin_x - 24.0 - 40.0;
    let ty = y + 10.0;
    p.icon(Section::Disk, rect(x0, ty + 4.0, x0 + 16.0, ty + 20.0), p.t.text);
    p.text("Disks", &fm.card_title, rect(x0 + 24.0, ty, x0 + 110.0, ty + CARD_TITLE_H), p.t.text);
    p.text("Flyout", &fm.small, rect(fly_x - 14.0, ty, fly_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    p.text("Taskbar", &fm.small, rect(pin_x - 14.0, ty, pin_x + 54.0, ty + CARD_TITLE_H), p.t.text_secondary);
    let mut ry = ty + CARD_TITLE_H + 2.0;
    if view.disks.is_empty() {
        p.text("No local disks found", &fm.label, rect(x0, ry, x1, ry + rh), p.t.text_secondary);
        ry += rh;
    }
    for (i, d) in view.disks.iter().enumerate() {
        p.text(&metrics::disk_name(d), &fm.row_label, rect(x0, ry, fly_x - 10.0, ry + rh), p.t.text);
        let toggle_y = ry + (rh - 20.0) / 2.0;
        p.toggle(fly_x, toggle_y, !cfg.disks_hidden.contains(&d.root), Action::DiskToggle(i, Target::Flyout));
        p.toggle(pin_x, toggle_y, cfg.disks_pinned.contains(&d.root), Action::DiskToggle(i, Target::Pinned));
        ry += rh;
    }
    p.text("Layout", &fm.label, rect(x0, ry, x0 + 60.0, ry + rh), p.t.text);
    let lay = cfg.disk_layout.as_str();
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        64.0,
        &[("Combined", lay == "combined", Action::DiskLayout("combined")), ("Split", lay == "split", Action::DiskLayout("split"))],
    );
    ry += rh;

    p.text("Show", &fm.label, rect(x0, ry, x0 + 40.0, ry + rh), p.t.text);
    let st = cfg.disk_style.as_str();
    p.segmented(
        x1,
        ry + (rh - 28.0) / 2.0,
        64.0,
        &[
            ("Used/total", st == "used_total", Action::DiskStyle("used_total")),
            ("% used", st == "percent", Action::DiskStyle("percent")),
            ("Free", st == "free", Action::DiskStyle("free")),
        ],
    );
    h
}

/// The PawnIO driver card: an at-a-glance status label, what it's for, and Install when it's missing.
fn pawn_card(p: &mut Painter, x: f32, y: f32, w: f32, view: &View) -> f32 {
    let fm = p.fm;
    let (tone, status): (D2D1_COLOR_F, String) = match (view.pawn, &view.setup) {
        (None, _) => (p.t.ok, "Active".into()),
        (Some(Unavailable::UnsupportedCpu), _) => (p.t.text_secondary, "Not supported on this CPU".into()),
        (Some(Unavailable::AccessDenied), _) => (p.t.warn, "Needs administrator".into()),
        (Some(_), SetupState::Working(t)) => (p.t.warn, t.to_string()),
        (Some(_), SetupState::Done { reboot: true }) => (p.t.warn, "Restart needed".into()),
        (Some(_), SetupState::Done { reboot: false }) => (p.t.warn, "Starting...".into()),
        (Some(Unavailable::NotInstalled), _) => (p.t.bad, "Not installed".into()),
        (Some(_), _) => (p.t.bad, "Unavailable".into()),
    };
    let msg = match &view.setup {
        SetupState::Failed(m) => Some(m.clone()),
        SetupState::Done { reboot: true } => Some("Restart Windows to finish setting up the driver.".to_string()),
        _ => view.pawn_msg.clone(),
    };
    let desc = msg.unwrap_or_else(|| {
        if view.pawn.is_none() {
            "CPU temperature and power are available.".to_string()
        } else {
            "Needed for CPU temperature and power. Everything else works without it.".to_string()
        }
    });
    let installable = view.pawn == Some(Unavailable::NotInstalled) && matches!(view.setup, SetupState::Idle | SetupState::Failed(_));
    let h = 10.0 + CARD_TITLE_H + 26.0 + NOTE_H + 4.0 + if installable { 36.0 } else { 0.0 } + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let ty = y + 10.0;
    p.text("PawnIO driver", &fm.card_title, rect(x0, ty, x1, ty + CARD_TITLE_H), p.t.text);
    let sy = ty + CARD_TITLE_H + 2.0;
    p.circle(x0 + 5.0, sy + 12.0, 4.5, tone);
    p.text(&status, &fm.status, rect(x0 + 16.0, sy, x1, sy + 24.0), p.t.text);
    let dy = sy + 26.0;
    p.text(&desc, &fm.note, rect(x0, dy, x1, dy + NOTE_H), p.t.text_secondary);
    if installable {
        let label = if matches!(view.setup, SetupState::Failed(_)) { "Retry" } else { "Install driver" };
        p.button(x1, dy + NOTE_H + 4.0, 110.0, label, Action::InstallPawn);
    }
    h
}

/// Reorder the taskbar widget's rows and columns by dragging chips. Returns the card height.
fn layout_card(p: &mut Painter, x: f32, y: f32, w: f32, cfg: &Config) -> f32 {
    let fm = p.fm;
    let rh = p.rh();
    let crh = CHIP_ROW_H + p.row_extra;
    let h = 10.0 + CARD_TITLE_H + 2.0 + 2.0 * crh + rh + 10.0;
    p.card(x, y, w, h);
    let (x0, x1) = (x + CARD_PAD, x + w - CARD_PAD);
    let ty = y + 10.0;
    p.text("Widget layout", &fm.card_title, rect(x0, ty, x0 + 160.0, ty + CARD_TITLE_H), p.t.text);
    p.text("Drag to reorder", &fm.subtitle, rect(x1 - 160.0, ty, x1, ty + CARD_TITLE_H), p.t.text_secondary);

    let chip_x = x0 + 66.0;
    let row_y = ty + CARD_TITLE_H + 2.0;
    p.text("Rows", &fm.label, rect(x0, row_y, x0 + 60.0, row_y + crh), p.t.text);
    let rows: Vec<(&'static str, &'static str, bool, Option<Section>)> = cfg
        .row_order
        .iter()
        .filter_map(|k| Section::from_key(k))
        .map(|s| (s.key(), s.label(), metrics::section_pinned(cfg, s), Some(s)))
        .collect();
    p.chip_strip(chip_x, row_y + (crh - CHIP_H) / 2.0, ListKind::Rows, &rows);

    let col_y = row_y + crh;
    p.text("Columns", &fm.label, rect(x0, col_y, x0 + 60.0, col_y + crh), p.t.text);
    let cols: Vec<(&'static str, &'static str, bool, Option<Section>)> =
        metrics::visible_kinds(cfg).into_iter().map(|k| (k.key(), k.label(), true, None)).collect();
    if cols.is_empty() {
        p.text("Turn on Taskbar for some metrics above to arrange them.", &fm.label, rect(chip_x, col_y, x1, col_y + crh), p.t.text_secondary);
    } else {
        p.chip_strip(chip_x, col_y + (crh - CHIP_H) / 2.0, ListKind::Cols, &cols);
    }

    // Size of the text and the space between values: both shape how this layout looks on the taskbar.
    let sy = col_y + crh;
    let mid = (x0 + x1) / 2.0;
    p.text("Text size", &fm.label, rect(x0, sy, x0 + 100.0, sy + rh), p.t.text);
    let font = cfg.font_size;
    p.stepper(
        mid - 16.0,
        sy + (rh - 28.0) / 2.0,
        &format!("{}", font as i32),
        (font > 9.0).then_some(Action::FontStep(-1)),
        (font < 15.0).then_some(Action::FontStep(1)),
    );
    p.text("Spacing", &fm.label, rect(mid + 16.0, sy, mid + 116.0, sy + rh), p.t.text);
    let gap = cfg.gap;
    p.stepper(
        x1,
        sy + (rh - 28.0) / 2.0,
        &format!("{}", gap as i32),
        (gap > 2.0).then_some(Action::GapStep(-1)),
        (gap < 24.0).then_some(Action::GapStep(1)),
    );
    h
}

/// Draws the three settings columns with `extra[i]` more height per row in column `i`. Returns each column's height
/// (including the gap below its last card).
///
/// 1. Processor side: CPU, the disks, and the driver the CPU's temperature and power depend on.
/// 2. Graphics and memory.
/// 3. Network and its adapters, then general options.
fn settings_columns(p: &mut Painter, cfg: &Config, view: &View, col_w: f32, extra: [f32; 3]) -> [f32; 3] {
    let xs = [PAD, 2.0 * PAD + col_w, 3.0 * PAD + 2.0 * col_w];
    let (mut c1, mut c2, mut c3) = (0.0, 0.0, 0.0);

    p.row_extra = extra[0];
    c1 += metrics_card(p, xs[0], c1, col_w, "CPU", Section::Cpu, cfg) + CARD_GAP;
    c1 += disks_card(p, xs[0], c1, col_w, cfg, view) + CARD_GAP;
    c1 += pawn_card(p, xs[0], c1, col_w, view) + CARD_GAP;

    p.row_extra = extra[1];
    if view.gpus.len() >= 2 {
        c2 += gpus_card(p, xs[1], c2, col_w, view) + CARD_GAP;
    }
    c2 += metrics_card(p, xs[1], c2, col_w, "GPU", Section::Gpu, cfg) + CARD_GAP;
    c2 += metrics_card(p, xs[1], c2, col_w, "Memory", Section::Ram, cfg) + CARD_GAP;

    p.row_extra = extra[2];
    c3 += network_card(p, xs[2], c3, col_w, cfg) + CARD_GAP;
    if view.adapters.len() >= 2 {
        c3 += adapters_card(p, xs[2], c3, col_w, view) + CARD_GAP;
    }
    c3 += general_card(p, xs[2], c3, col_w, cfg, view) + CARD_GAP;
    p.row_extra = 0.0;
    [c1, c2, c3]
}

/// Settings page body: three columns so it stays short, with related cards kept together. Along the bottom: the
/// widget's layout (two columns wide) and the look of its labels. Every column, and both bottom cards, end flush:
/// the shorter ones get a little more height per row instead of leaving a gap. Returns the content height.
fn settings_body(p: &mut Painter, cfg: &Config, view: &View) -> f32 {
    let col_w = (p.width - 4.0 * PAD) / 3.0;
    let wide = 2.0 * col_w + PAD;
    let xs = [PAD, 2.0 * PAD + col_w, 3.0 * PAD + 2.0 * col_w];

    // Each column's height grows by one pixel per row for every pixel of row_extra, so two dry runs give its height
    // and its row count.
    let measure = |p: &Painter, extra: f32| {
        let mut m = Painter::new(None, p.t, p.fm, p.dw, None, None, 0.0, f32::MAX, p.width);
        m.in_body = true;
        settings_columns(&mut m, cfg, view, col_w, [extra; 3])
    };
    let (h0, h1) = (measure(p, 0.0), measure(p, 1.0));
    // All rows start a little taller than the base; the shorter columns are stretched up to the tallest.
    let base = 2.0;
    let target = (0..3).map(|i| h0[i] + (h1[i] - h0[i]) * base).fold(0.0, f32::max);
    let mut extra = [0.0; 3];
    for i in 0..3 {
        let rows = (h1[i] - h0[i]).max(1.0);
        extra[i] = (target - h0[i]) / rows;
    }
    let top = settings_columns(p, cfg, view, col_w, extra).into_iter().fold(0.0, f32::max);

    // Bottom row: same idea for the two cards.
    let bottom = |p: &mut Painter, e: [f32; 2], draw: bool| -> [f32; 2] {
        let mut m;
        let q: &mut Painter = if draw {
            p
        } else {
            m = Painter::new(None, p.t, p.fm, p.dw, None, None, 0.0, f32::MAX, p.width);
            m.in_body = true;
            &mut m
        };
        q.row_extra = e[0];
        let a = layout_card(q, xs[0], top, wide, cfg);
        q.row_extra = e[1];
        let b = customization_card(q, xs[2], top, col_w, cfg);
        q.row_extra = 0.0;
        [a, b]
    };
    let (b0, b1) = (bottom(p, [0.0; 2], false), bottom(p, [1.0; 2], false));
    let btarget = b0[0].max(b0[1]);
    let be = [0, 1].map(|i| (btarget - b0[i]) / (b1[i] - b0[i]).max(1.0));
    let hb = bottom(p, be, true);
    top + hb[0].max(hb[1]) + PAD
}

/// Things the settings page needs besides the config.
/// A graphics card as the settings page needs it.
struct GpuInfo {
    name: String,
    hidden_in_flyout: bool,
    on_taskbar: bool,
}

/// A network adapter as the settings page needs it.
struct AdapterInfo {
    name: String,
    connected: bool,
    included: bool,
}

struct View {
    gpus: Vec<GpuInfo>,
    adapters: Vec<AdapterInfo>,
    disks: Vec<DiskStats>,
    autostart: bool,
    autostart_msg: Option<String>,
    pawn: Option<Unavailable>,
    setup: SetupState,
    pawn_msg: Option<String>,
}

impl Fly {
    fn view(&self) -> View {
        let cfg = config::get();
        let on_taskbar: Vec<&str> = metrics::taskbar_gpus(&cfg, &self.snap).iter().map(|g| g.key.as_str()).collect();
        let gpus = self
            .snap
            .gpus
            .iter()
            .map(|g| GpuInfo { name: g.name.clone(), hidden_in_flyout: cfg.gpus_hidden.contains(&g.key), on_taskbar: on_taskbar.contains(&g.key.as_str()) })
            .collect();
        let counted: Vec<&str> = self.snap.net.included(&cfg.net_adapters).map(|a| a.name.as_str()).collect();
        let adapters = self
            .snap
            .net
            .adapters
            .iter()
            .map(|a| AdapterInfo { name: a.name.clone(), connected: a.connected, included: counted.contains(&a.name.as_str()) })
            .collect();
        let mut v = View {
            gpus,
            adapters,
            disks: self.snap.disks.clone(),
            autostart: self.autostart,
            autostart_msg: self.autostart_msg.clone(),
            pawn: self.snap.pawn,
            setup: setup::state(),
            pawn_msg: self.pawn_msg.clone(),
        };
        // Developer preview of the driver card's states for screenshots: HWM_FAKE_PAWN=active|missing|restart|failed.
        if let Ok(f) = std::env::var("HWM_FAKE_PAWN") {
            match f.as_str() {
                "active" => v.pawn = None,
                "missing" => v.pawn = Some(Unavailable::NotInstalled),
                "restart" => {
                    v.pawn = Some(Unavailable::NotInstalled);
                    v.setup = SetupState::Done { reboot: true };
                }
                "failed" => {
                    v.pawn = Some(Unavailable::NotInstalled);
                    v.setup = SetupState::Failed("Download failed. Check your internet connection.".into());
                }
                _ => {}
            }
        }
        v
    }

    /// Window position for the slide-in: ease-out cubic from SLIDE DIPs below the resting place.
    fn apply_pos(&self) {
        let t = (self.opened.elapsed().as_secs_f32() / ANIM.as_secs_f32()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let offset = ((1.0 - eased) * SLIDE * self.scale) as i32;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                self.x,
                self.final_y + offset,
                self.surface.width as i32,
                self.surface.height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }

    fn content_height(&self, cfg: &Config, cards: &[Card]) -> f32 {
        let mut p = Painter::new(None, self.theme, &self.fmts, &self.dwrite, self.drag, None, 0.0, f32::MAX, page_width(self.page));
        p.in_body = true;
        match self.page {
            Page::Dashboard => dashboard_body(&mut p, cards),
            Page::Settings => settings_body(&mut p, cfg, &self.view()),
        }
    }

    /// Re-measures the page, resizes the window to fit (scrolling when it can't), and redraws.
    fn render(&mut self) {
        let cfg = config::get();
        let cards = metrics::flyout_cards(&cfg, &self.snap, &self.cpu_name);
        let content_h = self.content_height(&cfg, &cards);
        let win_h = (BODY_TOP + content_h).min(self.max_h_dip);
        let body_bottom = win_h;
        self.max_scroll = (BODY_TOP + content_h - win_h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);

        let win_w = page_width(self.page);
        let (w_px, h_px) = ((win_w * self.scale).ceil() as u32, (win_h * self.scale).ceil() as u32);
        if h_px != self.surface.height || w_px != self.surface.width {
            let _ = self.surface.resize(w_px, h_px);
            self.final_y = self.taskbar_top - h_px as i32 - (GAP_ABOVE_TASKBAR * self.scale) as i32;
            self.apply_pos();
        }

        let (t, fm, mouse, scroll, page, max_scroll) = (self.theme, &self.fmts, self.mouse, self.scroll, self.page, self.max_scroll);
        let (dw, drag) = (&self.dwrite, self.drag);
        let view = self.view();
        let mut hits = Vec::new();
        let mut drag_target = None;
        let _ = self.surface.frame(|dc| {
            let mut p = Painter::new(Some(dc), t, fm, dw, drag, mouse, scroll, body_bottom, win_w);
            header(&mut p, page);
            p.begin_body();
            match page {
                Page::Dashboard => dashboard_body(&mut p, &cards),
                Page::Settings => settings_body(&mut p, &cfg, &view),
            };
            p.end_body();
            if max_scroll > 0.0 {
                // Slim scroll indicator.
                let track = body_bottom - BODY_TOP - 8.0;
                let thumb = (track * track / (track + max_scroll)).max(24.0);
                let top = BODY_TOP + 4.0 + (track - thumb) * (scroll / max_scroll);
                let mut c = t.text_secondary;
                c.a *= 0.5;
                p.in_body = false;
                p.fill(rect(win_w - 5.0, top, win_w - 2.0, top + thumb), 1.5, c);
            }
            drag_target = p.drag_target;
            hits = p.hits;
        });
        self.hits = hits;
        self.drag_target = drag_target;
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<Action> {
        self.hit_full(x, y).map(|(a, _)| a)
    }

    fn hit_full(&self, x: f32, y: f32) -> Option<(Action, D2D_RECT_F)> {
        self.hits.iter().rev().find(|h| x >= h.r.left && x < h.r.right && y >= h.r.top && y < h.r.bottom).map(|h| (h.a, h.r))
    }

    /// Mouse-down on a chip starts a drag; the mouse is captured so it keeps tracking outside the window.
    fn begin_drag(&mut self, hwnd: HWND, x: f32, y: f32) {
        if let Some((Action::Chip(list, from), r)) = self.hit_full(x, y) {
            self.drag = Some(Drag { list, from, grab: x - r.left, x });
            self.drag_target = None;
            unsafe {
                SetCapture(hwnd);
            }
            self.render();
        }
    }

    /// Mouse-up while dragging: commit the new order, save it, and tell the widget.
    fn end_drag(&mut self) {
        let Some(d) = self.drag.take() else { return };
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some(to) = self.drag_target.take() {
            if to != d.from {
                let mut cfg = config::get();
                match d.list {
                    ListKind::Rows => {
                        let visible = cfg.row_order.clone();
                        cfg.row_order = reorder_visible(&cfg.row_order, &visible, d.from, to);
                    }
                    ListKind::Cols => {
                        let visible: Vec<String> = metrics::visible_kinds(&cfg).iter().map(|k| k.key().to_string()).collect();
                        cfg.col_order = reorder_visible(&cfg.col_order, &visible, d.from, to);
                    }
                }
                config::set(cfg);
                unsafe {
                    let _ = PostMessageW(Some(self.widget), WM_CONFIG, WPARAM(0), LPARAM(0));
                }
            }
        }
        self.render();
    }

    fn apply(&mut self, a: Action) {
        let mut cfg = config::get();
        match a {
            Action::Chip(..) => {}
            Action::Labels(l) => cfg.label_style = l.to_string(),
            Action::Badge(b) => cfg.label_badge = b,
            Action::DiskStyle(st) => cfg.disk_style = st.to_string(),
            Action::DiskLayout(l) => cfg.disk_layout = l.to_string(),
            Action::NetUnit(u) => cfg.net_unit = u.to_string(),
            Action::NetLayout(l) => cfg.net_layout = l.to_string(),
            Action::NetAdapter(i) => {
                if let Some(name) = self.snap.net.adapters.get(i).map(|a| a.name.clone()) {
                    // Start from what is counted now (no choice yet = every adapter), then flip this one.
                    let mut sel: Vec<String> = self.snap.net.included(&cfg.net_adapters).map(|a| a.name.clone()).collect();
                    match sel.iter().position(|n| *n == name) {
                        // At least one adapter always counts.
                        Some(pos) if sel.len() > 1 => {
                            sel.remove(pos);
                        }
                        Some(_) => {}
                        None => sel.push(name),
                    }
                    // Every adapter on again is "all", which also picks up adapters plugged in later.
                    cfg.net_adapters = if sel.len() == self.snap.net.adapters.len() { Vec::new() } else { sel };
                }
            }
            Action::GpuToggle(i, target) => {
                if let Some(key) = self.snap.gpus.get(i).map(|g| g.key.clone()) {
                    match target {
                        Target::Flyout => match cfg.gpus_hidden.iter().position(|k| *k == key) {
                            Some(pos) => {
                                cfg.gpus_hidden.remove(pos);
                            }
                            None => cfg.gpus_hidden.push(key),
                        },
                        Target::Pinned => {
                            // Start from what the taskbar effectively shows now (no choice yet = the first card).
                            let mut sel: Vec<String> = metrics::taskbar_gpus(&cfg, &self.snap).iter().map(|g| g.key.clone()).collect();
                            match sel.iter().position(|k| *k == key) {
                                // The taskbar always shows at least one card.
                                Some(pos) if sel.len() > 1 => {
                                    sel.remove(pos);
                                }
                                Some(_) => {}
                                None => sel.push(key),
                            }
                            cfg.gpus_pinned = sel;
                        }
                    }
                }
            }
            Action::DiskToggle(i, target) => {
                if let Some(d) = self.snap.disks.get(i) {
                    let root = d.root.clone();
                    let list = match target {
                        Target::Flyout => &mut cfg.disks_hidden,
                        Target::Pinned => &mut cfg.disks_pinned,
                    };
                    match list.iter().position(|r| *r == root) {
                        Some(pos) => {
                            list.remove(pos);
                        }
                        None => list.push(root),
                    }
                }
            }
            Action::FontStep(d) => cfg.font_size = (cfg.font_size + d as f32).clamp(9.0, 15.0),
            Action::GapStep(d) => cfg.gap = (cfg.gap + d as f32).clamp(2.0, 24.0),
            Action::OpenSettings => {
                self.page = Page::Settings;
                self.scroll = 0.0;
            }
            Action::Back => {
                self.page = Page::Dashboard;
                self.scroll = 0.0;
            }
            Action::Toggle(id, target) => {
                let list = match target {
                    Target::Flyout => &mut cfg.flyout,
                    Target::Pinned => &mut cfg.pinned,
                };
                if let Some(i) = list.iter().position(|x| x == id) {
                    list.remove(i);
                } else {
                    list.push(id.to_string());
                    list.sort_by_key(|x| metrics::order(x));
                }
            }
            Action::Autostart(on) => {
                if !self.elevated {
                    self.autostart_msg = Some("Run TaskbarHWM as administrator to change this.".into());
                } else {
                    let r = if on { autostart::enable() } else { autostart::disable() };
                    self.autostart_msg = r.err();
                    self.autostart = autostart::is_enabled();
                }
            }
            Action::InstallPawn => {
                if !self.elevated {
                    self.pawn_msg = Some("Run TaskbarHWM as administrator to install the driver.".into());
                } else {
                    self.pawn_msg = None;
                    setup::start_install(self.widget.0 as isize);
                }
            }
            Action::Fahrenheit(f) => cfg.fahrenheit = f,
            Action::Interval(ms) => cfg.interval_ms = ms,
            Action::Tint(t) => {
                cfg.tint = t;
                super::backdrop::apply(self.hwnd, self.theme.dark, t);
            }
        }
        if !matches!(a, Action::Chip(..) | Action::OpenSettings | Action::Back | Action::Autostart(_) | Action::InstallPawn) {
            config::set(cfg);
            unsafe {
                let _ = PostMessageW(Some(self.widget), WM_CONFIG, WPARAM(0), LPARAM(0));
            }
        }
        self.render();
    }
}

/// Moves one of the `visible` entries from index `from` to index `to` (among the visible ones), leaving entries
/// that aren't shown where they are in `full`.
fn reorder_visible(full: &[String], visible: &[String], from: usize, to: usize) -> Vec<String> {
    let mut moved = visible.to_vec();
    if from >= moved.len() {
        return full.to_vec();
    }
    let item = moved.remove(from);
    moved.insert(to.min(moved.len()), item);
    let mut it = moved.into_iter();
    full.iter().map(|k| if visible.contains(k) { it.next().unwrap_or_else(|| k.clone()) } else { k.clone() }).collect()
}

fn anim_tick() {
    FLY.with(|cell| {
        let Ok(guard) = cell.try_borrow() else { return };
        if let Some(f) = guard.as_ref() {
            f.apply_pos();
            if f.opened.elapsed() >= ANIM {
                unsafe {
                    let _ = KillTimer(Some(f.hwnd), TIMER_ANIM);
                }
            }
        }
    });
}

fn client_dip(lp: LPARAM, scale: f32) -> (f32, f32) {
    let x = (lp.0 & 0xFFFF) as i16 as f32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
    (x / scale, y / scale)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            // HWM_KEEP_OPEN is a developer switch for automated screenshots; users never set it.
            if std::env::var_os("HWM_KEEP_OPEN").is_none() {
                close(true);
            }
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            close(false);
            return LRESULT(0);
        }
        WM_TIMER if wp.0 == TIMER_ANIM => {
            anim_tick();
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            with_fly(|f| {
                let m = client_dip(lp, f.scale);
                f.mouse = Some(m);
                if let Some(d) = f.drag.as_mut() {
                    d.x = m.0;
                }
                if !f.tracking {
                    f.tracking = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                }
                f.render();
            });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_fly(|f| {
                f.mouse = None;
                f.tracking = false;
                f.render();
            });
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            with_fly(|f| {
                let (x, y) = client_dip(lp, f.scale);
                f.begin_drag(hwnd, x, y);
            });
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with_fly(|f| {
                if f.drag.is_some() {
                    // The release position is authoritative: repaint at it so the drop target matches.
                    let (x, _) = client_dip(lp, f.scale);
                    if let Some(d) = f.drag.as_mut() {
                        d.x = x;
                    }
                    f.render();
                    f.end_drag();
                    return;
                }
                let (x, y) = client_dip(lp, f.scale);
                if let Some(a) = f.hit_at(x, y) {
                    f.apply(a);
                }
            });
            return LRESULT(0);
        }
        // Lost the mouse capture some other way (e.g. Alt+Tab): abandon the drag.
        WM_CAPTURECHANGED => {
            with_fly(|f| {
                if f.drag.take().is_some() {
                    f.drag_target = None;
                    f.render();
                }
            });
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
            with_fly(|f| {
                if f.max_scroll > 0.0 {
                    f.scroll = (f.scroll - delta * WHEEL_STEP).clamp(0.0, f.max_scroll);
                    f.render();
                }
            });
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
