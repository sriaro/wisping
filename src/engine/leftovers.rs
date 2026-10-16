use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use windows::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW};
use windows::core::PCWSTR;

use super::fsx;
use super::programs;
use super::regbackup::Hive;
use super::registry;
use super::startup;

const DATA_AGE_DAYS: u64 = 90;
const PROGRAM_AGE_DAYS: u64 = 30;
const WALK_LIMIT: usize = 200_000;

const DENY: &[&str] = &[
    "microsoft", "microsoft shared", "windows", "windowsapps", "modifiablewindowsapps", "packages", "temp", "tmp", "programs", "publishers",
    "common files", "internet explorer", "windows defender", "windows defender advanced threat protection", "windows mail", "windows media player",
    "windows nt", "windows photo viewer", "windows portable devices", "windows security", "windowspowershell", "windows sidebar", "windows kits",
    "dotnet", "msbuild", "reference assemblies", "uninstall information", "package cache", "ssh", "systemcertificates", "connecteddevicesplatform",
    "d3dscache", "history", "comms", "crashdumps", "diagnostics", "peerdistrepub", "placeholdertilelogofolder", "pip", "npm", "npm-cache", "nuget",
    "wisp", "my games", "saved games", "documents", "desktop", "downloads", "nvidia", "nvidia corporation", "intel", "amd",
    "virtualstore", "softwaredistribution", "usoprivate", "usoshared", "application data", "start menu", "templates", "favorites", "windowsholographicdevices",
    "microsoft help", "system volume information", "recovery", "boot", "perflogs", "sentinel", "oracle", "java", "docker", "dockerdesktop",
];

#[derive(Clone, Copy, PartialEq)]
enum Rule {
    Data,
    Program,
}

struct Root {
    dir: PathBuf,
    rule: Rule,
    admin: bool,
}

#[derive(Clone, Debug)]
pub struct Leftover {
    pub path: PathBuf,
    pub size: u64,
    pub newest: Option<SystemTime>,
    pub admin: bool,
    pub risky: bool,
}

static TEST_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn set_test_root(dir: Option<PathBuf>) {
    *TEST_ROOT.lock().unwrap() = dir;
}

fn test_root() -> Option<PathBuf> {
    TEST_ROOT.lock().unwrap().clone()
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var(name).ok().filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn roots() -> Vec<Root> {
    if let Some(dir) = test_root() {
        return vec![Root { dir: dir.join("data"), rule: Rule::Data, admin: false }, Root { dir: dir.join("program"), rule: Rule::Program, admin: true }];
    }
    let mut out = Vec::new();
    for name in ["LOCALAPPDATA", "APPDATA"] {
        out.extend(env_dir(name).map(|dir| Root { dir, rule: Rule::Data, admin: false }));
    }
    out.extend(env_dir("ProgramData").map(|dir| Root { dir, rule: Rule::Data, admin: true }));
    for name in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        out.extend(env_dir(name).map(|dir| Root { dir, rule: Rule::Program, admin: true }));
    }
    let mut seen = HashSet::new();
    out.retain(|r| fsx::norm_key(&r.dir).is_some_and(|k| seen.insert(k)));
    out
}

fn norm(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric()).map(|t| t.to_lowercase()).filter(|t| t.chars().count() >= 3)
}

fn denied(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with('.') || lower.starts_with('{') || lower.starts_with("regid.") || lower.starts_with("windows") || lower.starts_with("microsoft") || DENY.contains(&lower.as_str())
}

#[derive(Default)]
struct NameSet {
    names: HashSet<String>,
    tokens: HashSet<String>,
}

impl NameSet {
    fn add(&mut self, text: &str) {
        let n = norm(text);
        if n.chars().count() >= 3 {
            self.names.insert(n);
        }
        self.tokens.extend(tokens(text));
    }

    fn matches(&self, name: &str) -> bool {
        let n = norm(name);
        let len = n.chars().count();
        if len < 4 || self.names.contains(&n) || self.tokens.contains(&n) {
            return true;
        }
        if self.names.iter().any(|x| x.starts_with(n.as_str()) || (x.chars().count() >= 4 && n.starts_with(x.as_str()))) {
            return true;
        }
        if len >= 5 && self.names.iter().any(|x| x.chars().count() >= 5 && (x.contains(&n) || n.contains(x.as_str()))) {
            return true;
        }
        tokens(name).any(|t| self.tokens.contains(&t) || self.names.contains(&t))
    }
}

#[derive(Default)]
pub struct Links {
    strict: NameSet,
    vendor: NameSet,
    paths: Vec<String>,
}

impl Links {
    pub fn add_name(&mut self, text: &str) {
        self.strict.add(text);
    }

    pub fn add_vendor(&mut self, text: &str) {
        self.vendor.add(text);
    }

    pub fn add_path(&mut self, text: &str) {
        if let Some(p) = registry::first_path(text).or_else(|| Some(text.trim().trim_matches('"').to_string())) {
            let key = fsx::expand_env(&p).replace('/', "\\").to_lowercase();
            if key.len() > 3 {
                self.paths.push(key);
            }
        }
    }

    pub fn linked(&self, dir: &Path) -> bool {
        let Some(name) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else { return true };
        if self.strict.matches(&name) {
            return true;
        }
        let Some(key) = fsx::norm_key(dir) else { return true };
        let prefix = format!("{}\\", key);
        self.paths.iter().any(|p| *p == key || p.starts_with(&prefix))
    }

