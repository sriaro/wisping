use super::*;
#[cfg(test)]
pub(super) use crate::engine::registry::target::native_name;
use winreg::enums::KEY_READ;

pub(super) fn open(saved: &SavedKey) -> io::Result<RegKey> {
    walk(saved, true)
}

fn walk(saved: &SavedKey, write: bool) -> io::Result<RegKey> {
    let flags = KEY_READ | KEY_WOW64_64KEY | if write { KEY_WRITE } else { 0 };
    registry::target::open(saved.machine, &saved.path, flags, write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_restore_accepts_windows_registry_aliases() {
        for path in ["SYSTEM\\CurrentControlSet", "SOFTWARE\\Classes\\Wow6432Node\\AppID",
            "SOFTWARE\\Classes\\Wow6432Node\\PROTOCOLS", "SOFTWARE\\Classes\\Wow6432Node\\TypeLib"] {
            let saved = SavedKey { machine: true, path: path.to_string(), values: Vec::new() };
            walk(&saved, false).unwrap();
        }
        walk(&SavedKey { machine: false, path: "Software\\Classes".to_string(), values: Vec::new() }, false).unwrap();
    }
}
