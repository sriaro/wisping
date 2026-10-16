use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn system_tool(program: &str) -> PathBuf {
    if program.contains(['\\', '/', ':']) {
        return PathBuf::from(program);
    }
    let root = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string()));
    [root.join("System32"), root].into_iter().map(|d| d.join(program)).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(program))
}

pub fn hidden(program: &str) -> Command {
    let mut c = Command::new(system_tool(program));
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

pub fn oem_text(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let need = unsafe { MultiByteToWideChar(CP_OEMCP, flags, bytes, None) };
    if need <= 0 {
        return String::from_utf8_lossy(bytes).trim().to_string();
    }
    let mut wide = vec![0u16; need as usize];
    let got = unsafe { MultiByteToWideChar(CP_OEMCP, flags, bytes, Some(&mut wide)) };
    String::from_utf16_lossy(&wide[..got.max(0) as usize]).trim().to_string()
}

pub fn command_failure(output: &[u8], code: Option<i32>) -> String {
    let text = oem_text(output);
    if !text.is_empty() {
        return text;
    }
    match code {
        Some(c) => format!("failed (exit code {})", c),
        None => "failed".to_string(),
    }
}

fn query_admin() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

pub fn is_admin() -> bool {
    static ADMIN: OnceLock<bool> = OnceLock::new();
    *ADMIN.get_or_init(query_admin)
}

pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> Option<R> + Sync) -> Vec<R> {
    let threads = super::cpu::io_threads().min(items.len() / 8 + 1);
    if threads <= 1 {
        return items.iter().filter_map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let chunk = (items.len() / (threads * 4)).max(1);
    let mut parts: Vec<(usize, Vec<R>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let start = next.fetch_add(chunk, Ordering::Relaxed);
                        if start >= items.len() {
                            break done;
                        }
                        let end = (start + chunk).min(items.len());
                        done.push((start, items[start..end].iter().filter_map(&f).collect::<Vec<R>>()));
                    }
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    parts.sort_unstable_by_key(|p| p.0);
    parts.into_iter().flat_map(|p| p.1).collect()
}

pub struct Stamp {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

pub fn now_local() -> Stamp {
    let t = unsafe { GetLocalTime() };
    Stamp { year: t.wYear, month: t.wMonth, day: t.wDay, hour: t.wHour, minute: t.wMinute, second: t.wSecond }
}

pub fn stamp_name() -> String {
    let s = now_local();
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", s.year, s.month, s.day, s.hour, s.minute, s.second)
}

pub fn filetime_now() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (since + 11_644_473_600) * 10_000_000
}
