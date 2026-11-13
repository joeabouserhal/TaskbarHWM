//! Minimal AMD ADLX binding. amdadlx64.dll ships with the Radeon driver (System32), nothing to install.
//!
//! ADLX is a COM-like C++ interface: every object is a pointer to a pointer to a table of functions. Only the few
//! entries needed for live GPU metrics are bound, by their position in the (MIT-licensed, public) SDK headers.
use std::ffi::{c_void, CStr};

use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

type Obj = *mut c_void;

type FnInit = unsafe extern "C" fn(u64, *mut Obj) -> i32;
type FnOutPtr = unsafe extern "system" fn(Obj, *mut Obj) -> i32;
type FnSize = unsafe extern "system" fn(Obj) -> u32;
type FnAt = unsafe extern "system" fn(Obj, u32, *mut Obj) -> i32;
type FnMetrics = unsafe extern "system" fn(Obj, Obj, *mut Obj) -> i32;
type FnOutInt = unsafe extern "system" fn(Obj, *mut i32) -> i32;
type FnOutDouble = unsafe extern "system" fn(Obj, *mut f64) -> i32;
type FnRelease = unsafe extern "system" fn(Obj) -> i32;

/// Entry `idx` of the function table behind `obj`.
unsafe fn entry<T: Copy>(obj: Obj, idx: usize) -> T {
    let table = *(obj as *const *const usize);
    std::mem::transmute_copy(&*table.add(idx))
}

// Positions in the function tables (each starts with Acquire, Release, QueryInterface where the interface has them).
const SYS_GET_GPUS: usize = 1;
const SYS_GET_PERF_SERVICES: usize = 9;
const LIST_SIZE: usize = 3;
const LIST_AT_GPU: usize = 11;
const GPU_TYPE: usize = 5;
const GPU_NAME: usize = 7;
const PERF_CURRENT_GPU_METRICS: usize = 18;
const REL: usize = 1;
const M_USAGE: usize = 4;
const M_CLOCK: usize = 5;
const M_VRAM_CLOCK: usize = 6;
const M_TEMP: usize = 7;
const M_POWER: usize = 9;

/// ADLX_GPU_TYPE: 0 undefined, 1 integrated, 2 discrete.
const GPUTYPE_INTEGRATED: i32 = 1;

struct Gpu {
    obj: Obj,
    name: String,
    integrated: bool,
}

pub struct Adlx {
    perf: Obj,
    gpus: Vec<Gpu>,
}

// Only ever used from the sampler thread.
unsafe impl Send for Adlx {}

#[derive(Default, Clone, Copy, Debug)]
pub struct AdlxStats {
    pub usage: Option<f32>,
    pub temp_c: Option<f32>,
    /// None for integrated GPUs: ADLX reports the whole APU socket's power there, not the graphics part's.
    pub power_w: Option<f32>,
    pub core_mhz: Option<f32>,
    pub mem_mhz: Option<f32>,
}

impl Adlx {
    pub fn load() -> Option<Adlx> {
        unsafe {
            let lib = LoadLibraryExW(w!("amdadlx64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let init: FnInit = std::mem::transmute(GetProcAddress(lib, s!("ADLXInitialize"))?);
            // Interface version 1.0 is understood by every driver that ships ADLX.
            let mut sys: Obj = std::ptr::null_mut();
            if init(1u64 << 48, &mut sys) != 0 || sys.is_null() {
                return None;
            }
            let mut list: Obj = std::ptr::null_mut();
            if entry::<FnOutPtr>(sys, SYS_GET_GPUS)(sys, &mut list) != 0 || list.is_null() {
                return None;
            }
            let mut perf: Obj = std::ptr::null_mut();
            if entry::<FnOutPtr>(sys, SYS_GET_PERF_SERVICES)(sys, &mut perf) != 0 || perf.is_null() {
                return None;
            }
            let mut gpus = Vec::new();
            for i in 0..entry::<FnSize>(list, LIST_SIZE)(list) {
                let mut g: Obj = std::ptr::null_mut();
                if entry::<FnAt>(list, LIST_AT_GPU)(list, i, &mut g) != 0 || g.is_null() {
                    continue;
                }
                let mut name: *const i8 = std::ptr::null();
                let named = entry::<FnOutPtr>(g, GPU_NAME)(g, &mut name as *mut *const i8 as *mut Obj) == 0 && !name.is_null();
                let mut kind = 0i32;
                entry::<FnOutInt>(g, GPU_TYPE)(g, &mut kind);
                gpus.push(Gpu {
                    obj: g,
                    name: if named { CStr::from_ptr(name).to_string_lossy().into_owned() } else { String::new() },
                    integrated: kind == GPUTYPE_INTEGRATED,
                });
            }
            (!gpus.is_empty()).then_some(Adlx { perf, gpus })
        }
    }

    /// (name, integrated) of each GPU ADLX knows, in its own order.
    pub fn gpus(&self) -> Vec<(&str, bool)> {
        self.gpus.iter().map(|g| (g.name.as_str(), g.integrated)).collect()
    }

    /// Live metrics of GPU `index` (into [`Adlx::gpus`]).
    pub fn read(&self, index: usize) -> AdlxStats {
        let mut out = AdlxStats::default();
        let Some(gpu) = self.gpus.get(index) else { return out };
        unsafe {
            let mut m: Obj = std::ptr::null_mut();
            if entry::<FnMetrics>(self.perf, PERF_CURRENT_GPU_METRICS)(self.perf, gpu.obj, &mut m) != 0 || m.is_null() {
                return out;
            }
            let dbl = |idx: usize| -> Option<f32> {
                let mut v = 0f64;
                (entry::<FnOutDouble>(m, idx)(m, &mut v) == 0 && v.is_finite()).then_some(v as f32)
            };
            let int = |idx: usize| -> Option<f32> {
                let mut v = 0i32;
                (entry::<FnOutInt>(m, idx)(m, &mut v) == 0 && v > 0).then_some(v as f32)
            };
            out.usage = dbl(M_USAGE).map(|v| v.clamp(0.0, 100.0));
            out.temp_c = dbl(M_TEMP).filter(|t| *t > 0.0 && *t < 150.0);
            out.core_mhz = int(M_CLOCK);
            out.mem_mhz = int(M_VRAM_CLOCK);
            if !gpu.integrated {
                out.power_w = dbl(M_POWER).filter(|p| *p > 0.0);
            }
            entry::<FnRelease>(m, REL)(m);
        }
        out
    }
}
