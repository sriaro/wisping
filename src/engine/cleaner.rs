use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::Scope;

use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHQUERYRBINFO, SHQueryRecycleBinW};
use windows::core::PCWSTR;

use winreg::RegKey;
use winreg::enums::*;

use super::data;
use super::fsx::{self, Fence, Mask, Removal};
use super::registry;
use super::sys;

static KEEP_RECENT: AtomicBool = AtomicBool::new(true);
static WALKERS: AtomicUsize = AtomicUsize::new(0);

pub fn set_keep_recent(value: bool) {
    KEEP_RECENT.store(value, Ordering::SeqCst);
}

pub fn keep_recent() -> bool {
    KEEP_RECENT.load(Ordering::SeqCst)
}

pub fn effective_age(s: &Source) -> u64 {
    if keep_recent() { s.min_age_hours } else { 0 }
}

#[derive(Clone)]
pub struct Source {
    pub dir: String,
    pub mask: String,
    pub recurse: bool,
    pub min_age_hours: u64,
    pub admin: bool,
}

impl Source {
    pub fn new(dir: &str, mask: &str, recurse: bool, min_age_hours: u64) -> Source {
        Source { dir: dir.to_string(), mask: mask.to_string(), recurse, min_age_hours, admin: false }
    }
}

#[derive(Clone, Default)]
pub struct CleanTarget {
    pub id: String,
    pub needs_admin: bool,
    pub selected: bool,
    pub recycle: bool,
    pub sources: Vec<Source>,
    pub reg: Vec<String>,
    pub removed: u64,
    pub bytes: u64,
    pub count: u64,
    pub freed: u64,
    pub failed: u64,
    pub fail_samples: Vec<String>,
    pub scanned: bool,
}

impl CleanTarget {
    pub fn custom(id: &str) -> CleanTarget {
        CleanTarget { id: id.to_string(), ..Default::default() }
    }
}

fn make(list: &mut Vec<CleanTarget>, id: &str, admin: bool, default_on: bool) -> usize {
    list.push(CleanTarget { id: id.to_string(), needs_admin: admin, selected: default_on, ..Default::default() });
    list.len() - 1
}

fn add_reg(list: &mut [CleanTarget], at: usize, path: &str) {
    list[at].reg.push(path.to_string());
}

const EXPLORER_KEYS: [&str; 6] = [
    "software\\microsoft\\windows\\currentversion\\explorer\\runmru",
    "software\\microsoft\\windows\\currentversion\\explorer\\typedpaths",
    "software\\microsoft\\windows\\currentversion\\explorer\\recentdocs",
    "software\\microsoft\\windows\\currentversion\\explorer\\wordwheelquery",
    "software\\microsoft\\windows\\currentversion\\explorer\\comdlg32\\opensavepidlmru",
    "software\\microsoft\\windows\\currentversion\\explorer\\comdlg32\\lastvisitedpidlmru",
];

const TOOL_KEYS: [&str; 4] = [
    "software\\microsoft\\microsoft management console\\recent file list",
    "software\\microsoft\\windows\\currentversion\\applets\\paint\\recent file list",
    "software\\microsoft\\windows\\currentversion\\applets\\wordpad\\recent file list",
    "software\\microsoft\\windows\\currentversion\\explorer\\map network drive mru",
];

const TOOL_VALUES: [(&str, &str); 1] = [("software\\microsoft\\windows\\currentversion\\applets\\regedit", "lastkey")];

const RDP_KEYS: [&str; 1] = ["software\\microsoft\\terminal server client\\default"];

const IE_KEYS: [&str; 2] = ["software\\microsoft\\internet explorer\\typedurls", "software\\microsoft\\internet explorer\\typedurlstime"];

const WINRAR_KEYS: [&str; 3] = ["software\\winrar\\archistory", "software\\winrar\\dialogedithistory\\extrpath", "software\\winrar\\dialogedithistory\\arcname"];

fn reg_split(entry: &str) -> (&str, Option<&str>) {
    match entry.split_once('|') {
        Some((k, v)) => (k, Some(v)),
        None => (entry, None),
    }
}

fn reg_path_ok(entry: &str) -> bool {
    let (path, value) = reg_split(entry);
    let lower = path.to_lowercase();
    if lower.contains("..") || lower.starts_with('\\') || lower.ends_with('\\') {
        return false;
    }
    let under = |base: &str| lower == base || fsx::key_within(&lower, base);
    let test = registry::allow_test_key() && under(&registry::TEST_KEY.to_lowercase());
    if let Some(v) = value {
        let v = v.to_lowercase();
        return TOOL_VALUES.iter().any(|(k, n)| lower == *k && v == *n) || (test && !v.is_empty());
    }
    test || [&EXPLORER_KEYS[..], &TOOL_KEYS, &RDP_KEYS, &IE_KEYS, &WINRAR_KEYS].iter().flat_map(|l| l.iter()).any(|k| under(k))
}

