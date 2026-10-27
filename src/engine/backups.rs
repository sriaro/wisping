use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Registry::{HKEY, REG_VALUE_TYPE, RegSetValueExW};
use windows::core::PCWSTR;
use winreg::RegKey;
use winreg::enums::{KEY_WRITE, KEY_WOW64_64KEY};
#[cfg(test)]
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

use super::data;
use super::fsx;
use super::registry;

const HEADER: &str = "Windows Registry Editor Version 5.00";

#[cfg(test)]
#[path = "backups/security_tests.rs"]
mod security_tests;
#[path = "backups/registry_target.rs"]
mod registry_target;
pub(super) const MAX_FILE: u64 = 16 * 1024 * 1024;
const CHILD_ONLY: [&str; 3] = [
    "HKEY_CURRENT_USER\\Software\\Classes",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes",
    "HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Services",
];

const OWN_KEYS: [&str; 26] = [
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\SharedDLLs",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\RunOnce",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\RunOnce",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\RunOnce",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer\\Run",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer\\Run",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run",
    "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Windows",
    "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Windows",
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

fn open_backup(file: &Path) -> Result<fs::File, &'static str> {
    let absolute = fsx::absolute(file).ok_or("could not be read")?;
    let dir = fsx::PinnedDir::open(absolute.parent().ok_or("could not be read")?).map_err(|_| "could not be read safely")?;
    let file = dir.read_file(absolute.file_name().ok_or("could not be read")?).map_err(|_| "could not be read safely")?;
    let meta = file.metadata().map_err(|_| "could not be read")?;
    if !plain_file(&meta) {
        return Err("is a link, not a plain file");
    }
    if meta.len() > MAX_FILE {
        return Err("is too large");
    }
    Ok(file)
}

fn read_backup(file: &Path) -> Result<Vec<u8>, &'static str> {
    let file = open_backup(file)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|_| "could not be read")?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("is too large");
    }
    Ok(bytes)
}

fn checked_text(bytes: &[u8]) -> Result<String, &'static str> {
    let text = decode(bytes);
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
                return Err("contains a line Wisping does not recognise");
            }
            continued = line.ends_with('\\');
            continue;
        }
        if line.starts_with('[') {
            let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
                return Err("contains a line Wisping does not recognise");
            };
            if key.starts_with('-') || key.contains(['[', ']']) {
                return Err("contains delete lines");
            }
            if !own_key(key) {
                return Err("names a key that Wisping does not back up");
            }
            keys += 1;
        } else if line.starts_with('"') || line.starts_with('@') {
            if keys == 0 {
                return Err("contains a value outside any key");
            }
            let Some(data) = value_data(line) else {
                return Err("contains a line Wisping does not recognise");
            };
            if data.starts_with('-') {
                return Err("contains delete lines");
            }
            continued = data.to_ascii_lowercase().starts_with("hex") && data.ends_with('\\');
        } else {
            return Err("contains a line Wisping does not recognise");
        }
    }
    if keys == 0 {
        return Err("has no registry keys");
    }
    Ok(text.to_string())
}

pub fn check_file(file: &Path) -> Result<(), &'static str> {
    read_backup(file).and_then(|bytes| checked_text(&bytes)).and_then(|text| parse_import(&text)).map(|_| ())
}

struct SavedValue {
    name: String,
    kind: u32,
    bytes: Vec<u8>,
}

struct SavedKey {
    machine: bool,
    path: String,
    values: Vec<SavedValue>,
}

fn unquote(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &text[i + 2..])),
            '\\' => match chars.next()?.1 {
                c @ ('\\' | '"') => out.push(c),
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

fn parse_value(line: &str) -> Option<SavedValue> {
    let (name, rest) = if let Some(rest) = line.strip_prefix('@') { (String::new(), rest) } else { unquote(line)? };
    let data = rest.trim_start().strip_prefix('=')?.trim();
    let (kind, bytes) = if data.starts_with('"') {
        let (value, rest) = unquote(data)?;
        if !rest.trim().is_empty() {
            return None;
        }
        (1, value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect())
    } else {
        let (tag, body) = data.split_once(':')?;
        if tag.eq_ignore_ascii_case("dword") {
            if body.is_empty() || body.len() > 8 || !body.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            (4, u32::from_str_radix(body, 16).ok()?.to_le_bytes().to_vec())
        } else {
            let tag = tag.to_ascii_lowercase();
            let kind = if tag == "hex" { 3 } else { u32::from_str_radix(tag.strip_prefix("hex(")?.strip_suffix(')')?, 16).ok()? };
            let bytes = if body.is_empty() {
                Vec::new()
            } else {
                body.split(',').map(|part| {
                    let part = part.trim();
                    (part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit())).then(|| u8::from_str_radix(part, 16).ok()).flatten()
                }).collect::<Option<Vec<_>>>()?
            };
            (kind, bytes)
        }
    };
    Some(SavedValue { name, kind, bytes })
}

