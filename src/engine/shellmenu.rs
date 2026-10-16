use std::collections::HashSet;
use std::io;
use std::path::Path;

use winreg::enums::*;

use super::data;
use super::fileinfo;
use super::fsx;
use super::regbackup::{self, Hive};
use super::registry;
use super::startup::{StartupEntry, StartupKind};

const ROOTS: [(&str, &str); 6] = [
    ("*", "All files"),
    ("AllFilesystemObjects", "Files and folders"),
    ("Directory", "Folders"),
    ("Directory\\Background", "Folder background"),
    ("Drive", "Drives"),
    ("Folder", "Folders"),
];

const BLOCKED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Shell Extensions\\Blocked";

fn classes(hive: Hive) -> &'static str {
    match hive {
        Hive::Machine => "SOFTWARE\\Classes",
        Hive::User => "Software\\Classes",
    }
}

fn is_guid(text: &str) -> bool {
    text.len() == 38 && text.starts_with('{') && text.ends_with('}')
}

fn clsid_server(clsid: &str) -> Option<String> {
    let probes = [
        (Hive::Machine, format!("SOFTWARE\\Classes\\CLSID\\{}\\InprocServer32", clsid)),
        (Hive::Machine, format!("SOFTWARE\\Classes\\WOW6432Node\\CLSID\\{}\\InprocServer32", clsid)),
        (Hive::User, format!("Software\\Classes\\CLSID\\{}\\InprocServer32", clsid)),
        (Hive::Machine, format!("SOFTWARE\\Classes\\CLSID\\{}\\LocalServer32", clsid)),
    ];
    for (hive, path) in probes {
        if let Ok(key) = hive.open(&path)
            && let Some(v) = regbackup::read_string(&key, "")
                && let Some(p) = registry::first_path(&v).or_else(|| Some(fsx::expand_env(v.trim().trim_matches('"')))).filter(|p| !p.is_empty()) {
                    return Some(p);
                }
    }
    None
}

fn clsid_name(clsid: &str) -> Option<String> {
    for hive in [Hive::Machine, Hive::User] {
        if let Ok(key) = hive.open(&format!("{}\\CLSID\\{}", classes(hive), clsid))
            && let Some(v) = regbackup::read_string(&key, "")
                && !v.trim().is_empty() && !v.starts_with('@') {
                    return Some(v.trim().to_string());
                }
    }
    None
}

fn blocked_names(hive: Hive) -> Vec<String> {
    hive.open(BLOCKED).map(|key| key.enum_values().flatten().map(|item| item.0.to_lowercase()).collect()).unwrap_or_default()
}

fn blocked_in(hive: Hive, clsid: &str) -> bool {
    blocked_names(hive).contains(&clsid.to_lowercase())
}

pub fn is_blocked(clsid: &str) -> bool {
    blocked_in(Hive::Machine, clsid) || blocked_in(Hive::User, clsid)
}

pub fn list() -> Vec<StartupEntry> {
    let blocked: HashSet<String> = blocked_names(Hive::Machine).into_iter().chain(blocked_names(Hive::User)).collect();
    let mut out = Vec::new();
    for hive in [Hive::Machine, Hive::User] {
        for (root, label) in ROOTS {
            let base = format!("{}\\{}", classes(hive), root);
            if let Ok(handlers) = hive.open(&format!("{}\\shellex\\ContextMenuHandlers", base)) {
                for name in handlers.enum_keys().flatten() {
                    let Ok(sub) = handlers.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY) else { continue };
                    let default = regbackup::read_string(&sub, "").unwrap_or_default();
                    let clsid = if is_guid(default.trim()) {
                        default.trim().to_string()
                    } else if is_guid(&name) {
                        name.clone()
                    } else {
                        continue;
                    };
                    let program = clsid_server(&clsid).unwrap_or_default();
                    let shown = if is_guid(&name) { clsid_name(&clsid).unwrap_or(name.clone()) } else { name.clone() };
                    let system = !program.is_empty() && fsx::is_protected(&program);
                    out.push(StartupEntry {
                        kind: StartupKind::Context,
                        name: shown,
                        command: program.clone(),
                        enabled: !blocked.contains(&clsid.to_lowercase()),
                        missing: !program.is_empty() && registry::missing(&program),
                        hive,
                        key_path: format!("{}\\shellex\\ContextMenuHandlers\\{}", base, name),
                        value_name: clsid,
                        publisher: fileinfo::publisher(&program),
                        system,
                        detail: label.to_string(),
                        ..Default::default()
                    });
                }
            }
            if let Ok(verbs) = hive.open(&format!("{}\\shell", base)) {
                for name in verbs.enum_keys().flatten() {
                    let Ok(sub) = verbs.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY) else { continue };
                    let command = sub.open_subkey_with_flags("command", KEY_READ | KEY_WOW64_64KEY).ok().and_then(|c| regbackup::read_string(&c, "")).unwrap_or_default();
                    let exe = registry::first_path(&command);
                    let shown = ["MUIVerb", ""]
                        .iter()
                        .filter_map(|v| regbackup::read_string(&sub, v))
                        .find(|v| !v.trim().is_empty() && !v.starts_with('@'))
                        .map(|v| v.replace('&', "").trim().to_string())
                        .unwrap_or_else(|| name.clone());
                    let system = hive == Hive::Machine && (command.trim().is_empty() || exe.as_deref().map(fsx::is_protected).unwrap_or(true));
                    out.push(StartupEntry {
                        kind: StartupKind::Context,
                        name: shown,
                        command: command.clone(),
                        enabled: !regbackup::has_value(&sub, "LegacyDisable"),
                        missing: exe.as_deref().map(registry::missing).unwrap_or(false),
                        hive,
                        key_path: format!("{}\\shell\\{}", base, name),
                        publisher: exe.as_deref().map(fileinfo::publisher).unwrap_or_default(),
                        system,
                        detail: label.to_string(),
                        ..Default::default()
                    });
                }
            }
        }
    }
    out
}

