//! Minimal NVML binding. nvml.dll ships with the NVIDIA driver (System32), nothing to install.
use std::ffi::c_void;
use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

type Dev = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct Util {
    gpu: u32,
    memory: u32,
}
#[repr(C)]
#[derive(Default)]
struct Mem {
    total: u64,
    free: u64,
    used: u64,
}

type FnInit = unsafe extern "C" fn() -> i32;
type FnCount = unsafe extern "C" fn(*mut u32) -> i32;
type FnHandle = unsafe extern "C" fn(u32, *mut Dev) -> i32;
type FnName = unsafe extern "C" fn(Dev, *mut u8, u32) -> i32;
type FnUtil = unsafe extern "C" fn(Dev, *mut Util) -> i32;
type FnU32 = unsafe extern "C" fn(Dev, *mut u32) -> i32;
type FnTemp = unsafe extern "C" fn(Dev, u32, *mut u32) -> i32;
type FnClock = unsafe extern "C" fn(Dev, u32, *mut u32) -> i32;
type FnMem = unsafe extern "C" fn(Dev, *mut Mem) -> i32;

pub struct Nvml {
    handles: Vec<Dev>,
    names: Vec<String>,
    util: FnUtil,
    temp: FnTemp,
    power: FnU32,
    clock: FnClock,
    mem: FnMem,
    fan: Option<FnU32>,
}

unsafe impl Send for Nvml {}

#[derive(Default, Clone, Copy, Debug)]
pub struct NvStats {
    pub usage: Option<f32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub core_mhz: Option<f32>,
    pub mem_mhz: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub fan_percent: Option<f32>,
}

impl Nvml {
    pub fn load() -> Option<Nvml> {
        unsafe {
            let lib = LoadLibraryExW(w!("nvml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            macro_rules! sym {
                ($n:expr, $t:ty) => {
                    std::mem::transmute::<_, $t>(GetProcAddress(lib, $n)?)
                };
            }
            let init: FnInit = sym!(s!("nvmlInit_v2"), FnInit);
            let count: FnCount = sym!(s!("nvmlDeviceGetCount_v2"), FnCount);
            let handle: FnHandle = sym!(s!("nvmlDeviceGetHandleByIndex_v2"), FnHandle);
            let name: FnName = sym!(s!("nvmlDeviceGetName"), FnName);
            let util: FnUtil = sym!(s!("nvmlDeviceGetUtilizationRates"), FnUtil);
            let temp: FnTemp = sym!(s!("nvmlDeviceGetTemperature"), FnTemp);
            let power: FnU32 = sym!(s!("nvmlDeviceGetPowerUsage"), FnU32);
            let clock: FnClock = sym!(s!("nvmlDeviceGetClockInfo"), FnClock);
            let mem: FnMem = sym!(s!("nvmlDeviceGetMemoryInfo"), FnMem);
            let fan = GetProcAddress(lib, s!("nvmlDeviceGetFanSpeed")).map(|p| std::mem::transmute::<_, FnU32>(p));

            if init() != 0 {
                return None;
            }
            let mut n = 0u32;
            if count(&mut n) != 0 || n == 0 {
                return None;
            }
            let (mut handles, mut names) = (Vec::new(), Vec::new());
            for i in 0..n {
                let mut h: Dev = std::ptr::null_mut();
                if handle(i, &mut h) != 0 {
                    continue;
                }
                let mut buf = [0u8; 96];
                let nm = if name(h, buf.as_mut_ptr(), buf.len() as u32) == 0 {
                    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                    String::from_utf8_lossy(&buf[..end]).into_owned()
                } else {
                    String::new()
                };
                handles.push(h);
                names.push(nm);
            }
            Some(Nvml { handles, names, util, temp, power, clock, mem, fan })
        }
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn read(&self, idx: usize) -> NvStats {
        let h = self.handles[idx];
        let mut s = NvStats::default();
        unsafe {
            let mut u = Util::default();
            if (self.util)(h, &mut u) == 0 {
                s.usage = Some(u.gpu as f32);
            }
            let mut v = 0u32;
            if (self.temp)(h, 0, &mut v) == 0 {
                s.temp_c = Some(v as f32);
            }
            if (self.power)(h, &mut v) == 0 {
                s.power_w = Some(v as f32 / 1000.0);
            }
            if (self.clock)(h, 0, &mut v) == 0 {
                s.core_mhz = Some(v as f32);
            }
            if (self.clock)(h, 2, &mut v) == 0 {
                s.mem_mhz = Some(v as f32);
            }
            let mut m = Mem::default();
            if (self.mem)(h, &mut m) == 0 {
                s.vram_used = Some(m.used);
                s.vram_total = Some(m.total);
            }
            if let Some(f) = self.fan {
                if f(h, &mut v) == 0 {
                    s.fan_percent = Some(v as f32);
                }
            }
        }
        s
    }
}
