//! The taskbar widget: a transparent topmost window drawn over the far-left of the taskbar.
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_numerics::Vector2;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::gfx::{Gfx, Surface};
use super::theme::Theme;
use crate::config::Config;
use crate::metrics;
use crate::shell::displays;
use crate::sensors::{Sampler, Snapshot};

const WM_SNAPSHOT: u32 = WM_APP + 1;
/// Posted by the flyout when the user changed a setting.
pub const WM_CONFIG: u32 = WM_APP + 2;
/// Posted by the right-click menu: open the flyout on its settings page.
const WM_OPEN_SETTINGS: u32 = WM_APP + 4;
const TIMER_TICK: usize = 1;
const FONT: windows::core::PCWSTR = w!("Segoe UI Variable Text");

// Layout, in DIPs.
const PAD_X: f32 = 10.0;
const BADGE_PAD: f32 = 6.0; // horizontal padding inside a label badge
const BADGE_RADIUS: f32 = 4.0;
/// Cap height of Segoe UI Variable as a fraction of the font size; used to center capitals and digits optically.
const CAP_HEIGHT: f32 = 0.70;
const EDGE: f32 = 4.0; // margin from the taskbar's left edge
const CORNER: f32 = 4.0;

/// A value, measured so it can be right-aligned in its column: the number, and its unit drawn smaller and muted.
struct Cell {
    num: IDWriteTextLayout,
    unit: Option<UnitPart>,
    /// Total width of number and unit.
    w: f32,
}

struct UnitPart {
    layout: IDWriteTextLayout,
    /// Where the unit starts, measured from the start of the number.
    x: f32,
}

struct GRow {
    /// Section whose icon leads the row (None for the text-only hint row).
    icon: Option<metrics::Section>,
    /// Draw this arrow instead of the section's icon.
    arrow: Option<metrics::Arrow>,
    /// Width the icon takes: a lone arrow only fills the middle of its square, so it gets a narrower slot.
    icon_w: f32,
    label: Option<IDWriteTextLayout>,
    /// Width of the label area (icon and/or text).
    label_w: f32,
    /// One slot per column of the block; None = blank cell.
    cells: Vec<Option<Cell>>,
    /// This row's own column widths (only used when the block's columns aren't shared).
    col_w: Vec<f32>,
}

/// Up to two rows that share columns. Every column has one fixed width for all its rows.
struct Block {
    rows: Vec<GRow>,
    /// Rows share one set of column widths (hardware rows); otherwise each row spaces its own values.
    shared: bool,
    /// Label rows sit in a rounded badge.
    badge: bool,
    label_w: f32,
    col_w: Vec<f32>,
    width: f32,
}

/// One widget window: the widget as drawn over the taskbar of one display.
struct Win {
    hwnd: HWND,
    surface: Surface,
    /// Top-left of the window on screen, in pixels (the taskbar's top-left).
    pos: (i32, i32),
    hover: bool,
    pressed: bool,
    /// The display it is on (`displays::Display::key`).
    monitor: String,
    /// The taskbar window it sits over.
    tray: HWND,
}

impl Win {
    fn create(gfx: &Gfx, monitor: String, tray: HWND) -> Option<Win> {
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                w!("TaskbarHWM.Widget"),
                w!("TaskbarHWM"),
                WS_POPUP,
                0,
                0,
                200,
                48,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .ok()?;
            let Ok(surface) = Surface::new(gfx, 200, 48, 96.0) else {
                let _ = DestroyWindow(hwnd);
                return None;
            };
            Some(Win { hwnd, surface, pos: (0, 0), hover: false, pressed: false, monitor, tray })
        }
    }
}

struct App {
    gfx: Gfx,
    /// A hidden window that owns the timer and receives the app's own messages (new readings, setting changes, the
    /// settings page's requests). The visible widget windows come and go with the chosen displays; this one stays.
    controller: HWND,
    wins: Vec<Win>,
    cfg: Config,
    shared: Arc<Mutex<Snapshot>>,
    snap: Snapshot,
    theme: Theme,
    fmt: IDWriteTextFormat,
    /// Size `fmt` was created at; recreated when the setting changes.
    fmt_size: f32,
    /// Semibold, slightly smaller: row labels must not read like the values.
    label_fmt: IDWriteTextFormat,
    /// Smaller still: the units ("GHz", "%", "GB") that follow a value.
    unit_fmt: IDWriteTextFormat,
    /// Moves a unit down so its baseline lands on the baseline of the (larger) number beside it.
    unit_dy: f32,
    /// Vertical nudges (DIPs) that put the cap-height midline of values / labels on the row's midline.
    val_dy: f32,
    label_dy: f32,
    blocks: Vec<Block>,
    width_dip: f32,
    hidden_fullscreen: bool,
    first_run_checked: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn measure(l: &IDWriteTextLayout) -> f32 {
    let mut m = DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = l.GetMetrics(&mut m);
    }
    m.widthIncludingTrailingWhitespace.ceil()
}

