use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

#[derive(Clone, Copy, Default, Debug)]
pub struct RamStats {
    pub used_bytes: u64,
    pub total_bytes: u64,
    pub percent: f32,
}

pub fn read() -> Option<RamStats> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()? };
    Some(RamStats {
        used_bytes: m.ullTotalPhys - m.ullAvailPhys,
        total_bytes: m.ullTotalPhys,
        percent: m.dwMemoryLoad as f32,
    })
}
