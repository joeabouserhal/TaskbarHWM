use super::cpu_pawnio::{PawnIo, Unavailable};
use super::pdh::{Counter, Query};

#[derive(Clone, Copy, Default, Debug)]
pub struct CpuStats {
    pub usage_percent: Option<f32>,
    pub clock_mhz: Option<f32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
}

pub struct CpuSampler {
    q: Query,
    utility: Option<Counter>,
    perf: Option<Counter>,
    base_mhz: Option<Counter>,
    pawn: Option<PawnIo>,
    /// Why CPU temperature/power are missing, for the settings page.
    pub pawn_status: Option<Unavailable>,
    retry: u32,
}

impl CpuSampler {
    pub fn new() -> Option<CpuSampler> {
        let q = Query::new()?;
        // "% Processor Utility" matches Task Manager; it can exceed 100 while boosting.
        let utility = q.add(r"\Processor Information(_Total)\% Processor Utility");
        let perf = q.add(r"\Processor Information(_Total)\% Processor Performance");
        let base_mhz = q.add(r"\Processor Information(_Total)\Processor Frequency");
        q.collect();
        let (pawn, pawn_status) = match PawnIo::open() {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        Some(CpuSampler { q, utility, perf, base_mhz, pawn, pawn_status, retry: 0 })
    }

    pub fn sample(&mut self) -> CpuStats {
        let mut s = CpuStats::default();
        if !self.q.collect() {
            return s;
        }
        // Not installed yet? Look again every few samples, so installing the driver takes effect immediately.
        if self.pawn.is_none() && self.pawn_status == Some(Unavailable::NotInstalled) {
            self.retry += 1;
            if self.retry % 4 == 0 {
                match PawnIo::open() {
                    Ok(p) => {
                        self.pawn = Some(p);
                        self.pawn_status = None;
                    }
                    Err(e) => self.pawn_status = Some(e),
                }
            }
        }
        if let Some(p) = self.pawn.as_mut() {
            s.temp_c = p.temperature_c();
            s.power_w = p.power_w();
        }
        s.usage_percent = self.utility.as_ref().and_then(|c| c.value()).map(|v| v.clamp(0.0, 100.0) as f32);
        if let (Some(p), Some(b)) = (
            self.perf.as_ref().and_then(|c| c.value()),
            self.base_mhz.as_ref().and_then(|c| c.value()),
        ) {
            s.clock_mhz = Some((b * p / 100.0) as f32);
        }
        s
    }
}

/// Marketing name of the CPU, e.g. "AMD Ryzen 7 7800X3D 8-Core Processor".
pub fn cpu_name() -> String {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut len),
        );
        if r.is_err() {
            return String::new();
        }
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}
