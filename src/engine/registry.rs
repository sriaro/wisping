use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use winreg::enums::*;

use super::data;
use super::fsx::{self, direct_child};
use super::regbackup::{self, Hive};
use super::leftovers;
use super::regscan;
use super::trayicons;

const CV: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
const MUI_PATH: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache";
pub const TEST_KEY: &str = "Software\\WispSelfTest";

static ALLOW_TEST_KEY: AtomicBool = AtomicBool::new(false);

pub fn set_allow_test_key(value: bool) {
    ALLOW_TEST_KEY.store(value, Ordering::SeqCst);
}

pub fn allow_test_key() -> bool {
    ALLOW_TEST_KEY.load(Ordering::SeqCst)
}

#[derive(Clone, Debug)]
pub struct RegIssue {
    pub hive: Hive,
    pub kind: String,
    pub key_path: String,
    pub value_name: Option<String>,
    pub detail: String,
    pub selected: bool,
}

impl RegIssue {
    pub fn new(hive: Hive, kind: &str, key_path: &str, value_name: Option<&str>, detail: &str) -> RegIssue {
        RegIssue {
            hive,
            kind: kind.to_string(),
            key_path: key_path.to_string(),
            value_name: value_name.map(|s| s.to_string()),
            detail: detail.to_string(),
            selected: false,
        }
    }

    pub fn id(&self) -> String {
        format!("{}|{}|{}", self.hive.short_name(), self.key_path.to_lowercase(), self.value_name.as_deref().unwrap_or("").to_lowercase())
    }

    pub fn display(&self) -> String {
        if is_leftover(&self.kind) {
            return self.key_path.clone();
        }
        match &self.value_name {
            Some(v) => format!("{}\\{}  [{}]", self.hive.short_name(), self.key_path, v),
            None => format!("{}\\{}", self.hive.short_name(), self.key_path),
        }
    }
}

pub fn first_path(command: &str) -> Option<String> {
    if command.trim().is_empty() {
        return None;
    }
    let s = fsx::expand_env(command.trim());
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return if end > 0 { Some(rest[..end].to_string()) } else { None };
    }
    if s.starts_with('@') {
        return None;
    }
    if s.starts_with("\\\\") {
        return cut_at_ext(&s, &[".exe"]);
    }
    if Path::new(&s).is_file() {
        return Some(s);
    }
    cut_at_ext(&s, &[".exe"])
}

pub fn cut_at_ext(s: &str, exts: &[&str]) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut best: Option<usize> = None;
    for ext in exts {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(ext) {
            let idx = from + pos;
            let end = idx + ext.len();
            let glued = bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'\\' | b'/' | b'$'));
            if idx > 0 && !glued {
                best = Some(best.map_or(end, |b| b.min(end)));
                break;
            }
            from = end;
        }
    }
    best.map(|end| s[..end].to_string())
}

pub fn missing(path: &str) -> bool {
    if path.is_empty() || path.starts_with('\\') {
        return false;
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return false;
    }
    let mut root = PathBuf::new();
    for c in p.components().take(2) {
        root.push(c.as_os_str());
    }
    if !root.is_dir() {
        return false;
    }
    !p.exists()
}

pub fn is_leftover(kind: &str) -> bool {
    kind == "leftover" || kind == "riskyleft"
}

const KINDS: [&str; 10] = ["shareddll", "apppath", "uninstall", "muicache", "trayicon", "com", "extension", "openwith", "leftover", "riskyleft"];

type Scanner = fn(&mut Vec<RegIssue>);

fn scan_kinds(kinds: &[&str]) -> Vec<RegIssue> {
    let mut list = Vec::new();
    let scanners: [(&str, Scanner); 9] = [
        ("shareddll", shared_dlls),
        ("apppath", app_paths),
        ("uninstall", uninstall),
        ("muicache", mui_cache),
        ("trayicon", tray_icons),
        ("com", regscan::com),
        ("extension", regscan::extensions),
        ("openwith", regscan::open_with),
        ("leftover", leftover_folders),
    ];
    for (kind, scan) in scanners {
        if kinds.contains(&kind) || (kind == "leftover" && kinds.contains(&"riskyleft")) {
            scan(&mut list);
        }
    }
    list.retain(|i| kinds.contains(&i.kind.as_str()));
    for i in &mut list {
        i.selected = matches!(i.kind.as_str(), "shareddll" | "apppath" | "muicache" | "trayicon");
    }
    list
}

pub fn run() -> Vec<RegIssue> {
    scan_kinds(&KINDS)
}

pub fn rescan(old: &[RegIssue], fixed: &[RegIssue]) -> Vec<RegIssue> {
    let kinds: Vec<&str> = KINDS.iter().copied().filter(|k| fixed.iter().any(|i| i.kind == *k)).collect();
    let picked: HashSet<String> = old.iter().filter(|i| i.selected).map(RegIssue::id).collect();
    let known: HashSet<String> = old.iter().map(RegIssue::id).collect();
    let mut fresh = scan_kinds(&kinds);
    for i in &mut fresh {
        if known.contains(&i.id()) {
            i.selected = picked.contains(&i.id());
        }
    }
    let rank = |k: &str| KINDS.iter().position(|x| *x == k).unwrap_or(KINDS.len());
    let mut out: Vec<RegIssue> = old.iter().filter(|i| !kinds.contains(&i.kind.as_str())).cloned().chain(fresh).collect();
    out.sort_by_key(|i| rank(&i.kind));
    out
}

