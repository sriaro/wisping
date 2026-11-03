use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::System::Registry::{HKEY, REG_VALUE_TYPE, RegSetValueExW};
use windows::core::PCWSTR;
use winreg::RegKey;
use winreg::enums::{KEY_WRITE, KEY_WOW64_64KEY};

use super::data;
use super::fsx;
use super::regbackup::Hive;
use super::regbackup::integrity;
use super::registry;
use super::tasks::decode;

const HEADER: &str = "Windows Registry Editor Version 5.00";

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
    OWN_KEYS.iter().copied().map(str::to_lowercase).any(|p| {
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
    read_held(&mut open_backup(file)?)
}

fn read_held(file: &mut fs::File) -> Result<Vec<u8>, &'static str> {
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
    pub restorable: bool,
}

pub struct DeleteReport {
    pub removed: usize,
    pub failed: usize,
    pub needs_admin: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
}

enum Saved {
    Registry,
    Task,
    Startup(Hive, OsString),
}

fn saved_kind(file: &Path) -> Option<Saved> {
    let name = file.file_name()?.to_str()?;
    let lower = name.to_ascii_lowercase();
    for (prefix, hive) in [("startup-user-", Hive::User), ("startup-common-", Hive::Machine)] {
        if lower.starts_with(prefix) {
            return Some(Saved::Startup(hive, OsString::from(&name[prefix.len()..])));
        }
    }
    if lower.ends_with(".reg") {
        Some(Saved::Registry)
    } else if lower.starts_with("task-") && lower.ends_with(".xml") {
        Some(Saved::Task)
    } else {
        None
    }
}

fn is_reg(file: &Path) -> bool {
    matches!(saved_kind(file), Some(Saved::Registry))
}

fn sealable(file: &Path) -> bool {
    match saved_kind(file) {
        Some(Saved::Registry) => true,
        Some(_) => fs::symlink_metadata(file).is_ok_and(|m| m.is_file() && !fsx::attrs_reparse(m.file_attributes()) && m.len() <= MAX_FILE),
        None => false,
    }
}

fn is_sealed(file: &Path) -> bool {
    read_backup(file).is_ok_and(|bytes| integrity::verify(file, &bytes).unwrap_or(false))
}

fn can_restore(file: &Path, elevated: bool, receipts: bool) -> bool {
    match saved_kind(file) {
        Some(Saved::Registry) => true,
        Some(Saved::Startup(Hive::User, _)) if !elevated => true,
        Some(_) => receipts && is_sealed(file),
        None => false,
    }
}

fn backup_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.metadata().is_ok_and(|m| plain_file(&m)))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn reg_files(dir: &Path) -> Vec<PathBuf> {
    backup_files(dir).into_iter().filter(|p| is_reg(p)).collect()
}

fn label_of(file: &Path) -> Option<String> {
    if is_reg(file) { key_of(file) } else { file.file_name().map(|n| n.to_string_lossy().into_owned()) }
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

fn dirs_in(root: &Path) -> Vec<String> {
    if fsx::plain_dir(root) != Some(true) {
        return Vec::new();
    }
    let Ok(rd) = fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir() && !fsx::attrs_reparse(m.file_attributes())))
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().to_string();
            valid_id(&id).then_some(id)
        })
        .collect();
    out.sort_by(|a, b| b.cmp(a));
    out
}

fn ids_in(root: &Path) -> Vec<(String, Vec<PathBuf>)> {
    dirs_in(root)
        .into_iter()
        .filter_map(|id| {
            let files = backup_files(&root.join(&id));
            (!files.is_empty()).then_some((id, files))
        })
        .collect()
}

pub fn count() -> usize {
    ids_in(&data::backup_root()).len()
}

pub fn list_in(root: &Path) -> Vec<Backup> {
    let elevated = super::sys::elevation().unwrap_or(true);
    let receipts = integrity::has_records().unwrap_or(false);
    ids_in(root)
        .into_iter()
        .map(|(id, files)| {
            let keys = files.iter().take(3).filter_map(|f| label_of(f)).collect();
            let restorable = files.iter().any(|f| can_restore(f, elevated, receipts));
            Backup { when: pretty_when(&id), restorable, id, files: files.len(), keys }
        })
        .collect()
}

pub fn restore(id: &str) -> io::Result<usize> {
    let root = data::backup_root();
    let elevated = super::sys::elevation()?;
    if !valid_id(id) {
        return Err(io::Error::other("refused"));
    }
    let dir = root.join(id);
    if fsx::plain_dir(&dir) != Some(true) {
        return Err(io::Error::other("gone"));
    }
    let files = reg_files(&dir);
    if files.is_empty() {
        return restore_copies(&dir, id, elevated);
    }
    let mut imports = Vec::new();
    let mut total = 0u64;
    for f in &files {
        let bytes = match read_backup(f) {
            Ok(bytes) => bytes,
            Err(why) => return Err(refusal(why)),
        };
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_RESTORE {
            return Err(io::Error::other("toobig"));
        }
        let checked = checked_text(&bytes).and_then(|text| parse_import(&text)).and_then(|keys| {
            if (elevated || keys.iter().any(|key| key.machine)) && !integrity::verify(f, &bytes).unwrap_or(false) {
                return Err(UNSEALED);
            }
            Ok(keys)
        });
        match checked {
            Ok(keys) => imports.push(keys),
            Err(why) => {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                data::log(&format!("restore {} refused: {} {}", id, name, why));
                return Err(refusal(why));
            }
        }
    }
    let mut restored = 0;
    let mut failed: Option<io::Error> = None;
    for keys in &imports {
        match import_keys(keys) {
            Ok(()) => restored += 1,
            Err(e) => {
                if failed.is_none() {
                    failed = Some(e);
                }
            }
        }
    }
    data::log(&format!("restore {} {} of {}", id, restored, files.len()));
    match failed {
        Some(e) => Err(e),
        None => Ok(restored),
    }
}