    pub fn vendor_linked(&self, dir: &Path) -> bool {
        dir.file_name().is_some_and(|n| self.vendor.matches(&n.to_string_lossy()))
    }
}

pub fn links() -> Links {
    let mut l = Links::default();
    for p in programs::list() {
        l.add_name(&p.name);
        l.add_name(&p.publisher);
        l.add_path(&p.location);
        l.add_path(&p.command);
    }
    for e in startup::list() {
        l.add_name(&e.name);
        l.add_name(&e.publisher);
        l.add_path(&e.command);
    }
    for (hive, path) in [(Hive::User, "Software"), (Hive::Machine, "SOFTWARE"), (Hive::Machine, "SOFTWARE\\WOW6432Node")] {
        if let Ok(key) = hive.open(path) {
            for name in key.enum_keys().flatten() {
                l.add_vendor(&name);
            }
        }
    }
    for dir in [env_dir("ProgramData"), env_dir("APPDATA")].into_iter().flatten() {
        if let Ok(rd) = fs::read_dir(dir.join("Microsoft\\Windows\\Start Menu\\Programs")) {
            for e in rd.flatten() {
                l.add_name(&e.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            }
        }
    }
    l
}

struct Measure {
    size: u64,
    newest: Option<SystemTime>,
    has_code: bool,
    has_link: bool,
}

fn is_code(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".exe", ".dll", ".sys", ".msi", ".com", ".scr", ".bat", ".cmd", ".ps1", ".lnk", ".jar", ".py"].iter().any(|e| lower.ends_with(e))
}

fn measure(dir: &Path, stop_at_code: bool) -> Option<Measure> {
    let mut m = Measure { size: 0, newest: None, has_code: false, has_link: false };
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > WALK_LIMIT {
                return None;
            }
            let Ok(meta) = fs::symlink_metadata(e.path()) else { continue };
            if fsx::attrs_reparse(std::os::windows::fs::MetadataExt::file_attributes(&meta)) {
                m.has_link = true;
                return Some(m);
            }
            let stamp = fsx::newest(&meta);
            m.newest = m.newest.max(stamp);
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                m.size += meta.len();
                if is_code(&e.file_name().to_string_lossy()) {
                    m.has_code = true;
                    if stop_at_code {
                        return Some(m);
                    }
                }
            }
        }
    }
    Some(m)
}

fn older_than(stamp: Option<SystemTime>, days: u64) -> bool {
    stamp.is_some_and(|s| SystemTime::now().duration_since(s).is_ok_and(|age| age >= Duration::from_secs(days * 86_400)))
}

fn own_dirs() -> Vec<String> {
    super::data::own_dirs().iter().filter_map(|d| fsx::norm_key(d)).collect()
}

fn candidates(root: &Root, links: &Links, own: &[String]) -> Vec<Leftover> {
    let Ok(rd) = fs::read_dir(&root.dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if denied(&name) || fsx::is_reparse(&path) || !path.is_dir() || links.linked(&path) {
            continue;
        }
        if fsx::norm_key(&path).is_some_and(|k| own.iter().any(|o| *o == k || fsx::key_within(o, &k))) {
            continue;
        }
        let Some(m) = measure(&path, root.rule == Rule::Program) else { continue };
        if m.has_link {
            continue;
        }
        let (code_ok, days) = match root.rule {
            Rule::Data => (true, DATA_AGE_DAYS),
            Rule::Program => (!m.has_code, PROGRAM_AGE_DAYS),
        };
        let stamp = m.newest.or_else(|| fs::symlink_metadata(&path).ok().and_then(|x| fsx::newest(&x)));
        if code_ok && older_than(stamp, days) {
            let risky = links.vendor_linked(&path) || (root.rule == Rule::Data && m.has_code);
            out.push(Leftover { path, size: m.size, newest: stamp, admin: root.admin, risky });
        }
    }
    out
}

pub fn scan() -> Vec<Leftover> {
    let links = links();
    scan_with(&links)
}

pub fn scan_with(links: &Links) -> Vec<Leftover> {
    let own = own_dirs();
    let mut out: Vec<Leftover> = roots().iter().flat_map(|r| candidates(r, links, &own)).collect();
    out.sort_by_key(|l| std::cmp::Reverse(l.size));
    out
}

pub fn allowed(path: &Path) -> bool {
    let (Some(key), Some(parent)) = (fsx::norm_key(path), path.parent().and_then(fsx::norm_key)) else { return false };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { return false };
    if denied(&name) || key.starts_with("\\\\") || fsx::is_reparse(path) || !path.is_dir() {
        return false;
    }
    roots().iter().any(|r| fsx::norm_key(&r.dir).is_some_and(|k| k == parent))
}

pub fn recycle(path: &Path) -> io::Result<()> {
    if !allowed(path) {
        return Err(io::Error::other("refused"));
    }
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(wide.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted.as_bool() || path.exists() {
        return Err(io::Error::other("could not move the folder to the Recycle Bin"));
    }
    Ok(())
}

pub fn month_text(stamp: Option<SystemTime>) -> String {
    let Some(s) = stamp else { return String::new() };
    let days = s.duration_since(SystemTime::UNIX_EPOCH).map(|d| (d.as_secs() / 86_400) as i64).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}", y, m)
}

pub fn display(l: &Leftover) -> String {
    format!("{} · {}", super::data::format_size(l.size), month_text(l.newest))
}
