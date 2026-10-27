use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::TOKEN_QUERY;
use windows::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::GetUserProfileDirectoryW;
use windows::core::PWSTR;
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

use super::{fsx, sys};

const MAX_UNITS: usize = 32768;
const MAX_DEPTH: u8 = 4;
const CURRENT_VERSION: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion";
const PROFILE_LIST: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList";
const SESSION_ENV: &str = "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment";

#[cfg(test)]
#[path = "winenv/tests.rs"]
mod tests;

fn from_api(f: impl Fn(Option<&mut [u16]>) -> u32) -> Option<String> {
    let mut buf = vec![0u16; 260];
    loop {
        let n = f(Some(&mut buf)) as usize;
        if n == 0 || n > MAX_UNITS {
            return None;
        }
        if n < buf.len() {
            return Some(String::from_utf16_lossy(&buf[..n]));
        }
        buf.resize(n + 1, 0);
    }
}

pub fn windows_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| PathBuf::from(from_api(|b| unsafe { GetSystemWindowsDirectoryW(b) }).unwrap_or_else(|| "C:\\Windows".to_string())))
}

pub fn system_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| from_api(|b| unsafe { GetSystemDirectoryW(b) }).map(PathBuf::from).unwrap_or_else(|| windows_dir().join("System32")))
}

fn query_profile() -> Option<PathBuf> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetUserProfileDirectoryW(token, None, &mut len);
        let mut buf = vec![0u16; (len as usize).clamp(1, MAX_UNITS)];
        len = buf.len() as u32;
        let ok = GetUserProfileDirectoryW(token, Some(PWSTR(buf.as_mut_ptr())), &mut len).is_ok();
        let _ = CloseHandle(token);
        if !ok {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let path = PathBuf::from(String::from_utf16_lossy(&buf[..end]));
        (path.is_absolute() && !path.to_string_lossy().starts_with("\\\\")).then_some(path)
    }
}

pub fn profile() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(query_profile).as_deref()
}

fn hklm(key: &str, value: &str) -> Option<String> {
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(key, KEY_READ).ok()?.get_value::<String, _>(value).ok().filter(|v| !v.trim().is_empty())
}

fn machine(name: &str, depth: u8) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    let raw = match name {
        "windir" | "systemroot" => return Some(windows_dir().to_string_lossy().into_owned()),
        "systemdrive" => return windows_dir().to_string_lossy().get(..2).map(str::to_string),
        "programfiles" => hklm(CURRENT_VERSION, "ProgramFilesDir"),
        "programfiles(x86)" => hklm(CURRENT_VERSION, "ProgramFilesDir (x86)"),
        "programw6432" => hklm(CURRENT_VERSION, "ProgramW6432Dir"),
        "commonprogramfiles" => hklm(CURRENT_VERSION, "CommonFilesDir"),
        "commonprogramfiles(x86)" => hklm(CURRENT_VERSION, "CommonFilesDir (x86)"),
        "commonprogramw6432" => hklm(CURRENT_VERSION, "CommonW6432Dir"),
        "programdata" | "allusersprofile" => hklm(PROFILE_LIST, "ProgramData"),
        "public" => hklm(PROFILE_LIST, "Public"),
        _ => None,
    }?;
    Some(fsx::expand_with(&raw, |n| machine(&n.to_ascii_lowercase(), depth + 1)))
}

pub fn program_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = ["programfiles", "programfiles(x86)", "programw6432"].iter().filter_map(|n| machine(n, 0)).map(PathBuf::from).filter(|p| p.is_absolute()).collect();
        dirs.dedup();
        dirs
    })
}

pub fn inside(path: &Path, base: &Path) -> bool {
    let text = path.to_string_lossy();
    if !path.is_absolute() || text.starts_with("\\\\") || text.contains('%') {
        return false;
    }
    match (fsx::norm_key(path), fsx::norm_key(base)) {
        (Some(k), Some(b)) => k == b || fsx::key_within(&k, &b),
        _ => false,
    }
}

fn user(name: &str, value: Option<String>) -> Option<String> {
    let sub = match name {
        "userprofile" => "",
        "localappdata" => "AppData\\Local",
        "appdata" => "AppData\\Roaming",
        "temp" | "tmp" => "AppData\\Local\\Temp",
        _ => return None,
    };
    let home = profile()?;
    if sub.is_empty() {
        return Some(home.to_string_lossy().into_owned());
    }
    let chosen = value.map(PathBuf::from).filter(|p| inside(p, home)).unwrap_or_else(|| home.join(sub));
    Some(chosen.to_string_lossy().into_owned())
}

fn session(name: &str, depth: u8) -> Option<String> {
    let raw = hklm(SESSION_ENV, name)?;
    Some(fsx::expand_with(&raw, |n| trusted(n, depth + 1)))
}

fn trusted(name: &str, depth: u8) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    machine(&lower, depth).or_else(|| user(&lower, std::env::var(name).ok())).or_else(|| session(name, depth))
}

pub fn var(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "windir" | "systemroot") {
        return Some(windows_dir().to_string_lossy().into_owned());
    }
    if !sys::is_admin() {
        return std::env::var(name).ok().filter(|v| !v.is_empty());
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&lower) {
        return hit.clone();
    }
    let value = trusted(name, 0);
    cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(lower, value.clone());
    value
}
