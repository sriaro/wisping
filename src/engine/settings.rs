use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::data;

const FILE: &str = "settings.txt";
const STATE: &str = "state.txt";

static LOG: AtomicBool = AtomicBool::new(false);
static REMEMBER: AtomicBool = AtomicBool::new(false);
static RESTORE_POINT: AtomicBool = AtomicBool::new(false);
static LOW_DISK: AtomicBool = AtomicBool::new(false);
static HISTORY: AtomicBool = AtomicBool::new(false);
static LANG: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WindowState {
    pub page: i32,
    pub width: i32,
    pub height: i32,
}

pub fn flag(text: &str, key: &str) -> Option<bool> {
    text_value(text, key).map(|v| v != "0")
}

fn text_value(text: &str, key: &str) -> Option<String> {
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())
}

pub fn lang_code(name: &str) -> u8 {
    match name {
        "en" => 1,
        "zh" => 2,
        _ => 0,
    }
}

pub fn lang_name(code: u8) -> &'static str {
    match code {
        1 => "en",
        2 => "zh",
        _ => "auto",
    }
}

pub fn load() {
    let text = fs::read_to_string(data::root().join(FILE)).unwrap_or_default();
    LOG.store(flag(&text, "log").unwrap_or(false), Ordering::SeqCst);
    REMEMBER.store(flag(&text, "remember").unwrap_or(false), Ordering::SeqCst);
    RESTORE_POINT.store(flag(&text, "restorepoint").unwrap_or(false), Ordering::SeqCst);
    LOW_DISK.store(flag(&text, "lowdisk").unwrap_or(false), Ordering::SeqCst);
    HISTORY.store(flag(&text, "history").unwrap_or(false), Ordering::SeqCst);
    LANG.store(text_value(&text, "lang").map_or(0, |v| lang_code(&v)), Ordering::SeqCst);
}

pub fn log_on() -> bool {
    LOG.load(Ordering::SeqCst)
}

pub fn remember_on() -> bool {
    REMEMBER.load(Ordering::SeqCst)
}

pub fn restore_point_on() -> bool {
    RESTORE_POINT.load(Ordering::SeqCst)
}

pub fn low_disk_on() -> bool {
    LOW_DISK.load(Ordering::SeqCst)
}

pub fn history_on() -> bool {
    HISTORY.load(Ordering::SeqCst)
}

pub fn lang_choice() -> u8 {
    LANG.load(Ordering::SeqCst)
}

fn save_flags() {
    data::ensure_root();
    let text = format!(
        "log={}\r\nremember={}\r\nrestorepoint={}\r\nlowdisk={}\r\nhistory={}\r\nlang={}\r\n",
        log_on() as u8,
        remember_on() as u8,
        restore_point_on() as u8,
        low_disk_on() as u8,
        history_on() as u8,
        lang_name(lang_choice())
    );
    let _ = fs::write(data::root().join(FILE), text);
}

pub fn set_log(on: bool) {
    LOG.store(on, Ordering::SeqCst);
    save_flags();
}

pub fn set_restore_point(on: bool) {
    RESTORE_POINT.store(on, Ordering::SeqCst);
    save_flags();
}

pub fn set_low_disk(on: bool) {
    LOW_DISK.store(on, Ordering::SeqCst);
    save_flags();
}

pub fn set_history(on: bool) {
    HISTORY.store(on, Ordering::SeqCst);
    save_flags();
}

pub fn set_lang(code: u8) {
    LANG.store(code, Ordering::SeqCst);
    save_flags();
}

pub fn set_remember(on: bool) {
    REMEMBER.store(on, Ordering::SeqCst);
    save_flags();
    if !on {
        let _ = fs::remove_file(data::root().join(STATE));
    }
}

pub fn parse_state(text: &str) -> Option<WindowState> {
    let get = |key: &str| text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).and_then(|(_, v)| v.trim().parse::<i32>().ok());
    let state = WindowState { page: get("page")?, width: get("width")?, height: get("height")? };
    (0..=7).contains(&state.page).then_some(state).filter(|s| (400..=16000).contains(&s.width) && (300..=16000).contains(&s.height))
}

pub fn load_state() -> Option<WindowState> {
    if !remember_on() {
        return None;
    }
    parse_state(&fs::read_to_string(data::root().join(STATE)).ok()?)
}

pub fn save_state(state: WindowState) {
    if !remember_on() {
        return;
    }
    data::ensure_root();
    let _ = fs::write(data::root().join(STATE), format!("page={}\r\nwidth={}\r\nheight={}\r\n", state.page, state.width, state.height));
}
