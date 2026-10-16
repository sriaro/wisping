use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::data;
use super::fsx;
use super::registry;
use super::sys;

const HEADER: &str = "Windows Registry Editor Version 5.00";
const MAX_FILE: u64 = 16 * 1024 * 1024;
const CHILD_ONLY: [&str; 3] = [
    "HKEY_CURRENT_USER\\Software\\Classes",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes",
    "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Services",
];

const OWN_KEYS: [&str; 18] = [
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\SharedDLLs",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run",
    "HKEY_CURRENT_USER\\Software\\Classes",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes",
    "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Services",
    "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings",
    "HKEY_CURRENT_USER\\Control Panel\\NotifyIconSettings",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Microsoft\\Windows\\Device Metadata",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Microsoft\\Windows\\Windows Error Reporting",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Device Metadata",
];

fn own_key(key: &str) -> bool {
    let k = key.to_lowercase();
    let test = registry::allow_test_key().then(|| format!("HKEY_CURRENT_USER\\{}", registry::TEST_KEY));
    OWN_KEYS.iter().copied().chain(test.as_deref()).map(str::to_lowercase).any(|p| {
        let child_only = CHILD_ONLY.iter().any(|c| c.eq_ignore_ascii_case(&p));
        (k == p && !child_only) || fsx::key_within(&k, &p)
    })
}

fn plain_file(meta: &fs::Metadata) -> bool {
    meta.is_file() && !fsx::attrs_reparse(meta.file_attributes())
}

fn quoted_name_end(line: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in line.char_indices().skip(1) {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(i + 1),
            _ => {}
        }
    }
    None
}

fn value_data(line: &str) -> Option<&str> {
    let rest = if let Some(rest) = line.strip_prefix('@') { rest } else { &line[quoted_name_end(line)?..] };
    rest.trim_start().strip_prefix('=').map(str::trim_start)
}

fn clean_chars(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(i, &b)| match b {
        b'\r' => bytes.get(i + 1) == Some(&b'\n'),
        b'\n' | b'\t' => true,
        0..=31 | 127 => false,
        _ => true,
    })
}

fn checked_text(file: &Path) -> Result<String, &'static str> {
    let meta = fs::symlink_metadata(file).map_err(|_| "could not be read")?;
    if !plain_file(&meta) {
        return Err("is a link, not a plain file");
    }
    if meta.len() > MAX_FILE {
        return Err("is too large");
    }
    let text = decode(&fs::read(file).map_err(|_| "could not be read")?);
    let text = text.trim_start_matches('\u{feff}');
    if !clean_chars(text) {
        return Err("contains control characters");
    }
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next() != Some(HEADER) {
        return Err("does not start with the registry file header");
    }
    let mut keys = 0;
    let mut continued = false;
    for line in lines {
        if continued {
            if !line.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b',' | b'\\')) {
                return Err("contains a line Wisp does not recognise");
            }
            continued = line.ends_with('\\');
            continue;
        }
        if line.starts_with('[') {
            let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
                return Err("contains a line Wisp does not recognise");
            };
            if key.starts_with('-') || key.contains(['[', ']']) {
                return Err("contains delete lines");
            }
            if !own_key(key) {
                return Err("names a key that Wisp does not back up");
            }
            keys += 1;
        } else if line.starts_with('"') || line.starts_with('@') {
            if keys == 0 {
                return Err("contains a value outside any key");
            }
            let Some(data) = value_data(line) else {
                return Err("contains a line Wisp does not recognise");
            };
            if data.starts_with('-') {
                return Err("contains delete lines");
            }
            continued = data.starts_with("hex") && data.ends_with('\\');
        } else {
            return Err("contains a line Wisp does not recognise");
        }
    }
    if keys == 0 {
        return Err("has no registry keys");
    }
    Ok(text.to_string())
}

pub fn check_file(file: &Path) -> Result<(), &'static str> {
    checked_text(file).map(|_| ())
}

pub struct Backup {
    pub id: String,
    pub when: String,
    pub files: usize,
    pub keys: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

fn reg_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.metadata().is_ok_and(|m| plain_file(&m)))
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("reg")))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).to_string()
    }
}

