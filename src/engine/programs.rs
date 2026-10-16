use std::collections::HashSet;
use std::io;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, w};

use winreg::enums::*;

use super::regbackup::{self, Hive};
use super::registry;
use super::sys;

const CV: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const CV_WOW: &str = "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub size: u64,
    pub date: String,
    pub location: String,
    pub command: String,
    pub user: bool,
}

fn date_of(raw: &str) -> String {
    let r = raw.trim();
    if r.len() == 8 && r.bytes().all(|b| b.is_ascii_digit()) {
        return format!("{}-{}-{}", &r[..4], &r[4..6], &r[6..8]);
    }
    String::new()
}

fn is_update(name: &str, release: &str) -> bool {
    let r = release.to_lowercase();
    if matches!(r.as_str(), "update" | "security update" | "hotfix" | "service pack" | "critical update") {
        return true;
    }
    let n = name.to_lowercase();
    n.starts_with("update for ") || n.starts_with("security update for ") || n.starts_with("hotfix for ")
}

pub fn list() -> Vec<Program> {
    let sources = [(Hive::Machine, CV), (Hive::Machine, CV_WOW), (Hive::User, CV)];
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (hive, path) in sources {
        let Ok(key) = hive.open(path) else { continue };
        for name in key.enum_keys().flatten() {
            let Ok(s) = key.open_subkey_with_flags(&name, KEY_READ | KEY_WOW64_64KEY) else { continue };
            if regbackup::read_dword(&s, "SystemComponent") == Some(1) || regbackup::has_value(&s, "ParentKeyName") {
                continue;
            }
            let Some(display) = regbackup::read_string(&s, "DisplayName").map(|d| d.trim().to_string()).filter(|d| !d.is_empty()) else { continue };
            if is_update(&display, &regbackup::read_string(&s, "ReleaseType").unwrap_or_default()) {
                continue;
            }
            let Some(command) = regbackup::read_string(&s, "UninstallString").map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) else { continue };
            let version = regbackup::read_string(&s, "DisplayVersion").unwrap_or_default().trim().to_string();
            let publisher = regbackup::read_string(&s, "Publisher").unwrap_or_default().trim().to_string();
            if !seen.insert(format!("{}|{}|{}", display.to_lowercase(), version, publisher.to_lowercase())) {
                continue;
            }
            out.push(Program {
                name: display,
                publisher,
                version,
                size: regbackup::read_dword(&s, "EstimatedSize").unwrap_or(0) as u64 * 1024,
                date: date_of(&regbackup::read_string(&s, "InstallDate").unwrap_or_default()),
                location: regbackup::read_string(&s, "InstallLocation").unwrap_or_default().trim().trim_matches('"').to_string(),
                command,
                user: hive == Hive::User,
            });
        }
    }
    out
}

pub fn split_for(command: &str, per_user: bool) -> Option<(String, String)> {
    let c = command.trim();
    if c.is_empty() || c.contains(['\r', '\n', '\0']) {
        return None;
    }
    let (exe, args) = if let Some(rest) = c.strip_prefix('"') {
        let end = rest.find('"')?;
        (rest[..end].to_string(), rest[end + 1..].trim().to_string())
    } else {
        let cut = registry::cut_at_ext(c, &[".exe"])?;
        let args = c[cut.len()..].trim().to_string();
        (cut, args)
    };
    let leaf = exe.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    if per_user && SHELLS.contains(&leaf.as_str()) {
        return None;
    }
    let exe = if leaf == "msiexec" || leaf == "msiexec.exe" {
        if per_user && !msi_args_ok(&args) {
            return None;
        }
        sys::system_tool("msiexec.exe").to_string_lossy().into_owned()
    } else {
        exe
    };
    let b = exe.as_bytes();
    let absolute = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    (absolute && exe.to_lowercase().ends_with(".exe") && !exe.contains(['<', '>', '|', '"', '*', '?'])).then_some((exe, args))
}

const SHELLS: [&str; 18] = [
    "cmd.exe", "cmd", "powershell.exe", "powershell", "pwsh.exe", "pwsh", "wscript.exe", "cscript.exe", "mshta.exe", "bash.exe", "wsl.exe", "conhost.exe", "rundll32.exe", "regsvr32.exe", "forfiles.exe", "schtasks.exe", "certutil.exe", "bitsadmin.exe",
];

fn msi_args_ok(args: &str) -> bool {
    let lower = args.to_lowercase();
    let mut words = lower.split_whitespace();
    let Some(first) = words.next() else { return false };
    let guid = match first.get(..2) {
        Some("/x") | Some("/i") if first.len() > 2 => first[2..].to_string(),
        Some("/x") | Some("/i") => words.next().unwrap_or("").to_string(),
        _ => return false,
    };
    let guid_ok = guid.len() == 38 && guid.starts_with('{') && guid.ends_with('}') && guid[1..37].chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    guid_ok && words.all(|w| ["/qb", "/qb-", "/qb!", "/qn", "/qr", "/quiet", "/passive", "/norestart", "/promptrestart", "/forcerestart"].contains(&w))
}

pub const ELEVATED_USER_ENTRY: &str = "per-user uninstaller refused while elevated";

pub fn refused_elevated(p: &Program, elevated: bool) -> bool {
    p.user && elevated
}

pub fn launch_uninstaller(p: &Program) -> io::Result<()> {
    if refused_elevated(p, sys::is_admin()) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, ELEVATED_USER_ENTRY));
    }
    let (exe, args) = split_for(&p.command, p.user).ok_or_else(|| io::Error::other("The uninstall command is not a plain program path, so it was not started."))?;
    if !std::path::Path::new(&exe).is_file() {
        return Err(io::Error::other(format!("The uninstaller was not found: {}", exe)));
    }
    let dir = std::path::Path::new(&exe).parent().map(|d| HSTRING::from(d.as_os_str()));
    let params = HSTRING::from(args.as_str());
    let result = unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(exe.as_str()), if args.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) }, dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())), SW_SHOWNORMAL)
    };
    if result.0 as isize > 32 { Ok(()) } else { Err(io::Error::other("Windows could not start the uninstaller.")) }
}

pub fn install_dirs() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = list()
        .into_iter()
        .filter(|p| p.location.len() > 5 && p.location.as_bytes()[1] == b':')
        .map(|p| (p.location.trim_end_matches('\\').to_lowercase(), p.name))
        .collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    v.dedup_by(|a, b| a.0 == b.0);
    v
}
