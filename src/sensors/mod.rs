pub mod cpu;
pub mod cpu_pawnio;
pub mod disk;
pub mod gpu;
mod pdh;
pub mod ram;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: cpu::CpuStats,
    pub ram: Option<ram::RamStats>,
    pub gpus: Vec<gpu::GpuStats>,
    pub disks: Vec<disk::DiskStats>,
    /// Why CPU temperature/power are unavailable (None = working or not yet known).
    pub pawn: Option<cpu_pawnio::Unavailable>,
}

pub struct Sampler {
    cpu: Option<cpu::CpuSampler>,
    gpu: gpu::GpuSampler,
    disk: disk::DiskSampler,
}

impl Sampler {
    pub fn new() -> Sampler {
        Sampler { cpu: cpu::CpuSampler::new(), gpu: gpu::GpuSampler::new(), disk: disk::DiskSampler::new() }
    }

    /// Why CPU temperature/power are unavailable (None = working).
    pub fn pawn_status(&self) -> Option<&cpu_pawnio::Unavailable> {
        self.cpu.as_ref().and_then(|c| c.pawn_status.as_ref())
    }

    pub fn sample(&mut self) -> Snapshot {
        Snapshot {
            cpu: self.cpu.as_mut().map(|c| c.sample()).unwrap_or_default(),
            ram: ram::read(),
            gpus: self.gpu.sample(),
            disks: self.disk.sample(),
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