fn parse_import(text: &str) -> Result<Vec<SavedKey>, &'static str> {
    let bad = "contains registry data Wisping does not recognise";
    let mut keys: Vec<SavedKey> = Vec::new();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if !clean_chars(text) || lines.next() != Some(HEADER) {
        return Err(bad);
    }
    while let Some(line) = lines.next() {
        if let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if !own_key(key) || key.contains(['[', ']', '\0']) {
                return Err(bad);
            }
            let (hive, path) = key.split_once('\\').ok_or(bad)?;
            let machine = if hive.eq_ignore_ascii_case("HKEY_LOCAL_MACHINE") { true } else if hive.eq_ignore_ascii_case("HKEY_CURRENT_USER") { false } else { return Err(bad) };
            keys.push(SavedKey { machine, path: path.to_string(), values: Vec::new() });
        } else {
            let mut value = line.to_string();
            while value.ends_with('\\') {
                if !value_data(&value).is_some_and(|data| data.to_ascii_lowercase().starts_with("hex")) {
                    return Err(bad);
                }
                value.pop();
                let next = lines.next().ok_or(bad)?;
                if !next.bytes().all(|b| b.is_ascii_hexdigit() || matches!(b, b',' | b'\\')) {
                    return Err(bad);
                }
                value.push_str(next);
            }
            keys.last_mut().ok_or(bad)?.values.push(parse_value(&value).ok_or(bad)?);
        }
    }
    if keys.is_empty() { Err(bad) } else { Ok(keys) }
}

fn import_keys(keys: &[SavedKey]) -> io::Result<()> {
    for saved in keys {
        let key = registry_target::open(saved)?;
        for value in &saved.values {
            let name: Vec<u16> = value.name.encode_utf16().chain([0]).collect();
            let status = unsafe { RegSetValueExW(HKEY(key.raw_handle()), PCWSTR(name.as_ptr()), None, REG_VALUE_TYPE(value.kind), Some(&value.bytes)) };
            if status.0 != 0 {
                return Err(io::Error::from_raw_os_error(status.0 as i32));
            }
        }
    }
    Ok(())
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
    let mut head = Vec::new();
    open_backup(file).ok()?.take(4096).read_to_end(&mut head).ok()?;
    let text = decode(&head);
    let line = text.lines().map(str::trim).find(|l| l.starts_with('[') && l.ends_with(']'))?;
    let path = line[1..line.len() - 1].replace("HKEY_CURRENT_USER", "HKCU").replace("HKEY_LOCAL_MACHINE", "HKLM");
    Some(path)
}

pub fn when_of(id: &str) -> String {
    pretty_when(id)
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

fn ids_in(root: &Path) -> Vec<(String, Vec<PathBuf>)> {
    if fsx::is_reparse(root) {
        return Vec::new();
    }
    let Ok(rd) = fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<(String, Vec<PathBuf>)> = rd
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir() && !fsx::attrs_reparse(m.file_attributes())))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_string();
            if !valid_id(&id) {
                return None;
            }
            let files = reg_files(&e.path());
            (!files.is_empty()).then_some((id, files))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

pub fn count() -> usize {
    ids_in(&data::backup_root()).len()
}

pub fn list_in(root: &Path) -> Vec<Backup> {
    ids_in(root)
        .into_iter()
        .map(|(id, files)| {
            let keys = files.iter().take(3).filter_map(|f| key_of(f)).collect();
            Backup { when: pretty_when(&id), id, files: files.len(), keys }
        })
        .collect()
}

pub fn restore(id: &str) -> io::Result<usize> {
    restore_with(&data::backup_root(), id, super::sys::elevation()?, super::regbackup::integrity::verify)
}

