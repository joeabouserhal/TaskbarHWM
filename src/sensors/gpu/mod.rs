mod nvml;

use super::pdh::{Counter, Query};
use std::collections::HashMap;
use windows::Win32::Graphics::Dxgi::*;

#[derive(Clone, Debug, Default)]
pub struct GpuStats {
    pub name: String,
    pub vendor: Vendor,
    pub luid: String,
    pub usage_percent: Option<f32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub core_mhz: Option<f32>,
    pub mem_mhz: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub fan_percent: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Vendor {
    #[default]
    Other,
    Nvidia,
    Amd,
    Intel,
}

struct Adapter {
    name: String,
    vendor: Vendor,
    luid: (u32, i32),
    dedicated: u64,
    /// Index into the NVML device list when this adapter is an NVIDIA GPU we can query.
    nv_index: Option<usize>,
}

pub struct GpuSampler {
    adapters: Vec<Adapter>,
    nvml: Option<nvml::Nvml>,
    q: Option<Query>,
    engine: Option<Counter>,
    vram: Option<Counter>,
}

fn luid_key(l: (u32, i32)) -> String {
    format!("luid_0x{:08X}_0x{:08X}", l.1 as u32, l.0)
}

fn luid_of(instance: &str) -> Option<String> {
    let l = instance.find("luid_")?;
    Some(instance[l..].split("_phys").next()?.to_string())
}

impl GpuSampler {
    pub fn new() -> GpuSampler {
        let nvml = nvml::Nvml::load();
        let mut adapters = Vec::new();
        unsafe {
            if let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() {
                let mut i = 0;
                while let Ok(a) = f.EnumAdapters1(i) {
                    i += 1;
                    let Ok(d) = a.GetDesc1() else { continue };
                    if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                        continue;
                    }
                    let end = d.Description.iter().position(|&c| c == 0).unwrap_or(128);
                    let vendor = match d.VendorId {
                        0x10DE => Vendor::Nvidia,
                        0x1002 | 0x1022 => Vendor::Amd,
                        0x8086 => Vendor::Intel,
                        _ => Vendor::Other,
                    };
                    adapters.push(Adapter {
                        name: String::from_utf16_lossy(&d.Description[..end]),
                        vendor,
                        luid: (d.AdapterLuid.LowPart, d.AdapterLuid.HighPart),
                        dedicated: d.DedicatedVideoMemory as u64,
                        nv_index: None,
                    });
                }
            }
        }
        // Pair DXGI NVIDIA adapters with NVML devices: by name first, then by order.
        if let Some(n) = &nvml {
            let mut used = vec![false; n.names().len()];
            for a in adapters.iter_mut().filter(|a| a.vendor == Vendor::Nvidia) {
                let pos = n
                    .names()
                    .iter()
                    .enumerate()
                    .position(|(i, nm)| !used[i] && *nm == a.name)
                    .or_else(|| used.iter().position(|u| !u));
                if let Some(p) = pos {
                    used[p] = true;
                    a.nv_index = Some(p);
                }
            }
        }
        let q = Query::new();
        let (engine, vram) = match &q {
            Some(q) => {
                let e = q.add(r"\GPU Engine(*)\Utilization Percentage");
                let v = q.add(r"\GPU Adapter Memory(*)\Dedicated Usage");
                q.collect();
                (e, v)
            }
            None => (None, None),
        };
        GpuSampler { adapters, nvml, q, engine, vram }
    }

    pub fn sample(&mut self) -> Vec<GpuStats> {
        // Generic fallback data (any vendor): usage per adapter and dedicated VRAM use.
        let mut usage: HashMap<String, f32> = HashMap::new();
        let mut vram: HashMap<String, u64> = HashMap::new();
        if let Some(q) = &self.q {
            if q.collect() {
                if let Some(c) = &self.engine {
                    // Sum per (adapter, engine type), then take the busiest engine type: what Task Manager shows.
                    let mut per: HashMap<(String, String), f64> = HashMap::new();
                    for (inst, v) in c.array() {
                        if let (Some(luid), Some(t)) = (luid_of(&inst), inst.find("engtype_")) {
                            *per.entry((luid, inst[t + 8..].to_string())).or_default() += v;
                        }
                    }
                    for ((luid, _), v) in per {
                        let e = usage.entry(luid).or_default();
                        *e = e.max(v as f32);
                    }
                }
                if let Some(c) = &self.vram {
                    for (inst, v) in c.array() {
                        if let Some(luid) = luid_of(&inst) {
                            *vram.entry(luid).or_default() += v as u64;
                        }
                    }
                }
            }
        }

        self.adapters
            .iter()
            .map(|a| {
                let key = luid_key(a.luid);
                let mut g = GpuStats {
                    name: a.name.clone(),
                    vendor: a.vendor,
                    luid: key.clone(),
                    usage_percent: usage.get(&key).map(|v| v.clamp(0.0, 100.0)),
                    vram_used: vram.get(&key).copied(),
                    vram_total: (a.dedicated > 0).then_some(a.dedicated),
                    ..Default::default()
                };
                // Vendor DLL data overrides the generic values where available.
                if let (Some(n), Some(i)) = (&self.nvml, a.nv_index) {
                    let s = n.read(i);
                    g.usage_percent = s.usage.or(g.usage_percent);
                    g.temp_c = s.temp_c;
                    g.power_w = s.power_w;
                    g.core_mhz = s.core_mhz;
                    g.mem_mhz = s.mem_mhz;
                    g.vram_used = s.vram_used.or(g.vram_used);
                    g.vram_total = s.vram_total.or(g.vram_total);
                    g.fan_percent = s.fan_percent;
                }
                g
            })
            .collect()
    }
}