fn reg_open(path: &str, write: bool) -> Option<RegKey> {
    let flags = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
    RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(path, flags).ok()
}

fn reg_entry_count(entry: &str) -> u64 {
    match reg_split(entry) {
        (path, Some(value)) => reg_open(path, false).map(|k| k.get_raw_value(value).is_ok() as u64).unwrap_or(0),
        (path, None) => reg_open(path, false).map(|k| reg_count(&k, 0)).unwrap_or(0),
    }
}

fn reg_entry_clear(entry: &str) -> (u64, u64) {
    match reg_split(entry) {
        (path, Some(value)) => match reg_open(path, true) {
            Some(k) if k.get_raw_value(value).is_ok() => {
                if k.delete_value(value).is_ok() { (1, 0) } else { (0, 1) }
            }
            _ => (0, 0),
        },
        (path, None) => reg_open(path, true).map(|k| reg_clear(&k, 0)).unwrap_or((0, 0)),
    }
}

fn reg_count(key: &RegKey, depth: u32) -> u64 {
    let mut n = key.enum_values().flatten().count() as u64;
    if depth < 4 {
        for sub in key.enum_keys().flatten() {
            if let Ok(child) = key.open_subkey_with_flags(&sub, KEY_READ) {
                n += reg_count(&child, depth + 1);
            }
        }
    }
    n
}

fn reg_clear(key: &RegKey, depth: u32) -> (u64, u64) {
    let (mut done, mut failed) = (0u64, 0u64);
    let names: Vec<String> = key.enum_values().flatten().map(|(n, _)| n).collect();
    for name in names {
        if key.delete_value(&name).is_ok() {
            done += 1;
        } else {
            failed += 1;
        }
    }
    if depth < 4 {
        let subs: Vec<String> = key.enum_keys().flatten().collect();
        for sub in subs {
            if let Ok(child) = key.open_subkey_with_flags(&sub, KEY_READ | KEY_WRITE) {
                let (d, f) = reg_clear(&child, depth + 1);
                done += d;
                failed += f;
            }
        }
    }
    (done, failed)
}

fn add(list: &mut [CleanTarget], at: usize, dir: &str, mask: &str, recurse: bool, hours: u64) {
    list[at].sources.push(Source::new(dir, mask, recurse, hours));
}

fn add_admin(list: &mut [CleanTarget], at: usize, dir: &str, mask: &str, recurse: bool, hours: u64) {
    list[at].sources.push(Source { admin: true, ..Source::new(dir, mask, recurse, hours) });
}

fn add_chromium(list: &mut [CleanTarget], at: usize, user_data: &str) {
    for n in ["Cache", "Code Cache", "GPUCache", "DawnCache", "Service Worker\\CacheStorage", "Service Worker\\ScriptCache"] {
        add(list, at, &format!("{}\\*\\{}", user_data, n), "*", true, 0);
    }
    for n in ["ShaderCache", "GrShaderCache", "GraphiteDawnCache"] {
        add(list, at, &format!("{}\\{}", user_data, n), "*", true, 0);
    }
}

const CHROMIUM_ROOTS: [&str; 4] = [
    "%LOCALAPPDATA%\\Google\\Chrome\\User Data",
    "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data",
    "%LOCALAPPDATA%\\BraveSoftware\\Brave-Browser\\User Data",
    "%LOCALAPPDATA%\\Vivaldi\\User Data",
];

const OPERA_ROOTS: [&str; 2] = ["%APPDATA%\\Opera Software\\Opera Stable", "%APPDATA%\\Opera Software\\Opera GX Stable"];

pub fn steam_ok(path: &str) -> bool {
    let p = Path::new(path);
    path.len() >= 6
        && !path.starts_with("\\\\")
        && p.is_absolute()
        && fsx::norm_key(p).is_some_and(|k| k != fsx::windir())
        && !fsx::is_protected(path)
        && !has_reparse_ancestor(p)
        && p.join("steam.exe").is_file()
}

fn steam_dir() -> Option<String> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags("Software\\Valve\\Steam", KEY_READ).ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    let path = path.trim().replace('/', "\\");
    steam_ok(&path).then_some(path)
}

pub fn group_of(id: &str) -> &'static str {
    match id {
        "chrome" | "edge" | "brave" | "otherbrowsers" | "firefox" | "history" | "cookies" | "sessions" | "metrics" | "iecache" | "iedata" => "browsers",
        "steam" | "onedrive" | "nvidia" | "winrar" | "devcache" => "apps",
        "aiapps" | "aicli" | "aigpu" | "aimodelcache" | "aihistory" | "aimodels" => "ai",
        _ => "windows",
    }
}

