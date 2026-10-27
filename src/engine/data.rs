use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use super::fsx;
use super::settings;
use super::sys;

const POINTER: &str = "wisping-location.txt";
const POINTER_FALLBACK: &str = "location.txt";
const MAX_POINTER: u64 = 4096;
const MAX_SETTING: u64 = 64 * 1024;

static ROOT: RwLock<Option<&'static Path>> = RwLock::new(None);
static OWN: RwLock<Vec<String>> = RwLock::new(Vec::new());
static CUSTOM: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
#[path = "data/security_tests.rs"]
mod security_tests;

#[cfg(test)]
pub(super) fn set_test_root(path: PathBuf) {
    set_root(path);
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

fn writable(dir: &Path) -> bool {
    fsx::PinnedDir::create(dir).is_ok_and(|dir| writable_in(&dir))
}

fn writable_in(dir: &fsx::PinnedDir) -> bool {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(".wisping-write-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    match dir.create_file(name.as_ref()) {
        Ok(file) => {
            drop(file);
            dir.remove_file(name.as_ref()).is_ok()
        }
        Err(_) => false,
    }
}

fn fallback_base() -> PathBuf {
    super::winenv::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| fsx::local_drive_path(&p.to_string_lossy()))
        .or_else(|| super::winenv::profile().map(|p| p.join("AppData\\Local")).filter(|p| fsx::local_drive_path(&p.to_string_lossy())))
        .unwrap_or_else(std::env::temp_dir)
        .join("Wisping")
}

fn pointer_files() -> [PathBuf; 2] {
    [exe_dir().join(POINTER), fallback_base().join(POINTER_FALLBACK)]
}

pub fn read_text(path: &Path, max: u64) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(max + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= max).then_some(text)
}

fn data_dir_allowed(path: &Path) -> bool {
    let Some(key) = fsx::norm_key(path) else { return false };
    let text = path.to_string_lossy();
    if key.len() <= 3 || !fsx::local_drive_path(&text) || fsx::is_protected(&text) {
        return false;
    }
    if super::winenv::inside(path, super::winenv::windows_dir()) {
        return false;
    }
    if super::winenv::program_dirs().iter().any(|dir| super::winenv::inside(path, dir)) {
        return false;
    }
    if ["ProgramData", "Public"].iter().any(|name| super::winenv::var(name).is_some_and(|dir| super::winenv::inside(path, Path::new(&dir)))) {
        return false;
    }
    if ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"].iter().any(|name| std::env::var_os(name).is_some_and(|dir| super::winenv::inside(path, Path::new(&dir)))) {
        return false;
    }
    if let Some(profile) = super::winenv::profile()
        && let Some(users) = profile.parent()
        && super::winenv::inside(path, users)
        && !super::winenv::inside(path, profile) {
            return false;
    }
    true
}

pub fn cleaned_root(dir: &Path) -> bool {
    let Some(key) = fsx::norm_key(dir) else { return true };
    let mut roots = vec![std::env::temp_dir()];
    for var in ["TEMP", "TMP"] {
        if let Ok(v) = std::env::var(var) {
            roots.push(PathBuf::from(v));
        }
    }
    if let Some(v) = super::winenv::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(v).join("Temp"));
    }
    if let Some(v) = super::winenv::var("SystemRoot") {
        roots.push(PathBuf::from(v).join("Temp"));
    }
    roots.iter().filter_map(|r| fsx::norm_key(r)).any(|r| key == r || fsx::key_within(&key, &r))
}

pub fn default_root() -> PathBuf {
    static DEFAULT: OnceLock<PathBuf> = OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            let exe = exe_dir();
            let data = exe.join("data");
            if data_dir_allowed(&data) && !cleaned_root(&exe) && writable(&exe) && (!data.exists() || writable(&data)) { data } else { fallback_base() }
        })
        .clone()
}

fn custom_root() -> Option<PathBuf> {
    pointer_files().iter().find_map(|f| {
        if !fsx::local_drive_path(&f.to_string_lossy()) {
            return None;
        }
        let text = read_text(f, MAX_POINTER)?;
        let path = PathBuf::from(text.trim());
        (path.is_absolute() && data_dir_allowed(&path) && path.is_dir() && writable(&path)).then_some(path)
    })
}

pub fn is_custom() -> bool {
    root();
    CUSTOM.load(Ordering::SeqCst)
}

fn resolve() -> PathBuf {
    let custom = custom_root();
    CUSTOM.store(custom.is_some(), Ordering::SeqCst);
    custom.unwrap_or_else(default_root)
}

fn set_root(path: PathBuf) {
    let leaked: &'static Path = Box::leak(path.into_boxed_path());
    *ROOT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(leaked);
    refresh_own();
}

pub fn root() -> &'static Path {
    if let Some(p) = *ROOT.read().unwrap_or_else(std::sync::PoisonError::into_inner) {
        return p;
    }
    let mut guard = ROOT.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(p) = *guard {
        return p;
    }
    let leaked: &'static Path = Box::leak(resolve().into_boxed_path());
    *guard = Some(leaked);
    drop(guard);
    refresh_own();
    leaked
}

