//! CPU temperature and package power through the PawnIO kernel driver.
//!
//! PawnIO is installed separately (see first-run setup). We load its LGPL helper library from the
//! install directory by absolute path, hand the driver an official signed module for the detected CPU,
//! and call the module's `ioctl_read_msr` / `ioctl_read_smn` exports. Needs administrator rights.
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

static AMD_MODULE: &[u8] = include_bytes!("../../res/pawnio/AMDFamily17.bin");
static INTEL_MODULE: &[u8] = include_bytes!("../../res/pawnio/IntelMSR.bin");

type FnOpen = unsafe extern "system" fn(*mut HANDLE) -> i32;
type FnLoad = unsafe extern "system" fn(HANDLE, *const u8, usize) -> i32;
type FnExec = unsafe extern "system" fn(HANDLE, *const u8, *const u64, usize, *mut u64, usize, *mut usize) -> i32;
type FnClose = unsafe extern "system" fn(HANDLE) -> i32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    AmdZen,
    Intel,
}

pub struct PawnIo {
    handle: HANDLE,
    exec: FnExec,
    close: FnClose,
    kind: Kind,
    /// Joules per energy-counter tick.
    energy_unit: f64,
    last_energy: Option<(u64, Instant)>,
    /// Intel: Tjmax for converting the digital readout to degrees.
    tjmax: f64,
    pci_mutex: Option<HANDLE>,
}

unsafe impl Send for PawnIo {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    NotInstalled,
    UnsupportedCpu,
    AccessDenied,
    Failed,
}

struct CpuId {
    amd: bool,
    intel: bool,
    family: u32,
}

fn cpu_id() -> CpuId {
    use std::arch::x86_64::__cpuid;
    {
        let v = __cpuid(0);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&v.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&v.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&v.ecx.to_le_bytes());
        let f = __cpuid(1).eax;
        let mut family = (f >> 8) & 0xF;
        if family == 0xF {
            family += (f >> 20) & 0xFF;
        }
        CpuId { amd: &vendor == b"AuthenticAMD", intel: &vendor == b"GenuineIntel", family }
    }
}

/// Where to look for PawnIOLib.dll, in order: next to our exe (the copy we ship and test with), then the
/// PawnIO install folder. Both are absolute paths in admin-only locations, so nothing can plant a DLL.
fn lib_candidates() -> Vec<Vec<u16>> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        paths.push(dir.join("PawnIOLib.dll"));
    }
    let pf = std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| "C:\\Program Files".into());
    paths.push(std::path::PathBuf::from(pf).join("PawnIO").join("PawnIOLib.dll"));
    paths
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().encode_utf16().chain(Some(0)).collect())
        .collect()
}