fn group_rank(id: &str) -> u8 {
    match group_of(id) {
        "windows" => 0,
        "browsers" => 1,
        "apps" => 2,
        _ => 3,
    }
}

pub fn risk_of(id: &str) -> u8 {
    match id {
        "recycle" | "aihistory" | "aimodels" => 2,
        "recent" | "shader" | "wu" | "logs" | "history" | "cookies" | "sessions" | "iedata" | "explorer" | "wintools" | "rdp" | "winrar" | "steam" | "prefetch" | "aicli" | "aimodelcache" => 1,
        _ => 0,
    }
}

const AI_APP_ROOTS: [&str; 8] = [
    "%LOCALAPPDATA%\\Packages\\Claude_*\\LocalCache\\Roaming\\Claude",
    "%APPDATA%\\Claude",
    "%APPDATA%\\Cursor",
    "%APPDATA%\\Windsurf",
    "%APPDATA%\\Trae",
    "%APPDATA%\\Kiro",
    "%APPDATA%\\LM Studio",
    "%APPDATA%\\Jan",
];

const AI_APP_CACHES: [&str; 15] = [
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GrShaderCache",
    "ShaderCache",
    "GPUPersistentCache",
    "CachedData",
    "CachedExtensionVSIXs",
    "Service Worker\\CacheStorage",
    "Service Worker\\ScriptCache",
    "logs",
    "Crashpad\\reports",
];

