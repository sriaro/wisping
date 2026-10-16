use std::ffi::{OsStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_DISPOSITION_INFO_EX_FLAGS, FILE_NAME_NORMALIZED, FILE_NAME_OPENED, FileDispositionInfo, FileDispositionInfoEx, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW,
    SetFileInformationByHandle,
};

const REPARSE_POINT: u32 = 0x400;
const READ_ONLY: u32 = 0x1;
const DELETE: u32 = 0x0001_0000;
const READ_ATTRIBUTES: u32 = 0x80;
const WRITE_ATTRIBUTES: u32 = 0x100;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;
const SHARE_ALL: u32 = 7;
const SEP: u16 = b'\\' as u16;

pub fn is_reparse(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(m) => attrs_reparse(m.file_attributes()),
        Err(_) => true,
    }
}

pub fn attrs_reparse(attrs: u32) -> bool {
    attrs & REPARSE_POINT != 0
}

pub fn expand_env(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            out.push('%');
            out.push_str(after);
            return out;
        };
        let name = &after[..end];
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

pub fn windir() -> &'static str {
    static WINDIR: OnceLock<String> = OnceLock::new();
    WINDIR.get_or_init(|| std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string()).to_lowercase().trim_end_matches('\\').to_string())
}

fn clean_lower(path: &str) -> String {
    expand_env(path.trim().trim_matches('"')).to_lowercase().replace('/', "\\")
}

pub fn under_windir(path: &str) -> bool {
    let p = clean_lower(path);
    let w = windir();
    p.len() > w.len() && p.starts_with(w) && p.as_bytes()[w.len()] == b'\\'
}

fn system_dirs() -> &'static [PathBuf] {
    static DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRS.get_or_init(|| {
        let w = Path::new(windir());
        ["system32", "", "syswow64", "system32\\windowspowershell\\v1.0", "system32\\wbem"].iter().map(|d| if d.is_empty() { w.to_path_buf() } else { w.join(d) }).collect()
    })
}

pub fn is_protected(path: &str) -> bool {
    let p = clean_lower(path);
    if p.is_empty() {
        return false;
    }
    if !p.contains('\\') {
        return !p.contains(':') && system_dirs().iter().any(|d| d.join(&p).is_file());
    }
    under_windir(&p) || ["\\windows defender", "\\windows security", "\\windowsapps\\"].iter().any(|m| p.contains(m))
}

