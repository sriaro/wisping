use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::HWND;
use windows::Win32::Security::Cryptography::{CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::core::{HSTRING, PCWSTR};

fn query(block: &[u8], sub: &str) -> Option<(*const u8, u32)> {
    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    let ok = unsafe { VerQueryValueW(block.as_ptr() as *const c_void, &HSTRING::from(sub), &mut ptr, &mut len) };
    if !ok.as_bool() || ptr.is_null() || len == 0 {
        return None;
    }
    Some((ptr as *const u8, len))
}

fn read_string(block: &[u8], sub: &str) -> Option<String> {
    let (p, chars) = query(block, sub)?;
    let units = unsafe { std::slice::from_raw_parts(p as *const u16, chars as usize) };
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn company(path: &Path) -> String {
    let wide = HSTRING::from(path.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(&wide, None) };
    if size == 0 || size > 4 * 1024 * 1024 {
        return String::new();
    }
    let mut block = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(&wide, None, size, block.as_mut_ptr() as *mut c_void) }.is_err() {
        return String::new();
    }
    let mut langs: Vec<String> = Vec::new();
    if let Some((p, bytes)) = query(&block, "\\VarFileInfo\\Translation") {
        let raw = unsafe { std::slice::from_raw_parts(p, bytes as usize) };
        for pair in raw.as_chunks::<4>().0.iter().take(4) {
            let lang = u16::from_le_bytes([pair[0], pair[1]]);
            let page = u16::from_le_bytes([pair[2], pair[3]]);
            langs.push(format!("{:04x}{:04x}", lang, page));
        }
    }
    langs.push("040904b0".to_string());
    langs.push("040904e4".to_string());
    langs.iter().find_map(|l| read_string(&block, &format!("\\StringFileInfo\\{}\\CompanyName", l))).unwrap_or_default()
}

fn cert_name(cert: *const CERT_CONTEXT) -> Option<String> {
    let need = unsafe { CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None) };
    if need <= 1 || need > 4096 {
        return None;
    }
    let mut buf = vec![0u16; need as usize];
    let got = unsafe { CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf)) } as usize;
    let end = buf[..got.min(buf.len())].iter().position(|&u| u == 0).unwrap_or(got.min(buf.len()));
    let text = String::from_utf16_lossy(&buf[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn signer(path: &Path) -> Option<String> {
    let wide = HSTRING::from(path.as_os_str());
    let mut file = WINTRUST_FILE_INFO { cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: PCWSTR(wide.as_ptr()), ..Default::default() };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut file },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let status = unsafe { WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut c_void) };
    let name = if status == 0 {
        unsafe {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let sgnr = if provider.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
            if sgnr.is_null() || (*sgnr).csCertChain == 0 || (*sgnr).pasCertChain.is_null() { None } else { cert_name((*(*sgnr).pasCertChain).pCert) }
        }
    } else {
        None
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut c_void) };
    name
}

const WRITABLE_UNDER_WINDOWS: [&str; 6] = ["Temp", "Tasks", "tracing", "System32\\Tasks", "System32\\spool", "SysWOW64\\Tasks"];

pub(super) fn admin_only(path: &Path) -> bool {
    use super::winenv::{inside, program_dirs, windows_dir};
    let win = windows_dir();
    if inside(path, win) {
        return !WRITABLE_UNDER_WINDOWS.iter().any(|d| inside(path, &win.join(d)));
    }
    program_dirs().iter().any(|d| inside(path, d))
}

pub fn publisher(path: &str) -> String {
    use std::os::windows::fs::MetadataExt;
    static CACHE: OnceLock<Mutex<HashMap<String, (u64, u64, String)>>> = OnceLock::new();
    if !super::fsx::local_drive_path(path) {
        return String::new();
    }
    let p = Path::new(path);
    let Some(meta) = p.is_absolute().then(|| std::fs::metadata(p).ok()).flatten().filter(|m| m.is_file()) else {
        return String::new();
    };
    let stamp = (meta.last_write_time(), meta.len());
    let cache = CACHE.get_or_init(Default::default);
    let key = path.to_lowercase();
    if let Some((time, size, v)) = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key)
        && (*time, *size) == stamp {
            return v.clone();
    }
    let value = if admin_only(p) { company(p) } else { signer(p).unwrap_or_default() };
    cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, (stamp.0, stamp.1, value.clone()));
    value
}
