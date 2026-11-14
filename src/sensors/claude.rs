//! Claude usage, from two places.
//!
//! 1. **Claude Code's `/usage` command**, run headlessly (`claude -p "/usage"`) on a timer. It is a built-in command
//!    that makes no model request, so it costs no tokens and can't start a limit window, and it reports the account's
//!    5-hour and weekly limits no matter where you used Claude: chat, the desktop app or a terminal. This is the main
//!    source. Nothing here reads a login or token: the installed Claude Code does its own talking.
//! 2. **The status line relay** (see `shell::claude_setup`): an optional script that saves what Claude Code's
//!    documented status line hands it. Only terminal sessions run it, but it adds the session's context use and cost.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How far along the optional status line connection to Claude Code is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// No `.claude` folder: Claude Code isn't installed for this user.
    #[default]
    NoClaudeCode,
    NotConnected,
    /// Claude Code already has a status line of its own, which we don't replace.
    OtherStatusLine,
    Connected,
}

/// What reading the limits with `claude /usage` has turned up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cli {
    /// Not asked yet.
    #[default]
    Checking,
    /// No Claude reading is switched on, so nothing is being asked.
    Idle,
    /// Claude Code isn't installed where we can find it.
    Missing,
    /// It answered, but with no limits (not signed in with a Pro, Max or Ultra plan, for example an API key).
    NoLimits,
    /// It couldn't be run, or its answer wasn't understood.
    Failed,
    Ok,
}

/// One limit window: how much is used and when it resets (Unix seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub pct: f32,
    pub resets: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageReading {
    /// The rolling 5-hour window ("Current session").
    pub session: Option<Window>,
    /// The weekly window across all models.
    pub week: Option<Window>,
    /// A weekly window for one model, when the plan has one ("Current week (Fable)").
    pub week_model: Option<(String, Window)>,
}