fn allowed(i: &RegIssue) -> bool {
    if is_leftover(&i.kind) {
        return i.value_name.is_none() && leftovers::allowed(Path::new(&i.key_path));
    }
    let key = i.key_path.as_str();
    if key.is_empty() || key.contains("..") || key.starts_with('\\') || key.ends_with('\\') || !regbackup::plain_name(key) {
        return false;
    }
    if i.value_name.as_deref().is_some_and(|v| !regbackup::plain_name(v)) {
        return false;
    }
    let lower = key.to_lowercase();
    let test = TEST_KEY.to_lowercase();
    if allow_test_key() && i.hive == Hive::User && (lower == test || lower.starts_with(&format!("{}\\", test))) {
        return true;
    }
    match i.kind.as_str() {
        "shareddll" => i.hive == Hive::Machine && i.value_name.is_some() && lower == format!("{}\\shareddlls", CV.to_lowercase()),
        "muicache" => i.hive == Hive::User && i.value_name.is_some() && lower == MUI_PATH.to_lowercase(),
        "trayicon" => {
            i.hive == Hive::User
                && i.value_name.is_none()
                && direct_child(&lower, &format!("{}\\", trayicons::ROOT.to_lowercase()))
                && lower.rsplit('\\').next().is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        }
        "apppath" => i.value_name.is_none() && direct_child(&lower, &format!("{}\\app paths\\", CV.to_lowercase())),
        "uninstall" => {
            i.value_name.is_none()
                && (direct_child(&lower, &format!("{}\\uninstall\\", CV.to_lowercase()))
                    || direct_child(&lower, "software\\wow6432node\\microsoft\\windows\\currentversion\\uninstall\\"))
        }
        other => regscan::allowed(other, i.hive, &lower) && i.value_name.is_none(),
    }
}

fn shared_dlls(list: &mut Vec<RegIssue>) {
    let path = format!("{}\\SharedDLLs", CV);
    let Ok(key) = Hive::Machine.open(&path) else { return };
    for item in key.enum_values().flatten() {
        let (name, value) = item;
        if value.vtype != REG_DWORD || !missing(&name) {
            continue;
        }
        list.push(RegIssue::new(Hive::Machine, "shareddll", &path, Some(&name), &name));
    }
}

fn app_paths(list: &mut Vec<RegIssue>) {
    for hive in [Hive::Machine, Hive::User] {
        let Ok(key) = hive.open(&format!("{}\\App Paths", CV)) else { continue };
        for name in key.enum_keys().flatten() {
            let Ok(sub) = key.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY) else { continue };
            let def = regbackup::read_string(&sub, "");
            let Some(p) = def.as_deref().and_then(first_path) else { continue };
            if !missing(&p) {
                continue;
            }
            list.push(RegIssue::new(hive, "apppath", &format!("{}\\App Paths\\{}", CV, name), None, &p));
        }
    }
}

fn uninstall(list: &mut Vec<RegIssue>) {
    let sources = [
        (Hive::Machine, format!("{}\\Uninstall", CV)),
        (Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall".to_string()),
        (Hive::User, format!("{}\\Uninstall", CV)),
    ];
    for (hive, path) in sources {
        let Ok(key) = hive.open(&path) else { continue };
        for name in key.enum_keys().flatten() {
            let Ok(s) = key.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY) else { continue };
            if regbackup::read_dword(&s, "SystemComponent") == Some(1) {
                continue;
            }
            if regbackup::read_dword(&s, "WindowsInstaller") == Some(1) {
                continue;
            }
            if regbackup::has_value(&s, "ParentKeyName") {
                continue;
            }
            let Some(display) = regbackup::read_string(&s, "DisplayName") else { continue };
            let cmd = regbackup::read_string(&s, "UninstallString").or_else(|| regbackup::read_string(&s, "QuietUninstallString"));
            let Some(p) = cmd.as_deref().and_then(first_path) else { continue };
            if p.to_lowercase().contains("msiexec") || !missing(&p) {
                continue;
            }
            if let Some(loc) = regbackup::read_string(&s, "InstallLocation") {
                let loc = loc.trim().trim_matches('"').to_string();
                if !loc.is_empty() && !missing(&loc) {
                    continue;
                }
            }
            list.push(RegIssue::new(hive, "uninstall", &format!("{}\\{}", path, name), None, &format!("{} -> {}", display, p)));
        }
    }
}

const KNOWN_FOLDERS: [(&str, &str); 8] = [
    ("{6D809377-6AF0-444B-8957-A3773F02200E}", "%ProgramW6432%"),
    ("{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}", "%ProgramFiles(x86)%"),
    ("{F38BF404-1D43-42F2-9305-67DE0B28FC23}", "%WINDIR%"),
    ("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}", "%WINDIR%\\System32"),
    ("{D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27}", "%WINDIR%\\SysWOW64"),
    ("{3EB685DB-65F9-4CF6-A03A-E3EF65729F3D}", "%APPDATA%"),
    ("{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}", "%LOCALAPPDATA%"),
    ("{62AB5D82-FDC1-4DC3-A9DD-070D1D495D97}", "%PROGRAMDATA%"),
];

