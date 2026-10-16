use winreg::RegKey;
use winreg::enums::*;

use super::fsx::{self, direct_child};
use super::regbackup::{self, Hive};
use super::registry::{self, RegIssue};
use super::sys;

const COM_PATHS: [(Hive, &str); 3] = [
    (Hive::User, "Software\\Classes\\CLSID"),
    (Hive::Machine, "SOFTWARE\\Classes\\CLSID"),
    (Hive::Machine, "SOFTWARE\\Classes\\WOW6432Node\\CLSID"),
];

const CLASSES: [(Hive, &str); 2] = [(Hive::User, "Software\\Classes"), (Hive::Machine, "SOFTWARE\\Classes")];

pub fn is_guid(name: &str) -> bool {
    let b = name.as_bytes();
    if b.len() != 38 || b[0] != b'{' || b[37] != b'}' {
        return false;
    }
    b.iter().enumerate().all(|(i, c)| match i {
        0 | 37 => true,
        9 | 14 | 19 | 24 => *c == b'-',
        _ => c.is_ascii_hexdigit(),
    })
}

pub fn server_path(raw: &str) -> Option<String> {
    let s = fsx::expand_env(raw.trim());
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return if end > 0 { Some(rest[..end].to_string()) } else { None };
    }
    registry::cut_at_ext(&s, &[".exe", ".dll", ".ocx", ".ax", ".tlb"])
}

fn skip_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    fsx::is_protected(path) || lower.contains("\\windowsapps\\") || lower.contains("\\systemapps\\") || lower.contains('%')
}

fn broken_server(clsid: &RegKey, wow: bool) -> Option<String> {
    if regbackup::has_value(clsid, "PackageId") {
        return None;
    }
    let mut first = None;
    let mut found = 0;
    for server in ["InprocServer32", "LocalServer32"] {
        let Ok(sub) = clsid.open_subkey_with_flags(server, KEY_READ | KEY_WOW64_64KEY) else { continue };
        let raw = regbackup::read_string(&sub, "")?;
        if wow && raw.contains('%') {
            return None;
        }
        let path = server_path(&raw)?;
        if skip_path(&path) || !registry::missing(&path) {
            return None;
        }
        found += 1;
        first.get_or_insert(path);
    }
    if found == 0 { None } else { first }
}

pub fn com(list: &mut Vec<RegIssue>) {
    for (hive, path) in COM_PATHS {
        let Ok(root) = hive.open(path) else { continue };
        let names: Vec<String> = root.enum_keys().flatten().filter(|n| is_guid(n)).collect();
        let wow = path.contains("WOW6432Node");
        list.extend(sys::par_map(&names, |name| {
            let full = format!("{}\\{}", path, name);
            let clsid = hive.open(&full).ok()?;
            broken_server(&clsid, wow).map(|p| RegIssue::new(hive, "com", &full, None, &p))
        }));
    }
}

fn progid_exists(name: &str) -> bool {
    CLASSES.iter().any(|(hive, path)| hive.open(&format!("{}\\{}", path, name)).is_ok())
}

pub fn extensions(list: &mut Vec<RegIssue>) {
    let path = "Software\\Classes";
    let Ok(root) = Hive::User.open(path) else { return };
    for name in root.enum_keys().flatten() {
        if !name.starts_with('.') || name.len() < 2 || name.contains(['\\', '/']) {
            continue;
        }
        let Ok(ext) = root.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY) else { continue };
        let Some(progid) = regbackup::read_string(&ext, "").filter(|p| !p.trim().is_empty()) else { continue };
        if progid_exists(progid.trim()) {
            continue;
        }
        if Hive::Machine.open(&format!("SOFTWARE\\Classes\\{}", name)).is_ok() {
            continue;
        }
        if ext.enum_keys().flatten().any(|k| !k.eq_ignore_ascii_case("OpenWithProgids") && !k.eq_ignore_ascii_case("OpenWithList")) {
            continue;
        }
        if ext.open_subkey_with_flags("OpenWithProgids", KEY_READ | KEY_WOW64_64KEY).map(|k| k.enum_values().count() > 0).unwrap_or(false) {
            continue;
        }
        list.push(RegIssue::new(Hive::User, "extension", &format!("{}\\{}", path, name), None, &format!("{} -> {}", name, progid.trim())));
    }
}

pub fn open_with(list: &mut Vec<RegIssue>) {
    for (hive, base) in CLASSES {
        let apps = format!("{}\\Applications", base);
        let Ok(root) = hive.open(&apps) else { continue };
        for name in root.enum_keys().flatten() {
            if !name.to_lowercase().ends_with(".exe") || name.contains(['\\', '/']) {
                continue;
            }
            let Ok(cmd) = root.open_subkey_with_flags(format!("{}\\shell\\open\\command", name), KEY_READ | KEY_WOW64_64KEY) else { continue };
            let Some(raw) = regbackup::read_string(&cmd, "") else { continue };
            let Some(path) = registry::first_path(&raw) else { continue };
            if skip_path(&path) || !registry::missing(&path) {
                continue;
            }
            list.push(RegIssue::new(hive, "openwith", &format!("{}\\{}", apps, name), None, &path));
        }
    }
}

fn last_segment(key: &str) -> Option<&str> {
    key.rsplit('\\').next()
}

pub fn allowed(kind: &str, hive: Hive, lower_key: &str) -> bool {
    match kind {
        "com" => COM_PATHS.iter().any(|(h, p)| *h == hive && direct_child(lower_key, &format!("{}\\", p.to_lowercase())) && is_guid(&lower_key[lower_key.rfind('\\').map_or(0, |i| i + 1)..])),
        "extension" => hive == Hive::User && direct_child(lower_key, "software\\classes\\") && last_segment(lower_key).is_some_and(|n| n.starts_with('.') && n.len() >= 2 && !n.contains('/')),
        "openwith" => CLASSES.iter().any(|(h, p)| *h == hive && direct_child(lower_key, &format!("{}\\applications\\", p.to_lowercase()))) && last_segment(lower_key).is_some_and(|n| n.ends_with(".exe") && !n.contains('/')),
        _ => false,
    }
}
