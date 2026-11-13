//! Catalog of every metric the user can show, and how to format it from a Snapshot.
use crate::config::Config;
use crate::sensors::disk::DiskStats;
use crate::sensors::gpu::GpuStats;
use crate::sensors::Snapshot;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Cpu,
    Gpu,
    Ram,
    Disk,
    Net,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Cpu => "CPU",
            Section::Gpu => "GPU",
            Section::Ram => "RAM",
            Section::Disk => "Disks",
            Section::Net => "Net",
        }
    }

    /// Stable id used in the config's row order.
    pub fn key(self) -> &'static str {
        match self {
            Section::Cpu => "cpu",
            Section::Gpu => "gpu",
            Section::Ram => "ram",
            Section::Disk => "disk",
            Section::Net => "net",
        }
    }

    pub fn from_key(k: &str) -> Option<Section> {
        [Section::Cpu, Section::Gpu, Section::Ram, Section::Disk, Section::Net].into_iter().find(|s| s.key() == k)
    }
}

/// What a metric measures. In the taskbar grid, each kind is a column, so a CPU value and the matching
/// GPU value line up underneath each other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Usage,
    Clock,
    MemClock,
    Temp,
    Power,
    Memory,
    Fan,
    Disk,
    /// Network throughput. Not a taskbar column: the Net row lays its values out by position, like the disks.
    Net,
}

impl Kind {
    pub const ALL: [Kind; 9] = [Kind::Usage, Kind::Clock, Kind::Temp, Kind::Power, Kind::Memory, Kind::MemClock, Kind::Fan, Kind::Disk, Kind::Net];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Usage => "usage",
            Kind::Clock => "clock",
            Kind::MemClock => "memclock",
            Kind::Temp => "temp",
            Kind::Power => "power",
            Kind::Memory => "vram", // key kept from when this column was GPU-only, so saved layouts still load
            Kind::Fan => "fan",
            Kind::Disk => "disk",
            Kind::Net => "net",
        }
    }

    pub fn from_key(k: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|c| c.key() == k)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Usage => "Usage",
            Kind::Clock => "Clock",
            Kind::MemClock => "Mem clock",
            Kind::Temp => "Temp",
            Kind::Power => "Power",
            Kind::Memory => "Memory",
            Kind::Fan => "Fan",
            Kind::Disk => "Disk",
            Kind::Net => "Network",
        }
    }

    /// The widest value this column realistically shows. Columns reserve this much room so the widget never
    /// changes size (or shifts its columns) as the numbers change.
    pub fn template(self, fahrenheit: bool, disk_style: &str) -> &'static str {
        match self {
            Kind::Usage | Kind::Fan => "100%",
            Kind::Clock => "9.99 GHz",
            Kind::MemClock => "99.99 GHz",
            Kind::Temp => {
                if fahrenheit {
                    "199\u{00B0}F"
                } else {
                    "100\u{00B0}C"
                }
            }
            Kind::Power => "999 W",
            Kind::Memory => "99.9 / 999 GB",
            Kind::Disk => disk_template(disk_style, false),
            Kind::Net => "99.9 MB/s",
        }
    }
}

pub const ROW_KEYS: &[&str] = &["cpu", "gpu", "ram", "disk", "net"];
pub const COL_KEYS: &[&str] = &["usage", "clock", "temp", "power", "vram", "memclock", "fan", "disk"];

pub struct MetricDef {
    pub id: &'static str,
    pub section: Section,
    /// Long name used in the flyout and settings.
    pub name: &'static str,
    /// Which taskbar column this metric belongs to.
    pub kind: Kind,
}