const UNSEALED: &str = "has changed or has no trusted integrity record; privileged restore requires a backup created by Wisping as administrator";
const NO_TASK: &str = "does not name a scheduled task Wisping can restore";
const MAX_RESTORE: u64 = 128 * 1024 * 1024;

fn refusal(why: &str) -> io::Error {
    io::Error::other(match why {
        UNSEALED => "unsealed",
        NO_TASK => "notask",
        "could not be read" | "could not be read safely" => "unreadable",
        _ => "damaged",
    })
}

fn restore_copies(dir: &Path, id: &str, elevated: bool) -> io::Result<usize> {
    let copies: Vec<(OsString, Saved)> = backup_files(dir)
        .into_iter()
        .filter_map(|f| Some((f.file_name()?.to_os_string(), saved_kind(&f)?)))
        .collect();
    if copies.is_empty() {
        return Err(io::Error::other("norestore"));
    }
    let pinned = fsx::PinnedDir::open(dir)?;
    let total = copies.len();
    let mut restored = 0;
    for (name, kind) in copies {
        let label = name.to_string_lossy().into_owned();
        let refuse = |why: &str| {
            data::log(&format!("restore {} refused: {} {}", id, label, why));
            refusal(why)
        };
        if !elevated && !matches!(kind, Saved::Startup(Hive::User, _)) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "needs administrator rights"));
        }
        let file = pinned.path().join(&name);
        let mut held = pinned.read_file(&name).map_err(|_| refuse("could not be read safely"))?;
        let bytes = if elevated {
            let bytes = read_held(&mut held).map_err(refuse)?;
            if !integrity::verify(&file, &bytes).unwrap_or(false) {
                return Err(refuse(UNSEALED));
            }
            held.seek(SeekFrom::Start(0))?;
            Some(bytes)
        } else {
            None
        };
        match kind {
            Saved::Task => {
                let task = bytes.as_deref().and_then(|b| super::tasks::task_path(&decode(b))).ok_or_else(|| refuse(NO_TASK))?;
                super::tasks::restore(&file, &task)?;
            }
            Saved::Startup(hive, leaf) => {
                let folder = super::startup::folder_of(hive)
                    .filter(|f| fsx::local_drive_path(&f.to_string_lossy()))
                    .ok_or_else(|| io::Error::other("nofolder"))?;
                fsx::PinnedDir::open(&folder)?.restore_file(&leaf, &mut held).map_err(|e| {
                    if e.kind() == io::ErrorKind::AlreadyExists { io::Error::new(io::ErrorKind::AlreadyExists, "exists") } else { e }
                })?;
                super::startup::invalidate_cache();
            }
            Saved::Registry => continue,
        }
        restored += 1;
    }
    data::log(&format!("restore {} {} of {}", id, restored, total));
    Ok(restored)
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
    let mut receipts: Vec<String> = Vec::new();
    if admin {
        for file in backup_files(dir.path()).into_iter().filter(|f| sealable(f)) {
            if let Ok(bytes) = read_backup(&file)
                && let Ok(receipt) = integrity::receipt(&file, &bytes) {
                    receipts.push(receipt);
            }
        }
    } else if integrity::has_records()? {
        for file in backup_files(dir.path()).into_iter().filter(|f| sealable(f)) {
            let bytes = read_backup(&file).map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "needs administrator rights"))?;
            if integrity::verify(&file, &bytes)? {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "needs administrator rights"));
            }
        }
    }
    remove_one(dir).map_err(|_| io::Error::other("undeletable"))?;
    if admin {
        for receipt in &receipts {
            let _ = integrity::forget_name(receipt);
        }
    }
    data::log(&format!("backup deleted {}", id));
    Ok(())
}

pub fn delete_all() -> DeleteReport {
    let mut report = DeleteReport { removed: 0, failed: 0, needs_admin: false };
    for id in dirs_in(&data::backup_root()) {
        match delete(&id) {
            Ok(()) => report.removed += 1,
            Err(e) => {
                report.failed += 1;
                report.needs_admin |= e.kind() == io::ErrorKind::PermissionDenied;
            }
        }
    }
    report
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
        for file in files.into_iter().filter(|f| sealable(f)) {
            let Ok(name) = read_backup(&file).map_err(io::Error::other).and_then(|bytes| integrity::receipt(&file, &bytes)) else { return };
            kept.insert(name.to_lowercase());
        }
    }
    let stale: Vec<String> = stored.into_iter().filter(|name| !kept.contains(&name.to_lowercase())).collect();
    if !stale.is_empty() && integrity::forget_names(&stale).is_ok() {
        data::log(&format!("backup receipts removed {}", stale.len()));
    }
}