fn locate(hive: Hive, key: &str) -> Option<bool> {
    if key.is_empty() || key.contains("..") || key.ends_with('\\') {
        return None;
    }
    let lower = key.to_lowercase();
    let mut prefixes = vec![format!("{}\\", classes(hive).to_lowercase())];
    let mut roots: Vec<String> = ROOTS.iter().map(|r| r.0.to_lowercase()).collect();
    if registry::allow_test_key() && hive == Hive::User {
        prefixes.push(format!("{}\\", registry::TEST_KEY.to_lowercase()));
        roots.push("ctxtest".to_string());
    }
    for prefix in prefixes {
        let Some(rest) = lower.strip_prefix(&prefix) else { continue };
        for root in &roots {
            if fsx::direct_child(rest, &format!("{}\\shellex\\contextmenuhandlers\\", root)) {
                return Some(true);
            }
            if fsx::direct_child(rest, &format!("{}\\shell\\", root)) {
                return Some(false);
            }
        }
    }
    None
}

pub fn set_blocked(hive: Hive, clsid: &str, on: bool) -> io::Result<()> {
    if !is_guid(clsid) {
        return Err(io::Error::other("bad handler"));
    }
    if on {
        let (key, _) = hive.root().create_subkey_with_flags(BLOCKED, KEY_READ | KEY_WRITE | KEY_WOW64_64KEY)?;
        key.set_value(clsid, &"")
    } else {
        let Ok(key) = hive.open_write(BLOCKED) else {
            return if blocked_in(hive, clsid) { Err(io::Error::new(io::ErrorKind::PermissionDenied, "access denied")) } else { Ok(()) };
        };
        let names: Vec<String> = key.enum_values().flatten().map(|v| v.0).filter(|n| n.eq_ignore_ascii_case(clsid)).collect();
        for n in names {
            key.delete_value(n)?;
        }
        Ok(())
    }
}

pub fn set_enabled(e: &mut StartupEntry, enable: bool) -> io::Result<()> {
    let handler = locate(e.hive, &e.key_path).ok_or_else(|| io::Error::other("bad entry"))?;
    if handler {
        if enable {
            let mut denied = false;
            for hive in [Hive::Machine, Hive::User] {
                if let Err(err) = set_blocked(hive, &e.value_name, false) {
                    denied |= err.kind() == io::ErrorKind::PermissionDenied;
                }
            }
            if denied {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "access denied"));
            }
        } else {
            set_blocked(Hive::Machine, &e.value_name, true)?;
        }
    } else {
        let key = e.hive.open_write(&e.key_path)?;
        if enable {
            if regbackup::has_value(&key, "LegacyDisable") {
                key.delete_value("LegacyDisable")?;
            }
        } else {
            key.set_value("LegacyDisable", &"")?;
        }
    }
    e.enabled = enable;
    data::log(&format!("context menu {} {}", if enable { "enabled" } else { "disabled" }, e.key_path));
    Ok(())
}

pub fn delete(e: &StartupEntry, backup_dir: &Path) -> io::Result<()> {
    locate(e.hive, &e.key_path).ok_or_else(|| io::Error::other("bad entry"))?;
    regbackup::export_key(e.hive, &e.key_path, backup_dir)?;
    let slash = e.key_path.rfind('\\').ok_or_else(|| io::Error::other("bad entry"))?;
    let parent = e.hive.open_write(&e.key_path[..slash])?;
    parent.delete_subkey_all(&e.key_path[slash + 1..])?;
    data::log(&format!("context menu deleted {}", e.key_path));
    Ok(())
}