pub const METRICS: &[MetricDef] = &[
    MetricDef { id: "cpu.usage", section: Section::Cpu, name: "Usage", kind: Kind::Usage },
    MetricDef { id: "cpu.clock", section: Section::Cpu, name: "Clock", kind: Kind::Clock },
    MetricDef { id: "cpu.temp", section: Section::Cpu, name: "Temperature", kind: Kind::Temp },
    MetricDef { id: "cpu.power", section: Section::Cpu, name: "Power", kind: Kind::Power },
    MetricDef { id: "gpu.usage", section: Section::Gpu, name: "Usage", kind: Kind::Usage },
    MetricDef { id: "gpu.temp", section: Section::Gpu, name: "Temperature", kind: Kind::Temp },
    MetricDef { id: "gpu.power", section: Section::Gpu, name: "Power", kind: Kind::Power },
    MetricDef { id: "gpu.core_clock", section: Section::Gpu, name: "Core clock", kind: Kind::Clock },
    MetricDef { id: "gpu.mem_clock", section: Section::Gpu, name: "Memory clock", kind: Kind::MemClock },
    MetricDef { id: "gpu.vram", section: Section::Gpu, name: "VRAM", kind: Kind::Memory },
    MetricDef { id: "gpu.fan", section: Section::Gpu, name: "Fan", kind: Kind::Fan },
    MetricDef { id: "ram.usage", section: Section::Ram, name: "Usage", kind: Kind::Usage },
    MetricDef { id: "ram.used", section: Section::Ram, name: "Used", kind: Kind::Memory },
    MetricDef { id: "net.down", section: Section::Net, name: "Download", kind: Kind::Net },
    MetricDef { id: "net.up", section: Section::Net, name: "Upload", kind: Kind::Net },
];

/// Position in the catalog; used to keep user-selected lists in a stable order.
pub fn order(id: &str) -> usize {
    METRICS.iter().position(|m| m.id == id).unwrap_or(usize::MAX)
}

pub fn def(id: &str) -> Option<&'static MetricDef> {
    METRICS.iter().find(|m| m.id == id)
}

/// GPUs on the taskbar: the ones the user chose, or the first (discrete) card if they chose none.
pub fn taskbar_gpus<'a>(cfg: &Config, snap: &'a Snapshot) -> Vec<&'a GpuStats> {
    let chosen: Vec<&GpuStats> = snap.gpus.iter().filter(|g| cfg.gpus_pinned.contains(&g.key)).collect();
    if chosen.is_empty() {
        snap.gpus.first().into_iter().collect()
    } else {
        chosen
    }
}

/// GPUs in the flyout: all of them except the ones the user hid.
pub fn flyout_gpus<'a>(cfg: &Config, snap: &'a Snapshot) -> Vec<&'a GpuStats> {
    snap.gpus.iter().filter(|g| !cfg.gpus_hidden.contains(&g.key)).collect()
}

/// "GPU" when a single card is shown, otherwise "GPU1", "GPU2", ... in display order.
pub fn gpu_label(shown: usize, index: usize) -> String {
    if shown <= 1 {
        "GPU".to_string()
    } else {
        format!("GPU{}", index + 1)
    }
}

fn temp(c: f32, cfg: &Config) -> String {
    if cfg.fahrenheit {
        format!("{:.0}\u{00B0}F", c * 9.0 / 5.0 + 32.0)
    } else {
        format!("{c:.0}\u{00B0}C")
    }
}

fn clock(mhz: f32) -> String {
    if mhz >= 1000.0 {
        format!("{:.2} GHz", mhz / 1000.0)
    } else {
        format!("{mhz:.0} MHz")
    }
}

fn gib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// Splits a formatted value into its number part and its unit, so the unit can be drawn smaller and muted:
/// "4.49 GHz" -> ("4.49", "GHz"), "45%" -> ("45", "%"), "12.2 / 31 GB" -> ("12.2 / 31", "GB"), "C: 67%" -> ("C: 67", "%").
/// The flag says whether a space separated them. Text with no digits ("n/a") has no unit.
pub fn split_unit(text: &str) -> (&str, Option<(&str, bool)>) {
    let Some(last_digit) = text.rfind(|c: char| c.is_ascii_digit()) else { return (text, None) };
    let (num, rest) = text.split_at(last_digit + 1);
    let unit = rest.trim();
    if unit.is_empty() {
        (num, None)
    } else {
        (num, Some((unit, rest.starts_with(char::is_whitespace))))
    }
}

