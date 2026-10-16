use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use winreg::RegValue;
use winreg::enums::*;

use super::data;
use super::fileinfo;
use super::fsx;
use super::regbackup::{self, Hive};
use super::registry;
use super::sys;

const CV: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
const APP_DATA: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\AppModel\\SystemAppData";

fn approved(sub: &str) -> String {
    format!("{}\\Explorer\\StartupApproved\\{}", CV, sub)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StartupKind {
    #[default]
    Run,
    Folder,
    Packaged,
    Task,
    Context,
    Service,
}

impl StartupKind {
    pub fn group(self) -> usize {
        match self {
            StartupKind::Run | StartupKind::Folder | StartupKind::Packaged => 0,
            StartupKind::Task => 1,
            StartupKind::Context => 2,
            StartupKind::Service => 3,
        }
    }
}

#[derive(Clone, Debug, Default)]
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
    let mut out = std::thread::scope(|scope| {
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
    out.sort_by_key(|e| (e.kind.group(), e.name.to_lowercase()));
    out
}

fn list_windows() -> Vec<StartupEntry> {
    let mut out = Vec::new();
    add_run(&mut out, Hive::User, &format!("{}\\Run", CV), "Run");
    add_run(&mut out, Hive::Machine, &format!("{}\\Run", CV), "Run");
    add_run(&mut out, Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run", "Run32");
    if let Ok(appdata) = std::env::var("APPDATA") {
        add_folder(&mut out, &Path::new(&appdata).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"), Hive::User);
    }
    if let Ok(pd) = std::env::var("ProgramData") {
        add_folder(&mut out, &Path::new(&pd).join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"), Hive::Machine);
    }
    add_packaged(&mut out);
    out
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

fn add_run(out: &mut Vec<StartupEntry>, hive: Hive, path: &str, sub: &str) {
    let Ok(key) = hive.open(path) else { return };
    for item in key.enum_values().flatten() {
        let (name, value) = item;
        if value.vtype != REG_SZ && value.vtype != REG_EXPAND_SZ {
            continue;
        }
        let cmd = regbackup::raw_string(&value.bytes);
        let p = registry::first_path(&cmd);
        out.push(StartupEntry {
            kind: StartupKind::Run,
            name: name.clone(),
            command: cmd,
            enabled: approved_enabled(hive, sub, &name),
            missing: p.as_deref().is_some_and(registry::missing),
            publisher: p.as_deref().map(fileinfo::publisher).unwrap_or_default(),
            policy: false,
            system: windows_run_entry(hive, &name, p.as_deref()),
            hive,
            key_path: path.to_string(),
            approved_sub: sub.to_string(),
            value_name: name,
            file_path: None,
            ..Default::default()
        });
    }
}

fn add_folder(out: &mut Vec<StartupEntry>, folder: &Path, hive: Hive) {
    let Ok(rd) = fs::read_dir(folder) else { return };
    for e in rd.flatten() {
        let path = e.path();
        if !path.is_file() {
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
        let Ok(package) = root.open_subkey_with_flags(&pfn, KEY_READ | KEY_WOW64_64KEY) else { continue };
        for task in package.enum_keys().flatten() {
            let Ok(t) = package.open_subkey_with_flags(&task, KEY_READ | KEY_WOW64_64KEY) else { continue };
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
    if e.policy {
        return Err(io::Error::other("policy"));
    }
    if e.system {
        return Err(io::Error::other("system"));
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
        let (key, _) = e.hive.root().create_subkey_with_flags(approved(&e.approved_sub), KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?;
        key.set_raw_value(&e.value_name, &RegValue { bytes: data.into(), vtype: REG_BINARY })?;
    }
    e.enabled = enable;
    data::log(&format!("startup {} {}", if enable { "enabled" } else { "disabled" }, e.name));
    Ok(())
}

pub fn delete(e: &StartupEntry, backup_dir: &Path) -> io::Result<()> {
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
    if e.kind == StartupKind::Run && (e.value_name.is_empty() || e.key_path.is_empty()) {
        return Err(io::Error::other("bad entry"));
    }
    if e.kind == StartupKind::Folder {
        let file = e.file_path.as_ref().filter(|p| p.is_file()).ok_or_else(|| io::Error::other("bad entry"))?;
        if fsx::is_reparse(file) {
            return Err(io::Error::other("bad entry"));
        }
        let leaf = file.file_name().ok_or_else(|| io::Error::other("bad entry"))?;
        let copy = backup_dir.join(leaf);
        fs::copy(file, &copy)?;
        if fs::metadata(&copy)?.len() != fs::metadata(file)?.len() {
            return Err(io::Error::other("backup copy failed"));
        }
        fs::remove_file(file)?;
    } else {
        regbackup::export_value(e.hive, &e.key_path, &e.value_name, backup_dir)?;
        let key = e.hive.open_write(&e.key_path)?;
        key.delete_value(&e.value_name)?;
    }
    if let Ok(key) = e.hive.open_write(&approved(&e.approved_sub)) {
        let _ = key.delete_value(&e.value_name);
    }
    data::log(&format!("startup deleted {}", e.name));
    Ok(())
}