pub fn targets() -> Vec<CleanTarget> {
    let mut l = Vec::new();
    let i = make(&mut l, "usertemp", false, true);
    add(&mut l, i, "%TEMP%", "*", true, 24);
    let i = make(&mut l, "wintemp", true, true);
    add(&mut l, i, "%WINDIR%\\Temp", "*", true, 24);
    let i = make(&mut l, "thumb", false, true);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer", "thumbcache_*.db", false, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer", "iconcache_*.db", false, 0);
    let i = make(&mut l, "recent", false, false);
    add(&mut l, i, "%APPDATA%\\Microsoft\\Windows\\Recent", "*.lnk", false, 0);
    add(&mut l, i, "%APPDATA%\\Microsoft\\Windows\\Recent\\AutomaticDestinations", "*", false, 0);
    add(&mut l, i, "%APPDATA%\\Microsoft\\Windows\\Recent\\CustomDestinations", "*", false, 0);
    let i = make(&mut l, "crash", false, true);
    add(&mut l, i, "%LOCALAPPDATA%\\CrashDumps", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\WER\\ReportArchive", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\WER\\ReportQueue", "*", true, 0);
    add_admin(&mut l, i, "%PROGRAMDATA%\\Microsoft\\Windows\\WER\\ReportArchive", "*", true, 0);
    add_admin(&mut l, i, "%PROGRAMDATA%\\Microsoft\\Windows\\WER\\ReportQueue", "*", true, 0);
    add_admin(&mut l, i, "%WINDIR%\\Minidump", "*", true, 0);
    add_admin(&mut l, i, "%WINDIR%", "MEMORY.DMP", false, 0);
    let i = make(&mut l, "shader", false, false);
    add(&mut l, i, "%LOCALAPPDATA%\\D3DSCache", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\NVIDIA\\DXCache", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\NVIDIA\\GLCache", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\AMD\\DxCache", "*", true, 0);
    let i = make(&mut l, "wu", true, false);
    add(&mut l, i, "%WINDIR%\\SoftwareDistribution\\Download", "*", true, 0);
    let i = make(&mut l, "do", true, true);
    add(&mut l, i, "%WINDIR%\\ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache", "*", true, 0);
    let i = make(&mut l, "logs", true, false);
    add(&mut l, i, "%WINDIR%\\Logs\\CBS", "*.log", false, 24);
    add(&mut l, i, "%WINDIR%\\Logs\\DISM", "*.log", false, 24);
    let i = make(&mut l, "chrome", false, true);
    add_chromium(&mut l, i, CHROMIUM_ROOTS[0]);
    let i = make(&mut l, "edge", false, true);
    add_chromium(&mut l, i, CHROMIUM_ROOTS[1]);
    let i = make(&mut l, "brave", false, true);
    add_chromium(&mut l, i, CHROMIUM_ROOTS[2]);
    let i = make(&mut l, "otherbrowsers", false, true);
    add_chromium(&mut l, i, CHROMIUM_ROOTS[3]);
    for root in ["Opera Stable", "Opera GX Stable"] {
        for n in ["Cache", "Code Cache", "GPUCache"] {
            add(&mut l, i, &format!("%LOCALAPPDATA%\\Opera Software\\{}\\{}", root, n), "*", true, 0);
        }
    }
    let i = make(&mut l, "firefox", false, true);
    add(&mut l, i, "%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\cache2", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\startupCache", "*", true, 0);
    let i = make(&mut l, "history", false, false);
    for root in CHROMIUM_ROOTS {
        for f in ["History", "History-journal", "Visited Links", "Top Sites", "Top Sites-journal", "Favicons", "Favicons-journal", "Network Action Predictor"] {
            add(&mut l, i, &format!("{}\\*", root), f, false, 0);
        }
    }
    for root in OPERA_ROOTS {
        for f in ["History", "History-journal", "Visited Links", "Top Sites", "Favicons"] {
            add(&mut l, i, root, f, false, 0);
        }
    }
    let i = make(&mut l, "cookies", false, false);
    for root in CHROMIUM_ROOTS {
        for f in ["Cookies", "Cookies-journal"] {
            add(&mut l, i, &format!("{}\\*\\Network", root), f, false, 0);
            add(&mut l, i, &format!("{}\\*", root), f, false, 0);
        }
    }
    for root in OPERA_ROOTS {
        for f in ["Cookies", "Cookies-journal"] {
            add(&mut l, i, &format!("{}\\Network", root), f, false, 0);
            add(&mut l, i, root, f, false, 0);
        }
    }
    add(&mut l, i, "%APPDATA%\\Mozilla\\Firefox\\Profiles\\*", "cookies.sqlite*", false, 0);
    let i = make(&mut l, "sessions", false, false);
    for root in CHROMIUM_ROOTS {
        add(&mut l, i, &format!("{}\\*\\Sessions", root), "*", true, 0);
        for f in ["Current Session", "Current Tabs", "Last Session", "Last Tabs"] {
            add(&mut l, i, &format!("{}\\*", root), f, false, 0);
        }
    }
    for root in OPERA_ROOTS {
        add(&mut l, i, &format!("{}\\Sessions", root), "*", true, 0);
    }
    add(&mut l, i, "%APPDATA%\\Mozilla\\Firefox\\Profiles\\*", "sessionstore.jsonlz4", false, 0);
    add(&mut l, i, "%APPDATA%\\Mozilla\\Firefox\\Profiles\\*\\sessionstore-backups", "*", true, 0);
    let i = make(&mut l, "metrics", false, true);
    for root in CHROMIUM_ROOTS {
        add(&mut l, i, &format!("{}\\BrowserMetrics", root), "*", true, 0);
        add(&mut l, i, root, "BrowserMetrics*.pma", false, 0);
        add(&mut l, i, &format!("{}\\Crashpad\\reports", root), "*", true, 0);
    }
    add(&mut l, i, "%APPDATA%\\Mozilla\\Firefox\\Crash Reports", "*", true, 0);
    let i = make(&mut l, "iecache", false, true);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\INetCache", "*", true, 0);
    let i = make(&mut l, "iedata", false, false);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\History", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\Windows\\INetCookies", "*", true, 0);
    for k in IE_KEYS {
        add_reg(&mut l, i, k);
    }
    let i = make(&mut l, "explorer", false, false);
    for k in EXPLORER_KEYS {
        add_reg(&mut l, i, k);
    }
    let i = make(&mut l, "wintools", false, false);
    for k in TOOL_KEYS {
        add_reg(&mut l, i, k);
    }
    for (k, v) in TOOL_VALUES {
        add_reg(&mut l, i, &format!("{}|{}", k, v));
    }
    let i = make(&mut l, "rdp", false, false);
    for k in RDP_KEYS {
        add_reg(&mut l, i, k);
    }
    let i = make(&mut l, "defender", true, false);
    add(&mut l, i, "%PROGRAMDATA%\\Microsoft\\Windows Defender\\Support", "MP*.log", false, 0);
    add(&mut l, i, "%PROGRAMDATA%\\Microsoft\\Windows Defender\\Support", "MpWpp*.bin", false, 0);
    let i = make(&mut l, "steam", false, false);
    add(&mut l, i, "%LOCALAPPDATA%\\Steam\\htmlcache", "*", true, 0);
    if let Some(steam) = steam_dir() {
        for n in ["logs", "dumps", "appcache\\httpcache"] {
            add(&mut l, i, &format!("{}\\{}", steam, n), "*", true, 0);
        }
    }
    let i = make(&mut l, "onedrive", false, false);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\OneDrive\\logs", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\Microsoft\\OneDrive\\setup\\logs", "*", true, 0);
    let i = make(&mut l, "nvidia", true, false);
    add(&mut l, i, "%PROGRAMDATA%\\NVIDIA Corporation\\Downloader", "*", true, 0);
    let i = make(&mut l, "winrar", false, false);
    for k in WINRAR_KEYS {
        add_reg(&mut l, i, k);
    }
    let i = make(&mut l, "devcache", false, false);
    add(&mut l, i, "%LOCALAPPDATA%\\npm-cache", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\pip\\Cache", "*", true, 0);
    add(&mut l, i, "%LOCALAPPDATA%\\NuGet\\v3-cache", "*", true, 0);
    let i = make(&mut l, "aiapps", false, true);
    for root in AI_APP_ROOTS {
        for n in AI_APP_CACHES {
            add(&mut l, i, &format!("{}\\{}", root, n), "*", true, 0);
        }
    }
    let i = make(&mut l, "aicli", false, false);
    for d in ["%USERPROFILE%\\.claude\\cache", "%USERPROFILE%\\.claude\\shell-snapshots", "%USERPROFILE%\\.codex\\cache", "%USERPROFILE%\\.codex\\.tmp", "%USERPROFILE%\\.gemini\\tmp"] {
        add(&mut l, i, d, "*", true, 0);
    }
    let i = make(&mut l, "aigpu", false, false);
    for d in ["%APPDATA%\\NVIDIA\\ComputeCache", "%USERPROFILE%\\.nv\\ComputeCache", "%USERPROFILE%\\.triton\\cache"] {
        add(&mut l, i, d, "*", true, 0);
    }
    let i = make(&mut l, "aimodelcache", false, false);
    for d in ["%USERPROFILE%\\.cache\\huggingface\\hub", "%USERPROFILE%\\.cache\\torch\\hub", "%USERPROFILE%\\.cache\\whisper", "%USERPROFILE%\\.cache\\codex-runtimes"] {
        add(&mut l, i, d, "*", true, 0);
    }
    let i = make(&mut l, "aihistory", false, false);
    add(&mut l, i, "%USERPROFILE%\\.claude\\file-history", "*", true, 0);
    let i = make(&mut l, "aimodels", false, false);
    for d in ["%USERPROFILE%\\.ollama\\models", "%USERPROFILE%\\.lmstudio\\models", "%USERPROFILE%\\.cache\\lm-studio\\models"] {
        add(&mut l, i, d, "*", true, 0);
    }
    let i = make(&mut l, "prefetch", true, false);
    add(&mut l, i, "%WINDIR%\\Prefetch", "*.pf", false, 0);
    let i = make(&mut l, "recycle", false, false);
    l[i].recycle = true;
    l.sort_by_key(|t| group_rank(&t.id));
    l
}

