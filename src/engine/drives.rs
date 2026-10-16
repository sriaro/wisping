use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
use windows::core::PCWSTR;

const DRIVE_FIXED: u32 = 3;

pub struct Drive {
    pub name: String,
    pub total: u64,
    pub free: u64,
}

impl Drive {
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.total - self.free.min(self.total)) as f32 / self.total as f32 }
    }
}

pub fn fixed() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let name = format!("{}:", (b'A' + i as u8) as char);
            let root: Vec<u16> = format!("{}\\", name).encode_utf16().chain(std::iter::once(0)).collect();
            if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
                return None;
            }
            let (mut free, mut total) = (0u64, 0u64);
            unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free as *mut u64), Some(&mut total as *mut u64), None) }.ok()?;
            (total > 0).then_some(Drive { name, total, free })
        })
        .collect()
}
