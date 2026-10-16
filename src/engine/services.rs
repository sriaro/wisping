use std::fs;
use std::io;
use std::path::Path;

use winreg::RegKey;
use winreg::enums::*;

use super::data;
use super::fileinfo;
use super::fsx;
use super::regbackup::{self, Hive};
use super::registry;
use super::startup::{StartupEntry, StartupKind};
use super::sys;

const SERVICES: &str = "SYSTEM\\CurrentControlSet\\Services";
const STATE_FILE: &str = "service-start.txt";
const DEFAULT_ENABLED_START: u32 = 3;
const PROTECTED: [&str; 12] = ["WinDefend", "WdNisSvc", "WdFilter", "Sense", "MDCoreSvc", "SecurityHealthService", "wscsvc", "mpssvc", "BFE", "EventLog", "RpcSs", "wuauserv"];

fn normalize(image: &str) -> String {
    let t = image.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("\\systemroot\\") {
        return format!("%SystemRoot%\\{}", &t[12..]);
    }
    if lower.starts_with("system32\\") {
        return format!("%SystemRoot%\\{}", t);
    }
    if lower.starts_with("\\??\\") {
        return t[4..].to_string();
    }
    t.to_string()
}

fn program_of(key: &RegKey) -> String {
    let image = regbackup::read_string(key, "ImagePath").map(|i| normalize(&i)).unwrap_or_default();
    let exe = registry::first_path(&image).unwrap_or_default();
    if exe.to_lowercase().ends_with("svchost.exe")
        && let Ok(params) = key.open_subkey_with_flags("Parameters", KEY_READ | KEY_WOW64_64KEY)
            && let Some(dll) = regbackup::read_string(&params, "ServiceDll") {
                return fsx::expand_env(dll.trim().trim_matches('"'));
            }
    if exe.is_empty() { fsx::expand_env(&image) } else { exe }
}

fn start_label(key: &RegKey, start: u32) -> &'static str {
    match start {
        0 => "Boot",
        1 => "System",
        2 if regbackup::read_dword(key, "DelayedAutostart") == Some(1) => "Automatic (delayed)",
        2 => "Automatic",
        3 => "Manual",
        4 => "Disabled",
        _ => "Unknown",
    }
}

fn entry(name: &str) -> Option<StartupEntry> {
    let key = Hive::Machine.open(&format!("{}\\{}", SERVICES, name)).ok()?;
    let kind = regbackup::read_dword(&key, "Type")?;
    if kind & 0x30 == 0 {
        return None;
    }
    let start = regbackup::read_dword(&key, "Start")?;
    let program = program_of(&key);
    if program.is_empty() {
        return None;
    }
    let display = regbackup::read_string(&key, "DisplayName").filter(|d| !d.trim().is_empty() && !d.starts_with('@')).unwrap_or_else(|| name.to_string());
    let system = fsx::is_protected(&program) || PROTECTED.iter().any(|n| n.eq_ignore_ascii_case(name));
    Some(StartupEntry {
        kind: StartupKind::Service,
        name: display,
        enabled: start != 4,
        missing: registry::missing(&program),
        hive: Hive::Machine,
        key_path: name.to_string(),
        value_name: name.to_string(),
        publisher: fileinfo::publisher(&program),
        system,
        detail: start_label(&key, start).to_string(),
        command: program,
        ..Default::default()
    })
}

pub fn list() -> Vec<StartupEntry> {
    let Ok(root) = Hive::Machine.open(SERVICES) else { return Vec::new() };
    let names: Vec<String> = root.enum_keys().flatten().collect();
    sys::par_map(&names, |name| entry(name))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 256 && !name.contains(['\\', '/', '"', '\n', '\r']) && !name.starts_with('.')
}

fn state_path() -> std::path::PathBuf {
    data::root().join(STATE_FILE)
}

fn read_state() -> Vec<(String, u32)> {
    let text = fs::read_to_string(state_path()).unwrap_or_default();
    text.lines().filter_map(|l| l.rsplit_once('=')).filter_map(|(n, v)| v.trim().parse().ok().map(|v| (n.to_string(), v))).collect()
}

fn write_state(items: &[(String, u32)]) -> io::Result<()> {
    data::ensure_root();
    let text: String = items.iter().map(|(n, v)| format!("{}={}\r\n", n, v)).collect();
    fs::write(state_path(), text)
}

pub fn set_enabled(e: &mut StartupEntry, enable: bool) -> io::Result<()> {
    if !valid_name(&e.value_name) {
        return Err(io::Error::other("bad entry"));
    }
    let key = Hive::Machine.open_write(&format!("{}\\{}", SERVICES, e.value_name))?;
    let current = regbackup::read_dword(&key, "Start").ok_or_else(|| io::Error::other("bad entry"))?;
    if (current != 4) == enable {
        e.enabled = enable;
        e.detail = start_label(&key, current).to_string();
        return Ok(());
    }
    let mut state = read_state();
    let target = if enable {
        state.iter().find(|(n, _)| n.eq_ignore_ascii_case(&e.value_name)).map(|(_, v)| *v).filter(|v| *v <= 3).unwrap_or(DEFAULT_ENABLED_START)
    } else {
        state.retain(|(n, _)| !n.eq_ignore_ascii_case(&e.value_name));
        state.push((e.value_name.clone(), current));
        write_state(&state).map_err(|err| io::Error::other(format!("could not save the current start type: {}", err)))?;
        4
    };
    key.set_value("Start", &target)?;
    e.enabled = enable;
    e.detail = start_label(&key, target).to_string();
    data::log(&format!("service {} {} (start {} -> {})", if enable { "enabled" } else { "disabled" }, e.value_name, current, target));
    Ok(())
}

pub fn delete(e: &StartupEntry, backup_dir: &Path) -> io::Result<()> {
    if !valid_name(&e.value_name) {
        return Err(io::Error::other("bad entry"));
    }
    let path = format!("{}\\{}", SERVICES, e.value_name);
    Hive::Machine.open_write(&path)?;
    regbackup::export_key(Hive::Machine, &path, backup_dir)?;
    let out = sys::hidden("sc.exe").arg("delete").arg(&e.value_name).output()?;
    if !out.status.success() {
        return Err(io::Error::other(sys::command_failure(&out.stdout, out.status.code())));
    }
    data::log(&format!("service deleted {}", e.value_name));
    Ok(())
}
