use super::*;
use winreg::enums::KEY_READ;

pub(super) fn open(saved: &SavedKey) -> io::Result<RegKey> {
    let flags = KEY_READ | KEY_WOW64_64KEY | KEY_WRITE;
    registry::target::open(saved.machine, &saved.path, flags, true)
}
