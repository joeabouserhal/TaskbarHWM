//! The taskbar widget: a transparent topmost window drawn over the far-left of the taskbar.
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_numerics::Vector2;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::gfx::{Gfx, Surface};
use super::theme::Theme;
use crate::config::Config;
use crate::metrics;
use crate::sensors::{Sampler, Snapshot};

const WM_SNAPSHOT: u32 = WM_APP + 1;
/// Posted by the flyout when the user changed a setting.
pub const WM_CONFIG: u32 = WM_APP + 2;
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

/// A value, measured so it can be right-aligned in its column.
struct Cell {
    layout: IDWriteTextLayout,
    w: f32,
}

struct GRow {
    /// Section whose icon leads the row (None for the text-only hint row).
    icon: Option<metrics::Section>,
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

struct App {
    gfx: Gfx,
    surface: Surface,
    hwnd: HWND,
    cfg: Config,
    shared: Arc<Mutex<Snapshot>>,
    snap: Snapshot,
    theme: Theme,
    fmt: IDWriteTextFormat,
    /// Size `fmt` was created at; recreated when the setting changes.
    fmt_size: f32,
    /// Semibold, slightly smaller: row labels must not read like the values.
    label_fmt: IDWriteTextFormat,
    /// Vertical nudges (DIPs) that put the cap-height midline of values / labels on the row's midline.
    val_dy: f32,
    label_dy: f32,
    blocks: Vec<Block>,
    width_dip: f32,
    pos: (i32, i32),
    hover: bool,
    pressed: bool,
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
            if let (Ok(f), Ok(l)) = (
                make_format(dw, self.cfg.font_size, DWRITE_FONT_WEIGHT_NORMAL),
                make_format(dw, label_size(self.cfg.font_size), DWRITE_FONT_WEIGHT_SEMI_BOLD),
            ) {
                self.fmt = f;
                self.label_fmt = l;
                self.fmt_size = self.cfg.font_size;
            }
        }
    }

    /// Rebuilds the grid (text layouts and column widths) from the latest snapshot. Returns the widget width in DIPs.
    fn rebuild(&mut self) -> f32 {
        self.ensure_font();
        self.val_dy = self.cap_dy(&self.fmt, self.cfg.font_size);
        self.label_dy = self.cap_dy(&self.label_fmt, label_size(self.cfg.font_size));
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
                let label_w = if want_icon { isz } else { 0.0 }
                    + if want_icon && want_text { 5.0 } else { 0.0 }
                    + text_w
                    + if badge { 2.0 * BADGE_PAD } else { 0.0 };
                let cells = r
                    .cells
                    .iter()
                    .map(|c| {
                        c.as_ref().and_then(|t| {
                            let layout = self.layout(t).ok()?;
                            let w = measure(&layout);
                            Some(Cell { layout, w })
                        })
                    })
                    .collect();
                // Rows that don't share columns reserve room per value, so each row's spacing is uniform and stable.
                let col_w: Vec<f32> = if b.shared {
                    Vec::new()
                } else {
                    r.templates
                        .iter()
                        .zip(&cells)
                        .map(|(t, c): (&&str, &Option<Cell>)| {
                            let reserve = self.layout(t).map(|l| measure(&l)).unwrap_or(0.0);
                            c.as_ref().map_or(reserve, |c| reserve.max(c.w))
                        })
                        .collect()
                };
                rows.push(GRow { icon: want_icon.then_some(r.section), label, label_w, cells, col_w });
            }
            let label_w = rows.iter().map(|r| r.label_w).fold(0.0, f32::max);
            // Each column is as wide as the widest value it may show, so it never resizes as numbers change.
            let col_w: Vec<f32> = b
                .templates
                .iter()
                .enumerate()
                .map(|(i, templates)| {
                    let reserve = templates.iter().map(|t| self.layout(t).map(|l| measure(&l)).unwrap_or(0.0)).fold(0.0, f32::max);
                    rows.iter().filter_map(|r| r.cells.get(i).and_then(|c| c.as_ref()).map(|c| c.w)).fold(reserve, f32::max)
                })
                .collect();
            blocks.push(Block { rows, shared: b.shared, badge, label_w, col_w, width: 0.0 });
        }
        if blocks.is_empty() {
            // Nothing to show yet (or nothing pinned): a single hint line.
            let msg = if self.cfg.pinned.is_empty() { "click to choose metrics" } else { "waiting for sensors" };
            if let (Ok(label), Ok(layout)) = (self.label_layout("TaskbarHWM"), self.layout(msg)) {
                let w = measure(&layout);
                let label_w = measure(&label);
                blocks.push(Block {
                    rows: vec![GRow { icon: None, label: Some(label), label_w, cells: vec![Some(Cell { layout, w })], col_w: Vec::new() }],
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

    /// Positions the window over the left end of the taskbar and resizes the bitmap to match.
    fn place(&mut self) {
        unsafe {
            if self.hidden_fullscreen {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), None) else { return };
            let mut r = RECT::default();
            if GetWindowRect(tray, &mut r).is_err() {
                return;
            }
            let (tw, th) = (r.right - r.left, r.bottom - r.top);
            if tw < th {
                // Vertical taskbars aren't supported yet.
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            let dpi = GetDpiForWindow(tray).max(96) as f32;
            let scale = dpi / 96.0;
            let w_px = ((self.width_dip + EDGE) * scale).ceil() as i32;
            self.pos = (r.left, r.top);
            let _ = self.surface.resize(w_px as u32, th as u32, dpi);
            // Size and position are applied by UpdateLayeredWindow in render(); this keeps us shown and on top.
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    fn render(&mut self) {
        let t = self.theme;
        let (hover, pressed) = (self.hover, self.pressed);
        let w_dip = self.surface.width as f32 * 96.0 / self.surface.dpi;
        let h_dip = self.surface.height as f32 * 96.0 / self.surface.dpi;
        let blocks = &self.blocks;
        let (row_h, col_gap, label_gap, block_gap) = (self.row_h(), self.col_gap(), self.label_gap(), self.block_gap());
        let isz = self.icon_size();
        let (val_dy, label_dy) = (self.val_dy, self.label_dy);
        let (px, py) = self.pos;
        let _ = self.surface.present(self.hwnd, px, py, |dc| unsafe {
            let Ok(text) = dc.CreateSolidColorBrush(&t.text, None) else { return };
            let Ok(dim) = dc.CreateSolidColorBrush(&t.text_secondary, None) else { return };
            let Ok(badge_fill) = dc.CreateSolidColorBrush(&t.badge_fill, None) else { return };
            let Ok(badge_text) = dc.CreateSolidColorBrush(&t.badge_text, None) else { return };
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
                        super::icons::draw(dc, brush, section, lx, y + (row_h - isz) / 2.0, isz);
                        lx += isz + if row.label.is_some() { 5.0 } else { 0.0 };
                    }
                    if let Some(label) = &row.label {
                        dc.DrawTextLayout(Vector2 { X: lx, Y: y + label_dy }, label, label_brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                    }
                    let mut cx = x + blk.label_w + label_gap;
                    let widths = if blk.shared { &blk.col_w } else { &row.col_w };
                    for (i, col_w) in widths.iter().enumerate() {
                        if let Some(Some(cell)) = row.cells.get(i) {
                            // Right-aligned: digits of different rows line up on their right edge.
                            dc.DrawTextLayout(Vector2 { X: cx + col_w - cell.w, Y: y + val_dy }, &cell.layout, &text, D2D1_DRAW_TEXT_OPTIONS_NONE);
                        }
                        cx += col_w + col_gap;
                    }
                    y += row_h;
                }
                x += blk.width + block_gap;
            }
        });
    }

    fn open_flyout(&self, settings: bool) {
        let scale = self.surface.dpi / 96.0;
        super::flyout::toggle(super::flyout::OpenParams {
            anchor_x: self.pos.0 + (EDGE * scale) as i32,
            taskbar_top: self.pos.1,
            scale,
            widget: self.hwnd,
            snap: &self.snap,
            theme: self.theme,
            dwrite: &self.gfx.dwrite,
            settings,
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
                self.open_flyout(true);
            }
        }
        let before = self.width_dip;
        self.rebuild();
        if (self.width_dip - before).abs() > 0.5 {
            self.place();
        }
        self.render();
    }

    fn on_tick(&mut self) {
        let state = unsafe { SHQueryUserNotificationState() };
        let fullscreen = matches!(state, Ok(s) if s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE);
        self.hidden_fullscreen = fullscreen;
        self.theme = Theme::current();
        // Re-asserting position also keeps us above the taskbar, which re-raises itself when clicked.
        self.place();
        if !fullscreen {
            self.render();
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

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, 1, w!("Exit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        if cmd.0 == 1 {
            PostQuitMessage(0);
        }
    }
}

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        // Handled outside the APP borrow: the menu runs a nested message loop.
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    let handled = APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return false };
        let Some(app) = guard.as_mut() else { return false };
        match msg {
            WM_SNAPSHOT => app.on_snapshot(),
            crate::shell::setup::WM_SETUP => super::flyout::update(&app.snap),
            WM_CONFIG => {
                app.cfg = crate::config::get();
                app.rebuild();
                app.place();
                app.render();
            }
            WM_TIMER if wp.0 == TIMER_TICK => app.on_tick(),
            WM_MOUSEMOVE => {
                if !app.hover {
                    app.hover = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    app.render();
                }
            }
            WM_MOUSELEAVE => {
                app.hover = false;
                app.pressed = false;
                app.render();
            }
            WM_LBUTTONDOWN => {
                app.pressed = true;
                app.render();
            }
            WM_LBUTTONUP => {
                let was_pressed = app.pressed;
                app.pressed = false;
                app.render();
                if was_pressed {
                    app.open_flyout(false);
                }
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                app.theme = Theme::current();
                app.place();
                app.render();
            }
            m if m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                app.place();
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

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class,
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
        )?;

        let gfx = Gfx::new()?;
        let surface = Surface::new(&gfx, 200, 48, 96.0)?;
        let fsize = cfg.font_size;
        let fmt = make_format(&gfx.dwrite, fsize, DWRITE_FONT_WEIGHT_NORMAL)?;
        let label_fmt = make_format(&gfx.dwrite, label_size(fsize), DWRITE_FONT_WEIGHT_SEMI_BOLD)?;

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let mut app = App {
            gfx,
            surface,
            hwnd,
            cfg,
            shared: shared.clone(),
            snap: Snapshot::default(),
            theme: Theme::current(),
            fmt,
            fmt_size: fsize,
            label_fmt,
            val_dy: 0.0,
            label_dy: 0.0,
            blocks: Vec::new(),
            width_dip: 120.0,
            pos: (0, 0),
            hover: false,
            pressed: false,
            hidden_fullscreen: false,
            first_run_checked: false,
        };
        app.rebuild();
        app.place();
        app.render();
        APP.with(|c| *c.borrow_mut() = Some(app));

        SetTimer(Some(hwnd), TIMER_TICK, 1000, None);
        spawn_sampler(hwnd, shared);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