fn recycle_info() -> Option<(u64, u64)> {
    let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
    unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info) }.ok()?;
    Some((info.i64Size.max(0) as u64, info.i64NumItems.max(0) as u64))
}

struct Plan {
    root: PathBuf,
    masks: Vec<Mask>,
    any: bool,
    recurse: bool,
    age: u64,
    fence: Option<Fence>,
}

fn broad(mask: &Mask) -> bool {
    matches!(mask, Mask::Any | Mask::General(_))
}

fn plans(t: &CleanTarget, fenced: bool) -> Vec<Plan> {
    let admin = sys::is_admin();
    let own = if fenced { data::own_dirs().to_vec() } else { Vec::new() };
    let mut groups: Vec<(&str, bool, u64, Vec<Mask>)> = Vec::new();
    for s in t.sources.iter().filter(|s| admin || !s.admin) {
        let age = effective_age(s);
        let mask = Mask::new(&s.mask);
        match groups.iter_mut().find(|g| g.0 == s.dir && g.1 == s.recurse && g.2 == age) {
            Some(g) => {
                if !g.3.contains(&mask) {
                    g.3.push(mask);
                }
            }
            None => groups.push((&s.dir, s.recurse, age, vec![mask])),
        }
    }
    let mut out = Vec::new();
    for (dir, recurse, age, masks) in groups {
        for root in fsx::expand_dirs(dir) {
            if has_reparse_ancestor(&root) || data::is_own(&root) {
                continue;
            }
            let wide_ok = !masks.iter().any(broad) || is_safe(&root, "*");
            let masks: Vec<Mask> = masks.iter().filter(|m| wide_ok || !broad(m)).cloned().collect();
            if masks.is_empty() {
                continue;
            }
            let fence = match fenced {
                true => match Fence::new(&root, &own) {
                    Some(f) => Some(f),
                    None => continue,
                },
                false => None,
            };
            let any = masks.iter().any(Mask::is_any);
            out.push(Plan { root, masks, any, recurse, age, fence });
        }
    }
    out
}

