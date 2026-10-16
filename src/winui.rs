use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Memory::{GetProcessHeap, HEAP_FLAGS, HeapCompact};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_CLOAK, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::{TASKDIALOG_COMMON_BUTTON_FLAGS, TD_ERROR_ICON, TaskDialog};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetCursorPos, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SW_SHOWNORMAL, GWL_STYLE, GetWindowLongW, SetWindowLongW, SetWindowPos, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_SYSMENU, SIZE_MINIMIZED, STYLESTRUCT, WM_NCDESTROY, WM_SIZE, WM_STYLECHANGING};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

unsafe extern "system" fn find_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        let mut buf = [0u16; 64];
        let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
        if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == "Wisp" {
            unsafe { *(lparam.0 as *mut isize) = hwnd.0 as isize };
            return BOOL(0);
        }
    }
    BOOL(1)
}

static CENTERED: AtomicBool = AtomicBool::new(false);

pub fn is_centered() -> bool {
    CENTERED.load(Ordering::SeqCst)
}

pub fn center_window() -> bool {
    if CENTERED.load(Ordering::SeqCst) {
        return true;
    }
    let Some(hwnd) = our_window() else { return false };
    let done = unsafe {
        let mut rect = RECT::default();
        let mut cursor = POINT::default();
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetWindowRect(hwnd, &mut rect).is_err() || GetCursorPos(&mut cursor).is_err() {
            return false;
        }
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let area = info.rcWork;
        let x = area.left + ((area.right - area.left) - (rect.right - rect.left)) / 2;
        let y = area.top + ((area.bottom - area.top) - (rect.bottom - rect.top)) / 2;
        SetWindowPos(hwnd, None, x.max(area.left), y.max(area.top), 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE).is_ok()
    };
    CENTERED.store(done, Ordering::SeqCst);
    done
}

static HWND_CACHE: AtomicIsize = AtomicIsize::new(0);
static STRIP_MENU: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_STYLECHANGING if wparam.0 as i32 == GWL_STYLE.0 && STRIP_MENU.load(Ordering::Relaxed) => {
            let style = lparam.0 as *mut STYLESTRUCT;
            if !style.is_null() {
                unsafe { (*style).styleNew &= !WS_SYSMENU.0 };
            }
        }
        WM_SIZE if wparam.0 as u32 == SIZE_MINIMIZED => {
            let _ = slint::invoke_from_event_loop(trim_memory);
        }
        WM_SIZE if STRIP_MENU.load(Ordering::Relaxed) => {
            let _ = slint::invoke_from_event_loop(|| {
                strip_caption_buttons();
            });
        }
        WM_NCDESTROY => {
            HWND_CACHE.store(0, Ordering::SeqCst);
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

const SUBCLASS_ID: usize = 0x5749_5350;

pub fn bind(handle: isize) {
    HWND_CACHE.store(handle, Ordering::SeqCst);
    unsafe {
        let _ = SetWindowSubclass(HWND(handle as *mut c_void), Some(subclass_proc), SUBCLASS_ID, 0);
    }
}

thread_local! {
    static TRIM: slint::Timer = slint::Timer::default();
}

pub fn trim_soon() {
    TRIM.with(|t| t.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(3000), trim_memory));
}

pub fn trim_memory() {
    unsafe {
        let _ = HeapCompact(GetProcessHeap().unwrap_or_default(), HEAP_FLAGS(0));
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

fn our_window() -> Option<HWND> {
    let cached = HWND_CACHE.load(Ordering::SeqCst);
    if cached != 0 {
        return Some(HWND(cached as *mut c_void));
    }
    let mut found: isize = 0;
    unsafe {
        let _ = EnumWindows(Some(find_window), LPARAM(&mut found as *mut isize as isize));
    }
    if found == 0 { None } else { Some(HWND(found as *mut c_void)) }
}

pub fn round_corners() {
    let Some(hwnd) = our_window() else { return };
    let preference: i32 = 2;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &preference as *const _ as *const c_void, std::mem::size_of::<i32>() as u32);
        let no_border: u32 = 0xFFFF_FFFE;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &no_border as *const _ as *const c_void, std::mem::size_of::<u32>() as u32);
    }
}

