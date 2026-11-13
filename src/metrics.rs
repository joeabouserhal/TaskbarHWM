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
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::Cpu => "CPU",
            Section::Gpu => "GPU",
            Section::Ram => "RAM",
            Section::Disk => "Disks",
        }
    }

    /// Stable id used in the config's row order.
    pub fn key(self) -> &'static str {
        match self {
            Section::Cpu => "cpu",
            Section::Gpu => "gpu",
            Section::Ram => "ram",
            Section::Disk => "disk",
        }
    }

    pub fn from_key(k: &str) -> Option<Section> {
        [Section::Cpu, Section::Gpu, Section::Ram, Section::Disk].into_iter().find(|s| s.key() == k)
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
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Usage, Kind::Clock, Kind::Temp, Kind::Power, Kind::Memory, Kind::MemClock, Kind::Fan, Kind::Disk];

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
        }
    }
}

pub const ROW_KEYS: &[&str] = &["cpu", "gpu", "ram", "disk"];
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
];

/// Position in the catalog; used to keep user-selected lists in a stable order.
pub fn order(id: &str) -> usize {
    METRICS.iter().position(|m| m.id == id).unwrap_or(usize::MAX)
}

pub fn def(id: &str) -> Option<&'static MetricDef> {
    METRICS.iter().find(|m| m.id == id)
}

/// The GPU the widget reports on: the adapter with the most dedicated VRAM (the discrete card, not the iGPU).
pub fn primary_gpu(snap: &Snapshot) -> Option<&GpuStats> {
    snap.gpus.iter().max_by_key(|g| g.vram_total.unwrap_or(0))
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

/// Formatted value for a metric, or None when the sensor isn't available (the UI hides it).
pub fn value(id: &str, snap: &Snapshot, cfg: &Config) -> Option<String> {
    let gpu = primary_gpu(snap);
    Some(match id {
        "cpu.usage" => format!("{:.0}%", snap.cpu.usage_percent?),
        "cpu.clock" => clock(snap.cpu.clock_mhz?),
        "cpu.temp" => temp(snap.cpu.temp_c?, cfg),
        "cpu.power" => format!("{:.0} W", snap.cpu.power_w?),
        "gpu.usage" => format!("{:.0}%", gpu?.usage_percent?),
        "gpu.temp" => temp(gpu?.temp_c?, cfg),
        "gpu.power" => format!("{:.0} W", gpu?.power_w?),
        "gpu.core_clock" => clock(gpu?.core_mhz?),
        "gpu.mem_clock" => clock(gpu?.mem_mhz?),
        "gpu.vram" => {
            let g = gpu?;
            format!("{:.1} / {:.0} GB", gib(g.vram_used?), gib(g.vram_total?))
        }
        "gpu.fan" => format!("{:.0}%", gpu?.fan_percent?),
        "ram.usage" => format!("{:.0}%", snap.ram?.percent),
        "ram.used" => {
            let r = snap.ram?;
            format!("{:.1} / {:.0} GB", gib(r.used_bytes), gib(r.total_bytes))
        }
        _ => return None,
    })
}

/// Short form for the taskbar, where space is tight (RAM shows a percentage, VRAM just used GB).
pub fn short_value(id: &str, snap: &Snapshot, cfg: &Config) -> Option<String> {
    match id {
        "gpu.vram" => Some(format!("{:.1} GB", gib(primary_gpu(snap)?.vram_used?))),
        _ => value(id, snap, cfg),
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

/// One row of the taskbar grid: a section label and one optional value per column of its block.
pub struct GridRow {
    /// Whose icon leads the row (the disk icon for drive rows).
    pub section: Section,
    /// Row label: "CPU", "GPU", "RAM", or a drive letter.
    pub label: String,
    pub cells: Vec<Option<String>>,
    /// Widest value each of this row's cells may show (used when the block's columns aren't shared).
    pub templates: Vec<&'static str>,
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
    /// One row for a single disk (the "split" layout).
    Disk(&'a DiskStats),
    /// One row listing every pinned disk (the "combined" layout).
    Disks(Vec<&'a DiskStats>),
}

impl Src<'_> {
    fn is_disk(&self) -> bool {
        !matches!(self, Src::Sec(_))
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
            if !g.iter().any(|s| s.is_disk()) {
                // Hardware only: one column per metric kind, shared by the rows.
                let kinds: Vec<Kind> = order.iter().copied().filter(|&k| g.iter().any(|src| matches!(src, Src::Sec(s) if has_value(*s, k)))).collect();
                let rows = g
                    .iter()
                    .filter_map(|src| match src {
                        Src::Sec(s) => Some(GridRow { section: *s, label: s.label().to_string(), cells: kinds.iter().map(|&k| cell(*s, k).flatten()).collect(), templates: Vec::new() }),
                        _ => None,
                    })
                    .collect();
                return GridBlock { shared: true, templates: kinds.iter().map(|&k| vec![template(k)]).collect(), rows };
            }

            // A block with a disk row: each row's values are laid out left to right after its label.
            let items: Vec<Vec<(String, &'static str)>> = g
                .iter()
                .map(|src| match src {
                    Src::Sec(s) => order.iter().filter_map(|&k| cell(*s, k).flatten().map(|t| (t, template(k)))).collect(),
                    Src::Disk(d) => vec![(disk_text(d, &cfg.disk_style, true), disk_template(&cfg.disk_style, false))],
                    // The combined row names each drive, since the cells carry no column heading.
                    Src::Disks(v) => v
                        .iter()
                        .map(|d| (format!("{} {}", d.root, disk_text(d, &cfg.disk_style, true)), disk_template(&cfg.disk_style, true)))
                        .collect(),
                })
                .collect();
            let rows = g
                .iter()
                .zip(items)
                .map(|(src, row)| {
                    let (section, label) = match src {
                        Src::Sec(s) => (*s, s.label().to_string()),
                        Src::Disk(d) => (Section::Disk, d.root.clone()),
                        Src::Disks(_) => (Section::Disk, Section::Disk.label().to_string()),
                    };
                    let (texts, templates): (Vec<String>, Vec<&'static str>) = row.into_iter().unzip();
                    GridRow { section, label, cells: texts.into_iter().map(Some).collect(), templates }
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
        "gpu.usage" => primary_gpu(snap)?.usage_percent? / 100.0,
        "gpu.vram" => {
            let g = primary_gpu(snap)?;
            g.vram_used? as f32 / g.vram_total? as f32
        }
        "ram.usage" => snap.ram?.percent / 100.0,
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
    for (section, title) in [(Section::Cpu, "CPU"), (Section::Gpu, "GPU"), (Section::Ram, "Memory")] {
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
            Section::Gpu => primary_gpu(snap).map(|g| g.name.clone()).unwrap_or_default(),
            Section::Ram | Section::Disk => String::new(),
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
