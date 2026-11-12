use std::ffi::{OsStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    FILE_DISPOSITION_INFO_EX_FLAGS, FILE_NAME_NORMALIZED, FILE_NAME_OPENED, FileDispositionInfo, FileDispositionInfoEx, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW,
    BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_ID_INFO, FileBasicInfo, FileIdInfo, GetDriveTypeW, GetFileInformationByHandle, GetFileInformationByHandleEx, SetFileInformationByHandle,
};
use windows::core::PCWSTR;

use super::lock::LockExt;

const REPARSE_POINT: u32 = 0x400;
const READ_ONLY: u32 = 0x1;
const DELETE: u32 = 0x0001_0000;
const READ_ATTRIBUTES: u32 = 0x80;
const LIST_DIRECTORY: u32 = 1;
const SHARE_READ: u32 = 1;
const WRITE_ATTRIBUTES: u32 = 0x100;
const WRITE_DAC: u32 = 0x0004_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;
const SHARE_ALL: u32 = 7;
const SEP: u16 = b'\\' as u16;
const PERSISTENT_ACLS: u32 = 8;

#[path = "fsx/copy_remove.rs"]
mod copy_remove;

pub fn is_reparse(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(m) => attrs_reparse(m.file_attributes()),
        Err(_) => true,
    }
}

pub fn attrs_reparse(attrs: u32) -> bool {
    attrs & REPARSE_POINT != 0
}

pub fn attrs_cloud(attrs: u32) -> bool {
    attrs & (0x0040_0000 | 0x0004_0000 | 0x1000) != 0
}

pub fn expand_env(text: &str) -> String {
    expand_with(text, super::winenv::var)
}

pub fn expand_with(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
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
        match lookup(name) {
            Some(value) if !name.is_empty() => out.push_str(&value),
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
    WINDIR.get_or_init(|| super::winenv::windows_dir().to_string_lossy().to_lowercase().trim_end_matches('\\').to_string())
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

#[derive(Clone, PartialEq)]
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

pub fn local_drive_path(path: &str) -> bool {
    let b = path.as_bytes();
    if !(b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')) {
        return false;
    }
    let root = [b[0] as u16, b':' as u16, b'\\' as u16, 0];
    matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 3 | 5 | 6)
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

pub fn plain_exists(path: &Path) -> Option<bool> {
    if !path.is_absolute() || !local_drive_path(&path.to_string_lossy()) {
        return None;
    }
    let mut chain: Vec<&Path> = path.ancestors().collect();
    chain.reverse();
    for part in chain {
        match fs::symlink_metadata(part) {
            Ok(meta) => {
                if attrs_reparse(meta.file_attributes()) {
                    return None;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(false),
            Err(_) => return None,
        }
    }
    Some(true)
}

pub fn plain_file(path: &Path) -> Option<bool> {
    if !plain_exists(path)? {
        return Some(false);
    }
    let meta = fs::symlink_metadata(path).ok()?;
    if attrs_reparse(meta.file_attributes()) {
        return None;
    }
    Some(meta.is_file())
}

pub fn plain_dir(path: &Path) -> Option<bool> {
    if !plain_exists(path)? {
        return Some(false);
    }
    let meta = fs::symlink_metadata(path).ok()?;
    if attrs_reparse(meta.file_attributes()) {
        return None;
    }
    Some(meta.is_dir())
}

pub fn open_plain(path: &Path) -> io::Result<File> {
    if !local_drive_path(&path.to_string_lossy()) {
        return Err(refused_path());
    }
    let parent = path.parent().ok_or_else(refused_path)?;
    let name = path.file_name().ok_or_else(refused_path)?;
    PinnedDir::open(parent)?.read_file(name)
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
                    if mask.matches(&e.file_name()) && plain_dir(&e.path()) == Some(true) {
                        next.push(e.path());
                    }
                }
            }
        } else {
            next.extend(current.iter().map(|c| c.join(&seg)).filter(|p| plain_dir(p) == Some(true)));
        }
        current = next;
    }
    current
}