#[derive(Default)]
struct Tally {
    bytes: u64,
    count: u64,
}

#[derive(Default)]
struct Sweep {
    freed: u64,
    removed: u64,
    failed: u64,
    samples: Vec<String>,
}

pub fn scan(t: &mut CleanTarget) {
    t.bytes = 0;
    t.count = 0;
    t.scanned = true;
    if t.recycle {
        if let Some((bytes, items)) = recycle_info() {
            t.bytes = bytes;
            t.count = items;
        }
        return;
    }
    t.count += t.reg.iter().filter(|p| reg_path_ok(p)).map(|p| reg_entry_count(p)).sum::<u64>();
    let plans = plans(t, false);
    let (tallies, _) = walk(&plans, false, |p, _, m, acc: &mut Tally| {
        if p.age > 0 && fsx::too_new(fsx::newest(m), p.age) {
            return;
        }
        acc.bytes += m.len();
        acc.count += 1;
    });
    for x in tallies {
        t.bytes += x.bytes;
        t.count += x.count;
    }
}

pub struct Detail {
    pub name: String,
    pub place: String,
    pub bytes: u64,
    pub count: u64,
    pub path: String,
}

#[derive(Default)]
struct Groups(std::collections::HashMap<(String, String), (u64, u64)>);

const DETAIL_ROWS: usize = 300;

fn ollama_names(root: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let base = root.join("manifests");
    let mut stack = vec![(base.clone(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(m) = e.metadata() else { continue };
            if fsx::is_reparse(&path) {
                continue;
            }
            if m.is_dir() {
                if depth < 6 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            if m.len() > 1 << 20 {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Ok(rel) = path.strip_prefix(&base) else { continue };
            let mut comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
            if comps.len() > 1 {
                comps.remove(0);
            }
            if comps.len() > 1 && comps[0] == "library" {
                comps.remove(0);
            }
            let tag = comps.pop().unwrap_or_default();
            let name = if comps.is_empty() { tag } else { format!("{}:{}", comps.join("/"), tag) };
            for part in text.split("sha256:").skip(1) {
                let hex: String = part.chars().take(64).collect();
                if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    out.entry(format!("sha256-{}", hex.to_lowercase())).or_insert_with(|| name.clone());
                }
            }
        }
    }
    out
}

pub fn details(t: &CleanTarget) -> Vec<Detail> {
    if t.recycle {
        return vec![Detail { name: "Recycle Bin".to_string(), place: String::new(), bytes: t.bytes, count: t.count, path: "shell:RecycleBinFolder".to_string() }];
    }
    let plans = plans(t, false);
    let depth = if t.id == "aimodels" { 2 } else { 1 };
    let (parts, _) = walk(&plans, false, |p, e, m, acc: &mut Groups| {
        if p.age > 0 && fsx::too_new(fsx::newest(m), p.age) {
            return;
        }
        let rel = match e.path().strip_prefix(&p.root) {
            Ok(r) => r.components().take(depth).map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("\\"),
            Err(_) => String::new(),
        };
        let slot = acc.0.entry((p.root.display().to_string(), rel)).or_default();
        slot.0 += m.len();
        slot.1 += 1;
    });
    let mut merged: std::collections::HashMap<(String, String), (u64, u64)> = std::collections::HashMap::new();
    for g in parts {
        for (k, v) in g.0 {
            let slot = merged.entry(k).or_default();
            slot.0 += v.0;
            slot.1 += v.1;
        }
    }
    let mut names: std::collections::HashMap<String, std::collections::HashMap<String, String>> = std::collections::HashMap::new();
    if t.id == "aimodels" {
        for p in &plans {
            if p.root.join("manifests").is_dir() {
                names.insert(p.root.display().to_string(), ollama_names(&p.root));
            }
        }
    }
    let mut rows: std::collections::HashMap<(String, String), (u64, u64, String)> = std::collections::HashMap::new();
    for ((place, rel), (bytes, count)) in merged {
        let joined = |r: &str| if r.is_empty() { place.clone() } else { format!("{}\\{}", place, r) };
        let (name, path) = match names.get(&place) {
            Some(map) => match rel.strip_prefix("blobs\\").and_then(|b| map.get(b)) {
                Some(model) => (model.clone(), joined(&rel)),
                None if rel.starts_with("manifests") => ("Model list".to_string(), joined("manifests")),
                None => (rel.clone(), joined(&rel)),
            },
            None => (rel.clone(), joined(&rel)),
        };
        let slot = rows.entry((place, name)).or_default();
        slot.0 += bytes;
        slot.1 += count;
        slot.2 = path;
    }
    let mut out: Vec<Detail> = rows.into_iter().map(|((place, name), (bytes, count, path))| Detail { name, place, bytes, count, path }).collect();
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    if out.len() > DETAIL_ROWS {
        let rest = out.split_off(DETAIL_ROWS);
        out.push(Detail { name: format!("{} more items", rest.len()), place: String::new(), bytes: rest.iter().map(|d| d.bytes).sum(), count: rest.iter().map(|d| d.count).sum(), path: String::new() });
    }
    for p in t.reg.iter().filter(|p| reg_path_ok(p)) {
        let count = reg_entry_count(p);
        if count > 0 {
            out.push(Detail { name: format!("HKCU\\{}", p.replace('|', " : ")), place: "Registry".to_string(), bytes: 0, count, path: String::new() });
        }
    }
    out
}

pub fn clean(t: &mut CleanTarget) {
    t.freed = 0;
    t.failed = 0;
    t.removed = 0;
    t.fail_samples.clear();
    if t.recycle {
        let (bytes, items) = recycle_info().unwrap_or((t.bytes, t.count));
        if bytes == 0 && items == 0 {
            return;
        }
        if unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), 7) }.is_ok() {
            t.freed = bytes;
            t.removed = items;
            data::log(&format!("recycle bin emptied {}", bytes));
        } else {
            t.failed += 1;
        }
        return;
    }
    for p in t.reg.iter().filter(|p| reg_path_ok(p)) {
        let (done, failed) = reg_entry_clear(p);
        t.removed += done;
        t.failed += failed;
    }
    data::ensure_root();
    let plans = plans(t, true);
    let (sweeps, dirs) = walk(&plans, true, |p, e, m, acc: &mut Sweep| {
        let Some(fence) = &p.fence else { return };
        if p.age > 0 && fsx::too_new(fsx::newest(m), p.age) {
            return;
        }
        let path = e.path();
        match fsx::remove_inside(&path, fence, p.age, false) {
            Removal::Removed(n) => {
                acc.freed += n;
                acc.removed += 1;
            }
            Removal::Failed => {
                acc.failed += 1;
                if acc.samples.len() < 8 {
                    acc.samples.push(path.display().to_string());
                }
            }
            Removal::Kept | Removal::Gone => {}
        }
    });
    for s in sweeps {
        t.freed += s.freed;
        t.removed += s.removed;
        t.failed += s.failed;
        t.fail_samples.extend(s.samples);
    }
    t.fail_samples.truncate(8);
    prune(&plans, dirs);
    data::log(&format!("cleaned {} freed {} failed {}", t.id, t.freed, t.failed));
}

