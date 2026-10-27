use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
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
    let system = super::winenv::system_dir();
    [system, super::winenv::windows_dir()].into_iter().map(|d| d.join(program)).find(|p| p.is_file()).unwrap_or_else(|| system.join(program))
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

fn token_elevated(token: HANDLE) -> std::io::Result<bool> {
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    }
    .map_err(|_| std::io::Error::last_os_error())?;
    Ok(elevation.TokenIsElevated != 0)
}

pub fn elevation() -> std::io::Result<bool> {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return Err(std::io::Error::last_os_error());
        }
        let result = token_elevated(token);
        let _ = CloseHandle(token);
        result
    }
}

pub struct ShellToken(HANDLE);

impl Drop for ShellToken {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn shell_token() -> Option<ShellToken> {
    use windows::Win32::Security::{DuplicateTokenEx, SecurityImpersonation, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TokenPrimary};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetShellWindow(), Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut token);
        let _ = CloseHandle(process);
        opened.ok()?;
        let token = ShellToken(token);
        if token_elevated(token.0).unwrap_or(true) {
            return None;
        }
        let mut primary = HANDLE::default();
        let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID;
        DuplicateTokenEx(token.0, access, None, SecurityImpersonation, TokenPrimary, &mut primary).ok()?;
        Some(ShellToken(primary))
    }
}

pub fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

pub fn spawn_with(token: &ShellToken, program: &Path, args: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Threading::{CREATE_PROCESS_LOGON_FLAGS, CreateProcessWithTokenW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW};
    use windows::core::{PCWSTR, PWSTR};
    let app: Vec<u16> = program.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut line: Vec<u16> = format!("{} {}", quoted(program), args).encode_utf16().chain(Some(0)).collect();
    let startup = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut info = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessWithTokenW(token.0, CREATE_PROCESS_LOGON_FLAGS(0), PCWSTR(app.as_ptr()), Some(PWSTR(line.as_mut_ptr())), PROCESS_CREATION_FLAGS(0), None, PCWSTR::null(), &startup, &mut info)
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(())
}

pub fn launch(program: &str, args: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let exe = system_tool(program);
    if !is_admin() {
        return Command::new(exe).raw_arg(args).spawn().map(drop);
    }
    let token = shell_token().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::PermissionDenied, "the desktop shell is unavailable"))?;
    spawn_with(&token, &exe, args)
}

pub fn user_sid() -> std::io::Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(std::io::Error::other)?;
    let sid = token_sid(token);
    let _ = unsafe { CloseHandle(token) };
    sid
}

pub fn shell_user_sid() -> Option<String> {
    let token = shell_token()?;
    token_sid(token.0).ok()
}

fn token_sid(token: HANDLE) -> std::io::Result<String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{TOKEN_USER, TokenUser};
    unsafe {
        let mut buf = [0u64; 64];
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), std::mem::size_of_val(&buf) as u32, &mut len).map_err(std::io::Error::other)?;
        let user = &*buf.as_ptr().cast::<TOKEN_USER>();
        let mut text = windows::core::PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).map_err(std::io::Error::other)?;
        let sid = text.to_string().map_err(std::io::Error::other);
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid
    }
}

pub fn is_admin() -> bool {
    static ADMIN: OnceLock<bool> = OnceLock::new();
    *ADMIN.get_or_init(|| elevation().unwrap_or(false))
}

pub fn shell_user_is_self() -> bool {
    static SAME: OnceLock<bool> = OnceLock::new();
    *SAME.get_or_init(|| !is_admin() || matches!((user_sid(), shell_user_sid()), (Ok(own), Some(shell)) if own == shell))
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