pub fn tray_exe(raw: &str) -> Option<String> {
    if let Some(rest) = raw.strip_prefix('{') {
        let end = rest.find('}')?;
        let guid = format!("{{{}}}", &rest[..end]);
        let base = KNOWN_FOLDERS.iter().find(|(g, _)| g.eq_ignore_ascii_case(&guid))?.1;
        let tail = rest[end + 1..].trim_start_matches('\\');
        let full = format!("{}\\{}", fsx::expand_env(base), tail);
        return (!full.contains('%')).then_some(full);
    }
    Path::new(raw).is_absolute().then(|| raw.to_string())
}

fn tray_icons(list: &mut Vec<RegIssue>) {
    let Ok(key) = Hive::User.open(trayicons::ROOT) else { return };
    for id in key.enum_keys().flatten() {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(sub) = key.open_subkey_with_flags(&id, KEY_READ | KEY_WOW64_64KEY) else { continue };
        let Some(raw) = regbackup::read_string(&sub, "ExecutablePath") else { continue };
        let Some(exe) = tray_exe(&raw) else { continue };
        let gone = matches!(std::fs::metadata(&exe), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        if exe.to_lowercase().contains("\\windowsapps\\") || !gone || !missing(&exe) {
            continue;
        }
        let tip = regbackup::read_string(&sub, "InitialTooltip").unwrap_or_default();
        let detail = if tip.is_empty() { exe } else { format!("{} -> {}", tip, exe) };
        list.push(RegIssue::new(Hive::User, "trayicon", &format!("{}\\{}", trayicons::ROOT, id), None, &detail));
    }
}

fn leftover_folders(list: &mut Vec<RegIssue>) {
    for l in leftovers::scan() {
        let hive = if l.admin { Hive::Machine } else { Hive::User };
        list.push(RegIssue::new(hive, if l.risky { "riskyleft" } else { "leftover" }, &l.path.to_string_lossy(), None, &leftovers::display(&l)));
    }
}

fn mui_cache(list: &mut Vec<RegIssue>) {
    let Ok(key) = Hive::User.open(MUI_PATH) else { return };
    for item in key.enum_values().flatten() {
        let (name, value) = item;
        let lower = name.to_lowercase();
        if !lower.ends_with(".friendlyappname") && !lower.ends_with(".applicationcompany") {
            continue;
        }
        if value.vtype != REG_SZ {
            continue;
        }
        let Some(cut) = name.rfind('.') else { continue };
        let target = &name[..cut];
        if !missing(target) {
            continue;
        }
        list.push(RegIssue::new(Hive::User, "muicache", MUI_PATH, Some(&name), target));
    }
}

pub fn fix(issues: &[RegIssue], backup_dir: &Path, errors: &mut Vec<String>) -> usize {
    let mut ok = 0;
    let mut live: HashSet<String> = HashSet::new();
    if !allow_test_key() {
        let kinds: Vec<&str> = KINDS.iter().copied().filter(|k| issues.iter().any(|i| i.kind == *k)).collect();
        live.extend(scan_kinds(&kinds).iter().map(RegIssue::id));
    }
    for i in issues {
        if !allowed(i) {
            errors.push(format!("{}: refused", i.display()));
            data::log(&format!("registry refused {}", i.display()));
            continue;
        }
        if i.hive == Hive::Machine && !allow_test_key() && !super::sys::is_admin() {
            errors.push(format!("{}: needs administrator rights", i.display()));
            continue;
        }
        if !allow_test_key() && !live.contains(&i.id()) {
            errors.push(format!("{}: no longer broken, skipped", i.display()));
            data::log(&format!("registry skipped {}", i.display()));
            continue;
        }
        match fix_one(i, backup_dir) {
            Ok(()) => {
                data::log(&format!("registry fixed {}", i.display()));
                ok += 1;
            }
            Err(e) => {
                errors.push(format!("{}: {}", i.display(), e));
                data::log(&format!("registry fix failed {} {}", i.display(), e));
            }
        }
    }
    ok
}

fn fix_one(i: &RegIssue, backup_dir: &Path) -> std::io::Result<()> {
    if is_leftover(&i.kind) {
        return leftovers::recycle(Path::new(&i.key_path));
    }
    match &i.value_name {
        None => {
            regbackup::export_key(i.hive, &i.key_path, backup_dir)?;
            let slash = i.key_path.rfind('\\').ok_or_else(|| std::io::Error::other("bad key"))?;
            let parent = i.hive.open_write(&i.key_path[..slash])?;
            parent.delete_subkey_all(&i.key_path[slash + 1..])
        }
        Some(v) => {
            regbackup::export_value(i.hive, &i.key_path, v, backup_dir)?;
            let key = i.hive.open_write(&i.key_path)?;
            key.delete_value(v)
        }
    }
}