fn label_size(font: f32) -> f32 {
    (font * 0.9).round().max(8.0)
}

fn unit_size(font: f32) -> f32 {
    // Not rounded: at the default size a whole-point step would be too coarse.
    (font * 0.88).max(8.0)
}

/// `a` moved `k` of the way toward `b`.
fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, k: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: a.r + (b.r - a.r) * k, g: a.g + (b.g - a.g) * k, b: a.b + (b.b - a.b) * k, a: a.a + (b.a - a.a) * k }
}

fn make_format(dwrite: &IDWriteFactory, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<IDWriteTextFormat> {
    unsafe {
        let f = dwrite.CreateTextFormat(
            FONT,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        // Center within the row so labels (smaller) and values (larger) share a visual midline.
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(f)
    }
}

impl App {
    // Geometry derived from the user's text size and spacing settings (DIPs).
    fn row_h(&self) -> f32 {
        (self.cfg.font_size * 1.35).round()
    }
    fn col_gap(&self) -> f32 {
        self.cfg.gap
    }
    fn icon_size(&self) -> f32 {
        (self.cfg.font_size * 1.3).round().max(11.0)
    }
    fn label_gap(&self) -> f32 {
        (self.cfg.gap * 0.75).max(3.0)
    }
    fn block_gap(&self) -> f32 {
        self.cfg.gap * 2.5 + 2.0
    }

    fn layout(&self, text: &str) -> Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe { self.gfx.dwrite.CreateTextLayout(&wide, &self.fmt, 2000.0, self.row_h()) }
    }

    fn unit_layout(&self, text: &str) -> Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe { self.gfx.dwrite.CreateTextLayout(&wide, &self.unit_fmt, 2000.0, self.row_h()) }
    }

    /// A value as a measured number plus its unit.
    fn cell(&self, text: &str) -> Result<Cell> {
        let (num, unit) = metrics::split_unit(text);
        let num_layout = self.layout(num)?;
        let num_w = measure(&num_layout);
        let Some((unit, spaced)) = unit else { return Ok(Cell { num: num_layout, unit: None, w: num_w }) };
        let layout = self.unit_layout(unit)?;
        // The space that separated them is replaced by a smaller one, since the unit itself is smaller.
        let x = num_w + if spaced { (self.cfg.font_size * 0.2).max(2.0) } else { 0.5 };
        let w = x + measure(&layout);
        Ok(Cell { num: num_layout, unit: Some(UnitPart { layout, x }), w })
    }

    /// Distance from the top of a row to the baseline of text drawn with `fmt` (lines are centered in the row).
    fn baseline(&self, fmt: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = "0".encode_utf16().collect();
        unsafe {
            let Ok(l) = self.gfx.dwrite.CreateTextLayout(&wide, fmt, 200.0, self.row_h()) else { return 0.0 };
            let mut lm = [DWRITE_LINE_METRICS::default(); 1];
            let mut n = 0u32;
            if l.GetLineMetrics(Some(&mut lm), &mut n).is_err() || n == 0 {
                return 0.0;
            }
            (self.row_h() - lm[0].height) / 2.0 + lm[0].baseline
        }
    }

    fn label_layout(&self, text: &str) -> Result<IDWriteTextLayout> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe { self.gfx.dwrite.CreateTextLayout(&wide, &self.label_fmt, 2000.0, self.row_h()) }
    }

    /// How far to move text drawn with `fmt` so its capitals/digits (not its whole line box, which reserves room for
    /// descenders) sit centered in the row. Without this, all-caps text such as "CPU" looks too low.
    fn cap_dy(&self, fmt: &IDWriteTextFormat, size: f32) -> f32 {
        let wide: Vec<u16> = "H".encode_utf16().collect();
        unsafe {
            let Ok(l) = self.gfx.dwrite.CreateTextLayout(&wide, fmt, 200.0, self.row_h()) else { return 0.0 };
            let mut lm = [DWRITE_LINE_METRICS::default(); 1];
            let mut n = 0u32;
            if l.GetLineMetrics(Some(&mut lm), &mut n).is_err() || n == 0 {
                return 0.0;
            }
            // The line box is centered in the row; move so the middle of the cap height lands on the row's middle.
            lm[0].height / 2.0 - lm[0].baseline + size * CAP_HEIGHT / 2.0
        }
    }

    /// Recreates the text formats if the font size setting changed.
    fn ensure_font(&mut self) {
        if (self.cfg.font_size - self.fmt_size).abs() > 0.01 {
            let dw = &self.gfx.dwrite;
            if let (Ok(f), Ok(l), Ok(u)) = (
                make_format(dw, self.cfg.font_size, DWRITE_FONT_WEIGHT_NORMAL),
                make_format(dw, label_size(self.cfg.font_size), DWRITE_FONT_WEIGHT_SEMI_BOLD),
                make_format(dw, unit_size(self.cfg.font_size), DWRITE_FONT_WEIGHT_NORMAL),
            ) {
                self.fmt = f;
                self.label_fmt = l;
                self.unit_fmt = u;
                self.fmt_size = self.cfg.font_size;
            }
        }
    }

    /// Rebuilds the grid (text layouts and column widths) from the latest snapshot. Returns the widget width in DIPs.
    fn rebuild(&mut self) -> f32 {
        self.ensure_font();
        self.val_dy = self.cap_dy(&self.fmt, self.cfg.font_size);
        self.label_dy = self.cap_dy(&self.label_fmt, label_size(self.cfg.font_size));
        self.unit_dy = self.baseline(&self.fmt) - self.baseline(&self.unit_fmt);
        let grid = metrics::widget_grid(&self.cfg, &self.snap);
        let mut blocks: Vec<Block> = Vec::new();
        for b in &grid {
            let mut rows = Vec::new();
            let style = self.cfg.label_style.as_str();
            let (want_icon, want_text) = (style != "text", style != "icon");
            let isz = self.icon_size();
            let badge = self.cfg.label_badge;
            for r in &b.rows {
                let label = if want_text { self.label_layout(&r.label).ok() } else { None };
                if want_text && label.is_none() {
                    continue;
                }
                let text_w = label.as_ref().map(measure).unwrap_or(0.0);
                let icon_w = if r.arrow.is_some() { (isz * 0.62).round() } else { isz };
                let label_w = if want_icon { icon_w } else { 0.0 }
                    + if want_icon && want_text { 5.0 } else { 0.0 }
                    + text_w
                    + if badge { 2.0 * BADGE_PAD } else { 0.0 };
                let cells = r
                    .cells
                    .iter()
                    .map(|c| {
                        c.as_ref().and_then(|t| self.cell(t).ok())
                    })
                    .collect();
                // Rows that don't share columns reserve room per value, so each row's spacing is uniform and stable.
                let col_w: Vec<f32> = if b.shared {
                    Vec::new()
                } else {
                    r.templates
                        .iter()
                        .zip(&cells)
                        .map(|(t, c): (&String, &Option<Cell>)| {
                            let reserve = self.cell(t).map(|c| c.w).unwrap_or(0.0);
                            c.as_ref().map_or(reserve, |c| reserve.max(c.w))
                        })
                        .collect()
                };
                rows.push(GRow { icon: want_icon.then_some(r.section), arrow: r.arrow, icon_w, label, label_w, cells, col_w });
            }
            let label_w = rows.iter().map(|r| r.label_w).fold(0.0, f32::max);
            // Each column is as wide as the widest value it may show, so it never resizes as numbers change.
            let col_w: Vec<f32> = b
                .templates
                .iter()
                .enumerate()
                .map(|(i, templates)| {
                    let reserve = templates.iter().map(|t| self.cell(t).map(|c| c.w).unwrap_or(0.0)).fold(0.0, f32::max);
                    rows.iter().filter_map(|r| r.cells.get(i).and_then(|c| c.as_ref()).map(|c| c.w)).fold(reserve, f32::max)
                })
                .collect();
            blocks.push(Block { rows, shared: b.shared, badge, label_w, col_w, width: 0.0 });
        }
        if blocks.is_empty() {
            // Nothing to show yet (or nothing pinned): a single hint line.
            let msg = if self.cfg.pinned.is_empty() { "click to choose metrics" } else { "waiting for sensors" };
            if let (Ok(label), Ok(cell)) = (self.label_layout("TaskbarHWM"), self.cell(msg)) {
                let w = cell.w;
                let label_w = measure(&label);
                blocks.push(Block {
                    rows: vec![GRow { icon: None, arrow: None, icon_w: 0.0, label: Some(label), label_w, cells: vec![Some(cell)], col_w: Vec::new() }],
                    shared: true,
                    badge: false,
                    label_w,
                    col_w: vec![w],
                    width: 0.0,
                });
            }
        }
        let (label_gap, col_gap, block_gap) = (self.label_gap(), self.col_gap(), self.block_gap());
        for blk in &mut blocks {
            let flow = |widths: &[f32]| widths.iter().sum::<f32>() + col_gap * widths.len().saturating_sub(1) as f32;
            let values_w = if blk.shared { flow(&blk.col_w) } else { blk.rows.iter().map(|r| flow(&r.col_w)).fold(0.0, f32::max) };
            blk.width = blk.label_w + label_gap + values_w;
        }
        let total: f32 = blocks.iter().map(|b| b.width).sum::<f32>() + block_gap * blocks.len().saturating_sub(1) as f32;
        self.blocks = blocks;
        self.width_dip = total + PAD_X * 2.0;
        self.width_dip
    }

    /// Makes sure there is one widget window over the taskbar of every display the user chose, and none elsewhere.
    fn sync_windows(&mut self) {
        let all = displays::list();
        let trays = displays::trays();
        let desired: Vec<(String, HWND)> = displays::selected(&self.cfg.widget_displays, &all)
            .into_iter()
            .filter_map(|d| trays.iter().find(|(_, key)| *key == d.key).map(|(tray, _)| (d.key.clone(), *tray)))
            .collect();
        // Drop windows that were destroyed (Explorer restarting takes its taskbar's windows with it) or are no longer wanted.
        let mut i = 0;
        while i < self.wins.len() {
            let alive = unsafe { IsWindow(Some(self.wins[i].hwnd)).as_bool() };
            if alive && desired.iter().any(|(key, _)| *key == self.wins[i].monitor) {
                i += 1;
            } else {
                let w = self.wins.remove(i);
                if alive {
                    unsafe {
                        let _ = DestroyWindow(w.hwnd);
                    }
                }
            }
        }
        for (key, tray) in desired {
            match self.wins.iter_mut().find(|w| w.monitor == key) {
                Some(w) => w.tray = tray,
                None => {
                    if let Some(w) = Win::create(&self.gfx, key, tray) {
                        self.wins.push(w);
                    }
                }
            }
        }
    }

    fn place_all(&mut self) {
        let (width_dip, hidden) = (self.width_dip, self.hidden_fullscreen);
        for w in &mut self.wins {
            place_win(w, width_dip, hidden);
        }
    }

    fn render(&self) {
        for i in 0..self.wins.len() {
            self.render_win(i);
        }
    }

    /// Draws the widget into window `idx`.
    fn render_win(&self, idx: usize) {
        let w = &self.wins[idx];
        let t = self.theme;
        let (hover, pressed) = (w.hover, w.pressed);
        let w_dip = w.surface.width as f32 * 96.0 / w.surface.dpi;
        let h_dip = w.surface.height as f32 * 96.0 / w.surface.dpi;
        let blocks = &self.blocks;
        let (row_h, col_gap, label_gap, block_gap) = (self.row_h(), self.col_gap(), self.label_gap(), self.block_gap());
        let isz = self.icon_size();
        let (val_dy, label_dy, unit_dy) = (self.val_dy, self.label_dy, self.unit_dy);
        let (px, py) = w.pos;
        let _ = w.surface.present(w.hwnd, px, py, |dc| unsafe {
            let Ok(text) = dc.CreateSolidColorBrush(&t.text, None) else { return };
            let Ok(dim) = dc.CreateSolidColorBrush(&t.text_secondary, None) else { return };
            let Ok(badge_fill) = dc.CreateSolidColorBrush(&t.badge_fill, None) else { return };
            let Ok(badge_text) = dc.CreateSolidColorBrush(&t.badge_text, None) else { return };
            // Units: the text color pulled most of the way toward the secondary color.
            let Ok(unit_brush) = dc.CreateSolidColorBrush(&mix(t.text, t.text_secondary, 0.7), None) else { return };
            // Near-invisible fill over the whole area: layered windows are click-through where alpha is 0.
            if let Ok(b) = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 / 255.0 }, None) {
                dc.FillRectangle(&D2D_RECT_F { left: 0.0, top: 0.0, right: w_dip, bottom: h_dip }, &b);
            }
            // Button-style hover/pressed plate, like the other taskbar buttons.
            if hover || pressed {
                let fill = if pressed { t.pressed_fill } else { t.hover_fill };
                if let Ok(b) = dc.CreateSolidColorBrush(&fill, None) {
                    let rect = D2D_RECT_F { left: EDGE, top: 4.0, right: w_dip, bottom: h_dip - 4.0 };
                    dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: CORNER, radiusY: CORNER }, &b);
                }
            }
            let single_line = blocks.len() == 1 && blocks[0].rows.len() == 1;
            let mut x = EDGE + PAD_X;
            for blk in blocks {
                // Every block starts at the first line of a two-row block, so a lone row never floats to the middle and
                // nothing shifts when a second row joins it. Only a widget that is one single row stays centered.
                let lines = if single_line { 1.0 } else { 2.0 };
                let mut y = (h_dip - row_h * lines) / 2.0;
                for row in &blk.rows {
                    // Label badge: a tinted chip, the same width for every row of the block so the chips line up.
                    let (pad, label_brush) = if blk.badge {
                        let chip = D2D_RECT_F { left: x, top: y + 1.0, right: x + blk.label_w, bottom: y + row_h - 1.0 };
                        dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: chip, radiusX: BADGE_RADIUS, radiusY: BADGE_RADIUS }, &badge_fill);
                        (BADGE_PAD, &badge_text)
                    } else {
                        (0.0, &dim)
                    };
                    let mut lx = x + pad;
                    if let Some(section) = row.icon {
                        // Without a badge, an icon that is the row's only label uses the full text color.
                        let brush = if blk.badge || row.label.is_some() { label_brush } else { &text };
                        match row.arrow {
                            // The arrow sits in the middle of its square: shift so that middle is the middle of the narrower slot.
                            Some(a) => super::icons::draw_arrow(dc, brush, a, lx + row.icon_w / 2.0 - isz / 2.0, y + (row_h - isz) / 2.0, isz),
                            None => super::icons::draw(dc, brush, section, lx, y + (row_h - isz) / 2.0, isz),
                        }
                        lx += row.icon_w + if row.label.is_some() { 5.0 } else { 0.0 };
                    }
                    if let Some(label) = &row.label {
                        dc.DrawTextLayout(Vector2 { X: lx, Y: y + label_dy }, label, label_brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                    let mut cx = x + blk.label_w + label_gap;
                    let widths = if blk.shared { &blk.col_w } else { &row.col_w };
                    for (i, col_w) in widths.iter().enumerate() {
                        if let Some(Some(cell)) = row.cells.get(i) {
                            // Right-aligned: digits of different rows line up on their right edge.
                            let x0 = cx + col_w - cell.w;
                            dc.DrawTextLayout(Vector2 { X: x0, Y: y + val_dy }, &cell.num, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
                            if let Some(u) = &cell.unit {
                                dc.DrawTextLayout(Vector2 { X: x0 + u.x, Y: y + val_dy + unit_dy }, &u.layout, &unit_brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                            }
                        }
                        cx += col_w + col_gap;
                    }
                    y += row_h;
                }
                x += blk.width + block_gap;
            }
        });
    }

    /// The window to anchor the flyout to: the one clicked, or the first.
    fn win_for(&self, from: Option<HWND>) -> Option<&Win> {
        from.and_then(|h| self.wins.iter().find(|w| w.hwnd == h)).or_else(|| self.wins.first())
    }

    fn open_flyout(&self, settings: bool, from: Option<HWND>) {
        let Some(w) = self.win_for(from) else { return };
        let scale = w.surface.dpi / 96.0;
        super::flyout::toggle(super::flyout::OpenParams {
            anchor_x: w.pos.0 + (EDGE * scale) as i32,
            taskbar_top: w.pos.1,
            scale,
            widget: self.controller,
            snap: &self.snap,
            theme: self.theme,
            dwrite: &self.gfx.dwrite,
            settings,
        });
    }

    /// Opens the settings page, whether or not the flyout is already open.
    fn open_settings(&self, from: Option<HWND>) {
        let Some(w) = self.win_for(from) else { return };
        let scale = w.surface.dpi / 96.0;
        super::flyout::show_settings(super::flyout::OpenParams {
            anchor_x: w.pos.0 + (EDGE * scale) as i32,
            taskbar_top: w.pos.1,
            scale,
            widget: self.controller,
            snap: &self.snap,
            theme: self.theme,
            dwrite: &self.gfx.dwrite,
            settings: true,
        });
    }

    fn on_snapshot(&mut self) {
        if let Ok(s) = self.shared.lock() {
            self.snap = s.clone();
        }
        super::flyout::update(&self.snap);
        // First launch: open the settings page once so people can see what is configurable.
        if !self.first_run_checked {
            self.first_run_checked = true;
            if !self.cfg.setup_shown {
                let mut c = crate::config::get();
                c.setup_shown = true;
                crate::config::set(c);
                self.cfg = crate::config::get();
                self.open_flyout(true, None);
            }
        }
        let before = self.width_dip;
        self.rebuild();
        if (self.width_dip - before).abs() > 0.5 {
            self.place_all();
        }
        self.render();
    }

    fn on_tick(&mut self) {
        let state = unsafe { SHQueryUserNotificationState() };
        let fullscreen = matches!(state, Ok(s) if s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE)
            && fullscreen_app_in_front();
        self.hidden_fullscreen = fullscreen;
        self.theme = Theme::current();
        // Displays and taskbars come and go; re-asserting position also keeps us above a taskbar that re-raises itself.
        self.sync_windows();
        self.place_all();
        if !fullscreen {
            self.render();
        }
    }
}

