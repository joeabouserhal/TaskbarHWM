//! Thin wrapper over the Windows Performance Data Helper (the same counters Task Manager uses).
use std::collections::HashMap;
use windows::core::{w, PCWSTR};
use windows::Win32::System::Performance::*;

const PDH_MORE_DATA: u32 = 0x800007D2;

pub struct Query {
    handle: PDH_HQUERY,
}

pub struct Counter(PDH_HCOUNTER);

impl Query {
    pub fn new() -> Option<Query> {
        let mut h = PDH_HQUERY::default();
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut h) != 0 {
                return None;
            }
        }
        Some(Query { handle: h })
    }

    /// `path` is the English counter path, e.g. `\Processor Information(_Total)\% Processor Utility`.
    pub fn add(&self, path: &str) -> Option<Counter> {
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut c = PDH_HCOUNTER::default();
        unsafe {
            if PdhAddEnglishCounterW(self.handle, PCWSTR(wide.as_ptr()), 0, &mut c) != 0 {
                return None;
            }
        }
        Some(Counter(c))
    }

    pub fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.handle) == 0 }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.handle);
        }
    }
}

impl Counter {
    pub fn value(&self) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        unsafe {
            if PdhGetFormattedCounterValue(self.0, PDH_FMT_DOUBLE, None, &mut v) != 0 || v.CStatus != 0 {
                return None;
            }
            Some(v.Anonymous.doubleValue)
        }
    }

    /// For wildcard counters: instance name -> value.
    pub fn array(&self) -> HashMap<String, f64> {
        let mut out = HashMap::new();
        let (mut size, mut count) = (0u32, 0u32);
        unsafe {
            let r = PdhGetFormattedCounterArrayW(self.0, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if r != PDH_MORE_DATA || size == 0 {
                return out;
            }
            // u64 backing store keeps the item array 8-byte aligned.
            let mut buf = vec![0u64; (size as usize + 7) / 8];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.0, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return out;
            }
            for i in 0..count as usize {
                let it = &*items.add(i);
                if it.FmtValue.CStatus != 0 {
                    continue;
                }
                let name = it.szName.to_string().unwrap_or_default();
                out.insert(name, it.FmtValue.Anonymous.doubleValue);
            }
        }
        out
    }
}

#[allow(dead_code)]
pub fn _unused() -> PCWSTR {
    w!("")
}