pub fn fixed_prefix(pattern: &str) -> Option<PathBuf> {
    let path = PathBuf::from(expand_env(pattern));
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push("\\"),
            Component::Normal(s) if !has_wildcard(&s.to_string_lossy()) => out.push(s),
            Component::Normal(_) => break,
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty() && out.has_root()).then_some(out)
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

pub struct PinnedDir {
    path: PathBuf,
    pins: Vec<Arc<File>>,
}

fn refused_path() -> io::Error {
    io::Error::other("changed")
}

fn child_name(name: &OsStr) -> io::Result<()> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() || name.as_encoded_bytes().contains(&b':') {
        return Err(refused_path());
    }
    Ok(())
}

fn copy_basic_info(from: &File, to: &File) -> io::Result<()> {
    const PLAIN_ATTRIBUTES: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x100 | 0x2000;
    let mut info = FILE_BASIC_INFO::default();
    let size = std::mem::size_of::<FILE_BASIC_INFO>() as u32;
    unsafe { GetFileInformationByHandleEx(HANDLE(from.as_raw_handle()), FileBasicInfo, (&mut info as *mut FILE_BASIC_INFO).cast(), size) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
    info.FileAttributes &= PLAIN_ATTRIBUTES;
    if info.FileAttributes == 0 {
        info.FileAttributes = 0x80;
    }
    info.ChangeTime = 0;
    unsafe { SetFileInformationByHandle(HANDLE(to.as_raw_handle()), FileBasicInfo, (&info as *const FILE_BASIC_INFO).cast(), size) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))
}

fn windows_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}

fn identity(handle: HANDLE) -> io::Result<(u64, [u8; 16])> {
    let mut info = FILE_ID_INFO::default();
    unsafe { GetFileInformationByHandleEx(handle, FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(),
        std::mem::size_of::<FILE_ID_INFO>() as u32) }.map_err(windows_error)?;
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}

fn handle_is_child(file: &File, parent: &File) -> bool {
    let (Some(path), Some(base)) = (final_path(file), final_path(parent)) else { return false };
    path.iter().rposition(|&c| c == SEP).is_some_and(|end| path[..end].len() == base.len() && same_prefix(&path[..end], &base))
}

fn private_users() -> io::Result<Vec<String>> {
    let mut users = vec![super::sys::user_sid()?];
    if super::sys::is_admin()
        && let Some(shell) = super::sys::shell_user_sid()
        && !users.contains(&shell) {
            users.push(shell);
    }
    Ok(users)
}