fn prune(plans: &[Plan], mut dirs: Vec<(usize, PathBuf)>) {
    dirs.sort_unstable_by_key(|d| std::cmp::Reverse(d.1.as_os_str().len()));
    for (i, d) in dirs {
        let p = &plans[i];
        if let Some(fence) = &p.fence {
            let _ = fsx::remove_inside(&d, fence, p.age, true);
        }
    }
}

fn has_reparse_ancestor(dir: &Path) -> bool {
    let Some(full) = fsx::absolute(dir) else { return true };
    full.ancestors().take_while(|p| p.parent().is_some()).any(fsx::is_reparse)
}

fn guarded_roots() -> &'static [String] {
    static ROOTS: OnceLock<Vec<String>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut out = Vec::new();
        for name in ["WINDIR", "SystemRoot", "USERPROFILE", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "LOCALAPPDATA", "APPDATA", "PUBLIC"] {
            if let Ok(value) = std::env::var(name) {
                let p = Path::new(&value);
                out.extend(fsx::norm_key(p));
                out.extend(fsx::canonical_key(p));
            }
        }
        out.sort();
        out.dedup();
        out
    })
}

fn personal_dirs() -> &'static [String] {
    static DIRS: OnceLock<Vec<String>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Ok(profile) = std::env::var("USERPROFILE") {
            for n in ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos", "OneDrive", "Favorites", "Contacts", "Links", "Saved Games", "Searches", "3D Objects"] {
                paths.push(Path::new(&profile).join(n));
            }
        }
        for name in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
            if let Ok(v) = std::env::var(name) {
                paths.push(PathBuf::from(v));
            }
        }
        let shell = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders";
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(shell, KEY_READ) {
            for n in ["Desktop", "Personal", "My Pictures", "My Music", "My Video", "Favorites", "{374DE290-123F-4565-9164-39C4925E467B}"] {
                if let Ok(v) = key.get_value::<String, _>(n) {
                    paths.push(PathBuf::from(fsx::expand_env(&v)));
                }
            }
        }
        let mut out: Vec<String> = paths.iter().flat_map(|p| [fsx::norm_key(p), fsx::canonical_key(p)]).flatten().filter(|k| k.split('\\').filter(|s| !s.is_empty()).count() >= 2).collect();
        out.sort();
        out.dedup();
        out
    })
}