pub fn direct_child(key: &str, prefix: &str) -> bool {
    key.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty() && !rest.contains('\\'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mask {
    Any,
    Exact(Box<[u8]>),
    Prefix(Box<[u8]>),
    Suffix(Box<[u8]>),
    Around(Box<[u8]>, Box<[u8]>),
    General(Box<[char]>),
}

impl Mask {
    pub fn new(mask: &str) -> Mask {
        if mask.is_ascii() && !mask.contains('?') {
            let m = mask.to_ascii_lowercase().into_bytes();
            let stars: Vec<usize> = m.iter().enumerate().filter(|(_, b)| **b == b'*').map(|(i, _)| i).collect();
            match stars.as_slice() {
                [] => return Mask::Exact(m.into()),
                [at] => {
                    let (head, tail) = (&m[..*at], &m[at + 1..]);
                    return match (head.is_empty(), tail.is_empty()) {
                        (true, true) => Mask::Any,
                        (false, true) => Mask::Prefix(head.into()),
                        (true, false) => Mask::Suffix(tail.into()),
                        _ => Mask::Around(head.into(), tail.into()),
                    };
                }
                _ => {}
            }
        }
        Mask::General(mask.to_lowercase().chars().collect())
    }

    pub fn is_any(&self) -> bool {
        matches!(self, Mask::Any)
    }

    pub fn matches(&self, name: &OsStr) -> bool {
        let b = name.as_encoded_bytes();
        let head = |h: &[u8]| b.len() >= h.len() && b[..h.len()].eq_ignore_ascii_case(h);
        let tail = |t: &[u8]| b.len() >= t.len() && b[b.len() - t.len()..].eq_ignore_ascii_case(t);
        match self {
            Mask::Any => true,
            Mask::Exact(m) => b.eq_ignore_ascii_case(m),
            Mask::Prefix(h) => head(h),
            Mask::Suffix(t) => tail(t),
            Mask::Around(h, t) => b.len() >= h.len() + t.len() && head(h) && tail(t),
            Mask::General(m) => {
                let n: Vec<char> = name.to_string_lossy().to_lowercase().chars().collect();
                glob(&n, m)
            }
        }
    }
}

fn glob(n: &[char], m: &[char]) -> bool {
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while i < n.len() {
        if j < m.len() && (m[j] == '?' || m[j] == n[i]) {
            i += 1;
            j += 1;
        } else if j < m.len() && m[j] == '*' {
            star = Some(j);
            mark = i;
            j += 1;
        } else if let Some(s) = star {
            j = s + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while j < m.len() && m[j] == '*' {
        j += 1;
    }
    j == m.len()
}

pub fn has_wildcard(text: &str) -> bool {
    text.contains(['*', '?'])
}

pub fn absolute(path: &Path) -> Option<PathBuf> {
    std::path::absolute(path).ok()
}

pub fn norm_key(path: &Path) -> Option<String> {
    let full = absolute(path)?;
    let mut text = full.to_string_lossy().replace('/', "\\");
    while text.len() > 3 && text.ends_with('\\') {
        text.pop();
    }
    Some(text.to_lowercase())
}

pub fn expand_dirs(pattern: &str) -> Vec<PathBuf> {
    let path = PathBuf::from(expand_env(pattern));
    let mut base = PathBuf::new();
    let mut segments: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => base.push(p.as_os_str()),
            Component::RootDir => base.push("\\"),
            Component::Normal(s) => segments.push(s.to_string_lossy().into_owned()),
            _ => return Vec::new(),
        }
    }
    if base.as_os_str().is_empty() || !base.has_root() {
        return Vec::new();
    }
    let mut current = vec![base];
    for seg in segments {
        let mut next = Vec::new();
        if has_wildcard(&seg) {
            let mask = Mask::new(&seg);
            for c in &current {
                let Ok(rd) = fs::read_dir(c) else { continue };
                for e in rd.flatten() {
                    if e.file_type().is_ok_and(|t| t.is_dir()) && mask.matches(&e.file_name()) {
                        next.push(e.path());
                    }
                }
            }
        } else {
            next.extend(current.iter().map(|c| c.join(&seg)).filter(|p| p.is_dir()));
        }
        current = next;
    }
    current
}

pub fn newest(meta: &fs::Metadata) -> Option<SystemTime> {
    match (meta.modified(), meta.created()) {
        (Ok(a), Ok(b)) => Some(a.max(b)),
        (Ok(a), Err(_)) => Some(a),
        (Err(_), Ok(b)) => Some(b),
        _ => None,
    }
}

pub fn too_new(stamp: Option<SystemTime>, min_age_hours: u64) -> bool {
    let Some(stamp) = stamp else { return true };
    SystemTime::now().duration_since(stamp).map(|age| age < Duration::from_secs(min_age_hours * 3600)).unwrap_or(true)
}

fn open_with(path: &Path, access: u32, flags: u32) -> io::Result<File> {
    OpenOptions::new().access_mode(access).share_mode(SHARE_ALL).custom_flags(flags).open(path)
}

pub fn open_nofollow(path: &Path, access: u32) -> io::Result<File> {
    open_with(path, access, OPEN_REPARSE_POINT | BACKUP_SEMANTICS)
}

pub fn final_path(file: &File) -> Option<Vec<u16>> {
    final_path_as(file, FILE_NAME_NORMALIZED).or_else(|| final_path_as(file, FILE_NAME_OPENED))
}

fn final_path_as(file: &File, flags: GETFINALPATHNAMEBYHANDLE_FLAGS) -> Option<Vec<u16>> {
    let handle = HANDLE(file.as_raw_handle());
    let mut buf = vec![0u16; 300];
    loop {
        let n = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, flags) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            buf.truncate(n);
            while buf.len() > 1 && buf.last() == Some(&SEP) {
                buf.pop();
            }
            return Some(buf);
        }
        buf.resize(n + 1, 0);
    }
}

fn dos_key(units: &[u16]) -> String {
    let text = String::from_utf16_lossy(units);
    let text = match text.strip_prefix("\\\\?\\UNC\\") {
        Some(rest) => format!("\\\\{}", rest),
        None => text.strip_prefix("\\\\?\\").map(str::to_string).unwrap_or(text),
    };
    let mut lower = text.to_lowercase();
    while lower.len() > 3 && lower.ends_with('\\') {
        lower.pop();
    }
    lower
}