pub fn set_dark(dark: bool) {
    let Some(hwnd) = our_window() else { return };
    let value: i32 = dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &value as *const _ as *const c_void, std::mem::size_of::<i32>() as u32);
    }
}

pub fn cloak(on: bool) {
    let Some(hwnd) = our_window() else { return };
    let value: i32 = on as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &value as *const _ as *const c_void, std::mem::size_of::<i32>() as u32);
    }
}

pub fn apply_backdrop(dark: bool) -> bool {
    let Some(hwnd) = our_window() else { return false };
    let kind: i32 = 3;
    let set = unsafe { DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &kind as *const _ as *const c_void, std::mem::size_of::<i32>() as u32) };
    if set.is_err() {
        return false;
    }
    set_dark(dark);
    let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) }.is_ok()
}

pub fn window_style() -> Option<u32> {
    let hwnd = our_window()?;
    Some(unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32)
}

pub fn strip_caption_buttons() -> bool {
    let Some(hwnd) = our_window() else { return false };
    STRIP_MENU.store(true, Ordering::SeqCst);
    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
    if style as u32 & WS_SYSMENU.0 == 0 {
        return false;
    }
    unsafe {
        SetWindowLongW(hwnd, GWL_STYLE, style & !(WS_SYSMENU.0 as i32));
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }
    true
}

static INSTANCE: AtomicIsize = AtomicIsize::new(0);

pub fn claim_instance(wait_ms: u64) -> bool {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    loop {
        let created = unsafe { CreateMutexW(None, true, w!("Local\\Wisp.SingleInstance")) };
        let taken = match created {
            Ok(handle) => {
                if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                    let _ = unsafe { CloseHandle(handle) };
                    true
                } else {
                    INSTANCE.store(handle.0 as isize, Ordering::SeqCst);
                    false
                }
            }
            Err(e) => e.code() == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0),
        };
        if !taken {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

unsafe extern "system" fn find_other(hwnd: HWND, lparam: LPARAM) -> BOOL {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        let mut title = [0u16; 16];
        let n = unsafe { GetWindowTextW(hwnd, &mut title) };
        let mut class = [0u16; 32];
        let c = unsafe { GetClassNameW(hwnd, &mut class) };
        if n > 0 && c > 0 && String::from_utf16_lossy(&title[..n as usize]) == "Wisp" && String::from_utf16_lossy(&class[..c as usize]) == "Window Class" {
            unsafe { *(lparam.0 as *mut isize) = hwnd.0 as isize };
            return BOOL(0);
        }
    }
    BOOL(1)
}

pub fn activate_existing() {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};
    let mut found: isize = 0;
    unsafe {
        let _ = EnumWindows(Some(find_other), LPARAM(&mut found as *mut isize as isize));
    }
    if found != 0 {
        let hwnd = HWND(found as *mut c_void);
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub fn restart_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let result = unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), w!("--relaunch"), PCWSTR::null(), SW_SHOWNORMAL) };
    result.0 as isize > 32
}

pub fn fatal(title: &str, text: &str) {
    unsafe {
        let _ = TaskDialog(None, None, &HSTRING::from(title), &HSTRING::from("Wisp"), &HSTRING::from(text), TASKDIALOG_COMMON_BUTTON_FLAGS(1), TD_ERROR_ICON, None);
    }
}

pub fn pick_folder() -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        dialog.Show(our_window()).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const c_void));
        path.map(std::path::PathBuf::from)
    }
}

pub fn work_size() -> Option<(i32, i32)> {
    unsafe {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor).ok()?;
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        Some((info.rcWork.right - info.rcWork.left, info.rcWork.bottom - info.rcWork.top))
    }
}
