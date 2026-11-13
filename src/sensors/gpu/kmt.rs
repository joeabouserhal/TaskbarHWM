//! Vendor-neutral GPU telemetry from the Windows graphics kernel (D3DKMT), the same source Task Manager uses
//! for its GPU temperature. Any WDDM driver that implements "adapter perf data" answers (NVIDIA, AMD, Intel);
//! drivers that don't simply return an error and the value stays unavailable.
//!
//! The D3DKMT calls are exports of gdi32.dll that the `windows` crate doesn't wrap, so they are bound by hand.
use std::ffi::c_void;

use windows::core::{s, w};
use windows::Win32::Foundation::LUID;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// KMTQAITYPE_ADAPTERPERFDATA
const QUERY_ADAPTER_PERF_DATA: u32 = 62;

#[repr(C)]
struct OpenFromLuid {
    luid: LUID,
    h_adapter: u32,
}

#[repr(C)]
struct QueryInfo {
    h_adapter: u32,
    kind: u32,
    data: *mut c_void,
    size: u32,
}

/// D3DKMT_ADAPTER_PERFDATA
#[repr(C)]
#[derive(Default)]
struct PerfData {
    physical_adapter_index: u32,
    memory_frequency: u64,
    max_memory_frequency: u64,
    max_memory_frequency_oc: u64,
    memory_bandwidth: u64,
    pcie_bandwidth: u64,
    fan_rpm: u32,
    power: u32,
    /// Tenths of a degree Celsius.
    temperature: u32,
    power_state_override: u8,
}

type FnOpen = unsafe extern "system" fn(*mut OpenFromLuid) -> i32;
type FnQuery = unsafe extern "system" fn(*mut QueryInfo) -> i32;
type FnClose = unsafe extern "system" fn(*const u32) -> i32;

#[derive(Clone, Copy)]
pub struct Kmt {
    open: FnOpen,
    query: FnQuery,
    close: FnClose,
}

impl Kmt {
    pub fn load() -> Option<Kmt> {
        unsafe {
            let gdi32 = GetModuleHandleW(w!("gdi32.dll")).ok()?;
            Some(Kmt {
                open: std::mem::transmute(GetProcAddress(gdi32, s!("D3DKMTOpenAdapterFromLuid"))?),
                query: std::mem::transmute(GetProcAddress(gdi32, s!("D3DKMTQueryAdapterInfo"))?),
                close: std::mem::transmute(GetProcAddress(gdi32, s!("D3DKMTCloseAdapter"))?),
            })
        }
    }

    /// Opens the adapter with the given DXGI LUID.
    pub fn adapter(&self, low: u32, high: i32) -> Option<KmtAdapter> {
        let mut req = OpenFromLuid { luid: LUID { LowPart: low, HighPart: high }, h_adapter: 0 };
        let status = unsafe { (self.open)(&mut req) };
        (status == 0 && req.h_adapter != 0).then_some(KmtAdapter { k: *self, handle: req.h_adapter })
    }
}

pub struct KmtAdapter {
    k: Kmt,
    handle: u32,
}

unsafe impl Send for KmtAdapter {}

impl KmtAdapter {
    fn perf(&self) -> Option<PerfData> {
        let mut data = PerfData::default();
        let mut q = QueryInfo {
            h_adapter: self.handle,
            kind: QUERY_ADAPTER_PERF_DATA,
            data: &mut data as *mut PerfData as *mut c_void,
            size: std::mem::size_of::<PerfData>() as u32,
        };
        let status = unsafe { (self.k.query)(&mut q) };
        (status == 0).then_some(data)
    }

    /// GPU temperature in degrees Celsius, if the driver reports one.
    pub fn temperature_c(&self) -> Option<f32> {
        let t = self.perf()?.temperature as f32 / 10.0;
        (t > 0.0 && t < 150.0).then_some(t)
    }
}

impl Drop for KmtAdapter {
    fn drop(&mut self) {
        unsafe {
            (self.k.close)(&self.handle);
        }
    }
}