/// A transfer rate. "bytes": KB/s, MB/s, GB/s (1024-based, like file sizes). "bits": Kbps, Mbps, Gbps (1000-based,
/// like link speeds). Each unit switches over just before it would need a fourth digit.
fn rate(bytes_per_s: f32, unit: &str) -> String {
    if unit == "bits" {
        let bits = bytes_per_s * 8.0;
        if bits >= 999.5e6 {
            format!("{:.2} Gbps", bits / 1e9)
        } else if bits >= 99.95e6 {
            format!("{:.0} Mbps", bits / 1e6)
        } else if bits >= 999.5e3 {
            format!("{:.1} Mbps", bits / 1e6)
        } else {
            format!("{:.0} Kbps", bits / 1e3)
        }
    } else {
        const KB: f32 = 1024.0;
        const MB: f32 = KB * 1024.0;
        const GB: f32 = MB * 1024.0;
        if bytes_per_s >= 999.5 * MB {
            format!("{:.2} GB/s", bytes_per_s / GB)
        } else if bytes_per_s >= 99.95 * MB {
            format!("{:.0} MB/s", bytes_per_s / MB)
        } else if bytes_per_s >= 999.5 * KB {
            format!("{:.1} MB/s", bytes_per_s / MB)
        } else {
            format!("{:.0} KB/s", bytes_per_s / KB)
        }
    }
}

/// The widest network value, for reserving column room so the widget doesn't resize as speeds change.
fn net_template(unit: &str) -> &'static str {
    if unit == "bits" { "99.9 Mbps" } else { "99.9 MB/s" }
}

/// Formatted value for a CPU, RAM or network metric, or None when the sensor isn't available (the UI hides it).
pub fn value(id: &str, snap: &Snapshot, cfg: &Config) -> Option<String> {
    Some(match id {
        "cpu.usage" => format!("{:.0}%", snap.cpu.usage_percent?),
        "cpu.clock" => clock(snap.cpu.clock_mhz?),
        "cpu.temp" => temp(snap.cpu.temp_c?, cfg),
        "cpu.power" => format!("{:.0} W", snap.cpu.power_w?),
        "ram.usage" => format!("{:.0}%", snap.ram?.percent),
        "net.down" => rate(snap.net.total(&cfg.net_adapters).0, &cfg.net_unit),
        "net.up" => rate(snap.net.total(&cfg.net_adapters).1, &cfg.net_unit),
        "ram.used" => {
            let r = snap.ram?;
            format!("{:.1} / {:.0} GB", gib(r.used_bytes), gib(r.total_bytes))
        }
        _ => return None,
    })
}

/// Formatted value of a GPU metric for one specific GPU.
pub fn gpu_value(id: &str, g: &GpuStats, cfg: &Config) -> Option<String> {
    Some(match id {
        "gpu.usage" => format!("{:.0}%", g.usage_percent?),
        "gpu.temp" => temp(g.temp_c?, cfg),
        "gpu.power" => format!("{:.0} W", g.power_w?),
        "gpu.core_clock" => clock(g.core_mhz?),
        "gpu.mem_clock" => clock(g.mem_mhz?),
        "gpu.vram" => {
            let (used, total) = (gib(g.vram_used?), gib(g.vram_total?));
            // Integrated graphics reserve well under a gigabyte: keep a decimal so it doesn't read "0 GB".
            if total < 10.0 { format!("{used:.1} / {total:.1} GB") } else { format!("{used:.1} / {total:.0} GB") }
        }
        "gpu.fan" => format!("{:.0}%", g.fan_percent?),
        _ => return None,
    })
}

/// Short form for the taskbar, where space is tight (VRAM shows just the used GB).
pub fn short_value(id: &str, snap: &Snapshot, cfg: &Config) -> Option<String> {
    value(id, snap, cfg)
}