#[derive(Clone, Debug, Default)]
pub struct ClaudeStats {
    pub link: Link,
    pub cli: Cli,
    /// Unix seconds when `/usage` last answered.
    pub polled: Option<i64>,
    pub five_hour_pct: Option<f32>,
    pub five_hour_resets: Option<i64>,
    pub weekly_pct: Option<f32>,
    pub weekly_resets: Option<i64>,
    pub week_model: Option<(String, Window)>,
    /// Context window use of the terminal session that reported last.
    pub context_pct: Option<f32>,
    pub cost_usd: Option<f32>,
    pub model: Option<String>,
    /// Unix seconds when the relay last wrote its file.
    pub updated: Option<i64>,
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A limit window's usage right now: unknown without data, and 0 once its reset time has passed.
fn window_pct(pct: Option<f32>, resets: Option<i64>, now: i64) -> Option<f32> {
    let p = pct?;
    match resets {
        Some(r) if now >= r => Some(0.0),
        _ => Some(p),
    }
}

/// Seconds until a window resets, if known and still in the future.
fn window_left(resets: Option<i64>, now: i64) -> Option<i64> {
    resets.filter(|&r| r > now).map(|r| r - now)
}

impl ClaudeStats {
    pub fn five_hour_now(&self, now: i64) -> Option<f32> {
        window_pct(self.five_hour_pct, self.five_hour_resets, now)
    }
    pub fn weekly_now(&self, now: i64) -> Option<f32> {
        window_pct(self.weekly_pct, self.weekly_resets, now)
    }
    pub fn five_hour_left(&self, now: i64) -> Option<i64> {
        window_left(self.five_hour_resets, now)
    }
    pub fn weekly_left(&self, now: i64) -> Option<i64> {
        window_left(self.weekly_resets, now)
    }
    /// The per-model weekly window as (model name, usage now).
    pub fn week_model_now(&self, now: i64) -> Option<(&str, f32)> {
        let (name, w) = self.week_model.as_ref()?;
        Some((name.as_str(), window_pct(Some(w.pct), w.resets, now)?))
    }
    /// True once the status line relay has delivered anything.
    pub fn has_data(&self) -> bool {
        self.updated.is_some()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Reading `/usage`
// ---------------------------------------------------------------------------------------------------------------

/// When a limit resets, as written in `/usage` ("Nov 14, 10:09pm", "3am", "10:09pm (Asia/Beirut)"): local time, and
/// the date is left out when it is today or tomorrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetSpec {
    pub month: Option<u32>,
    pub day: Option<u32>,
    pub hour: u32,
    pub minute: u32,
}

fn month_number(s: &str) -> Option<u32> {
    const M: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let s = s.get(..3)?.to_ascii_lowercase();
    M.iter().position(|m| *m == s).map(|i| i as u32 + 1)
}

/// "Nov 14, 10:09pm (Asia/Beirut)" -> month 11, day 14, 22:09.
pub fn parse_reset(text: &str) -> Option<ResetSpec> {
    // The zone name in brackets is the machine's own; drop it.
    let t = text.split('(').next()?.trim();
    let (date, time) = match t.split_once(',') {
        Some((d, tm)) => (Some(d.trim()), tm.trim()),
        None => (None, t),
    };
    let (month, day) = match date {
        Some(d) => {
            let mut it = d.split_whitespace();
            (Some(month_number(it.next()?)?), Some(it.next()?.parse::<u32>().ok()?))
        }
        None => (None, None),
    };
    let lower = time.to_ascii_lowercase();
    let pm = lower.ends_with("pm");
    if !pm && !lower.ends_with("am") {
        return None;
    }
    let clock = &lower[..lower.len() - 2];
    let (h, m) = match clock.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None => (clock.trim().parse::<u32>().ok()?, 0),
    };
    if h == 0 || h > 12 || m > 59 {
        return None;
    }
    Some(ResetSpec { month, day, hour: h % 12 + if pm { 12 } else { 0 }, minute: m })
}

/// "23% used" -> 23.0
fn parse_used(s: &str) -> Option<f32> {
    let end = s.find("% used")?;
    s[..end].trim().rsplit(|c: char| !(c.is_ascii_digit() || c == '.')).next()?.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Reads the `/usage` text. `epoch` turns a reset time written in local time into Unix seconds.
pub fn parse_usage(text: &str, epoch: impl Fn(ResetSpec) -> Option<i64>) -> UsageReading {
    let mut r = UsageReading::default();
    for line in text.lines() {
        let line = line.trim();
        let Some((label, rest)) = line.split_once(':') else { continue };
        let Some(pct) = parse_used(rest) else { continue };
        let resets = rest.split_once("resets").and_then(|(_, t)| parse_reset(t.trim())).and_then(&epoch);
        let w = Window { pct, resets };
        let label = label.trim();
        if label.eq_ignore_ascii_case("current session") {
            r.session = Some(w);
        } else if let Some(model) = label.strip_prefix("Current week (").and_then(|m| m.strip_suffix(')')) {
            if model.eq_ignore_ascii_case("all models") {
                r.week = Some(w);
            } else if r.week_model.is_none() {
                r.week_model = Some((model.trim().to_string(), w));
            }
        }
    }
    r
}

#[cfg(windows)]
mod local_time {
    use super::ResetSpec;
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::SystemInformation::GetLocalTime;
    use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};

    fn epoch_of(y: u16, mo: u16, d: u16, h: u16, mi: u16) -> Option<i64> {
        let local = SYSTEMTIME { wYear: y, wMonth: mo, wDay: d, wHour: h, wMinute: mi, ..Default::default() };
        let mut utc = SYSTEMTIME::default();
        let mut ft = FILETIME::default();
        unsafe {
            TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).ok()?;
            SystemTimeToFileTime(&utc, &mut ft).ok()?;
        }
        let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
        Some((ticks / 10_000_000) as i64 - 11_644_473_600)
    }