fn restore_with(root: &Path, id: &str, elevated: bool, trusted: impl Fn(&Path, &[u8]) -> io::Result<bool>) -> io::Result<usize> {
    if !valid_id(id) {
        return Err(io::Error::other("refused"));
    }
    let dir = root.join(id);
    if !dir.is_dir() || fsx::is_reparse(&dir) || fsx::is_reparse(root) {
        return Err(io::Error::other("This backup no longer exists."));
    }
    let files = reg_files(&dir);
    if files.is_empty() {
        return Err(io::Error::other("This backup has no registry files."));
    }
    let mut imports = Vec::new();
    for f in &files {
        let checked = read_backup(f).and_then(|bytes| {
            let keys = checked_text(&bytes).and_then(|text| parse_import(&text))?;
            if (elevated || keys.iter().any(|key| key.machine)) && !trusted(f, &bytes).unwrap_or(false) {
                return Err("has changed or has no trusted integrity record; privileged restore requires a backup created by Wisping as administrator");
            }
            Ok(keys)
        });
        match checked {
            Ok(keys) => imports.push(keys),
            Err(why) => {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                data::log(&format!("restore {} refused: {} {}", id, name, why));
                return Err(io::Error::other(format!("This backup was not restored: {} {}.", name, why)));
            }
        }
    }
    let mut restored = 0;
    let mut failed: Option<String> = None;
    for keys in &imports {
        match import_keys(keys) {
            Ok(()) => restored += 1,
            Err(e) => {
                if failed.is_none() {
                    failed = Some(e.to_string());
                }
            }
        }
    }
    data::log(&format!("restore {} {} of {}", id, restored, files.len()));
    match failed {
        Some(msg) => Err(io::Error::other(if msg.is_empty() { "Windows refused part of this backup. Some entries need administrator rights.".to_string() } else { msg })),
        None => Ok(restored),
    }
}

fn remove_one(dir: fsx::PinnedDir) -> io::Result<()> {
    for entry in fs::read_dir(dir.path())? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            dir.remove_file(&entry.file_name())?;
        }
    }
    dir.remove()
}

pub fn delete(id: &str) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::other("refused"));
    }
    let root = fsx::PinnedDir::open(&data::backup_root())?;
    let dir = root.deletable_child(id.as_ref())?;
    let admin = super::sys::is_admin();
    let mut receipts: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    if admin || super::regbackup::integrity::has_records()? {
        for file in reg_files(dir.path()) {
            let bytes = read_backup(&file).map_err(|why| io::Error::other(format!("Wisping cannot verify this backup: {why}.")))?;
            if !admin && super::regbackup::integrity::verify(&file, &bytes)? {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "This backup was created by Wisping as administrator. Restart Wisping as administrator to delete it."));
            }
            receipts.push((file, bytes));
        }
    }
    remove_one(dir).map_err(|_| io::Error::other("Wisping could not delete that backup."))?;
    if admin {
        for (file, bytes) in &receipts {
            let _ = super::regbackup::integrity::forget(file, bytes);
        }
    }
    data::log(&format!("backup deleted {}", id));
    Ok(())
}

pub fn delete_all() -> usize {
    let mut removed = 0;
    for (id, _) in ids_in(&data::backup_root()) {
        if delete(&id).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub fn prune(keep: usize) -> usize {
    let mut removed = 0;
    for (id, _) in ids_in(&data::backup_root()).into_iter().skip(keep) {
        if delete(&id).is_ok() {
            removed += 1;
        }
    }
    sweep_receipts();
    removed
}

fn sweep_receipts() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use super::regbackup::integrity;
    static SWEPT: AtomicBool = AtomicBool::new(false);
    if !super::sys::is_admin() || SWEPT.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(stored) = integrity::names() else { return };
    if stored.is_empty() {
        return;
    }
    let mut kept = HashSet::new();
    for (_, files) in ids_in(&data::backup_root()) {
        for file in files {
            let Ok(name) = read_backup(&file).map_err(io::Error::other).and_then(|bytes| integrity::receipt(&file, &bytes)) else { return };
            kept.insert(name.to_lowercase());
        }
    }
    let stale: Vec<String> = stored.into_iter().filter(|name| !kept.contains(&name.to_lowercase())).collect();
    if !stale.is_empty() && integrity::forget_names(&stale).is_ok() {
        data::log(&format!("backup receipts removed {}", stale.len()));
    }
}
