//! One-click install of the PawnIO driver (needed for CPU temperature and power).
//!
//! The app runs elevated, so the installer it launches does too. To keep that safe:
//!  * the installer is downloaded into a brand-new folder only SYSTEM and Administrators can write to,
//!    so nothing unelevated can swap the file between checking it and running it;
//!  * its SHA-256 must match the value pinned below (the digest GitHub publishes for the official
//!    namazso/PawnIO.Setup 2.2.0 release asset), otherwise it is deleted and never executed.
//! Bumping the PawnIO version means updating URL, size and hash together.
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{LocalFree, HLOCAL, HWND, LPARAM, WPARAM};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::Cryptography::*;
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::CreateDirectoryW;
use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const URL: &str = "https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe";
const SIZE: u64 = 3_410_960;
const SHA256_HEX: &str = "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032";

/// Posted to the widget window when the install state changes.
pub const WM_SETUP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupState {
    Idle,
    Working(&'static str),
    /// Installed. `reboot` means Windows asked for a restart before the driver loads.
    Done { reboot: bool },
    Failed(String),
}

static STATE: Mutex<SetupState> = Mutex::new(SetupState::Idle);

pub fn state() -> SetupState {
    STATE.lock().map(|s| s.clone()).unwrap_or(SetupState::Idle)
}

fn set(s: SetupState, notify: isize) {
    if let Ok(mut g) = STATE.lock() {
        *g = s;
    }
    unsafe {
        let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_SETUP, WPARAM(0), LPARAM(0));
    }
}

/// Starts the install on a worker thread. `notify` is the widget HWND, which is told about every state change.
pub fn start_install(notify: isize) {
    {
        let Ok(mut g) = STATE.lock() else { return };
        if matches!(*g, SetupState::Working(_)) {
            return;
        }
        *g = SetupState::Working("Downloading...");
    }
    std::thread::spawn(move || {
        let result = run(notify);
        match result {
            Ok(reboot) => set(SetupState::Done { reboot }, notify),
            Err(e) => set(SetupState::Failed(e), notify),
        }
    });
}

fn run(notify: isize) -> Result<bool, String> {
    let dir = make_secure_dir()?;
    let exe = dir.join("PawnIO_setup.exe");
    let outcome = (|| {
        set(SetupState::Working("Downloading..."), notify);
        download(&exe)?;
        set(SetupState::Working("Verifying..."), notify);
        verify(&exe)?;
        set(SetupState::Working("Installing..."), notify);
        install(&exe)
    })();
    // Best-effort cleanup regardless of outcome.
    let _ = std::fs::remove_file(&exe);
    let _ = std::fs::remove_dir(&dir);
    outcome
}

fn make_secure_dir() -> Result<PathBuf, String> {
    let base = PathBuf::from(std::env::var_os("ProgramData").ok_or("ProgramData is not set")?).join("TaskbarHWM");
    std::fs::create_dir_all(&base).map_err(|e| format!("Couldn't create a work folder ({e})"))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    // A fresh, uniquely named folder each run: CreateDirectory fails if it already exists, so we never inherit a
    // pre-planted folder or its permissions.
    let dir = base.join(format!("setup-{}-{stamp}", std::process::id()));
    unsafe {
        // Protected DACL: full control for SYSTEM and Administrators only.
        let mut sd = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"), SDDL_REVISION_1, &mut sd, None)
            .map_err(|e| format!("Couldn't set folder permissions ({e})"))?;
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
        let r = CreateDirectoryW(&HSTRING::from(dir.as_os_str()), Some(&sa));
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        r.map_err(|e| format!("Couldn't create a work folder ({e})"))?;
    }
    Ok(dir)
}

fn download(dest: &Path) -> Result<(), String> {
    unsafe {
        URLDownloadToFileW(None, &HSTRING::from(URL), &HSTRING::from(dest.as_os_str()), 0, None)
            .map_err(|_| "Download failed. Check your internet connection.".to_string())
    }
}

fn verify(file: &Path) -> Result<(), String> {
    let bytes = std::fs::read(file).map_err(|_| "Couldn't read the download".to_string())?;
    if bytes.len() as u64 != SIZE {
        return Err("The download has the wrong size.".into());
    }
    let digest = sha256(&bytes).ok_or("Couldn't verify the download")?;
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != SHA256_HEX {
        return Err("The download failed its integrity check, so it was not run.".into());
    }
    Ok(())
}

fn sha256(data: &[u8]) -> Option<[u8; 32]> {
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_SHA256_ALGORITHM, PCWSTR::null(), BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0)).ok().ok()?;
        let mut h = BCRYPT_HASH_HANDLE::default();
        let mut out = [0u8; 32];
        let ok = BCryptCreateHash(alg, &mut h, None, None, 0).ok().is_ok()
            && BCryptHashData(h, data, 0).ok().is_ok()
            && BCryptFinishHash(h, &mut out, 0).ok().is_ok();
        if !h.is_invalid() {
            let _ = BCryptDestroyHash(h);
        }
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        ok.then_some(out)
    }
}

/// Runs the official installer in silent mode. Exit code 0 = done, 3010 = done but a restart is required.
fn install(exe: &Path) -> Result<bool, String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = Command::new(exe)
        .args(["-install", "-silent"])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|_| "Couldn't start the installer".to_string())?;
    match status.code() {
        Some(0) => Ok(false),
        Some(3010) | Some(1641) => Ok(true),
        Some(c) => Err(format!("The installer stopped with code {c}.")),
        None => Err("The installer was interrupted.".into()),
    }
}

/// Dev check for `--verify-installer`: runs the secure-folder, download and hash steps and reports, without ever
/// executing the installer. Also proves a tampered copy is rejected and shows who can access the folder.
pub fn selftest() -> String {
    use std::fmt::Write;
    let mut log = String::new();
    let dir = match make_secure_dir() {
        Ok(d) => d,
        Err(e) => return format!("make_secure_dir failed: {e}\n"),
    };
    let _ = writeln!(log, "secure dir: {}", dir.display());
    if let Ok(o) = Command::new("icacls").arg(&dir).creation_flags(0x0800_0000).output() {
        let _ = writeln!(log, "permissions:\n{}", String::from_utf8_lossy(&o.stdout).trim());
    }
    let exe = dir.join("PawnIO_setup.exe");
    match download(&exe) {
        Ok(()) => {
            let size = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
            let _ = writeln!(log, "download: ok ({size} bytes)");
            let _ = writeln!(log, "verify (genuine file): {:?}", verify(&exe));
            // A copy with one flipped byte must be rejected.
            if let Ok(mut b) = std::fs::read(&exe) {
                let n = b.len() / 2;
                b[n] ^= 0xFF;
                let bad = dir.join("tampered.exe");
                let _ = std::fs::write(&bad, b);
                let _ = writeln!(log, "verify (tampered copy): {:?}", verify(&bad));
                let _ = std::fs::remove_file(&bad);
            }
        }
        Err(e) => {
            let _ = writeln!(log, "download: FAILED {e}");
        }
    }
    let _ = std::fs::remove_file(&exe);
    let _ = std::fs::remove_dir(&dir);
    let _ = writeln!(log, "cleaned up: {}", !dir.exists());
    log
}