/// Puts a widget window back above its taskbar if the taskbar has ended up on top of it.
unsafe fn raise_if_covered(w: &Win) {
    if IsWindowVisible(w.hwnd).as_bool() && !MENU_OPEN.load(Ordering::Relaxed) && is_below(w.hwnd, w.tray) {
        let _ = SetWindowPos(w.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// WinEvent hook: something in the system was raised or reordered.
unsafe extern "system" fn on_z_event(_hook: HWINEVENTHOOK, event: u32, _hwnd: HWND, _object: i32, _child: i32, _thread: u32, _time: u32) {
    APP.with(|cell| {
        // A new foreground window can mean a fullscreen app came or went: check right away instead of at the next tick.
        if event == EVENT_SYSTEM_FOREGROUND {
            if let Ok(mut guard) = cell.try_borrow_mut() {
                if let Some(app) = guard.as_mut() {
                    app.on_tick();
                    return;
                }
            }
        }
        if let Ok(guard) = cell.try_borrow() {
            if let Some(app) = guard.as_ref() {
                for w in &app.wins {
                    raise_if_covered(w);
                }
            }
        }
    });
}

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_OBJECT_REORDER: u32 = 0x8004;

/// Whether an application window is really covering a whole screen (a fullscreen game or video).
///
/// Windows' own "busy" flag is not enough on its own: it also reads as busy while the desktop is in front (after
/// Win+D) and for some dialogs. So the window in front must be a normal application window that fills its monitor.
fn fullscreen_app_in_front() -> bool {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() || !IsWindowVisible(fg).as_bool() || IsIconic(fg).as_bool() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(fg, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return false;
        }
        let mut r = RECT::default();
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetWindowRect(fg, &mut r).is_err() || !GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// Positions a widget window over the left end of its taskbar and resizes its bitmap to match.
fn place_win(w: &mut Win, width_dip: f32, hidden: bool) {
    unsafe {
        if hidden {
            hide_self(w.hwnd);
            return;
        }
        let tray = w.tray;
        let mut r = RECT::default();
        if GetWindowRect(tray, &mut r).is_err() {
            return;
        }
        let (tw, th) = (r.right - r.left, r.bottom - r.top);
        if tw < th {
            // Vertical taskbars aren't supported yet.
            hide_self(w.hwnd);
            return;
        }
        let dpi = GetDpiForWindow(tray).max(96) as f32;
        let scale = dpi / 96.0;
        let w_px = ((width_dip + EDGE) * scale).ceil() as i32;
        w.pos = (r.left, r.top);
        let _ = w.surface.resize(w_px as u32, th as u32, dpi);
        // Size and position are applied by UpdateLayeredWindow when drawing; the rest keeps us shown and on top.
        //
        // The taskbar owns us: Windows keeps an owned window directly above its owner, so when the taskbar
        // raises itself (a right-click on it, an app opening from it) we are raised in the same step instead of
        // staying hidden until the next tick. It also means we never have to push ourselves above other
        // topmost windows, such as menus and jump lists, just to be seen.
        if GetWindowLongPtrW(w.hwnd, GWLP_HWNDPARENT) != tray.0 as isize {
            SetWindowLongPtrW(w.hwnd, GWLP_HWNDPARENT, tray.0 as isize);
        }
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW;
        if !IsWindowVisible(w.hwnd).as_bool() || (!MENU_OPEN.load(Ordering::Relaxed) && is_below(w.hwnd, tray)) {
            // Only when the taskbar has actually ended up above us (and no menu of ours is open): never over a menu.
            let _ = SetWindowPos(w.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags);
        } else {
            let _ = SetWindowPos(w.hwnd, None, 0, 0, 0, 0, flags | SWP_NOZORDER);
        }
    }
}

fn spawn_sampler(hwnd: HWND, shared: Arc<Mutex<Snapshot>>) {
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let mut sampler = Sampler::new();
        let mut n: u32 = 0;
        loop {
            let snap = sampler.sample();
            n = n.wrapping_add(1);
            // Once after init (NVML etc. leave a lot of cold pages), then occasionally.
            if n == 3 || n % 60 == 0 {
                crate::sensors::trim_working_set();
            }
            if let Ok(mut s) = shared.lock() {
                *s = snap;
            }
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut c_void)), WM_SNAPSHOT, WPARAM(0), LPARAM(0));
            }
            std::thread::sleep(Duration::from_millis(crate::config::INTERVAL_MS.load(Ordering::Relaxed) as u64));
        }
    });
}