fn private_descriptor(descriptor: windows::Win32::Security::PSECURITY_DESCRIPTOR, users: &[String]) -> bool {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, IsValidAcl, IsValidSecurityDescriptor, PSID, SE_DACL_PROTECTED,
    };
    use windows::core::{BOOL, PWSTR};
    if descriptor.0.is_null() || !unsafe { IsValidSecurityDescriptor(descriptor) }.as_bool() {
        return false;
    }
    let allowed = |sid: PSID| {
        if sid.0.is_null() {
            return false;
        }
        let mut text = PWSTR::null();
        if unsafe { ConvertSidToStringSidW(sid, &mut text) }.is_err() {
            return false;
        }
        let name = unsafe { text.to_string() };
        unsafe { LocalFree(Some(HLOCAL(text.0.cast()))); }
        name.is_ok_and(|name| name == "S-1-5-18" || name == "S-1-5-32-544" || users.contains(&name))
    };
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.is_err()
        || control & SE_DACL_PROTECTED.0 == 0 {
        return false;
    }
    let mut owner = PSID::default();
    let mut defaulted = BOOL(0);
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) }.is_err() || !allowed(owner) {
        return false;
    }
    let mut present = BOOL(0);
    let mut acl = std::ptr::null_mut();
    if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }.is_err()
        || !present.as_bool() || acl.is_null() || !unsafe { IsValidAcl(acl) }.as_bool() {
        return false;
    }
    for index in 0..unsafe { (*acl).AceCount } {
        let mut raw = std::ptr::null_mut();
        if unsafe { GetAce(acl, index as u32, &mut raw) }.is_err() || raw.is_null() {
            return false;
        }
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        if header.AceType == 1 {
            continue;
        }
        if header.AceType != 0 || (header.AceSize as usize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>() {
            return false;
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        if !allowed(PSID((&raw const ace.SidStart).cast_mut().cast())) {
            return false;
        }
    }
    true
}

impl PinnedDir {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::walk(path, false, false)
    }

    pub fn create(path: &Path) -> io::Result<Self> {
        Self::walk(path, true, false)
    }

    pub fn create_private(path: &Path) -> io::Result<Self> {
        Self::walk(path, true, true)
    }

    fn walk(path: &Path, create: bool, private: bool) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(refused_path());
        }
        let mut current = PathBuf::new();
        let mut dir = None;
        for part in path.components() {
            match part {
                Component::Prefix(p) => {
                    use std::path::Prefix;
                    if !matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(..) | Prefix::VerbatimUNC(..)) {
                        return Err(refused_path());
                    }
                    current.push(p.as_os_str());
                }
                Component::RootDir => {
                    current.push("\\");
                    let file = Self::pin(&current, None, READ_ATTRIBUTES)?;
                    dir = Some(Self { path: current.clone(), pins: vec![Arc::new(file)] });
                }
                Component::Normal(name) => dir = Some(dir.ok_or_else(refused_path)?.child_inner(name, create, READ_ATTRIBUTES, private)?),
                _ => return Err(refused_path()),
            }
        }
        dir.ok_or_else(refused_path)
    }

    fn pin(path: &Path, parent: Option<&File>, access: u32) -> io::Result<File> {
        let file = OpenOptions::new().access_mode(access | LIST_DIRECTORY).share_mode(SHARE_READ).custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS).open(path)
            .map_err(|e| io::Error::new(e.kind(), format!("{}: {}", path.display(), e)))?;
        let meta = file.metadata()?;
        if !meta.is_dir() || attrs_reparse(meta.file_attributes()) || parent.is_some_and(|p| !handle_is_child(&file, p)) {
            return Err(refused_path());
        }
        Ok(file)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn keeps_acls(&self) -> bool {
        use windows::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
        let mut flags = 0u32;
        unsafe { GetVolumeInformationByHandleW(HANDLE(self.handle().as_raw_handle()), None, None, None, Some(&mut flags), None) }.is_err() || flags & PERSISTENT_ACLS != 0
    }

    fn security_handle(&self, access: u32) -> io::Result<File> {
        let file = OpenOptions::new().access_mode(access | READ_CONTROL | READ_ATTRIBUTES).share_mode(SHARE_ALL).custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS).open(&self.path)?;
        if identity(HANDLE(file.as_raw_handle()))? != identity(HANDLE(self.handle().as_raw_handle()))? {
            return Err(refused_path());
        }
        Ok(file)
    }

    pub fn make_private(&self) -> io::Result<()> {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT, SetSecurityInfo};
        use windows::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
        if !self.keeps_acls() {
            return Ok(());
        }
        let sids = private_users()?;
        let users: String = sids.iter().map(|sid| format!("(A;OICI;FA;;;{})", sid)).collect();
        let text: Vec<u16> = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){}", users).encode_utf16().chain([0]).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), 1, &mut descriptor, None) }.map_err(io::Error::other)?;
        let mut present = windows::core::BOOL(0);
        let mut defaulted = windows::core::BOOL(0);
        let mut acl: *mut ACL = std::ptr::null_mut();
        let result = unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }.map_err(io::Error::other).and_then(|_| {
            let handle = self.security_handle(WRITE_DAC)?;
            unsafe { SetSecurityInfo(HANDLE(handle.as_raw_handle()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, None, None, Some(acl), None) }.ok().map_err(io::Error::other)
        });
        unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        result
    }

    pub fn is_private(&self) -> bool {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
        use windows::Win32::Security::{DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
        let Ok(users) = private_users() else { return false };
        let Ok(handle) = self.security_handle(0) else { return false };
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let status = unsafe { GetSecurityInfo(HANDLE(handle.as_raw_handle()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION, None, None, None, None, Some(&mut descriptor)) };
        drop(handle);
        if status.is_err() || descriptor.0.is_null() {
            return false;
        }
        let protected = private_descriptor(descriptor, &users);
        unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        protected
    }

    fn handle(&self) -> &File {
        self.pins.last().unwrap()
    }

    pub fn child(&self, name: &OsStr, create: bool) -> io::Result<Self> {
        self.child_with_access(name, create, READ_ATTRIBUTES)
    }

    pub fn private_child(&self, name: &OsStr) -> io::Result<Self> {
        self.child_inner(name, true, READ_ATTRIBUTES, true)
    }

    pub fn deletable_child(&self, name: &OsStr) -> io::Result<Self> {
        self.child_with_access(name, false, READ_ATTRIBUTES | DELETE)
    }

    pub fn under(&self, path: &Path) -> io::Result<Self> {
        let rest = path.strip_prefix(&self.path).map_err(|_| refused_path())?;
        let mut dir = Self { path: self.path.clone(), pins: self.pins.clone() };
        for part in rest.components() {
            match part {
                Component::Normal(name) => dir = dir.child(name, false)?,
                _ => return Err(refused_path()),
            }
        }
        Ok(dir)
    }

    pub fn hold_child_dir(&self, name: &OsStr) -> io::Result<File> {
        child_name(name)?;
        let file = OpenOptions::new().access_mode(READ_ATTRIBUTES).share_mode(SHARE_ALL).custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS).open(self.path.join(name))?;
        let meta = file.metadata()?;
        if !meta.is_dir() || attrs_reparse(meta.file_attributes()) || !handle_is_child(&file, self.handle()) {
            return Err(refused_path());
        }
        Ok(file)
    }

    fn child_with_access(&self, name: &OsStr, create: bool, access: u32) -> io::Result<Self> {
        self.child_inner(name, create, access, false)
    }

    fn child_inner(&self, name: &OsStr, create: bool, access: u32, private: bool) -> io::Result<Self> {
        child_name(name)?;
        let path = self.path.join(name);
        let made = create && match fs::create_dir(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e),
        };
        let file = Self::pin(&path, Some(self.handle()), access)?;
        let mut pins = self.pins.clone();
        pins.push(Arc::new(file));
        let dir = Self { path, pins };
        if made && private {
            dir.make_private()?;
        }
        Ok(dir)
    }

    pub fn read_file(&self, name: &OsStr) -> io::Result<File> {
        child_name(name)?;
        let file = OpenOptions::new().read(true).share_mode(SHARE_READ).custom_flags(OPEN_REPARSE_POINT).open(self.path.join(name))?;
        self.plain_child(&file)?;
        Ok(file)
    }

    pub fn has_plain_entry(&self, name: &OsStr) -> io::Result<bool> {
        child_name(name)?;
        let opened = OpenOptions::new().access_mode(READ_ATTRIBUTES).share_mode(SHARE_READ).custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS).open(self.path.join(name));
        let file = match opened {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let meta = file.metadata()?;
        if attrs_reparse(meta.file_attributes()) || !(meta.is_file() || meta.is_dir()) || !handle_is_child(&file, self.handle()) {
            return Err(refused_path());
        }
        Ok(true)
    }

    pub fn create_file(&self, name: &OsStr) -> io::Result<File> {
        child_name(name)?;
        let file = OpenOptions::new().write(true).create_new(true).share_mode(0).custom_flags(OPEN_REPARSE_POINT).open(self.path.join(name))?;
        self.plain_child(&file)?;
        Ok(file)
    }

    fn update_file(&self, name: &OsStr, durable: bool, update: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
        static WRITERS: Mutex<()> = Mutex::new(());
        let _writer = WRITERS.locked();
        child_name(name)?;
        let mut file = OpenOptions::new().read(true).write(true).create(true).share_mode(SHARE_READ)
            .custom_flags(OPEN_REPARSE_POINT).open(self.path.join(name))?;
        self.plain_child(&file)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
        if info.nNumberOfLinks != 1 {
            return Err(refused_path());
        }
        update(&mut file)?;
        if durable { file.sync_all() } else { Ok(()) }
    }

    pub fn write_file(&self, name: &OsStr, contents: &[u8]) -> io::Result<()> {
        self.update_file(name, true, |file| {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(contents)
        })
    }

    pub fn append_file(&self, name: &OsStr, contents: &[u8]) -> io::Result<()> {
        self.update_file(name, false, |file| {
            file.seek(SeekFrom::End(0))?;
            file.write_all(contents)
        })
    }

    pub fn append_lines(&self, name: &OsStr, line: &str, keep: usize) -> io::Result<()> {
        const TAIL: u64 = 1 << 20;
        self.update_file(name, true, |file| {
            let len = file.metadata()?.len();
            let mut bytes = Vec::new();
            if len > TAIL {
                file.seek(SeekFrom::Start(len - TAIL))?;
            }
            file.read_to_end(&mut bytes)?;
            if len > TAIL {
                let cut = bytes.iter().position(|&b| b == b'\n').map_or(bytes.len(), |i| i + 1);
                bytes.drain(..cut);
            }
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            text.push_str(line);
            let lines: Vec<&str> = text.lines().collect();
            let text = if lines.len() > keep { lines[lines.len() - keep..].join("\n") + "\n" } else { text };
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(text.as_bytes())
        })
    }

    pub fn copy_file_to(&self, name: &OsStr, target: &Self) -> io::Result<()> {
        let mut source = self.read_file(name)?;
        let mut copy = target.create_file(name)?;
        let copied = io::copy(&mut source, &mut copy).and_then(|_| copy_basic_info(&source, &copy)).and_then(|_| copy.sync_all());
        if copied.is_err() {
            drop(copy);
            let _ = target.remove_file(name);
        }
        copied
    }

    fn plain_child(&self, file: &File) -> io::Result<()> {
        let meta = file.metadata()?;
        if !meta.is_file() || attrs_reparse(meta.file_attributes()) || !handle_is_child(file, self.handle()) {
            return Err(refused_path());
        }
        Ok(())
    }

    pub fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        child_name(name)?;
        let file = OpenOptions::new().access_mode(DELETE | READ_ATTRIBUTES).share_mode(SHARE_READ).custom_flags(OPEN_REPARSE_POINT).open(self.path.join(name))?;
        self.plain_child(&file)?;
        if dispose(&file, false) { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    pub fn backup_and_remove_file(&self, name: &OsStr, target: &Self, kept: &OsStr, seal: impl FnOnce(&Path, &mut File) -> io::Result<()>) -> io::Result<()> {
        copy_remove::run(self, name, target, kept, seal)
    }

    pub fn restore_file(&self, name: &OsStr, from: &mut File) -> io::Result<()> {
        let mut file = self.create_file(name)?;
        let copied = io::copy(from, &mut file).and_then(|_| copy_basic_info(from, &file)).and_then(|_| file.sync_all());
        if copied.is_err() {
            drop(file);
            let _ = self.remove_file(name);
        }
        copied
    }

    pub fn remove(self) -> io::Result<()> {
        if dispose(self.handle(), false) { Ok(()) } else { Err(io::Error::last_os_error()) }
    }
}

