use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub interval_ms: u32,
    pub fahrenheit: bool,
    /// Flyout tint opacity (0..=255). Lower is more see-through.
    pub tint: u32,
    /// Taskbar widget text size in DIPs (9..=15).
    pub font_size: f32,
    /// Space between the widget's value columns in DIPs (2..=24). Label and block gaps scale with it.
    pub gap: f32,
    /// How rows are labelled in the taskbar widget: "icon", "text" or "both".
    pub label_style: String,
    /// Draw the taskbar row labels inside a colored badge so they stand apart from the values.
    pub label_badge: bool,
    /// Disks ("C:") hidden from the flyout. Everything connected is shown unless listed here.
    pub disks_hidden: Vec<String>,
    /// Disks pinned to the taskbar widget (each becomes a row).
    pub disks_pinned: Vec<String>,
    /// How disk space is shown: "used_total", "percent" or "free".
    pub disk_style: String,
    /// "combined": all disks in one Disks section; "split": every disk is its own section.
    pub disk_layout: String,
    /// Set once the first-run welcome (settings page) has been shown.
    pub setup_shown: bool,
    /// Bumped when a release changes what saved settings mean, so one-time migrations run once.
    pub schema: u32,
    /// Metric ids pinned to the taskbar widget (kept in catalog order).
    pub pinned: Vec<String>,
    /// Order of the taskbar widget's rows ("cpu", "gpu", "ram").
    pub row_order: Vec<String>,
    /// Order of the taskbar widget's columns (metric kinds, e.g. "usage", "temp").
    pub col_order: Vec<String>,
    /// Metric ids shown in the flyout cards (kept in catalog order).
    pub flyout: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect();
        Config {
            interval_ms: 1000,
            fahrenheit: false,
            tint: 150,
            font_size: 12.0,
            gap: 8.0,
            label_style: "both".to_string(),
            label_badge: true,
            disks_hidden: Vec::new(),
            disks_pinned: Vec::new(),
            disk_style: "used_total".to_string(),
            disk_layout: "combined".to_string(),
            setup_shown: false,
            schema: 0,
            pinned: s(&["cpu.usage", "cpu.temp", "gpu.usage", "gpu.temp"]),
            row_order: crate::metrics::ROW_KEYS.iter().map(|k| k.to_string()).collect(),
            col_order: crate::metrics::COL_KEYS.iter().map(|k| k.to_string()).collect(),
            flyout: s(&[
                "cpu.usage",
                "cpu.clock",
                "cpu.temp",
                "cpu.power",
                "gpu.usage",
                "gpu.temp",
                "gpu.power",
                "gpu.core_clock",
                "gpu.mem_clock",
                "gpu.vram",
                "ram.usage",
                "ram.used",
            ]),
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("TaskbarHWM").join("config.toml"))
}

/// Keeps only valid, unique keys (in their current order) and appends any that are missing.
fn fix_order(list: &mut Vec<String>, valid: &[&str]) {
    let mut seen: Vec<String> = Vec::new();
    list.retain(|k| {
        let ok = valid.contains(&k.as_str()) && !seen.contains(k);
        if ok {
            seen.push(k.clone());
        }
        ok
    });
    for v in valid {
        if !list.iter().any(|k| k == v) {
            list.push(v.to_string());
        }
    }
}

impl Config {
    /// Repairs hand-edited or older config files (unknown, duplicate or missing row/column keys).
    pub fn normalize(&mut self) {
        if !matches!(self.disk_layout.as_str(), "combined" | "split") {
            self.disk_layout = "combined".to_string();
        }
        if !matches!(self.disk_style.as_str(), "used_total" | "percent" | "free") {
            self.disk_style = "used_total".to_string();
        }
        if !matches!(self.label_style.as_str(), "icon" | "text" | "both") {
            self.label_style = "both".to_string();
        }
        self.font_size = if self.font_size.is_finite() { self.font_size.round().clamp(9.0, 15.0) } else { 12.0 };
        self.gap = if self.gap.is_finite() { self.gap.round().clamp(2.0, 24.0) } else { 8.0 };
        fix_order(&mut self.row_order, crate::metrics::ROW_KEYS);
        fix_order(&mut self.col_order, crate::metrics::COL_KEYS);
    }

    pub fn load() -> Config {
        let mut c: Config = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        c.normalize();
        if c.schema < 1 {
            // RAM's "In use" row used to show "x / y GB"; it is now "Usage" (percent) plus a separate "Used".
            // Keep the amount visible in the flyout for people who had the old row on.
            if c.flyout.iter().any(|i| i == "ram.usage") && !c.flyout.iter().any(|i| i == "ram.used") {
                c.flyout.push("ram.used".to_string());
                c.flyout.sort_by_key(|i| crate::metrics::order(i));
            }
            c.schema = 1;
        }
        c
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Ok(t) = toml::to_string_pretty(self) {
                let _ = std::fs::write(p, t);
            }
        }
    }
}

// ---- Shared live config: the flyout edits it, the widget and sampler thread read it. ----

static CURRENT: OnceLock<Mutex<Config>> = OnceLock::new();
/// Sampling interval, readable from the sampler thread without locking.
pub static INTERVAL_MS: AtomicU32 = AtomicU32::new(1000);

fn clamp_interval(ms: u32) -> u32 {
    ms.clamp(250, 10_000)
}

/// Loads the config from disk and makes it the live config.
pub fn init() -> Config {
    let c = Config::load();
    INTERVAL_MS.store(clamp_interval(c.interval_ms), Ordering::Relaxed);
    let _ = CURRENT.set(Mutex::new(c.clone()));
    c
}

pub fn get() -> Config {
    CURRENT.get().and_then(|m| m.lock().ok().map(|c| c.clone())).unwrap_or_default()
}

/// Replaces the live config, applies the interval, and saves it to disk.
pub fn set(c: Config) {
    INTERVAL_MS.store(clamp_interval(c.interval_ms), Ordering::Relaxed);
    c.save();
    if let Some(m) = CURRENT.get() {
        if let Ok(mut g) = m.lock() {
            *g = c;
        }
    }
}
