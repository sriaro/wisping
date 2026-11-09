use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use winreg::{RegKey, RegValue};
use winreg::enums::*;

use super::data;
use super::fileinfo;
use super::fsx;
use super::regbackup::{self, Hive};
use super::registry;
use super::sys;

const CV: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
const APP_DATA: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData";
const WINLOGON: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon";
const LEGACY_WINDOWS: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Windows";
const IFEO: [&str; 2] = [
    "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options",
    "SOFTWARE\\WOW6432Node\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options",
];
const APP_INIT: [&str; 2] = [LEGACY_WINDOWS, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows NT\\CurrentVersion\\Windows"];
const SESSION_MANAGER: &str = "SYSTEM\\CurrentControlSet\\Control\\Session Manager";
const LSA: &str = "SYSTEM\\CurrentControlSet\\Control\\Lsa";
const LSA_PACKAGES: [&str; 3] = ["Authentication Packages", "Notification Packages", "Security Packages"];
const PRINT_MONITORS: &str = "SYSTEM\\CurrentControlSet\\Control\\Print\\Monitors";
const SHELL_OBJECTS: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\ShellServiceObjectDelayLoad";
const EXECUTE_HOOKS: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\ShellExecuteHooks";
const SHELL_FOLDERS: [&str; 2] = [
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders",
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders",
];
const ACTIVE_SETUP: [&str; 2] = ["SOFTWARE\\Microsoft\\Active Setup\\Installed Components", "SOFTWARE\\WOW6432Node\\Microsoft\\Active Setup\\Installed Components"];
const DEFAULT_SHELL_OBJECTS: [(&str, &str); 1] = [("WebCheck", "{E6FB5E20-DE35-11CF-9C87-00AA005127ED}")];

fn approved(sub: &str) -> String {
    format!("{}\\Explorer\\StartupApproved\\{}", CV, sub)
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum StartupKind {
    #[default]
    Run,
    Folder,
    Packaged,
    Task,
    Context,
    Service,
    Advanced,
}

impl StartupKind {
    pub fn group(self) -> usize {
        match self {
            StartupKind::Run | StartupKind::Folder | StartupKind::Packaged | StartupKind::Advanced => 0,
            StartupKind::Task => 1,
            StartupKind::Context => 2,
            StartupKind::Service => 3,
        }
    }
}

#[derive(Clone, Default)]
pub struct StartupEntry {
    pub publisher: String,
    pub system: bool,
    pub detail: String,
    pub kind: StartupKind,
    pub name: String,
    pub command: String,
    pub enabled: bool,
    pub missing: bool,
    pub policy: bool,
    pub hive: Hive,
    pub key_path: String,
    pub approved_sub: String,
    pub value_name: String,
    pub file_path: Option<PathBuf>,
}

pub fn list() -> Vec<StartupEntry> {
    let mut out = super::pool::scope(|scope| {
        let windows = scope.spawn(list_windows);
        let tasks = scope.spawn(super::tasks::list);
        let menus = scope.spawn(super::shellmenu::list);
        let services = scope.spawn(super::services::list);
        let mut all = Vec::new();
        for part in [windows.join(), tasks.join(), menus.join(), services.join()] {
            all.extend(part.unwrap_or_default());
        }
        all
    });
    fileinfo::persist();
    out.sort_by_key(|e| (e.kind.group(), e.name.to_lowercase()));
    out
}

type Cached = Option<(Instant, Vec<StartupEntry>)>;

static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
const CACHE_AGE: Duration = Duration::from_secs(5);

pub fn invalidate_cache() {
    if let Ok(mut cache) = CACHE.get_or_init(Default::default).lock() {
        *cache = None;
    }
}

pub fn list_cached() -> Vec<StartupEntry> {
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(guard) = cache.lock()
        && let Some((at, list)) = guard.as_ref()
        && at.elapsed() < CACHE_AGE {
            return list.clone();
        }
    let list = list();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), list.clone()));
    }
    list
}

