use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::*;

use super::{backups, fsx, registry::target, sys};

pub(super) mod integrity;

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Hive {
    Machine,
    #[default]
    User,
}

impl Hive {
    pub fn long_name(self) -> &'static str {
        match self {
            Hive::Machine => "HKEY_LOCAL_MACHINE",
            Hive::User => "HKEY_CURRENT_USER",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Hive::Machine => "HKLM",
            Hive::User => "HKCU",
        }
    }

    pub fn open(self, path: &str) -> io::Result<RegKey> {
        target::open(self == Hive::Machine, path, KEY_READ | KEY_WOW64_64KEY, false)
    }

    pub fn open_write(self, path: &str) -> io::Result<RegKey> {
        target::open(self == Hive::Machine, path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY, false)
    }

    pub fn create(self, path: &str) -> io::Result<RegKey> {
        target::open(self == Hive::Machine, path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY, true)
    }

    pub(super) fn open_delete(self, path: &str) -> io::Result<RegKey> {
        target::open(self == Hive::Machine, path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY | target::DELETE, false)
    }

    pub(super) fn tree(self, path: &str) -> io::Result<target::Tree> {
        target::Tree::open(self.open_delete(path)?, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY | target::DELETE)
    }
}

pub fn raw_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

pub fn read_string(key: &RegKey, name: &str) -> Option<String> {
    let v = key.get_raw_value(name).ok()?;
    match v.vtype {
        REG_SZ | REG_EXPAND_SZ => Some(raw_string(&v.bytes)),
        _ => None,
    }
}

pub fn read_dword(key: &RegKey, name: &str) -> Option<u32> {
    let v = key.get_raw_value(name).ok()?;
    if v.vtype == REG_DWORD && v.bytes.len() >= 4 {
        Some(u32::from_le_bytes([v.bytes[0], v.bytes[1], v.bytes[2], v.bytes[3]]))
    } else {
        None
    }
}

pub fn has_value(key: &RegKey, name: &str) -> bool {
    key.get_raw_value(name).is_ok()
}

