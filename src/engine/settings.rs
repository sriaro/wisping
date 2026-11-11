use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::data;

const FILE: &str = "settings.txt";
const STATE: &str = "state.txt";
const THEME: &str = "theme.txt";
const CLEAN: &str = "clean.txt";

static LOG: AtomicBool = AtomicBool::new(false);
static REMEMBER: AtomicBool = AtomicBool::new(false);
static RESTORE_POINT: AtomicBool = AtomicBool::new(false);
static LOW_DISK: AtomicBool = AtomicBool::new(false);
static HISTORY: AtomicBool = AtomicBool::new(false);
static LANG: AtomicU8 = AtomicU8::new(0);

pub struct WindowState {
    pub page: i32,
    pub width: i32,
    pub height: i32,
}

pub fn flag(text: &str, key: &str) -> Option<bool> {
    match text_value(text, key)?.as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
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
    let text = data::read_text(&data::root().join(FILE), 64 * 1024).unwrap_or_default();
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
    let text = format!(
        "log={}\r\nremember={}\r\nrestorepoint={}\r\nlowdisk={}\r\nhistory={}\r\nlang={}\r\n",
        log_on() as u8,
        remember_on() as u8,
        restore_point_on() as u8,
        low_disk_on() as u8,
        history_on() as u8,
        lang_name(lang_choice())
    );
    data::write_later(FILE, &text);
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
        data::remove_later(STATE);
        data::remove_later(THEME);
    }
}

pub fn save_theme(dark: bool) {
    if remember_on() {
        data::write_later(THEME, if dark { "dark" } else { "light" });
    }
}

pub fn saved_theme() -> Option<bool> {
    if !remember_on() {
        return None;
    }
    data::read_setting(THEME).map(|value| value == "dark")
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
    parse_state(&data::read_text(&data::root().join(STATE), 64 * 1024)?)
}

pub fn save_state(state: WindowState) {
    if !remember_on() {
        return;
    }
    let text = format!("page={}\r\nwidth={}\r\nheight={}\r\n", state.page, state.width, state.height);
    data::write_later(STATE, &text);
}

pub fn save_clean_list(ids: &[&str]) {
    data::write_setting(CLEAN, &ids.join("\r\n"));
}

pub fn clean_list() -> Option<Vec<String>> {
    data::read_setting(CLEAN).map(|text| text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}