pub fn harden_default() {
    if is_custom() {
        return;
    }
    if let Ok(dir) = fsx::PinnedDir::open(root())
        && !dir.is_private() {
            let _ = dir.make_private();
    }
}

pub fn ensure_root() -> &'static Path {
    let root = root();
    let _ = fsx::PinnedDir::create_private(root);
    root
}

pub fn backup_root() -> PathBuf {
    root().join("backup")
}

pub fn own_dirs() -> [PathBuf; 2] {
    [root().to_path_buf(), exe_dir()]
}

fn refresh_own() {
    let keys: Option<Vec<String>> = own_dirs().iter().map(|d| fsx::norm_key(d).map(|k| k + "\\")).collect();
    *OWN.write().unwrap_or_else(std::sync::PoisonError::into_inner) = keys.unwrap_or_default();
}

pub fn is_own(path: &Path) -> bool {
    root();
    let own = OWN.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut key) = fsx::norm_key(path) else { return true };
    if own.is_empty() {
        return true;
    }
    key.push('\\');
    own.iter().any(|o| key.starts_with(o.as_str()))
}

fn copy_missing(from: &fsx::PinnedDir, to: &fsx::PinnedDir) -> std::io::Result<()> {
    for entry in fs::read_dir(from.path())? {
        let entry = entry?;
        let name = entry.file_name();
        let meta = fs::symlink_metadata(entry.path())?;
        use std::os::windows::fs::MetadataExt;
        if fsx::attrs_reparse(meta.file_attributes()) {
            continue;
        }
        if meta.is_dir() {
            let source = from.child(&name, false)?;
            let target = to.private_child(&name)?;
            copy_missing(&source, &target)?;
        } else if meta.is_file() {
            if to.has_plain_entry(&name)? {
                continue;
            }
            match from.copy_file_to(&name, to) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !to.has_plain_entry(&name)? {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
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
    if fsx::local_drive_path(&primary.to_string_lossy()) && writable(&exe_dir()) && fsx::write_file(&primary, text.as_bytes()).is_ok() {
        remove_pointer(&fallback);
        return Ok(());
    }
    if !fsx::local_drive_path(&fallback.to_string_lossy()) {
        return Err(std::io::Error::other("Wisping cannot save the folder in a local location."));
    }
    fsx::write_file(&fallback, text.as_bytes())
}

fn remove_pointer(path: &Path) {
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let _ = fsx::PinnedDir::open(parent).and_then(|dir| dir.remove_file(name));
    }
}

fn clear_pointer() {
    for f in pointer_files() {
        remove_pointer(&f);
    }
}

pub fn set_location(new: &Path) -> std::io::Result<()> {
    if !new.is_absolute() {
        return Err(std::io::Error::other("Choose a full folder path."));
    }
    if !data_dir_allowed(new) {
        return Err(std::io::Error::other("Choose a local folder outside Windows, Program Files, ProgramData, Public, and OneDrive."));
    }
    let current = root().to_path_buf();
    if same_dir(new, &current) {
        return Ok(());
    }
    if inside(new, &current) || inside(&current, new) {
        return Err(std::io::Error::other("Choose a folder that is not inside the current data folder, and not one that contains it."));
    }
    let source = match fsx::PinnedDir::open(&current) {
        Ok(dir) => Some(dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let destination = fsx::PinnedDir::create_private(new)?;
    if !writable_in(&destination) {
        return Err(std::io::Error::other("Wisping cannot write to that folder."));
    }
    if let Some(source) = &source {
        copy_missing(source, &destination)?;
    }
    if same_dir(new, &default_root()) {
        clear_pointer();
        CUSTOM.store(false, Ordering::SeqCst);
    } else {
        write_pointer(new)?;
        CUSTOM.store(true, Ordering::SeqCst);
    }
    set_root(new.to_path_buf());
    harden_default();
    Ok(())
}

pub fn reset_location() -> std::io::Result<()> {
    set_location(&default_root())
}

pub fn new_backup_dir() -> std::io::Result<PathBuf> {
    ensure_root();
    let stem = sys::stamp_name();
    let parent = fsx::PinnedDir::create_private(root())?.private_child("backup".as_ref())?;
    for n in 0..100 {
        let name = if n == 0 { stem.clone() } else { format!("{}-{}", stem, n) };
        let dir = parent.path().join(&name);
        match fs::create_dir(&dir) {
            Ok(()) => {
                let target = parent.child(name.as_ref(), false)?;
                let _ = target.make_private();
                let _ = super::backups::prune(100);
                return Ok(target.path().to_path_buf());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
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
    const MAX_LOG: u64 = 1024 * 1024;
    ensure_root();
    let path = log_path();
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG) {
        let _ = fsx::write_file(&path, b"");
    }
    let s = sys::now_local();
    let stamp = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", s.year, s.month, s.day, s.hour, s.minute, s.second);
    let _ = fsx::append_file(&path, format!("{} {}\n", stamp, line).as_bytes());
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
    read_text(&root().join(name), MAX_SETTING).map(|text| text.trim().to_string())
}

pub fn write_setting(name: &str, value: &str) {
    if !settings::remember_on() {
        return;
    }
    ensure_root();
    let _ = fsx::write_file(&root().join(name), value.as_bytes());
}
