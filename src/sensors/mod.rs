pub mod claude;
pub mod cpu;
pub mod cpu_pawnio;
pub mod disk;
pub mod gpu;
pub mod net;
mod pdh;
pub mod ram;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: cpu::CpuStats,
    pub ram: Option<ram::RamStats>,
    pub gpus: Vec<gpu::GpuStats>,
    pub disks: Vec<disk::DiskStats>,
    pub net: net::NetStats,
    pub claude: claude::ClaudeStats,
    /// Why CPU temperature/power are unavailable (None = working or not yet known).
    pub pawn: Option<cpu_pawnio::Unavailable>,
}

pub struct Sampler {
    cpu: Option<cpu::CpuSampler>,
    gpu: gpu::GpuSampler,
    disk: disk::DiskSampler,
    net: net::NetSampler,
    claude: claude::ClaudeSampler,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler { cpu: cpu::CpuSampler::new(), gpu: gpu::GpuSampler::new(), disk: disk::DiskSampler::new(), net: net::NetSampler::new(), claude: claude::ClaudeSampler::new() }
    }

    /// Why CPU temperature/power are unavailable (None = working).
    pub fn pawn_status(&self) -> Option<&cpu_pawnio::Unavailable> {
        self.cpu.as_ref().and_then(|c| c.pawn_status.as_ref())
    }

    pub fn sample(&mut self) -> Snapshot {
        let mut gpus = self.gpu.sample();
        // Intel integrated graphics have no vendor DLL here: their power comes from the CPU's RAPL counters.
        let intel_igpu = |g: &gpu::GpuStats| g.vendor == gpu::Vendor::Intel && g.integrated && g.power_w.is_none();
        if gpus.iter().any(intel_igpu) {
            if let Some(w) = self.cpu.as_mut().and_then(|c| c.igpu_power()) {
                for g in gpus.iter_mut().filter(|g| intel_igpu(g)) {
                    g.power_w = Some(w);
                }
            }
        }
        Snapshot {
            cpu: self.cpu.as_mut().map(|c| c.sample()).unwrap_or_default(),
            ram: ram::read(),
            gpus,
            disks: self.disk.sample(),
            net: self.net.sample(),
            claude: self.claude.sample(),
            pawn: self.pawn_status().copied(),
        }
    }
}

/// Hands untouched pages (driver init data from NVML etc.) back to the OS. They fault back in on demand,
/// so this is cheap; it keeps the resident footprint at what a sampling tick actually uses.
pub fn trim_working_set() {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}