pub fn is_safe(dir: &Path, mask: &str) -> bool {
    if !broad(&Mask::new(mask)) {
        return true;
    }
    let (Some(key), Some(canon)) = (fsx::norm_key(dir), fsx::canonical_key(dir)) else { return false };
    [key, canon].iter().all(|k| {
        !k.starts_with("\\\\")
            && k.split('\\').filter(|s| !s.is_empty()).count() >= 3
            && !guarded_roots().contains(k)
            && !personal_dirs().iter().any(|p| p == k || fsx::key_within(k, p))
    })
}

pub fn worker_count() -> usize {
    super::cpu::io_threads()
}

struct Queue {
    jobs: Vec<(usize, PathBuf)>,
    busy: usize,
    idle: usize,
}

struct Walk<'a, A, F> {
    plans: &'a [Plan],
    visit: &'a F,
    queue: Mutex<Queue>,
    ready: Condvar,
    results: Mutex<Vec<A>>,
    dirs: Option<Mutex<Vec<(usize, PathBuf)>>>,
}

struct Slot;

impl Drop for Slot {
    fn drop(&mut self) {
        WALKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reserve_walker() -> Option<Slot> {
    let cap = super::cpu::io_threads() * 2;
    WALKERS.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < cap).then_some(n + 1)).ok().map(|_| Slot)
}

fn walk<A, F>(plans: &[Plan], keep_dirs: bool, visit: F) -> (Vec<A>, Vec<(usize, PathBuf)>)
where
    A: Default + Send,
    F: Fn(&Plan, &fs::DirEntry, &fs::Metadata, &mut A) + Sync,
{
    let jobs = plans.iter().enumerate().rev().map(|(i, p)| (i, p.root.clone())).collect();
    let w = Walk {
        plans,
        visit: &visit,
        queue: Mutex::new(Queue { jobs, busy: 0, idle: 0 }),
        ready: Condvar::new(),
        results: Mutex::new(Vec::new()),
        dirs: keep_dirs.then(|| Mutex::new(Vec::new())),
    };
    std::thread::scope(|s| work(&w, s));
    (w.results.into_inner().unwrap(), w.dirs.map(|d| d.into_inner().unwrap()).unwrap_or_default())
}

fn work<'scope, 'env, A, F>(w: &'env Walk<'env, A, F>, s: &'scope Scope<'scope, 'env>)
where
    A: Default + Send,
    F: Fn(&Plan, &fs::DirEntry, &fs::Metadata, &mut A) + Sync,
{
    let mut acc = A::default();
    let mut sub: Vec<PathBuf> = Vec::new();
    loop {
        let (plan, dir) = {
            let mut q = w.queue.lock().unwrap();
            loop {
                if let Some(job) = q.jobs.pop() {
                    q.busy += 1;
                    break job;
                }
                if q.busy == 0 {
                    drop(q);
                    w.ready.notify_all();
                    w.results.lock().unwrap().push(acc);
                    return;
                }
                q.idle += 1;
                q = w.ready.wait(q).unwrap();
                q.idle -= 1;
            }
        };
        list(&w.plans[plan], &dir, &mut sub, &mut acc, w.visit);
        let grow = {
            let mut q = w.queue.lock().unwrap();
            q.busy -= 1;
            if let Some(d) = &w.dirs {
                d.lock().unwrap().extend(sub.iter().map(|x| (plan, x.clone())));
            }
            let added = !sub.is_empty();
            q.jobs.extend(sub.drain(..).map(|d| (plan, d)));
            if added || (q.busy == 0 && q.jobs.is_empty()) {
                w.ready.notify_all();
            }
            if q.jobs.len() > q.idle + 1 { reserve_walker() } else { None }
        };
        if let Some(slot) = grow {
            s.spawn(move || {
                let _slot = slot;
                crate::engine::cpu::urgent();
                work(w, s);
            });
        }
    }
}

fn list<A, F>(p: &Plan, dir: &Path, sub: &mut Vec<PathBuf>, acc: &mut A, visit: &F)
where
    F: Fn(&Plan, &fs::DirEntry, &fs::Metadata, &mut A),
{
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if fsx::attrs_reparse(meta.file_attributes()) {
            continue;
        }
        if meta.is_dir() {
            if p.recurse {
                let path = entry.path();
                if !data::is_own(&path) {
                    sub.push(path);
                }
            }
        } else if meta.is_file() && (p.any || p.masks.iter().any(|m| m.matches(&entry.file_name()))) {
            visit(p, &entry, &meta, acc);
        }
    }
}