/// Lets this app's menus follow the system's light or dark mode (they are always light otherwise).
/// Uses the two uxtheme exports Windows' own shell uses for this (by ordinal: SetPreferredAppMode, FlushMenuThemes);
/// if they are missing the menu just stays light.
fn allow_dark_menus() {
    use windows::core::PCSTR;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(ux) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        let set_mode = GetProcAddress(ux, PCSTR(135 as *const u8));
        let flush = GetProcAddress(ux, PCSTR(136 as *const u8));
        if let (Some(set_mode), Some(flush)) = (set_mode, flush) {
            let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            set_mode(1); // APPMODE_ALLOWDARK: follow the system setting
            flush();
        }
    });
}

/// Ends the process right away.
///
/// Returning from `main` makes Windows run its normal process shutdown, which crashed every time (exit code
/// 0xC000000D from inside ntdll) while the libraries this app loads (the GPU vendors' DLLs and the PawnIO helper) were
/// being detached. Nothing here needs an orderly shutdown: settings are saved as they change and the windows go away
/// with the process, so it simply terminates itself.
fn quit_now() -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    std::process::exit(0)
}

/// Set while this app is hiding its own widget windows on purpose.
static SELF_HIDE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hides one of our widget windows, bypassing the guard against the system hiding it.
unsafe fn hide_self(hwnd: HWND) {
    SELF_HIDE.store(true, Ordering::Relaxed);
    let _ = ShowWindow(hwnd, SW_HIDE);
    SELF_HIDE.store(false, Ordering::Relaxed);
}

