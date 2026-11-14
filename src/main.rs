#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod metrics;
mod ui;
mod sensors;
mod shell;

use std::time::Duration;

fn gib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0 * 1024.0)
}

fn opt(v: Option<f32>, unit: &str) -> String {
    v.map(|v| format!("{v:.1}{unit}")).unwrap_or_else(|| "n/a".into())
}

/// `--dump [file]`: sample live sensor values for a few seconds and print them (or write them to `file`,
/// which is needed for the GUI-subsystem release build). Used to verify against Task Manager and HWiNFO.
fn dump(path: Option<&str>) {
    use std::fmt::Write;
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let mut out = String::new();
    let mut s = sensors::Sampler::new();
    s.sample(); // first PDH sample is only a baseline
    match s.pawn_status() {
        None => writeln!(out, "PawnIO: ok").ok(),
        Some(e) => writeln!(out, "PawnIO: unavailable ({e:?})").ok(),
    };
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(1000));
        let snap = s.sample();
        writeln!(out, "--- sample ---").ok();
        writeln!(
            out,
            "CPU  usage {}  clock {}  temp {}  power {}",
            opt(snap.cpu.usage_percent, "%"),
            opt(snap.cpu.clock_mhz, " MHz"),
            opt(snap.cpu.temp_c, " C"),
            opt(snap.cpu.power_w, " W")
        )
        .ok();
        if let Some(r) = snap.ram {
            writeln!(out, "RAM  {:.1} / {:.1} GiB ({}%)", gib(r.used_bytes), gib(r.total_bytes), r.percent).ok();
        }
        let (down, up) = snap.net.total(&[]);
        writeln!(out, "NET  down {down:.0} B/s  up {up:.0} B/s").ok();
        for a in &snap.net.adapters {
            writeln!(out, "     {} [{}] down {:.0} B/s  up {:.0} B/s", a.name, if a.connected { "connected" } else { "not connected" }, a.down_bps, a.up_bps).ok();
        }
        for g in &snap.gpus {
            writeln!(out, "GPU  {} [{:?}{}] key='{}'", g.name, g.vendor, if g.integrated { ", integrated" } else { ", discrete" }, g.key).ok();
            writeln!(
                out,
                "     usage {}  temp {}  power {}  core {}  mem {}",
                opt(g.usage_percent, "%"),
                opt(g.temp_c, " C"),
                opt(g.power_w, " W"),
                opt(g.core_mhz, " MHz"),
                opt(g.mem_mhz, " MHz")
            )
            .ok();
            writeln!(
                out,
                "     vram {} / {} GiB  fan {}",
                g.vram_used.map(|v| format!("{:.2}", gib(v))).unwrap_or("n/a".into()),
                g.vram_total.map(|v| format!("{:.2}", gib(v))).unwrap_or("n/a".into()),
                opt(g.fan_percent, "%")
            )
            .ok();
        }
    }
    match path {
        Some(p) => {
            let _ = std::fs::write(p, out);
        }
        None => print!("{out}"),
    }
}

fn main() {
    install_crash_log();
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--verify-installer") {
        let report = shell::setup::selftest();
        match args.get(i + 1) {
            Some(path) => {
                let _ = std::fs::write(path, report);
            }
            None => print!("{report}"),
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--dump") {
        dump(args.get(i + 1).map(|s| s.as_str()));
        return;
    }
    // Used by the installer/uninstaller: create or remove the "Start with Windows" task, then exit.
    // Exit code 0 = done, 1 = failed (typically: not elevated).
    if let Some(i) = args.iter().position(|a| a == "--autostart") {
        let result = match args.get(i + 1).map(|s| s.as_str()) {
            Some("on") => shell::autostart::enable(),
            Some("off") => {
                // Removing a task that doesn't exist is fine.
                if shell::autostart::is_enabled() { shell::autostart::disable() } else { Ok(()) }
            }
            _ => Err("usage: --autostart on|off".to_string()),
        };
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    // Used by the uninstaller: take TaskbarHWM out of Claude Code's status line again, then exit.
    if let Some(i) = args.iter().position(|a| a == "--claude") {
        let result = match args.get(i + 1).map(|s| s.as_str()) {
            Some("on") => shell::claude_setup::connect(),
            Some("off") => {
                // Nothing to undo if it was never connected (or Claude Code has some other status line).
                if shell::claude_setup::link_state() == sensors::claude::Link::Connected { shell::claude_setup::disconnect() } else { Ok(()) }
            }
            _ => Err("usage: --claude on|off".to_string()),
        };
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    // Single instance: a second launch just exits.
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let _guard = unsafe { CreateMutexW(None, false, w!("Local\\TaskbarHWM.SingleInstance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return;
    }

    let cfg = config::init();
    cfg.save();
    // If "Start with Windows" is on but points at an old exe location, repoint it.
    std::thread::spawn(shell::autostart::heal);
    if let Err(e) = ui::widget::run(cfg) {
        eprintln!("TaskbarHWM failed to start: {e}");
    }
}


/// The release build has no console, so a panic (which aborts) would vanish silently.
/// Record it in %LOCALAPPDATA%\TaskbarHWM\crash.log instead.
fn install_crash_log() {
    std::panic::set_hook(Box::new(|info| {
        if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
            let dir = std::path::PathBuf::from(dir).join("TaskbarHWM");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), format!("{info}
"));
        }
    }));
}