pub fn gpu_short(id: &str, g: &GpuStats, cfg: &Config) -> Option<String> {
    match id {
        "gpu.vram" => Some(format!("{:.1} GB", gib(g.vram_used?))),
        _ => gpu_value(id, g, cfg),
    }
}

/// A disk's volume name, or a generic one for unlabeled volumes.
pub fn disk_label(d: &DiskStats) -> &str {
    if !d.label.is_empty() {
        &d.label
    } else if d.removable {
        "Removable Disk"
    } else {
        "Local Disk"
    }
}

/// "C:  Windows": drive letter plus the volume name.
pub fn disk_name(d: &DiskStats) -> String {
    format!("{}  {}", d.root, disk_label(d))
}

/// `text` with every digit as a 9. Digits are the same width in the taskbar font, so this measures like the widest
/// value with the same number of digits: a column reserves exactly that, with no spare room for a digit it won't show.
fn widest_like(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_digit() { '9' } else { c }).collect()
}

/// The widest disk text a column realistically shows (`prefixed`: led by a drive letter, as in the combined row).
pub fn disk_template(style: &str, prefixed: bool) -> &'static str {
    match (style, prefixed) {
        ("percent", false) => "100%",
        ("percent", true) => "C: 100%",
        ("free", false) => "999 GB",
        ("free", true) => "C: 999 GB",
        (_, false) => "999 / 999 GB",
        (_, true) => "C: 999 / 999 GB",
    }
}

/// A disk's space as text. `short` drops the word "free" for the taskbar.
pub fn disk_text(d: &DiskStats, style: &str, short: bool) -> String {
    let (div, unit) = if d.total >= 1u64 << 40 { (1024f64.powi(4), "TB") } else { (1024f64.powi(3), "GB") };
    let total = d.total as f64 / div;
    // Whole numbers for big disks, one decimal for small ones.
    let num = |b: u64| {
        let v = b as f64 / div;
        if total >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}") }
    };
    match style {
        "percent" => format!("{:.0}%", d.fraction() * 100.0),
        "free" if short => format!("{} {unit}", num(d.free)),
        "free" => format!("{} {unit} free", num(d.free)),
        _ => format!("{} / {} {unit}", num(d.used()), num(d.total)),
    }
}

pub use crate::ui::icons::Arrow;

/// One row of the taskbar grid: a section label and one optional value per column of its block.
pub struct GridRow {
    /// Whose icon leads the row (the disk icon for drive rows).
    pub section: Section,
    /// Row label: "CPU", "GPU", "RAM", or a drive letter.
    pub label: String,
    pub cells: Vec<Option<String>>,
    /// Widest value each of this row's cells may show (used when the block's columns aren't shared).
    pub templates: Vec<String>,
    /// A lone arrow icon instead of the section's icon (the split network rows).
    pub arrow: Option<Arrow>,
}

/// Up to two rows sharing the same columns. (A taskbar is only tall enough for two lines of text, so a third
/// row starts a second block to the right.)
pub struct GridBlock {
    /// True: every row uses the block's common columns (`templates`). False: each row spaces its own values.
    pub shared: bool,
    /// For each shared column, the widest values it may show. Columns reserve room for these, so the widget keeps
    /// its size (and its columns stay put) as the numbers change.
    pub templates: Vec<Vec<&'static str>>,
    pub rows: Vec<GridRow>,
}

/// Kinds that have at least one pinned metric, in the user's column order.
pub fn visible_kinds(cfg: &Config) -> Vec<Kind> {
    cfg.col_order
        .iter()
        .filter_map(|k| Kind::from_key(k))
        .filter(|&k| (k == Kind::Disk && !cfg.disks_pinned.is_empty()) || cfg.pinned.iter().filter_map(|i| def(i)).any(|d| d.kind == k))
        .collect()
}

pub fn section_pinned(cfg: &Config, s: Section) -> bool {
    if s == Section::Disk {
        return !cfg.disks_pinned.is_empty();
    }
    cfg.pinned.iter().filter_map(|i| def(i)).any(|d| d.section == s)
}

