//! Local disks (drive letters): volume name, used and total space.
//!
//! Only fixed and removable drives are listed. Network and optical drives are skipped on purpose: asking a
//! disconnected network share for its free space can block for a long time. Space changes slowly, so the list
//! is refreshed every few seconds instead of on every sensor tick.
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
use windows::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, SEM_FAILCRITICALERRORS};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const REFRESH: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default)]
pub struct DiskStats {
    /// Drive letter with colon, e.g. "C:".
    pub root: String,
    /// Volume label ("" if the volume has none).
    pub label: String,
    pub total: u64,
    /// Bytes available to the current user.
    pub free: u64,
    pub removable: bool,
}

impl DiskStats {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    /// Fraction of the disk that is full, 0..=1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.used() as f64 / self.total as f64).clamp(0.0, 1.0) as f32
        }
    }
}

pub struct DiskSampler {
    cache: Vec<DiskStats>,
    last: Option<Instant>,
}

impl DiskSampler {
    pub fn new() -> DiskSampler {
        // Never pop up "There is no disk in the drive" style dialogs for empty card readers.
        unsafe {
            let _ = SetThreadErrorMode(SEM_FAILCRITICALERRORS, None);
        }
        DiskSampler { cache: Vec::new(), last: None }
    }

    pub fn sample(&mut self) -> Vec<DiskStats> {
        if self.last.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.cache = query();
            self.last = Some(Instant::now());
        }
        self.cache.clone()
    }
}

fn query() -> Vec<DiskStats> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
        let root_p = PCWSTR(root.as_ptr());
        let kind = unsafe { GetDriveTypeW(root_p) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // Fails for empty card readers and locked (BitLocker) volumes: just skip those.
        if unsafe { GetDiskFreeSpaceExW(root_p, Some(&mut avail), Some(&mut total), Some(&mut free)) }.is_err() || total == 0 {
            continue;
        }
        let mut name = [0u16; 64];
        let label = unsafe { GetVolumeInformationW(root_p, Some(&mut name), None, None, None, None) }
            .ok()
            .map(|_| {
                let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                String::from_utf16_lossy(&name[..n])
            })
            .unwrap_or_default();
        out.push(DiskStats { root: format!("{letter}:"), label, total, free: avail, removable: kind == DRIVE_REMOVABLE });
    }
    out
}