fn reserve(dir: &fsx::PinnedDir) -> io::Result<(PathBuf, fs::File)> {
    let start = fs::read_dir(dir.path()).map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("reg"))).count()).unwrap_or(0) + 1;
    for n in start..start + 1000 {
        let name = format!("{:04}.reg", n);
        match dir.create_file(name.as_ref()) {
            Ok(file) => return Ok((dir.path().join(name), file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("could not create a backup file"))
}

fn publish(dir: &Path, text: &str) -> io::Result<()> {
    let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
    if bytes.len() as u64 > backups::MAX_FILE {
        return Err(io::Error::other("backup is too large"));
    }
    let elevated = sys::elevation()?;
    let dir = fsx::PinnedDir::open(dir)?;
    let (path, mut file) = reserve(&dir)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    if elevated {
        integrity::record(&path, &bytes)?;
    }
    Ok(())
}

pub fn plain_name(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
}

pub fn export_key(hive: Hive, path: &str, dir: &Path) -> io::Result<()> {
    if path.is_empty() || !plain_name(path) {
        return Err(io::Error::other("bad key name"));
    }
    let text = key_snapshot(hive, path)?;
    publish(dir, &text)
}

fn key_snapshot(hive: Hive, path: &str) -> io::Result<String> {
    let tree = target::Tree::open(hive.open(path)?, KEY_READ | KEY_WOW64_64KEY)?;
    tree_snapshot(hive, path, &tree)
}

pub(super) fn export_tree(hive: Hive, path: &str, tree: &target::Tree, dir: &Path) -> io::Result<()> {
    publish(dir, &tree_snapshot(hive, path, tree)?)
}

fn tree_snapshot(hive: Hive, path: &str, tree: &target::Tree) -> io::Result<String> {
    let mut text = "Windows Registry Editor Version 5.00\r\n".to_string();
    for (relative, key) in tree.entries() {
        let path = if relative.is_empty() { path.to_string() } else { format!("{path}\\{relative}") };
        if !plain_name(&path) || path.contains(['[', ']']) {
            return Err(io::Error::other("bad key name"));
        }
        text.push_str(&format!("\r\n[{}\\{}]\r\n", hive.long_name(), path));
        if text.len() as u64 > backups::MAX_FILE {
            return Err(io::Error::other("backup is too large"));
        }
        append_values(key, &mut text)?;
    }
    Ok(text)
}

fn append_values(key: &RegKey, text: &mut String) -> io::Result<()> {
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS};
    use windows::Win32::System::Registry::{HKEY, RegEnumValueW};
    use windows::core::PWSTR;

    let mut name = vec![0u16; 16384];
    let mut bytes = vec![0u8; 256];
    let mut index = 0;
    loop {
        let mut name_len = name.len() as u32;
        let mut data_len = bytes.len() as u32;
        let mut kind = 0;
        let status = unsafe { RegEnumValueW(HKEY(key.raw_handle()), index, Some(PWSTR(name.as_mut_ptr())), &mut name_len,
            None, Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut data_len)) };
        if status == ERROR_NO_MORE_ITEMS {
            return Ok(());
        }
        if data_len as u64 > backups::MAX_FILE / 6 {
            return Err(io::Error::other("registry value cannot be backed up"));
        }
        if status == ERROR_MORE_DATA {
            if data_len as usize <= bytes.len() {
                return Err(io::Error::other("registry value name is too long"));
            }
            bytes.resize(data_len as usize, 0);
            continue;
        }
        status.ok().map_err(io::Error::other)?;
        let name = String::from_utf16(&name[..name_len as usize]).map_err(io::Error::other)?;
        if !plain_name(&name) {
            return Err(io::Error::other("bad value name"));
        }
        text.push_str(&format!("{}={}\r\n", quote(&name), hex(kind, &bytes[..data_len as usize])));
        if text.len() as u64 > backups::MAX_FILE {
            return Err(io::Error::other("backup is too large"));
        }
        index += 1;
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn hex(kind: u32, bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = format!("hex({kind:x}):");
    text.reserve(bytes.len().saturating_mul(3));
    for (index, byte) in bytes.iter().enumerate() {
        if index != 0 {
            text.push(',');
        }
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

fn sz_data(bytes: &[u8]) -> String {
    let text = raw_string(bytes);
    let mut plain: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let exact = plain == bytes || {
        plain.extend_from_slice(&[0, 0]);
        plain == bytes
    };
    if exact && !text.chars().any(|c| c < ' ') { quote(&text) } else { hex(1, bytes) }
}

pub fn value_line(name: &str, vtype: &RegType, bytes: &[u8]) -> io::Result<String> {
    if !plain_name(name) {
        return Err(io::Error::other("bad value name"));
    }
    let data = match vtype {
        REG_SZ => sz_data(bytes),
        REG_EXPAND_SZ => hex(2, bytes),
        REG_DWORD => {
            let raw: [u8; 4] = bytes.get(..4).and_then(|b| b.try_into().ok()).ok_or_else(|| io::Error::other("bad dword"))?;
            format!("dword:{:08x}", u32::from_le_bytes(raw))
        }
        _ => return Err(io::Error::other("unsupported value type")),
    };
    Ok(format!("{}={}", quote(name), data))
}

pub(super) fn export_open_value(hive: Hive, key_path: &str, key: &RegKey, value_name: &str, dir: &Path) -> io::Result<()> {
    if key_path.is_empty() || !plain_name(key_path) {
        return Err(io::Error::other("bad key name"));
    }
    let raw = key.get_raw_value(value_name)?;
    let line = value_line(value_name, &raw.vtype, &raw.bytes)?;
    let text = format!("Windows Registry Editor Version 5.00\r\n\r\n[{}\\{}]\r\n{}\r\n", hive.long_name(), key_path, line);
    publish(dir, &text)
}