/// Where a taskbar row's data comes from.
enum Src<'a> {
    Sec(Section),
    /// One GPU row (with its "GPU" / "GPU1" / "GPU2" label).
    Gpu(&'a GpuStats, String),
    /// One row for a single disk (the "split" layout).
    Disk(&'a DiskStats),
    /// One row listing every pinned disk (the "combined" layout).
    Disks(Vec<&'a DiskStats>),
    /// The network row: download and upload speeds.
    Net,
    /// The download speed as a row of its own (the "split" network layout).
    NetDown,
    /// The upload speed as a row of its own.
    NetUp,
}

impl Src<'_> {
    /// Rows whose values don't line up by metric kind (they carry their own arrows or drive letters).
    fn positional(&self) -> bool {
        matches!(self, Src::Disk(_) | Src::Disks(_) | Src::Net | Src::NetDown | Src::NetUp)
    }
}

/// The taskbar widget's contents laid out as a grid, two rows per block in the order the user arranged:
/// sections (CPU, GPU, RAM) and the disks, either one row each or one combined row.
///
/// Rows made only of hardware metrics are aligned by metric kind (every block column is one kind, and a
/// missing value leaves a blank cell). A block that contains a disk row has no common kinds to line up, so its
/// values are aligned by position instead, starting right after each row's label.
pub fn widget_grid(cfg: &Config, snap: &Snapshot) -> Vec<GridBlock> {
    // Some(None) = pinned but no data right now; None = not pinned.
    let cell = |section: Section, kind: Kind| -> Option<Option<String>> {
        let d = METRICS.iter().find(|m| m.section == section && m.kind == kind && cfg.pinned.iter().any(|p| p == m.id))?;
        Some(short_value(d.id, snap, cfg))
    };
    let has_value = |s: Section, k: Kind| matches!(cell(s, k), Some(Some(_)));
    // The same, for one specific GPU (GPU metrics are per card).
    let gpu_cell = |g: &GpuStats, kind: Kind| -> Option<Option<String>> {
        let d = METRICS.iter().find(|m| m.section == Section::Gpu && m.kind == kind && cfg.pinned.iter().any(|p| p == m.id))?;
        Some(gpu_short(d.id, g, cfg))
    };
    let gpu_has = |g: &GpuStats, k: Kind| matches!(gpu_cell(g, k), Some(Some(_)));
    let order: Vec<Kind> = cfg.col_order.iter().filter_map(|k| Kind::from_key(k)).collect();
    let template = |k: Kind| k.template(cfg.fahrenheit, &cfg.disk_style);

    // Row sources in the user's row order; "disk" expands to the pinned, connected disks.
    let mut srcs: Vec<Src> = Vec::new();
    for key in &cfg.row_order {
        match Section::from_key(key) {
            Some(Section::Disk) => {
                let pinned: Vec<&DiskStats> = snap.disks.iter().filter(|d| cfg.disks_pinned.contains(&d.root)).collect();
                if cfg.disk_layout == "split" {
                    srcs.extend(pinned.into_iter().map(Src::Disk));
                } else if !pinned.is_empty() {
                    srcs.push(Src::Disks(pinned));
                }
            }
            Some(Section::Net) => {
                let pinned = |id: &str| cfg.pinned.iter().any(|p| p == id);
                if cfg.net_layout == "split" {
                    if pinned("net.down") {
                        srcs.push(Src::NetDown);
                    }
                    if pinned("net.up") {
                        srcs.push(Src::NetUp);
                    }
                } else if section_pinned(cfg, Section::Net) {
                    srcs.push(Src::Net);
                }
            }
            Some(Section::Gpu) => {
                let shown = taskbar_gpus(cfg, snap);
                let n = shown.len();
                for (i, g) in shown.into_iter().enumerate() {
                    if Kind::ALL.iter().any(|&k| gpu_has(g, k)) {
                        srcs.push(Src::Gpu(g, gpu_label(n, i)));
                    }
                }
            }
            Some(s) if Kind::ALL.iter().any(|&k| has_value(s, k)) => srcs.push(Src::Sec(s)),
            _ => {}
        }
    }

    // Two rows per block (a taskbar fits two lines of text).
    let mut groups: Vec<Vec<Src>> = Vec::new();
    for src in srcs {
        match groups.last_mut() {
            Some(g) if g.len() < 2 => g.push(src),
            _ => groups.push(vec![src]),
        }
    }

    groups
        .into_iter()
        .map(|g| {
            if !g.iter().any(|s| s.positional()) {
                // Hardware only: one column per metric kind, shared by the rows.
                let kinds: Vec<Kind> = order
                    .iter()
                    .copied()
                    .filter(|&k| {
                        g.iter().any(|src| match src {
                            Src::Sec(s) => has_value(*s, k),
                            Src::Gpu(gpu, _) => gpu_has(gpu, k),
                            _ => false,
                        })
                    })
                    .collect();
                let rows = g
                    .iter()
                    .filter_map(|src| match src {
                        Src::Sec(s) => Some(GridRow { section: *s, label: s.label().to_string(), cells: kinds.iter().map(|&k| cell(*s, k).flatten()).collect(), templates: Vec::new(), arrow: None }),
                        Src::Gpu(gpu, label) => Some(GridRow {
                            section: Section::Gpu,
                            label: label.clone(),
                            cells: kinds.iter().map(|&k| gpu_cell(gpu, k).flatten()).collect(),
                            templates: Vec::new(),
                            arrow: None,
                        }),
                        _ => None,
                    })
                    .collect();
                return GridBlock { shared: true, templates: kinds.iter().map(|&k| vec![template(k)]).collect(), rows };
            }

            // A block with a disk row: each row's values are laid out left to right after its label.
            let items: Vec<Vec<(String, String)>> = g
                .iter()
                .map(|src| match src {
                    Src::Sec(s) => order.iter().filter_map(|&k| cell(*s, k).flatten().map(|t| (t, template(k).to_string()))).collect(),
                    Src::Gpu(gpu, _) => order.iter().filter_map(|&k| gpu_cell(gpu, k).flatten().map(|t| (t, template(k).to_string()))).collect(),
                    Src::Disk(d) => {
                        let t = disk_text(d, &cfg.disk_style, true);
                        vec![(t.clone(), widest_like(&t))]
                    }
                    // Arrows say which is which; the template is the widest speed so the row doesn't resize.
                    // A row of its own is named by its label and arrow icon ("Down"/"Up"), so the value is just the speed.
                    Src::NetDown | Src::NetUp => {
                        let id = if matches!(src, Src::NetDown) { "net.down" } else { "net.up" };
                        value(id, snap, cfg).map(|v| (v, net_template(&cfg.net_unit).to_string())).into_iter().collect()
                    }
                    Src::Net => [("net.down", "\u{2193}"), ("net.up", "\u{2191}")]
                        .into_iter()
                        .filter(|(id, _)| cfg.pinned.iter().any(|p| p == id))
                        .filter_map(|(id, arrow)| {
                            let v = value(id, snap, cfg)?;
                            Some((format!("{arrow} {v}"), format!("{arrow} {}", net_template(&cfg.net_unit))))
                        })
                        .collect(),
                    // The combined row names each drive, since the cells carry no column heading.
                    Src::Disks(v) => v
                        .iter()
                        .map(|d| {
                            let t = format!("{} {}", d.root, disk_text(d, &cfg.disk_style, true));
                            let w = widest_like(&t);
                            (t, w)
                        })
                        .collect(),
                })
                .collect();
            let rows = g
                .iter()
                .zip(items)
                .map(|(src, row)| {
                    let (section, label) = match src {
                        Src::Sec(s) => (*s, s.label().to_string()),
                        Src::Gpu(_, label) => (Section::Gpu, label.clone()),
                        Src::Disk(d) => (Section::Disk, d.root.clone()),
                        Src::Disks(_) => (Section::Disk, Section::Disk.label().to_string()),
                        Src::Net => (Section::Net, Section::Net.label().to_string()),
                        Src::NetDown => (Section::Net, "Down".to_string()),
                        Src::NetUp => (Section::Net, "Up".to_string()),
                    };
                    let (texts, templates): (Vec<String>, Vec<String>) = row.into_iter().unzip();
                    let arrow = match src {
                        Src::NetDown => Some(Arrow::Down),
                        Src::NetUp => Some(Arrow::Up),
                        _ => None,
                    };
                    GridRow { section, label, cells: texts.into_iter().map(Some).collect(), templates, arrow }
                })
                .collect();
            GridBlock { shared: false, templates: Vec::new(), rows }
        })
        .collect()
}

/// Fill amount (0..1) for metrics that get a progress bar in the flyout.
pub fn fraction(id: &str, snap: &Snapshot) -> Option<f32> {
    let f = match id {
        "cpu.usage" => snap.cpu.usage_percent? / 100.0,
        "ram.usage" => snap.ram?.percent / 100.0,
        _ => return None,
    };
    Some(f.clamp(0.0, 1.0))
}

/// Progress-bar fill (0..1) for a GPU metric.
pub fn gpu_fraction(id: &str, g: &GpuStats) -> Option<f32> {
    let f = match id {
        "gpu.usage" => g.usage_percent? / 100.0,
        "gpu.vram" => g.vram_used? as f32 / g.vram_total? as f32,
        _ => return None,
    };
    Some(f.clamp(0.0, 1.0))
}

pub struct RowData {
    pub label: String,
    pub value: String,
    pub frac: Option<f32>,
    /// Draw the bar in the warning color (a nearly full disk).
    pub hot: bool,
}

pub struct Card {
    pub section: Section,
    pub title: String,
    pub subtitle: String,
    pub rows: Vec<RowData>,
    /// Optional hint shown under the rows (clicking it opens settings).
    pub note: Option<&'static str>,
}

/// Cards for the flyout: one per section, containing the metrics the user enabled that have data.
pub fn flyout_cards(cfg: &Config, snap: &Snapshot, cpu_name: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    for (section, title) in [(Section::Cpu, "CPU"), (Section::Gpu, "GPU"), (Section::Ram, "Memory"), (Section::Net, "Network")] {
        if section == Section::Gpu {
            // One card per shown GPU, labelled GPU1 / GPU2 when there is more than one.
            let shown = flyout_gpus(cfg, snap);
            let n = shown.len();
            for (i, g) in shown.into_iter().enumerate() {
                let rows: Vec<RowData> = cfg
                    .flyout
                    .iter()
                    .filter_map(|id| {
                        let d = def(id).filter(|d| d.section == Section::Gpu)?;
                        Some(RowData { label: d.name.to_string(), value: gpu_value(id, g, cfg)?, frac: gpu_fraction(id, g), hot: false })
                    })
                    .collect();
                if !rows.is_empty() {
                    cards.push(Card { section: Section::Gpu, title: gpu_label(n, i), subtitle: g.name.clone(), rows, note: None });
                }
            }
            continue;
        }
        let mut rows = Vec::new();
        for id in &cfg.flyout {
            let Some(d) = def(id) else { continue };
            if d.section != section {
                continue;
            }
            if let Some(value) = value(id, snap, cfg) {
                rows.push(RowData { label: d.name.to_string(), value, frac: fraction(id, snap), hot: false });
            }
        }
        if rows.is_empty() {
            continue;
        }
        let subtitle = match section {
            Section::Cpu => cpu_name.to_string(),
            Section::Gpu | Section::Ram | Section::Disk | Section::Net => String::new(),
        };
        let wants_driver = cfg.flyout.iter().chain(&cfg.pinned).any(|i| i == "cpu.temp" || i == "cpu.power");
        let note = (section == Section::Cpu
            && wants_driver
            && snap.pawn == Some(crate::sensors::cpu_pawnio::Unavailable::NotInstalled))
        .then_some("Temperature and power need the PawnIO driver. Click here to set it up.");
        cards.push(Card { section, title: title.to_string(), subtitle, rows, note });
    }

    // Disks: every connected local drive except the ones the user hid.
    let shown: Vec<&DiskStats> = snap.disks.iter().filter(|d| !cfg.disks_hidden.contains(&d.root)).collect();
    if cfg.disk_layout == "split" {
        // Each disk is its own section, with the detail a CPU or GPU card has.
        for d in shown {
            let f = d.fraction();
            let rows = vec![
                RowData { label: "Usage".to_string(), value: disk_text(d, "percent", false), frac: Some(f), hot: f >= 0.9 },
                RowData { label: "Used".to_string(), value: disk_text(d, "used_total", false), frac: None, hot: false },
                RowData { label: "Free".to_string(), value: disk_text(d, "free", true), frac: None, hot: false },
            ];
            cards.push(Card { section: Section::Disk, title: d.root.clone(), subtitle: disk_label(d).to_string(), rows, note: None });
        }
    } else if !shown.is_empty() {
        let rows: Vec<RowData> = shown
            .iter()
            .map(|d| RowData { label: disk_name(d), value: disk_text(d, &cfg.disk_style, false), frac: Some(d.fraction()), hot: d.fraction() >= 0.9 })
            .collect();
        cards.push(Card { section: Section::Disk, title: "Disks".to_string(), subtitle: String::new(), rows, note: None });
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_rates_switch_units_before_a_fourth_digit() {
        assert_eq!(rate(0.0, "bytes"), "0 KB/s");
        assert_eq!(rate(845.0 * 1024.0, "bytes"), "845 KB/s");
        assert_eq!(rate(1000.0 * 1024.0, "bytes"), "1.0 MB/s");
        assert_eq!(rate(12.34 * 1048576.0, "bytes"), "12.3 MB/s");
        assert_eq!(rate(150.0 * 1048576.0, "bytes"), "150 MB/s");
        assert_eq!(rate(1.5 * 1073741824.0, "bytes"), "1.50 GB/s");
    }

    #[test]
    fn bit_rates_use_decimal_units() {
        assert_eq!(rate(0.0, "bits"), "0 Kbps");
        // 125,000 bytes/s is exactly one megabit.
        assert_eq!(rate(125_000.0, "bits"), "1.0 Mbps");
        assert_eq!(rate(1_250_000.0, "bits"), "10.0 Mbps");
        assert_eq!(rate(25_000_000.0, "bits"), "200 Mbps");
        assert_eq!(rate(125_000_000.0, "bits"), "1.00 Gbps");
        assert_eq!(rate(12_500.0, "bits"), "100 Kbps");
    }

    #[test]
    fn digits_measure_as_nines() {
        assert_eq!(widest_like("C: 67%"), "C: 99%");
        assert_eq!(widest_like("238 / 465 GB"), "999 / 999 GB");
    }

    #[test]
    fn values_split_into_number_and_unit() {
        assert_eq!(split_unit("4.49 GHz"), ("4.49", Some(("GHz", true))));
        assert_eq!(split_unit("45%"), ("45", Some(("%", false))));
        assert_eq!(split_unit("50\u{00B0}C"), ("50", Some(("\u{00B0}C", false))));
        assert_eq!(split_unit("38 W"), ("38", Some(("W", true))));
        assert_eq!(split_unit("12.2 / 31 GB"), ("12.2 / 31", Some(("GB", true))));
        assert_eq!(split_unit("C: 67%"), ("C: 67", Some(("%", false))));
        assert_eq!(split_unit("\u{2193} 1.2 MB/s"), ("\u{2193} 1.2", Some(("MB/s", true))));
        assert_eq!(split_unit("waiting for sensors"), ("waiting for sensors", None));
        assert_eq!(split_unit("42"), ("42", None));
    }
}