fn list_windows() -> Vec<StartupEntry> {
    let mut out = Vec::new();
    add_run(&mut out, Hive::User, &format!("{}\\Run", CV), "Run");
    add_run(&mut out, Hive::Machine, &format!("{}\\Run", CV), "Run");
    add_run(&mut out, Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run", "Run32");
    add_run(&mut out, Hive::User, &format!("{}\\RunOnce", CV), "");
    add_run(&mut out, Hive::Machine, &format!("{}\\RunOnce", CV), "");
    add_run(&mut out, Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\RunOnce", "");
    add_run(&mut out, Hive::User, &format!("{}\\Policies\\Explorer\\Run", CV), "");
    add_run(&mut out, Hive::Machine, &format!("{}\\Policies\\Explorer\\Run", CV), "");
    add_legacy_run(&mut out);
    for hive in [Hive::User, Hive::Machine] {
        if let Some(folder) = folder_of(hive) {
            add_folder(&mut out, &folder, hive);
        }
    }
    add_packaged(&mut out);
    add_advanced(&mut out);
    out
}

pub(super) fn folder_of(hive: Hive) -> Option<PathBuf> {
    match hive {
        Hive::User => startup_folder(hive, "Startup", super::winenv::var("APPDATA")),
        Hive::Machine => startup_folder(hive, "Common Startup", super::winenv::var("ProgramData")),
    }
}

fn startup_folder(hive: Hive, value: &str, base: Option<String>) -> Option<PathBuf> {
    let configured = SHELL_FOLDERS.iter().find_map(|key| {
        let raw = regbackup::read_string(&hive.open(key).ok()?, value)?;
        let path = PathBuf::from(fsx::expand_env(raw.trim().trim_matches('"')));
        let text = path.to_string_lossy();
        (path.is_absolute() && !text.contains('%') && fsx::local_drive_path(&text) && !fileinfo::admin_only(&path)).then_some(path)
    });
    configured.or_else(|| base.map(|b| Path::new(&b).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup")).filter(|p| fsx::local_drive_path(&p.to_string_lossy())))
}

fn advanced(hive: Hive, key_path: &str, value_name: &str, name: String, command: String, enabled: bool, system: bool) -> StartupEntry {
    let exe = registry::first_path(&command);
    StartupEntry {
        kind: StartupKind::Advanced,
        name,
        enabled,
        missing: exe.as_deref().is_some_and(registry::missing),
        publisher: exe.as_deref().map(fileinfo::publisher).unwrap_or_default(),
        system,
        hive,
        key_path: key_path.to_string(),
        value_name: value_name.to_string(),
        command,
        ..Default::default()
    }
}

pub fn only_windows(raw: &str, leaf: &str) -> bool {
    let parts: Vec<String> = raw.split(',').map(|p| fsx::expand_env(p.trim().trim_matches('"'))).filter(|p| !p.is_empty()).collect();
    !parts.is_empty() && parts.iter().all(|p| {
        let lower = p.to_lowercase();
        (lower == leaf || lower.ends_with(&format!("\\{}", leaf))) && (!p.contains('\\') || fsx::under_windir(p))
    })
}

fn add_advanced(out: &mut Vec<StartupEntry>) {
    for hive in [Hive::Machine, Hive::User] {
        let Ok(key) = hive.open(WINLOGON) else { continue };
        for (value, leaf) in [("Shell", "explorer.exe"), ("Userinit", "userinit.exe")] {
            let Some(raw) = regbackup::read_string(&key, value).filter(|r| !r.trim().is_empty()) else { continue };
            let system = hive == Hive::Machine && only_windows(&raw, leaf);
            out.push(advanced(hive, WINLOGON, value, format!("Winlogon {}", value), raw, true, system));
        }
    }
    for path in IFEO {
        let Ok(root) = Hive::Machine.open(path) else { continue };
        for exe in root.enum_keys().flatten() {
            let Ok(sub) = registry::target::open_child(&root, &exe, KEY_READ | KEY_WOW64_64KEY) else { continue };
            let Some(debugger) = regbackup::read_string(&sub, "Debugger").filter(|d| !d.trim().is_empty()) else { continue };
            out.push(advanced(Hive::Machine, &format!("{}\\{}", path, exe), "Debugger", format!("{} debugger", exe), debugger, true, false));
        }
    }
    for path in APP_INIT {
        let Ok(key) = Hive::Machine.open(path) else { continue };
        let Some(dlls) = regbackup::read_string(&key, "AppInit_DLLs").filter(|d| !d.trim().is_empty()) else { continue };
        let enabled = regbackup::read_dword(&key, "LoadAppInit_DLLs") == Some(1);
        out.push(advanced(Hive::Machine, path, "AppInit_DLLs", "AppInit_DLLs".to_string(), dlls, enabled, false));
    }
    for path in ACTIVE_SETUP {
        let Ok(root) = Hive::Machine.open(path) else { continue };
        for id in root.enum_keys().flatten() {
            let Ok(sub) = registry::target::open_child(&root, &id, KEY_READ | KEY_WOW64_64KEY) else { continue };
            let Some(stub) = regbackup::read_string(&sub, "StubPath").filter(|s| !s.trim().is_empty()) else { continue };
            let name = regbackup::read_string(&sub, "").map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && !n.starts_with('@')).unwrap_or_else(|| id.clone());
            let system = registry::first_path(&stub).is_none_or(|exe| fsx::is_protected(&exe));
            let enabled = regbackup::read_dword(&sub, "IsInstalled") != Some(0);
            out.push(advanced(Hive::Machine, &format!("{}\\{}", path, id), "StubPath", name, stub, enabled, system));
        }
    }
    if let Ok(key) = Hive::Machine.open(SESSION_MANAGER)
        && let Some(entries) = multi_string(&key, "BootExecute").filter(|e| !e.is_empty()) {
            let system = entries.iter().all(|e| e.to_lowercase().starts_with("autocheck autochk"));
            out.push(advanced(Hive::Machine, SESSION_MANAGER, "BootExecute", "Boot execute".to_string(), entries.join("; "), true, system));
    }
    if let Ok(key) = Hive::Machine.open(LSA) {
        for name in LSA_PACKAGES {
            let Some(entries) = multi_string(&key, name).filter(|e| !e.is_empty()) else { continue };
            let system = entries.iter().all(|e| system_module(e));
            out.push(advanced(Hive::Machine, LSA, name, format!("LSA {}", name.to_lowercase()), entries.join("; "), true, system));
        }
    }
    if let Ok(root) = Hive::Machine.open(PRINT_MONITORS) {
        for name in root.enum_keys().flatten() {
            let Ok(sub) = registry::target::open_child(&root, &name, KEY_READ | KEY_WOW64_64KEY) else { continue };
            let Some(driver) = regbackup::read_string(&sub, "Driver").filter(|d| !d.trim().is_empty()) else { continue };
            let system = system_module(&driver);
            out.push(advanced(Hive::Machine, &format!("{}\\{}", PRINT_MONITORS, name), "Driver", format!("Print monitor {}", name), driver, true, system));
        }
    }
    for hive in [Hive::Machine, Hive::User] {
        if let Ok(key) = hive.open(SHELL_OBJECTS) {
            for (name, value) in key.enum_values().flatten() {
                if value.vtype != REG_SZ && value.vtype != REG_EXPAND_SZ {
                    continue;
                }
                let clsid = regbackup::raw_string(&value.bytes).trim().to_string();
                let (command, system, resolved) = clsid_target(&clsid);
                let system = system || (hive == Hive::Machine && default_shell_object(&name, &clsid));
                let mut entry = advanced(hive, SHELL_OBJECTS, &name, format!("Shell object {}", name), command, true, system);
                entry.missing = !resolved;
                out.push(entry);
            }
        }
        if let Ok(key) = hive.open(EXECUTE_HOOKS) {
            for (name, _) in key.enum_values().flatten() {
                let (command, system, resolved) = clsid_target(&name);
                let shown = super::shellmenu::clsid_name(&name).unwrap_or_else(|| name.clone());
                let mut entry = advanced(hive, EXECUTE_HOOKS, &name, format!("Execute hook {}", shown), command, true, system);
                entry.missing = !resolved;
                out.push(entry);
            }
        }
    }
}

fn default_shell_object(name: &str, clsid: &str) -> bool {
    DEFAULT_SHELL_OBJECTS.iter().any(|(n, c)| name.eq_ignore_ascii_case(n) && clsid.eq_ignore_ascii_case(c))
}

fn blank(entry: &str) -> bool {
    entry.trim_matches(|c: char| c == '"' || c.is_whitespace()).is_empty()
}

fn multi_string(key: &RegKey, name: &str) -> Option<Vec<String>> {
    let v = key.get_raw_value(name).ok()?;
    let entries: Vec<String> = match v.vtype {
        REG_SZ | REG_EXPAND_SZ => vec![regbackup::raw_string(&v.bytes)],
        REG_MULTI_SZ => {
            let units: Vec<u16> = v.bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            units.split(|&u| u == 0).map(String::from_utf16_lossy).collect()
        }
        _ => return None,
    };
    Some(entries.into_iter().map(|e| e.trim().to_string()).filter(|e| !blank(e)).collect())
}

pub fn system_module(name: &str) -> bool {
    let module = name.trim().trim_matches('"');
    if module.is_empty() || module.contains(['*', '?']) {
        return false;
    }
    let file = if Path::new(module).extension().is_some() { module.to_string() } else { format!("{}.dll", module) };
    fsx::is_protected(&file)
}

fn clsid_target(clsid: &str) -> (String, bool, bool) {
    match super::shellmenu::clsid_server(clsid) {
        Some(server) => {
            let system = fsx::is_protected(&server);
            (server, system, true)
        }
        None => (clsid.to_string(), false, false),
    }
}

fn approved_enabled(hive: Hive, sub: &str, name: &str) -> bool {
    let Ok(key) = hive.open(&approved(sub)) else { return true };
    match key.get_raw_value(name) {
        Ok(v) if !v.bytes.is_empty() => v.bytes[0] & 1 == 0,
        _ => true,
    }
}

pub fn windows_run_entry(hive: Hive, name: &str, exe: Option<&str>) -> bool {
    hive == Hive::Machine && name.to_lowercase().starts_with("securityhealth") && exe.is_some_and(|p| p.contains('\\') && fsx::under_windir(p))
}

fn run_entry(hive: Hive, path: &str, sub: &str, name: String, cmd: String) -> StartupEntry {
    let p = registry::first_path(&cmd);
    StartupEntry {
        kind: StartupKind::Run,
        enabled: sub.is_empty() || approved_enabled(hive, sub, &name),
        missing: p.as_deref().is_some_and(registry::missing),
        publisher: p.as_deref().map(fileinfo::publisher).unwrap_or_default(),
        system: windows_run_entry(hive, &name, p.as_deref()),
        hive,
        key_path: path.to_string(),
        approved_sub: sub.to_string(),
        value_name: name.clone(),
        name,
        command: cmd,
        ..Default::default()
    }
}

fn add_run(out: &mut Vec<StartupEntry>, hive: Hive, path: &str, sub: &str) {
    let Ok(key) = hive.open(path) else { return };
    for (name, value) in key.enum_values().flatten() {
        if value.vtype != REG_SZ && value.vtype != REG_EXPAND_SZ {
            continue;
        }
        out.push(run_entry(hive, path, sub, name, regbackup::raw_string(&value.bytes)));
    }
}

fn add_legacy_run(out: &mut Vec<StartupEntry>) {
    for hive in [Hive::User, Hive::Machine] {
        let Ok(key) = hive.open(LEGACY_WINDOWS) else { continue };
        for name in ["Load", "Run"] {
            let Some(cmd) = regbackup::read_string(&key, name).filter(|c| !c.trim().is_empty()) else { continue };
            out.push(run_entry(hive, LEGACY_WINDOWS, "", name.to_string(), cmd));
        }
    }
}

fn add_folder(out: &mut Vec<StartupEntry>, folder: &Path, hive: Hive) {
    if !fsx::local_drive_path(&folder.to_string_lossy()) {
        return;
    }
    let Ok(directory) = fsx::PinnedDir::open(folder) else { return };
    let Ok(rd) = fs::read_dir(folder) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if directory.read_file(&e.file_name()).is_err() {
            continue;
        }
        let leaf = e.file_name().to_string_lossy().into_owned();
        if leaf.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| leaf.clone());
        out.push(StartupEntry {
            kind: StartupKind::Folder,
            name: stem,
            command: path.to_string_lossy().into_owned(),
            enabled: approved_enabled(hive, "StartupFolder", &leaf),
            missing: false,
            policy: false,
            hive,
            key_path: String::new(),
            approved_sub: "StartupFolder".to_string(),
            value_name: leaf,
            file_path: Some(path),
            ..Default::default()
        });
    }
}

fn add_packaged(out: &mut Vec<StartupEntry>) {
    let Ok(root) = Hive::User.open(APP_DATA) else { return };
    for pfn in root.enum_keys().flatten() {
        let Ok(package) = registry::target::open_child(&root, &pfn, KEY_READ | KEY_WOW64_64KEY) else { continue };
        for task in package.enum_keys().flatten() {
            let Ok(t) = registry::target::open_child(&package, &task, KEY_READ | KEY_WOW64_64KEY) else { continue };
            let Some(state) = regbackup::read_dword(&t, "State") else { continue };
            let short = pfn.split('_').next().unwrap_or(&pfn).to_string();
            out.push(StartupEntry {
                kind: StartupKind::Packaged,
                name: format!("{} / {}", short, task),
                command: pfn.clone(),
                enabled: state == 2 || state == 4,
                missing: false,
                policy: state == 3 || state == 4,
                hive: Hive::User,
                key_path: format!("{}\\{}\\{}", APP_DATA, pfn, task),
                approved_sub: String::new(),
                value_name: String::new(),
                file_path: None,
                ..Default::default()
            });
        }
    }
}

pub fn set_enabled(e: &mut StartupEntry, enable: bool) -> io::Result<()> {
    invalidate_cache();
    if e.policy {
        return Err(io::Error::other("policy"));
    }
    if e.system {
        return Err(io::Error::other("system"));
    }
    if e.kind == StartupKind::Advanced {
        return Err(io::Error::other("locked"));
    }
    if e.kind == StartupKind::Run && e.approved_sub.is_empty() {
        return Err(io::Error::other("nodisable"));
    }
    match e.kind {
        StartupKind::Task => return super::tasks::set_enabled(e, enable),
        StartupKind::Context => return super::shellmenu::set_enabled(e, enable),
        StartupKind::Service => return super::services::set_enabled(e, enable),
        _ => {}
    }
    if e.kind == StartupKind::Packaged {
        let key = e.hive.open_write(&e.key_path)?;
        key.set_value("State", &(if enable { 2u32 } else { 1u32 }))?;
    } else {
        let mut data = vec![0u8; 12];
        data[0] = if enable { 2 } else { 3 };
        if !enable {
            data[4..12].copy_from_slice(&sys::filetime_now().to_le_bytes());
        }
        let key = e.hive.create(&approved(&e.approved_sub))?;
        key.set_raw_value(&e.value_name, &RegValue { bytes: data.into(), vtype: REG_BINARY })?;
    }
    e.enabled = enable;
    data::log(&format!("startup {} {}", if enable { "enabled" } else { "disabled" }, e.name));
    Ok(())
}

pub fn delete(e: &StartupEntry, backup_dir: &Path) -> io::Result<()> {
    invalidate_cache();
    if e.system {
        return Err(io::Error::other("system"));
    }
    match e.kind {
        StartupKind::Task => return super::tasks::delete(e, backup_dir),
        StartupKind::Context => return super::shellmenu::delete(e, backup_dir),
        StartupKind::Service => return super::services::delete(e, backup_dir),
        _ => {}
    }
    if e.kind == StartupKind::Packaged {
        return Err(io::Error::other("nodelete"));
    }
    if e.kind == StartupKind::Advanced {
        return Err(io::Error::other("locked"));
    }
    if e.kind == StartupKind::Run && (e.value_name.is_empty() || e.key_path.is_empty()) {
        return Err(io::Error::other("bad entry"));
    }
    if e.kind == StartupKind::Folder {
        let file = e.file_path.as_ref().ok_or_else(|| io::Error::other("bad entry"))?;
        let parent = file.parent().ok_or_else(|| io::Error::other("bad entry"))?;
        let leaf = file.file_name().ok_or_else(|| io::Error::other("bad entry"))?;
        let source = fsx::PinnedDir::open(parent)?;
        let backup = fsx::PinnedDir::open(backup_dir)?;
        let mut kept = std::ffi::OsString::from(if e.hive == Hive::Machine { "startup-common-" } else { "startup-user-" });
        kept.push(leaf);
        let elevated = sys::elevation()?;
        source.backup_and_remove_file(leaf, &backup, &kept, |path, copy| {
            if !elevated {
                return Ok(());
            }
            let mut bytes = Vec::new();
            copy.take(super::backups::MAX_FILE + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > super::backups::MAX_FILE {
                return Ok(());
            }
            regbackup::integrity::record(path, &bytes)
        })?;
    } else {
        let key = e.hive.open_write(&e.key_path)?;
        regbackup::export_open_value(e.hive, &e.key_path, &key, &e.value_name, backup_dir)?;
        key.delete_value(&e.value_name)?;
    }
    if !e.approved_sub.is_empty()
        && let Ok(key) = e.hive.open_write(&approved(&e.approved_sub)) {
            let _ = key.delete_value(&e.value_name);
    }
    data::log(&format!("startup deleted {}", e.name));
    Ok(())
}