    /// Unix seconds of a reset time written in this machine's local time. A time without a date means the next time
    /// the clock reads that (today, or tomorrow once it has passed); a date without a year means this year, or next
    /// year if that would put it well in the past.
    pub fn resolve(spec: ResetSpec, now: i64) -> Option<i64> {
        let today = unsafe { GetLocalTime() };
        let (m, d) = (spec.month.map(|m| m as u16), spec.day.map(|d| d as u16));
        match (m, d) {
            (Some(m), Some(d)) => {
                let t = epoch_of(today.wYear, m, d, spec.hour as u16, spec.minute as u16)?;
                if t < now - 3 * 86_400 { epoch_of(today.wYear + 1, m, d, spec.hour as u16, spec.minute as u16) } else { Some(t) }
            }
            _ => {
                let t = epoch_of(today.wYear, today.wMonth, today.wDay, spec.hour as u16, spec.minute as u16)?;
                Some(if t < now - 60 { t + 86_400 } else { t })
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Running Claude Code
// ---------------------------------------------------------------------------------------------------------------

/// Where TaskbarHWM keeps its Claude files (`%LOCALAPPDATA%\TaskbarHWM`, or `HWM_DATA_DIR` for tests).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("HWM_DATA_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("TaskbarHWM")
}

/// Claude Code's folder (`%USERPROFILE%\.claude`, or `HWM_CLAUDE_DIR` for tests).
pub fn claude_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("HWM_CLAUDE_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join(".claude")
}

/// The `claude` command: its usual install spot, then anywhere on the PATH.
fn find_claude() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        candidates.push(PathBuf::from(home).join(".local").join("bin").join("claude.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("claude.exe")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Runs `claude -p "/usage"` and returns what it printed.
///
/// Trimmed to start fast and touch nothing else: no connectors (an empty MCP list), no user settings or hooks, no
/// tools, and no session saved. `/usage` makes no model request, so this costs nothing against your limits.
fn run_usage(claude: &Path) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mcp = dir.join("claude-empty-mcp.json");
    if !mcp.exists() {
        std::fs::write(&mcp, "{\"mcpServers\":{}}").map_err(|e| e.to_string())?;
    }
    let mut child = Command::new(claude)
        .current_dir(&dir)
        .args(["-p", "/usage", "--model", "haiku", "--tools", "", "--max-turns", "1", "--no-session-persistence", "--system-prompt", ".", "--strict-mcp-config", "--mcp-config"])
        .arg(&mcp)
        .args(["--setting-sources", ""])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    reader.join().map_err(|_| "no output".to_string())
}

#[derive(Default)]
struct Polled {
    reading: Option<UsageReading>,
    at: Option<i64>,
    cli: Cli,
}

/// Set to ask the poller to read `/usage` again right away (the flyout does this when it opens).
static REFRESH_NOW: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH_NOW.store(true, Ordering::Relaxed);
}

fn wants_claude(cfg: &crate::config::Config) -> bool {
    cfg.pinned.iter().chain(&cfg.flyout).any(|id| id.starts_with("claude."))
}

/// Reads `/usage` on a timer, but only while a Claude reading is switched on, and never more often than the chosen
/// interval (the settings page's "Refresh every").
fn spawn_poller(shared: Arc<Mutex<Polled>>) {
    std::thread::spawn(move || {
        let mut last: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            let cfg = crate::config::get();
            if !wants_claude(&cfg) || cfg.claude_poll_minutes == 0 {
                if let Ok(mut s) = shared.lock() {
                    if s.cli == Cli::Checking {
                        s.cli = Cli::Idle;
                    }
                }
                continue;
            }
            let every = Duration::from_secs(cfg.claude_poll_minutes as u64 * 60);
            let asked = REFRESH_NOW.swap(false, Ordering::Relaxed);
            let due = last.is_none_or(|t| t.elapsed() >= every) || (asked && last.is_none_or(|t| t.elapsed() >= Duration::from_secs(15)));
            if !due {
                continue;
            }
            last = Some(Instant::now());
            let now = now_secs();
            let result = match find_claude() {
                None => (Cli::Missing, None),
                Some(exe) => match run_usage(&exe) {
                    Err(_) => (Cli::Failed, None),
                    Ok(text) => {
                        let r = parse_usage(&text, |spec| local_time::resolve(spec, now));
                        if r.session.is_some() || r.week.is_some() {
                            (Cli::Ok, Some(r))
                        } else if text.to_ascii_lowercase().contains("usage") {
                            (Cli::NoLimits, None)
                        } else {
                            (Cli::Failed, None)
                        }
                    }
                },
            };
            if let Ok(mut s) = shared.lock() {
                s.cli = result.0;
                if let Some(r) = result.1 {
                    s.reading = Some(r);
                    s.at = Some(now_secs());
                }
            }
        }
    });
}

/// Parses the relay's `key=value` lines. Unknown keys and unreadable values are ignored.
pub fn parse(text: &str) -> ClaudeStats {
    let mut s = ClaudeStats::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        let f = || v.parse::<f32>().ok().filter(|x| x.is_finite());
        let i = || v.parse::<i64>().ok();
        match k.trim() {
            "five_hour_pct" => s.five_hour_pct = f(),
            "five_hour_resets" => s.five_hour_resets = i(),
            "seven_day_pct" => s.weekly_pct = f(),
            "seven_day_resets" => s.weekly_resets = i(),
            "context_pct" => s.context_pct = f(),
            "cost_usd" => s.cost_usd = f(),
            "model" if !v.is_empty() => s.model = Some(v.to_string()),
            "updated" => s.updated = i(),
            _ => {}
        }
    }
    s
}

pub struct ClaudeSampler {
    file: PathBuf,
    mtime: Option<SystemTime>,
    relay: ClaudeStats,
    link: Link,
    link_checked: Option<Instant>,
    polled: Arc<Mutex<Polled>>,
}

impl ClaudeSampler {
    pub fn new() -> ClaudeSampler {
        crate::shell::claude_setup::refresh_script();
        let polled = Arc::new(Mutex::new(Polled::default()));
        spawn_poller(polled.clone());
        ClaudeSampler { file: data_dir().join("claude-status.txt"), mtime: None, relay: ClaudeStats::default(), link: Link::default(), link_checked: None, polled }
    }

