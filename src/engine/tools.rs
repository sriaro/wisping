use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use winreg::enums::*;

use super::data;
use super::regbackup::{self, Hive};
use super::registry;
use super::sys;
use super::trayicons;

const GAME_DVR: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\GameDVR";
const GAME_STORE: &str = "System\\GameConfigStore";
const NOTIFY_SETTINGS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
const NOTIFY_ICONS: &str = "Control Panel\\NotifyIconSettings";
const TRAY: &str = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\CurrentVersion\\TrayNotify";
const TRAY_VALUES: [&str; 2] = ["IconStreams", "PastIconsStream"];
const DEVMETA_POLICY: &str = "SOFTWARE\\Policies\\Microsoft\\Windows\\Device Metadata";
const DEVMETA_USER: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Device Metadata";
const DEVMETA_VALUE: &str = "PreventDeviceMetadataFromNetwork";
const TELEMETRY_POLICY: &str = "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection";
const WER_POLICY: &str = "SOFTWARE\\Policies\\Microsoft\\Windows\\Windows Error Reporting";

struct Policy {
    id: &'static str,
    key: &'static str,
    mirror: Option<&'static str>,
    name: &'static str,
    value: u32,
}

const POLICIES: [Policy; 3] = [
    Policy { id: "devmeta", key: DEVMETA_POLICY, mirror: Some(DEVMETA_USER), name: DEVMETA_VALUE, value: 1 },
    Policy { id: "telemetry", key: TELEMETRY_POLICY, mirror: None, name: "AllowTelemetry", value: 0 },
    Policy { id: "wer", key: WER_POLICY, mirror: None, name: "Disabled", value: 1 },
];

pub struct ToolInfo {
    pub id: &'static str,
    pub icon: &'static str,
    pub tint: i32,
    pub toggle: bool,
    pub admin: bool,
}

pub const TOOLS: [ToolInfo; 7] = [
    ToolInfo { id: "gamebar", icon: "zap", tint: 2, toggle: true, admin: false },
    ToolInfo { id: "devmeta", icon: "shield", tint: 5, toggle: true, admin: true },
    ToolInfo { id: "telemetry", icon: "shield-alert", tint: 4, toggle: true, admin: true },
    ToolInfo { id: "wer", icon: "alert", tint: 6, toggle: true, admin: true },
    ToolInfo { id: "nvtray", icon: "shield", tint: 5, toggle: true, admin: false },
    ToolInfo { id: "notify", icon: "alert", tint: 3, toggle: false, admin: false },
    ToolInfo { id: "tray", icon: "refresh", tint: 1, toggle: false, admin: false },
];

fn key_allowed(path: &str) -> bool {
    let lower = path.to_lowercase();
    [NOTIFY_SETTINGS, NOTIFY_ICONS].iter().any(|k| lower == k.to_lowercase()) || (test_key(&lower))
}

fn test_key(lower: &str) -> bool {
    registry::allow_test_key() && {
        let base = registry::TEST_KEY.to_lowercase();
        lower == base || super::fsx::key_within(lower, &base)
    }
}

fn value_allowed(path: &str, name: &str) -> bool {
    (path.eq_ignore_ascii_case(TRAY) && TRAY_VALUES.iter().any(|v| v.eq_ignore_ascii_case(name))) || (test_key(&path.to_lowercase()) && !name.is_empty())
}

fn flags(id: &str) -> Option<[(&'static str, &'static str); 2]> {
    match id {
        "gamebar" => Some([(GAME_DVR, "AppCaptureEnabled"), (GAME_STORE, "GameDVR_Enabled")]),
        _ => None,
    }
}

fn flag_pairs_ok(pairs: &[(&str, &str)]) -> bool {
    pairs.iter().all(|(k, n)| {
        let lower = k.to_lowercase();
        (lower == GAME_DVR.to_lowercase() && *n == "AppCaptureEnabled") || (lower == GAME_STORE.to_lowercase() && *n == "GameDVR_Enabled") || (test_key(&lower) && !n.is_empty())
    })
}

pub fn read_flags(pairs: &[(&str, &str)]) -> bool {
    pairs.iter().all(|(k, n)| match Hive::User.open(k) {
        Ok(key) => regbackup::read_dword(&key, n).map(|v| v != 0).unwrap_or(true),
        Err(_) => true,
    })
}

