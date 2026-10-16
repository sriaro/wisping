use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use super::fsx;
use super::settings;
use super::sys;

const POINTER: &str = "wisp-location.txt";
const POINTER_FALLBACK: &str = "location.txt";

static ROOT: RwLock<Option<&'static Path>> = RwLock::new(None);
static OWN: RwLock<Vec<String>> = RwLock::new(Vec::new());

pub fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".pc-write-test");
    fs::create_dir_all(dir).is_ok() && fs::write(&probe, b"").is_ok() && fs::remove_file(&probe).is_ok()
}

fn fallback_base() -> PathBuf {
    std::env::var("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join("Wisp")
}

fn pointer_files() -> [PathBuf; 2] {
    [exe_dir().join(POINTER), fallback_base().join(POINTER_FALLBACK)]
}

pub fn cleaned_root(dir: &Path) -> bool {
    let Some(key) = fsx::norm_key(dir) else { return true };
    let mut roots = vec![std::env::temp_dir()];
    for var in ["TEMP", "TMP"] {
        if let Ok(v) = std::env::var(var) {
            roots.push(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(v).join("Temp"));
    }
    if let Ok(v) = std::env::var("SystemRoot") {
        roots.push(PathBuf::from(v).join("Temp"));
    }
    roots.iter().filter_map(|r| fsx::norm_key(r)).any(|r| key == r || fsx::key_within(&key, &r))
}

pub fn default_root() -> PathBuf {
    let exe = exe_dir();
    if !cleaned_root(&exe) && writable(&exe) { exe.join("data") } else { fallback_base() }
}

fn custom_root() -> Option<PathBuf> {
    pointer_files().iter().find_map(|f| {
        let text = fs::read_to_string(f).ok()?;
        let path = PathBuf::from(text.trim());
        (path.is_absolute() && path.is_dir() && writable(&path)).then_some(path)
    })
}

pub fn is_custom() -> bool {
    custom_root().is_some()
}

fn resolve() -> PathBuf {
    custom_root().unwrap_or_else(default_root)
}

fn set_root(path: PathBuf) {
    let leaked: &'static Path = Box::leak(path.into_boxed_path());
    *ROOT.write().unwrap() = Some(leaked);
    refresh_own();
}

pub fn root() -> &'static Path {
    if let Some(p) = *ROOT.read().unwrap() {
        return p;
    }
    let mut guard = ROOT.write().unwrap();
    if let Some(p) = *guard {
        return p;
    }
    let leaked: &'static Path = Box::leak(resolve().into_boxed_path());
    *guard = Some(leaked);
    drop(guard);
    refresh_own();
    leaked
}

pub fn ensure_root() -> &'static Path {
    let _ = fs::create_dir_all(root());
    root()
}

pub fn backup_root() -> PathBuf {
    root().join("backup")
}

pub fn own_dirs() -> [PathBuf; 2] {
    [root().to_path_buf(), exe_dir()]
}

fn refresh_own() {
    let keys: Option<Vec<String>> = own_dirs().iter().map(|d| fsx::norm_key(d).map(|k| k + "\\")).collect();
    *OWN.write().unwrap() = keys.unwrap_or_default();
}

pub fn is_own(path: &Path) -> bool {
    root();
    let own = OWN.read().unwrap();
    let Some(mut key) = fsx::norm_key(path) else { return true };
    if own.is_empty() {
        return true;
    }
    key.push('\\');
    own.iter().any(|o| key.starts_with(o.as_str()))
}

fn copy_missing(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_missing(&entry.path(), &target)?;
        } else if !target.exists() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fsx::norm_key(a), fsx::norm_key(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn inside(child: &Path, parent: &Path) -> bool {
    match (fsx::norm_key(child), fsx::norm_key(parent)) {
        (Some(c), Some(p)) => c.starts_with(&format!("{}\\", p)),
        _ => false,
    }
}

fn write_pointer(target: &Path) -> std::io::Result<()> {
    let [primary, fallback] = pointer_files();
    let text = target.to_string_lossy().to_string();
    if writable(&exe_dir()) && fs::write(&primary, &text).is_ok() {
        let _ = fs::remove_file(&fallback);
        return Ok(());
    }
    fs::create_dir_all(fallback_base())?;
    fs::write(&fallback, text)
}

fn clear_pointer() {
    for f in pointer_files() {
        let _ = fs::remove_file(f);
    }
}

pub fn set_location(new: &Path) -> std::io::Result<()> {
    if !new.is_absolute() {
        return Err(std::io::Error::other("Choose a full folder path."));
    }
    let current = root().to_path_buf();
    if same_dir(new, &current) {
        return Ok(());
    }
    if inside(new, &current) || inside(&current, new) {
        return Err(std::io::Error::other("Choose a folder that is not inside the current data folder, and not one that contains it."));
    }
    if !writable(new) {
        return Err(std::io::Error::other("Wisp cannot write to that folder."));
    }
    if current.is_dir() {
        copy_missing(&current, new)?;
    }
    if same_dir(new, &default_root()) {
        clear_pointer();
    } else {
        write_pointer(new)?;
    }
    set_root(new.to_path_buf());
    Ok(())
}

pub fn reset_location() -> std::io::Result<()> {
    set_location(&default_root())
}

pub fn new_backup_dir() -> std::io::Result<PathBuf> {
    let stem = sys::stamp_name();
    ensure_root();
    fs::create_dir_all(backup_root())?;
    for n in 0..100 {
        let name = if n == 0 { stem.clone() } else { format!("{}-{}", stem, n) };
        let dir = backup_root().join(name);
        if fs::create_dir(&dir).is_ok() {
            return Ok(dir);
        }
    }
    Err(std::io::Error::other("could not create a backup folder"))
}

pub fn log_path() -> PathBuf {
    root().join("cleaner.log")
}

pub fn log(line: &str) {
    if !settings::log_on() {
        return;
    }
    let s = sys::now_local();
    let stamp = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", s.year, s.month, s.day, s.hour, s.minute, s.second);
    ensure_root();
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = writeln!(f, "{} {}", stamp, line);
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{} B", bytes) } else { format!("{:.1} {}", v, UNITS[u]) }
}

pub fn read_setting(name: &str) -> Option<String> {
    fs::read_to_string(root().join(name)).ok().map(|t| t.trim().to_string())
}

pub fn write_setting(name: &str, value: &str) {
    if !settings::remember_on() {
        return;
    }
    ensure_root();
    let _ = fs::write(root().join(name), value);
}