fn key_of(file: &Path) -> Option<String> {
    let text = decode(&fs::read(file).ok()?);
    let line = text.lines().map(str::trim).find(|l| l.starts_with('[') && l.ends_with(']'))?;
    let path = line[1..line.len() - 1].replace("HKEY_CURRENT_USER", "HKCU").replace("HKEY_LOCAL_MACHINE", "HKLM");
    Some(path)
}

fn pretty_when(id: &str) -> String {
    let digits: String = id.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = id.get(digits.len()..).unwrap_or("");
    let time: String = rest.trim_start_matches('-').chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() == 8 && time.len() == 6 {
        format!("{}-{}-{} {}:{}", &digits[0..4], &digits[4..6], &digits[6..8], &time[0..2], &time[2..4])
    } else {
        id.to_string()
    }
}

pub fn list() -> Vec<Backup> {
    list_in(&data::backup_root())
}

pub fn list_in(root: &Path) -> Vec<Backup> {
    if fsx::is_reparse(root) {
        return Vec::new();
    }
    let Ok(rd) = fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<Backup> = rd
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir() && !fsx::attrs_reparse(m.file_attributes())))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_string();
            if !valid_id(&id) {
                return None;
            }
            let files = reg_files(&e.path());
            if files.is_empty() {
                return None;
            }
            let keys = files.iter().take(3).filter_map(|f| key_of(f)).collect();
            Some(Backup { when: pretty_when(&id), id, files: files.len(), keys })
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

pub fn restore(id: &str) -> io::Result<usize> {
    if !valid_id(id) {
        return Err(io::Error::other("refused"));
    }
    let dir = data::backup_root().join(id);
    if !dir.is_dir() || fsx::is_reparse(&dir) || fsx::is_reparse(&data::backup_root()) {
        return Err(io::Error::other("This backup no longer exists."));
    }
    let files = reg_files(&dir);
    if files.is_empty() {
        return Err(io::Error::other("This backup has no registry files."));
    }
    let mut texts = Vec::new();
    for f in &files {
        match checked_text(f) {
            Ok(text) => texts.push((f.file_name().map(|n| n.to_os_string()).unwrap_or_default(), text)),
            Err(why) => {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                data::log(&format!("restore {} refused: {} {}", id, name, why));
                return Err(io::Error::other(format!("This backup was not restored: {} {}.", name, why)));
            }
        }
    }
    let work = data::root().join("restore-work");
    fs::create_dir_all(&work)?;
    if fsx::is_reparse(&work) {
        return Err(io::Error::other("refused"));
    }
    let mut restored = 0;
    let mut failed: Option<String> = None;
    for (name, text) in &texts {
        let copy = work.join(name);
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let written = fs::OpenOptions::new().write(true).create_new(true).open(&copy).and_then(|mut f| io::Write::write_all(&mut f, &bytes));
        let out = written.and_then(|_| sys::hidden("reg.exe").arg("import").arg(&copy).arg("/reg:64").output());
        let _ = fs::remove_file(&copy);
        match out {
            Ok(out) if out.status.success() => restored += 1,
            Ok(out) => {
                if failed.is_none() {
                    failed = Some(sys::oem_text(&out.stderr).trim().to_string());
                }
            }
            Err(e) => {
                let _ = fs::remove_dir(&work);
                return Err(e);
            }
        }
    }
    let _ = fs::remove_dir(&work);
    data::log(&format!("restore {} {} of {}", id, restored, files.len()));
    match failed {
        Some(msg) => Err(io::Error::other(if msg.is_empty() { "Windows refused part of this backup. Some entries need administrator rights.".to_string() } else { msg })),
        None => Ok(restored),
    }
}

fn remove_one(dir: &Path) -> bool {
    let Ok(rd) = fs::read_dir(dir) else { return false };
    for e in rd.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = fs::remove_file(&p);
        }
    }
    fs::remove_dir(dir).is_ok()
}

pub fn delete(id: &str) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::other("refused"));
    }
    let dir = data::backup_root().join(id);
    if !dir.is_dir() || super::fsx::is_reparse(&dir) {
        return Err(io::Error::other("This backup no longer exists."));
    }
    if !remove_one(&dir) {
        return Err(io::Error::other("Wisp could not delete that backup."));
    }
    data::log(&format!("backup deleted {}", id));
    Ok(())
}

pub fn delete_all() -> usize {
    let mut removed = 0;
    for b in list() {
        if delete(&b.id).is_ok() {
            removed += 1;
        }
    }
    removed
}