pub fn write_flags(pairs: &[(&str, &str)], on: bool) -> io::Result<()> {
    if !flag_pairs_ok(pairs) {
        return Err(io::Error::other("refused"));
    }
    for (k, n) in pairs {
        let (key, _) = Hive::User.root().create_subkey_with_flags(k, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?;
        key.set_value(n, &(on as u32))?;
    }
    Ok(())
}

fn policy_allowed(hive: Hive, policy: &str, mirror: Option<&str>, name: &str, value: u32) -> bool {
    let listed = hive == Hive::Machine && POLICIES.iter().any(|p| p.key == policy && p.mirror == mirror && p.name == name && p.value == value);
    listed || (hive == Hive::User && test_key(&policy.to_lowercase()) && mirror.is_none_or(|m| test_key(&m.to_lowercase())) && !name.is_empty())
}

pub fn read_policy(hive: Hive, policy: &str, name: &str, value: u32) -> bool {
    hive.open(policy).ok().and_then(|k| regbackup::read_dword(&k, name)).is_some_and(|v| v == value)
}

pub fn set_policy(hive: Hive, policy: &str, mirror: Option<&str>, name: &str, value: u32, on: bool, new_backup: &dyn Fn() -> io::Result<PathBuf>) -> io::Result<()> {
    if !policy_allowed(hive, policy, mirror, name, value) {
        return Err(io::Error::other("refused"));
    }
    if hive == Hive::Machine && !sys::is_admin() {
        return Err(io::Error::other("This needs administrator rights. Use \"Restart as admin\" in the sidebar."));
    }
    let keys: Vec<&str> = std::iter::once(policy).chain(mirror).collect();
    let state: Vec<(bool, Option<u32>)> = keys.iter().map(|k| hive.open(k).map(|key| (regbackup::has_value(&key, name), regbackup::read_dword(&key, name))).unwrap_or((false, None))).collect();
    if state.iter().any(|s| s.0 && s.1.is_none()) {
        return Err(io::Error::other("unexpected value type"));
    }
    let ids: Vec<String> = keys.iter().map(|k| mark_id(hive, k, name)).collect();
    let mut marks = read_marks();
    let mark_of = |marks: &[(String, Option<u32>)], i: usize| marks.iter().find(|m| m.0 == ids[i]).map(|m| m.1);
    if on {
        let present: Vec<usize> = (0..keys.len()).filter(|&i| state[i].0).collect();
        backup_values(hive, &keys, &present, name, new_backup)?;
        for i in 0..keys.len() {
            if mark_of(&marks, i).is_none() {
                marks.push((ids[i].clone(), state[i].1));
            }
        }
        write_marks(&marks)?;
        for k in &keys {
            let (key, _) = hive.root().create_subkey_with_flags(k, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?;
            key.set_value(name, &value)?;
        }
        return Ok(());
    }
    let mut acts: Vec<(usize, Option<u32>)> = Vec::new();
    for (i, entry) in state.iter().enumerate().take(keys.len()) {
        match mark_of(&marks, i).or(if i == 0 { Some(None) } else { None }) {
            Some(None) if entry.1 == Some(value) => acts.push((i, None)),
            Some(Some(prev)) if entry.1 != Some(prev) => acts.push((i, Some(prev))),
            _ => {}
        }
    }
    let present: Vec<usize> = acts.iter().map(|a| a.0).filter(|&i| state[i].0).collect();
    backup_values(hive, &keys, &present, name, new_backup)?;
    for &(i, restore) in &acts {
        match restore {
            Some(prev) => hive.root().create_subkey_with_flags(keys[i], KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?.0.set_value(name, &prev)?,
            None => hive.open_write(keys[i])?.delete_value(name)?,
        }
    }
    marks.retain(|m| !ids.contains(&m.0));
    write_marks(&marks)?;
    let deleted = acts.iter().any(|a| a.0 == 0 && a.1.is_none());
    let empty = deleted && hive.open(policy).map(|k| k.enum_values().count() == 0 && k.enum_keys().count() == 0).unwrap_or(false);
    if empty
        && let Some(slash) = policy.rfind('\\') {
            let _ = hive.open_write(&policy[..slash]).and_then(|p| p.delete_subkey(&policy[slash + 1..]));
        }
    Ok(())
}

const POLICY_STATE: &str = "policy-state.txt";

fn mark_id(hive: Hive, key: &str, name: &str) -> String {
    format!("{}|{}|{}", hive.short_name(), key.to_lowercase(), name.to_lowercase())
}

fn read_marks() -> Vec<(String, Option<u32>)> {
    let text = fs::read_to_string(data::root().join(POLICY_STATE)).unwrap_or_default();
    text.lines().filter_map(|l| l.rsplit_once('=')).map(|(k, v)| (k.to_string(), v.trim().parse().ok())).collect()
}

fn write_marks(marks: &[(String, Option<u32>)]) -> io::Result<()> {
    if marks.is_empty() {
        return match fs::remove_file(data::root().join(POLICY_STATE)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    data::ensure_root();
    let text: String = marks.iter().map(|(k, v)| format!("{}={}\r\n", k, v.map_or_else(|| "none".to_string(), |v| v.to_string()))).collect();
    fs::write(data::root().join(POLICY_STATE), text)
}

fn backup_values(hive: Hive, keys: &[&str], idx: &[usize], name: &str, new_backup: &dyn Fn() -> io::Result<PathBuf>) -> io::Result<()> {
    if idx.is_empty() {
        return Ok(());
    }
    let dir = new_backup()?;
    for &i in idx {
        regbackup::export_value(hive, keys[i], name, &dir)?;
    }
    Ok(())
}

fn policy_of(id: &str) -> Option<&'static Policy> {
    POLICIES.iter().find(|p| p.id == id)
}

pub fn is_on(id: &str) -> bool {
    if let Some(g) = trayicons::group(id) {
        return trayicons::is_hidden(trayicons::ROOT, g.needles);
    }
    match policy_of(id) {
        Some(p) => read_policy(Hive::Machine, p.key, p.name, p.value),
        None => flags(id).map(|p| !read_flags(&p)).unwrap_or(false),
    }
}

pub fn set_on(id: &str, on: bool) -> io::Result<()> {
    if let Some(g) = trayicons::group(id) {
        trayicons::set_hidden(trayicons::ROOT, g.needles, on, &data::new_backup_dir)?;
        data::log(&format!("tool {} {}", id, if on { "enabled" } else { "disabled" }));
        return Ok(());
    }
    match policy_of(id) {
        Some(p) => set_policy(Hive::Machine, p.key, p.mirror, p.name, p.value, on, &data::new_backup_dir)?,
        None => {
            let pairs = flags(id).ok_or_else(|| io::Error::other("unknown tool"))?;
            write_flags(&pairs, !on)?;
        }
    }
    data::log(&format!("tool {} {}", id, if on { "enabled" } else { "disabled" }));
    Ok(())
}

pub fn telemetry_limited() -> bool {
    let edition = Hive::Machine.open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion").ok().and_then(|k| regbackup::read_string(&k, "EditionID")).unwrap_or_default().to_lowercase();
    !(edition.contains("enterprise") || edition.contains("education") || edition.contains("server"))
}

pub fn detail(id: &str) -> String {
    match id {
        "telemetry" if telemetry_limited() => "limited".to_string(),
        "notify" => Hive::User.open(NOTIFY_SETTINGS).map(|k| k.enum_keys().count()).map(|n| n.to_string()).unwrap_or_else(|_| "0".to_string()),
        "tray" => Hive::User.open(NOTIFY_ICONS).map(|k| k.enum_keys().count()).map(|n| n.to_string()).unwrap_or_else(|_| "0".to_string()),
        _ => String::new(),
    }
}

pub fn delete_tree(path: &str, backup_dir: &Path) -> io::Result<bool> {
    if !key_allowed(path) {
        return Err(io::Error::other("refused"));
    }
    if Hive::User.open(path).is_err() {
        return Ok(false);
    }
    regbackup::export_key(Hive::User, path, backup_dir)?;
    let slash = path.rfind('\\').ok_or_else(|| io::Error::other("bad key"))?;
    Hive::User.open_write(&path[..slash])?.delete_subkey_all(&path[slash + 1..])?;
    Ok(true)
}

pub fn delete_value(path: &str, name: &str, backup_dir: &Path) -> io::Result<bool> {
    if !value_allowed(path, name) {
        return Err(io::Error::other("refused"));
    }
    let Ok(key) = Hive::User.open(path) else { return Ok(false) };
    if key.get_raw_value(name).is_err() {
        return Ok(false);
    }
    regbackup::export_value(Hive::User, path, name, backup_dir)?;
    Hive::User.open_write(path)?.delete_value(name)?;
    Ok(true)
}

fn session_id() -> Option<u32> {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    let mut id = 0u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id).ok()? };
    Some(id)
}

fn kill_explorer() {
    if let Some(id) = session_id() {
        let filter = format!("SESSION eq {}", id);
        let _ = sys::hidden("taskkill.exe").args(["/FI", filter.as_str(), "/IM", "explorer.exe", "/F"]).output();
        thread::sleep(Duration::from_millis(900));
    }
}

fn start_explorer() {
    let _ = std::process::Command::new(sys::system_tool("explorer.exe")).spawn();
}

pub fn run(id: &str, backup_dir: &Path) -> io::Result<()> {
    match id {
        "notify" => {
            delete_tree(NOTIFY_SETTINGS, backup_dir)?;
        }
        "tray" => {
            kill_explorer();
            let first = delete_tree(NOTIFY_ICONS, backup_dir);
            let mut second = Ok(false);
            for v in TRAY_VALUES {
                if let Err(e) = delete_value(TRAY, v, backup_dir) {
                    second = Err(e);
                }
            }
            start_explorer();
            first?;
            second?;
        }
        _ => return Err(io::Error::other("unknown tool")),
    }
    data::log(&format!("tool {} run", id));
    Ok(())
}

pub fn restart_pc() {
    let _ = sys::hidden("shutdown.exe").args(["/r", "/t", "20"]).output();
}
