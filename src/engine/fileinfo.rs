use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::core::HSTRING;

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

pub fn publisher(path: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    if path.is_empty() || path.starts_with("\\\\") {
        return String::new();
    }
    let cache = CACHE.get_or_init(Default::default);
    let key = path.to_lowercase();
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let p = Path::new(path);
    let value = if p.is_absolute() && p.is_file() { company(p) } else { String::new() };
    cache.lock().unwrap().insert(key, value.clone());
    value
}