/// True while the right-click menu is on screen, so the widget doesn't re-raise itself over it.
static MENU_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether `other` is above `window` in the z-order.
fn is_below(window: HWND, other: HWND) -> bool {
    unsafe {
        let mut w = GetWindow(window, GW_HWNDPREV).ok();
        for _ in 0..4096 {
            match w {
                Some(h) if h == other => return true,
                Some(h) => w = GetWindow(h, GW_HWNDPREV).ok(),
                None => return false,
            }
        }
        false
    }
}

const MENU_SETTINGS: usize = 1;
const MENU_EXIT: usize = 2;

fn show_context_menu(hwnd: HWND) {
    allow_dark_menus();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("Settings"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Open the menu above the widget instead of over it (bottom edge at the widget's top edge).
        let mut wr = RECT::default();
        let y = if GetWindowRect(hwnd, &mut wr).is_ok() { wr.top } else { pt.y };
        let _ = SetForegroundWindow(hwnd);
        MENU_OPEN.store(true, Ordering::Relaxed);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON, pt.x, y, None, hwnd, None);
        MENU_OPEN.store(false, Ordering::Relaxed);
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            MENU_SETTINGS => {
                // Not opened here: the menu ran inside this message, and the flyout is driven from the app's own handler.
                let controller = HWND(CONTROLLER.load(Ordering::Relaxed) as *mut c_void);
                let _ = PostMessageW(Some(controller), WM_OPEN_SETTINGS, WPARAM(hwnd.0 as usize), LPARAM(0));
            }
            MENU_EXIT => quit_now(),
            _ => {}
        }
    }
}

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// The hidden controller window; closing it ends the app (the visible widget windows can come and go).
static CONTROLLER: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        // Windows hides every window it can on Win+D ("show desktop"); the widget should stay, so that hide is cancelled
        // as it happens (hiding it ourselves, for a fullscreen app, is the only hide that goes through).
        WM_WINDOWPOSCHANGING if !SELF_HIDE.load(Ordering::Relaxed) => {
            let pos = &mut *(lp.0 as *mut WINDOWPOS);
            if pos.flags.contains(SWP_HIDEWINDOW) {
                pos.flags &= !SWP_HIDEWINDOW;
            }
        }
        // Handled outside the APP borrow: the menu runs a nested message loop.
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            return LRESULT(0);
        }
        // Asked to close (for example by the installer, before it replaces the app): that means the whole app.
        WM_CLOSE => quit_now(),
        WM_DESTROY => {
            if hwnd.0 as isize == CONTROLLER.load(Ordering::Relaxed) {
                quit_now();
            }
            return LRESULT(0);
        }
        _ => {}
    }
    let handled = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return false };
        let Some(app) = guard.as_mut() else { return false };
        // Which widget window a mouse message is for (the controller has none).
        let idx = app.wins.iter().position(|w| w.hwnd == hwnd);
        match msg {
            WM_SNAPSHOT => app.on_snapshot(),
            WM_OPEN_SETTINGS => app.open_settings(Some(HWND(wp.0 as *mut c_void))),
            crate::shell::setup::WM_SETUP => super::flyout::update(&app.snap),
            WM_CONFIG => {
                app.cfg = crate::config::get();
                app.sync_windows();
                app.rebuild();
                app.place_all();
                app.render();
            }
            WM_TIMER if wp.0 == TIMER_TICK => app.on_tick(),
            WM_MOUSEMOVE => {
                let Some(i) = idx else { return false };
                if !app.wins[i].hover {
                    app.wins[i].hover = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    app.render_win(i);
                }
            }
            WM_MOUSELEAVE => {
                let Some(i) = idx else { return false };
                app.wins[i].hover = false;
                app.wins[i].pressed = false;
                app.render_win(i);
            }
            WM_LBUTTONDOWN => {
                let Some(i) = idx else { return false };
                app.wins[i].pressed = true;
                app.render_win(i);
            }
            WM_LBUTTONUP => {
                let Some(i) = idx else { return false };
                let was_pressed = app.wins[i].pressed;
                app.wins[i].pressed = false;
                app.render_win(i);
                if was_pressed {
                    app.open_flyout(false, Some(hwnd));
                }
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                app.theme = Theme::current();
                app.sync_windows();
                app.place_all();
                app.render();
            }
            m if m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                app.sync_windows();
                app.place_all();
                app.render();
            }
            _ => return false,
        }
        true
    });
    if handled {
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn run(cfg: Config) -> Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("TaskbarHWM.Widget");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        // The hidden controller window (never shown): it owns the timer and receives the app's own messages.
        let controller = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!("TaskbarHWM.Controller"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None)?;
        CONTROLLER.store(controller.0 as isize, Ordering::Relaxed);

        let gfx = Gfx::new()?;
        let fsize = cfg.font_size;
        let fmt = make_format(&gfx.dwrite, fsize, DWRITE_FONT_WEIGHT_NORMAL)?;
        let label_fmt = make_format(&gfx.dwrite, label_size(fsize), DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
        let unit_fmt = make_format(&gfx.dwrite, unit_size(fsize), DWRITE_FONT_WEIGHT_NORMAL)?;

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let mut app = App {
            gfx,
            controller,
            wins: Vec::new(),
            cfg,
            shared: shared.clone(),
            snap: Snapshot::default(),
            theme: Theme::current(),
            fmt,
            fmt_size: fsize,
            label_fmt,
            unit_fmt,
            unit_dy: 0.0,
            val_dy: 0.0,
            label_dy: 0.0,
            blocks: Vec::new(),
            width_dip: 120.0,
            hidden_fullscreen: false,
            first_run_checked: false,
        };
        app.sync_windows();
        app.rebuild();
        app.place_all();
        app.render();
        APP.with(|c| *c.borrow_mut() = Some(app));

        SetTimer(Some(controller), TIMER_TICK, 1000, None);
        // The taskbar jumps above the widget whenever it is raised (Win+D, clicking it, opening Start); the tick would
        // put the widget back a moment later, which shows as a flicker. These events let it go back right away.
        const WINEVENT_OUTOFCONTEXT: u32 = 0;
        for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_REORDER] {
            let _ = SetWinEventHook(event, event, None, Some(on_z_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        }
        spawn_sampler(controller, shared);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