pub fn canonical_key(path: &Path) -> Option<String> {
    let file = open_nofollow(path, READ_ATTRIBUTES).ok()?;
    Some(dos_key(&final_path(&file)?))
}

pub fn key_within(key: &str, base: &str) -> bool {
    key.len() > base.len() && key.starts_with(base) && key.as_bytes()[base.len()] == b'\\'
}

fn same_unit(a: u16, b: u16) -> bool {
    a == b || (a < 128 && b < 128 && (a as u8).eq_ignore_ascii_case(&(b as u8)))
}

fn same_prefix(path: &[u16], base: &[u16]) -> bool {
    path.len() >= base.len() && path.iter().zip(base).all(|(a, b)| same_unit(*a, *b))
}

fn within(path: &[u16], base: &[u16]) -> bool {
    path.len() > base.len() && path[base.len()] == SEP && same_prefix(path, base)
}

fn same_or_within(path: &[u16], base: &[u16]) -> bool {
    (path.len() == base.len() && same_prefix(path, base)) || within(path, base)
}

pub struct Fence {
    root: Vec<u16>,
    own: Vec<Vec<u16>>,
}

impl Fence {
    pub fn new(root: &Path, own: &[PathBuf]) -> Option<Fence> {
        let file = open_nofollow(root, READ_ATTRIBUTES).ok()?;
        if attrs_reparse(file.metadata().ok()?.file_attributes()) {
            return None;
        }
        let root = final_path(&file)?;
        let mut guarded = Vec::new();
        for p in own {
            for flags in [OPEN_REPARSE_POINT | BACKUP_SEMANTICS, BACKUP_SEMANTICS] {
                if let Some(f) = open_with(p, READ_ATTRIBUTES, flags).ok().and_then(|f| final_path(&f)) {
                    guarded.push(f);
                }
            }
        }
        if guarded.iter().any(|o| same_or_within(&root, o)) {
            return None;
        }
        Some(Fence { root, own: guarded })
    }

    pub fn admits(&self, path: &[u16]) -> bool {
        within(path, &self.root) && !self.own.iter().any(|o| same_or_within(path, o))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    Removed(u64),
    Kept,
    Gone,
    Failed,
}

pub fn remove_inside(path: &Path, fence: &Fence, min_age_hours: u64, dir: bool) -> Removal {
    let opened = open_nofollow(path, DELETE | READ_ATTRIBUTES | WRITE_ATTRIBUTES)
        .or_else(|e| if e.kind() == io::ErrorKind::PermissionDenied { open_nofollow(path, DELETE | READ_ATTRIBUTES) } else { Err(e) });
    let file = match opened {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Removal::Gone,
        Err(_) => return Removal::Failed,
    };
    let Ok(meta) = file.metadata() else { return Removal::Failed };
    let attrs = meta.file_attributes();
    if attrs_reparse(attrs) || meta.is_dir() != dir {
        return Removal::Failed;
    }
    if min_age_hours > 0 {
        let stamp = if dir { meta.created().ok() } else { newest(&meta) };
        if too_new(stamp, min_age_hours) {
            return Removal::Kept;
        }
    }
    if !final_path(&file).is_some_and(|p| fence.admits(&p)) {
        return Removal::Failed;
    }
    if dispose(&file, attrs & READ_ONLY != 0) { Removal::Removed(if dir { 0 } else { meta.len() }) } else { Removal::Failed }
}

fn dispose(file: &File, read_only: bool) -> bool {
    let handle = HANDLE(file.as_raw_handle());
    let flags = FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0 | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE.0;
    let ex = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_INFO_EX_FLAGS(flags) };
    let size = std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32;
    if unsafe { SetFileInformationByHandle(handle, FileDispositionInfoEx, &ex as *const _ as *const c_void, size) }.is_ok() {
        return true;
    }
    if read_only {
        let Ok(meta) = file.metadata() else { return false };
        let mut perm = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        if file.set_permissions(perm).is_err() {
            return false;
        }
    }
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let size = std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32;
    unsafe { SetFileInformationByHandle(handle, FileDispositionInfo, &info as *const _ as *const c_void, size) }.is_ok()
}