    pub fn sample(&mut self) -> ClaudeStats {
        // Whether the status line is set up changes rarely: look every few seconds, not every tick.
        if self.link_checked.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
            self.link = crate::shell::claude_setup::link_state();
            self.link_checked = Some(Instant::now());
        }
        // The relay file only changes when a terminal session runs its status line: re-read it only then.
        let mtime = std::fs::metadata(&self.file).and_then(|m| m.modified()).ok();
        if mtime != self.mtime {
            self.mtime = mtime;
            self.relay = match mtime {
                Some(_) => std::fs::read_to_string(&self.file).map(|t| parse(&t)).unwrap_or_default(),
                None => ClaudeStats::default(),
            };
        }
        match self.polled.lock() {
            Ok(p) => merge(self.link, &self.relay, p.cli, p.reading.as_ref(), p.at, now_secs()),
            Err(_) => ClaudeStats { link: self.link, ..self.relay.clone() },
        }
    }
}

/// How long a `/usage` reading stays trusted once `/usage` has stopped answering.
const STALE_AFTER: i64 = 600;

/// Combines the two sources into what the widget shows.
///
/// The limits come from `/usage` while it works. If it stops (Claude Code removed, or its output changed so it can't be
/// read), its last reading is trusted only for a few minutes; after that the terminal connection (the status line
/// relay), if it has delivered anything, is the source. Whichever spoke last wins while both are available. Context use,
/// cost and the model always come from the relay, since `/usage` doesn't have them.
fn merge(link: Link, relay: &ClaudeStats, cli: Cli, reading: Option<&UsageReading>, at: Option<i64>, now: i64) -> ClaudeStats {
    let mut s = ClaudeStats { link, cli, polled: at, ..relay.clone() };
    if let (Some(r), Some(at)) = (reading, at) {
        let current = cli == Cli::Ok || now - at < STALE_AFTER;
        let relay_newer = relay.five_hour_pct.is_some() && relay.updated.is_some_and(|u| u > at);
        if current && !relay_newer {
            if let Some(w) = &r.session {
                s.five_hour_pct = Some(w.pct);
                s.five_hour_resets = w.resets;
            }
            if let Some(w) = &r.week {
                s.weekly_pct = Some(w.pct);
                s.weekly_resets = w.resets;
            }
        }
        if current {
            s.week_model = r.week_model.clone();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "five_hour_pct=23.5\nfive_hour_resets=1738425600\nseven_day_pct=41.2\nseven_day_resets=1738857600\ncontext_pct=8\ncost_usd=1.25\nmodel=Opus\nupdated=1738420000\nnonsense\nfuture_key=1\n";

    const USAGE: &str = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 23% used · resets Nov 14, 10:09pm (Asia/Beirut)\nCurrent week (all models): 8% used · resets Nov 21, 2:59am (Asia/Beirut)\nCurrent week (Fable): 0% used · resets Nov 21, 3am (Asia/Beirut)\n\nWhat's contributing to your usage?\nApproximate · sessions on this machine only · shares overlap\n\nLast 24h · 646 requests · 4 sessions\n  93%  Start fresh when you switch tasks · run /clear\n";

    #[test]
    fn parses_the_relay_file() {
        let s = parse(SAMPLE);
        assert_eq!(s.five_hour_pct, Some(23.5));
        assert_eq!(s.five_hour_resets, Some(1738425600));
        assert_eq!(s.weekly_pct, Some(41.2));
        assert_eq!(s.context_pct, Some(8.0));
        assert_eq!(s.cost_usd, Some(1.25));
        assert_eq!(s.model.as_deref(), Some("Opus"));
        assert!(s.has_data());
    }

    #[test]
    fn missing_fields_stay_unknown() {
        let s = parse("updated=5\n");
        assert_eq!(s.five_hour_pct, None);
        assert_eq!(s.weekly_pct, None);
        assert!(s.has_data());
        assert!(!parse("").has_data());
    }

    #[test]
    fn a_window_reads_zero_once_it_has_reset() {
        let s = parse(SAMPLE);
        assert_eq!(s.five_hour_now(1738420000), Some(23.5));
        assert_eq!(s.five_hour_left(1738420000), Some(5600));
        assert_eq!(s.five_hour_now(1738425600), Some(0.0));
        assert_eq!(s.five_hour_left(1738425600), None);
        assert_eq!(s.weekly_now(1738425600), Some(41.2));
    }

    #[test]
    fn reads_reset_times_in_every_form_usage_writes_them() {
        assert_eq!(parse_reset("Nov 14, 10:09pm (Asia/Beirut)"), Some(ResetSpec { month: Some(11), day: Some(14), hour: 22, minute: 9 }));
        assert_eq!(parse_reset("Nov 21, 3am (Asia/Beirut)"), Some(ResetSpec { month: Some(11), day: Some(21), hour: 3, minute: 0 }));
        assert_eq!(parse_reset("10:09pm (Asia/Beirut)"), Some(ResetSpec { month: None, day: None, hour: 22, minute: 9 }));
        assert_eq!(parse_reset("12am"), Some(ResetSpec { month: None, day: None, hour: 0, minute: 0 }));
        assert_eq!(parse_reset("12:30pm"), Some(ResetSpec { month: None, day: None, hour: 12, minute: 30 }));
        assert_eq!(parse_reset("soon"), None);
        assert_eq!(parse_reset("Foo 3, 5pm"), None);
    }

    #[test]
    fn reads_the_usage_command_output() {
        // Stand-in for the local-time conversion: encode the spec so the test can see what it was given.
        let r = parse_usage(USAGE, |s| Some(i64::from(s.month.unwrap_or(0)) * 10_000 + i64::from(s.day.unwrap_or(0)) * 100 + i64::from(s.hour)));
        let s = r.session.unwrap();
        assert_eq!((s.pct, s.resets), (23.0, Some(11 * 10_000 + 14 * 100 + 22)));
        let w = r.week.unwrap();
        assert_eq!((w.pct, w.resets), (8.0, Some(11 * 10_000 + 21 * 100 + 2)));
        let (name, m) = r.week_model.unwrap();
        assert_eq!((name.as_str(), m.pct), ("Fable", 0.0));
    }

    fn relay(five: f32, updated: i64) -> ClaudeStats {
        ClaudeStats { five_hour_pct: Some(five), weekly_pct: Some(1.0), updated: Some(updated), context_pct: Some(7.0), ..Default::default() }
    }

    fn reading(five: f32) -> UsageReading {
        UsageReading { session: Some(Window { pct: five, resets: Some(9_999) }), week: Some(Window { pct: 8.0, resets: None }), week_model: Some(("Fable".into(), Window { pct: 0.0, resets: None })) }
    }

    #[test]
    fn usage_is_the_source_while_it_works_and_the_relay_keeps_context_and_cost() {
        let s = merge(Link::Connected, &relay(5.0, 1_000), Cli::Ok, Some(&reading(30.0)), Some(2_000), 2_100);
        assert_eq!(s.five_hour_pct, Some(30.0));
        assert_eq!(s.weekly_pct, Some(8.0));
        assert_eq!(s.context_pct, Some(7.0)); // not in /usage: from the relay
        assert_eq!(s.week_model.as_ref().map(|m| m.0.as_str()), Some("Fable"));
    }

    #[test]
    fn a_newer_terminal_reading_beats_an_older_usage_one() {
        let s = merge(Link::Connected, &relay(41.0, 3_000), Cli::Ok, Some(&reading(30.0)), Some(2_000), 3_100);
        assert_eq!(s.five_hour_pct, Some(41.0));
    }

    #[test]
    fn a_recent_usage_reading_is_kept_for_a_while_after_usage_stops_working() {
        let s = merge(Link::NotConnected, &ClaudeStats::default(), Cli::Failed, Some(&reading(30.0)), Some(2_000), 2_000 + STALE_AFTER - 1);
        assert_eq!(s.five_hour_pct, Some(30.0));
    }

    #[test]
    fn once_usage_is_long_gone_the_terminal_connection_takes_over() {
        let late = 2_000 + STALE_AFTER + 1;
        // With the relay connected, its numbers are shown ...
        let s = merge(Link::Connected, &relay(5.0, 1_000), Cli::Failed, Some(&reading(30.0)), Some(2_000), late);
        assert_eq!((s.five_hour_pct, s.weekly_pct), (Some(5.0), Some(1.0)));
        assert!(s.week_model.is_none());
        // ... and without it there is nothing to show, rather than an old number passed off as current.
        let none = merge(Link::NotConnected, &ClaudeStats::default(), Cli::Failed, Some(&reading(30.0)), Some(2_000), late);
        assert_eq!(none.five_hour_pct, None);
    }

    #[test]
    fn a_plan_without_limits_reads_as_nothing() {
        let r = parse_usage("You are currently using your API key to power Claude Code.\nNo usage limits apply.\n", |_| None);
        assert_eq!(r, UsageReading::default());
    }
}