pub fn write_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(refused_path)?;
    let name = path.file_name().ok_or_else(refused_path)?;
    PinnedDir::create(parent)?.write_file(name, contents)
}

pub fn append_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(refused_path)?;
    let name = path.file_name().ok_or_else(refused_path)?;
    PinnedDir::create(parent)?.append_file(name, contents)
}

pub struct Fence {
    root: Vec<u16>,
    own: Vec<Vec<u16>>,
}

impl Fence {
    pub fn guard_paths(own: &[PathBuf]) -> Vec<Vec<u16>> {
        let mut guarded = Vec::new();
        for p in own {
            for flags in [OPEN_REPARSE_POINT | BACKUP_SEMANTICS, BACKUP_SEMANTICS] {
                if let Some(f) = open_with(p, READ_ATTRIBUTES, flags).ok().and_then(|f| final_path(&f)) {
                    guarded.push(f);
                }
            }
        }
        guarded
    }

    pub fn guarded(dir: &PinnedDir, guarded: &[Vec<u16>]) -> Option<Fence> {
        let root = final_path(dir.handle())?;
        if guarded.iter().any(|o| same_or_within(&root, o)) {
            return None;
        }
        Some(Fence { root, own: guarded.to_vec() })
    }

    pub fn admits(&self, path: &[u16]) -> bool {
        within(path, &self.root) && !self.own.iter().any(|o| same_or_within(path, o))
    }
}

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