impl PawnIo {
    pub fn open() -> Result<PawnIo, Unavailable> {
        let id = cpu_id();
        let (kind, module) = if id.amd && (0x17..=0x1A).contains(&id.family) {
            (Kind::AmdZen, AMD_MODULE)
        } else if id.intel {
            (Kind::Intel, INTEL_MODULE)
        } else {
            return Err(Unavailable::UnsupportedCpu);
        };
        unsafe {
            let lib = lib_candidates()
                .iter()
                .find_map(|path| {
                    LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32).ok()
                })
                .ok_or(Unavailable::NotInstalled)?;
            macro_rules! sym {
                ($n:literal, $t:ty) => {
                    match GetProcAddress(lib, windows::core::PCSTR(concat!($n, "\0").as_ptr())) {
                        Some(p) => std::mem::transmute::<_, $t>(p),
                        None => return Err(Unavailable::Failed),
                    }
                };
            }
            let open: FnOpen = sym!("pawnio_open", FnOpen);
            let load: FnLoad = sym!("pawnio_load", FnLoad);
            let exec: FnExec = sym!("pawnio_execute", FnExec);
            let close: FnClose = sym!("pawnio_close", FnClose);

            let mut h = HANDLE::default();
            let hr = open(&mut h);
            if hr < 0 {
                // E_ACCESSDENIED / ERROR_ACCESS_DENIED: not elevated. Anything else: driver not running.
                return Err(if hr as u32 == 0x80070005 { Unavailable::AccessDenied } else { Unavailable::NotInstalled });
            }
            if load(h, module.as_ptr(), module.len()) < 0 {
                close(h);
                return Err(Unavailable::UnsupportedCpu);
            }
            let mut p = PawnIo {
                handle: h,
                exec,
                close,
                kind,
                energy_unit: 1.0 / 65536.0,
                last_energy: None,
                tjmax: 100.0,
                pci_mutex: CreateMutexW(None, false, windows::core::w!("Global\\Access_PCI")).ok(),
            };
            p.init_units();
            Ok(p)
        }
    }

    fn call(&self, name: &[u8], input: &[u64]) -> Option<u64> {
        let mut out = [0u64; 1];
        let mut n = 0usize;
        let hr = unsafe { (self.exec)(self.handle, name.as_ptr(), input.as_ptr(), input.len(), out.as_mut_ptr(), 1, &mut n) };
        (hr >= 0 && n >= 1).then_some(out[0])
    }

    fn msr(&self, msr: u32) -> Option<u64> {
        self.call(b"ioctl_read_msr\0", &[msr as u64])
    }

    /// The module asks callers to hold the shared PCI-config mutex around SMN accesses.
    fn smn(&self, offset: u32) -> Option<u64> {
        unsafe {
            let locked = match self.pci_mutex {
                Some(m) => {
                    let r = WaitForSingleObject(m, 100);
                    if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
                        return None;
                    }
                    true
                }
                None => false,
            };
            let v = self.call(b"ioctl_read_smn\0", &[offset as u64]);
            if locked {
                let _ = ReleaseMutex(self.pci_mutex.unwrap());
            }
            v
        }
    }

    fn init_units(&mut self) {
        match self.kind {
            Kind::AmdZen => {
                // MSR_PWR_UNIT: bits 12:8 = energy status unit (1 / 2^n J).
                if let Some(v) = self.msr(0xC001_0299) {
                    self.energy_unit = 1.0 / (1u64 << ((v >> 8) & 0x1F)) as f64;
                }
            }
            Kind::Intel => {
                // MSR_RAPL_POWER_UNIT: bits 12:8 = energy unit. MSR_TEMPERATURE_TARGET bits 23:16 = Tjmax.
                if let Some(v) = self.msr(0x606) {
                    self.energy_unit = 1.0 / (1u64 << ((v >> 8) & 0x1F)) as f64;
                }
                if let Some(v) = self.msr(0x1A2) {
                    let t = (v >> 16) & 0xFF;
                    if t != 0 {
                        self.tjmax = t as f64;
                    }
                }
            }
        }
    }

    pub fn temperature_c(&self) -> Option<f32> {
        match self.kind {
            Kind::AmdZen => {
                // THM_TCON_CUR_TMP: bits 31:21 = temperature in 0.125 C steps; bit 19 selects the -49 C range.
                let v = self.smn(0x0005_9800)? as u32;
                let mut t = ((v >> 21) & 0x7FF) as f64 / 8.0;
                if v & (1 << 19) != 0 {
                    t -= 49.0;
                }
                (t > 0.0 && t < 150.0).then_some(t as f32)
            }
            Kind::Intel => {
                // IA32_PACKAGE_THERM_STATUS: bits 22:16 = digital readout, degrees below Tjmax.
                let v = self.msr(0x1B1)?;
                if v & (1 << 31) == 0 {
                    return None; // reading not valid
                }
                Some((self.tjmax - ((v >> 16) & 0x7F) as f64) as f32)
            }
        }
    }

    /// Package power from the energy counter delta since the previous call.
    pub fn power_w(&mut self) -> Option<f32> {
        let reg = match self.kind {
            Kind::AmdZen => 0xC001_029B, // PKG_ENERGY_STAT
            Kind::Intel => 0x611,        // MSR_PKG_ENERGY_STATUS
        };
        let raw = self.msr(reg)? & 0xFFFF_FFFF;
        let now = Instant::now();
        let prev = self.last_energy.replace((raw, now));
        let (p_raw, p_t) = prev?;
        let ticks = if raw >= p_raw { raw - p_raw } else { raw + (1u64 << 32) - p_raw }; // 32-bit counter wraps
        let dt = now.duration_since(p_t).as_secs_f64();
        (dt > 0.05).then(|| (ticks as f64 * self.energy_unit / dt) as f32)
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.handle);
            if let Some(m) = self.pci_mutex {
                let _ = CloseHandle(m);
            }
        }
    }
}
