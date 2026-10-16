use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::*;

use super::sys;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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

    pub fn root(self) -> RegKey {
        RegKey::predef(match self {
            Hive::Machine => HKEY_LOCAL_MACHINE,
            Hive::User => HKEY_CURRENT_USER,
        })
    }

    pub fn open(self, path: &str) -> io::Result<RegKey> {
        self.root().open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
    }

    pub fn open_write(self, path: &str) -> io::Result<RegKey> {
        self.root().open_subkey_with_flags(path, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)
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

fn reserve(dir: &Path) -> io::Result<PathBuf> {
    let start = fs::read_dir(dir).map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("reg"))).count()).unwrap_or(0) + 1;
    for n in start..start + 1000 {
        let file = dir.join(format!("{:04}.reg", n));
        match fs::OpenOptions::new().write(true).create_new(true).open(&file) {
            Ok(_) => return Ok(file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("could not create a backup file"))
}

fn verify(file: &Path, what: &str) -> io::Result<()> {
    match fs::metadata(file) {
        Ok(m) if m.len() >= 40 => Ok(()),
        _ => {
            let _ = fs::remove_file(file);
            Err(io::Error::other(format!("backup not written for {}", what)))
        }
    }
}

pub fn plain_name(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
}

pub fn export_key(hive: Hive, path: &str, dir: &Path) -> io::Result<()> {
    if path.is_empty() || !plain_name(path) {
        return Err(io::Error::other("bad key name"));
    }
    let file = reserve(dir)?;
    let target = format!("{}\\{}", hive.long_name(), path);
    let out = sys::hidden("reg.exe").arg("export").arg(&target).arg(&file).arg("/y").arg("/reg:64").output()?;
    if !out.status.success() {
        let _ = fs::remove_file(&file);
        return Err(io::Error::other(format!("reg export failed for {}", path)));
    }
    verify(&file, path)
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn hex(kind: u32, bytes: &[u8]) -> String {
    let body: Vec<String> = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!("hex({:x}):{}", kind, body.join(","))
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

pub fn export_value(hive: Hive, key_path: &str, value_name: &str, dir: &Path) -> io::Result<()> {
    if key_path.is_empty() || !plain_name(key_path) {
        return Err(io::Error::other("bad key name"));
    }
    let key = hive.open(key_path).map_err(|_| io::Error::other(format!("key missing {}", key_path)))?;
    let raw = key.get_raw_value(value_name)?;
    let line = value_line(value_name, &raw.vtype, &raw.bytes)?;
    let text = format!("Windows Registry Editor Version 5.00\r\n\r\n[{}\\{}]\r\n{}\r\n", hive.long_name(), key_path, line);
    let bytes: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
    let file = reserve(dir)?;
    fs::write(&file, bytes)?;
    verify(&file, value_name)
}
