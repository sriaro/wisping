use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, WinBuiltinAdministratorsSid, WinLocalSystemSid,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_OPTION_NON_VOLATILE,
    RegCreateKeyExW, RegGetKeySecurity,
};
use windows::core::{BOOL, PCWSTR, w};
use winreg::RegKey;

const STORE: &str = "SOFTWARE\\Wisping\\BackupIntegrity\\v1";
const SECURITY: PCWSTR = w!("O:BAG:BAD:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;KR;;;BU)");

struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn new(text: PCWSTR) -> io::Result<Self> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(text, 1, &mut descriptor, None) }
            .map_err(io::Error::other)?;
        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(HLOCAL(self.0.0))); }
    }
}

fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "The backup integrity store is not protected.")
}

fn privileged_sid(sid: PSID) -> bool {
    [WinLocalSystemSid, WinBuiltinAdministratorsSid].iter().any(|&kind| {
        let mut bytes = [0u64; 9];
        let mut size = std::mem::size_of_val(&bytes) as u32;
        let known = PSID(bytes.as_mut_ptr().cast());
        unsafe { CreateWellKnownSid(kind, None, Some(known), &mut size).is_ok() && EqualSid(sid, known).is_ok() }
    })
}

fn check_descriptor(descriptor: PSECURITY_DESCRIPTOR) -> io::Result<()> {
    let mut control = 0;
    let mut revision = 0;
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.map_err(io::Error::other)?;
    if control & SE_DACL_PROTECTED.0 == 0 {
        return Err(refused());
    }
    let mut owner = PSID::default();
    let mut defaulted = BOOL(0);
    unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) }.map_err(io::Error::other)?;
    if owner.0.is_null() || !privileged_sid(owner) {
        return Err(refused());
    }
    let mut present = BOOL(0);
    let mut acl: *mut ACL = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }.map_err(io::Error::other)?;
    if !present.as_bool() || acl.is_null() {
        return Err(refused());
    }
    for i in 0..unsafe { (*acl).AceCount } {
        let mut raw = std::ptr::null_mut();
        unsafe { GetAce(acl, i as u32, &mut raw) }.map_err(io::Error::other)?;
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        if header.AceType != 0 || (header.AceSize as usize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>() {
            return Err(refused());
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        let sid = PSID((&raw const ace.SidStart).cast_mut().cast());
        let read_only = KEY_READ.0 | 0x8000_0000 | 0x2000_0000;
        if !privileged_sid(sid) && ace.Mask & !read_only != 0 {
            return Err(refused());
        }
    }
    Ok(())
}

fn check_store(key: &RegKey) -> io::Result<()> {
    let flags = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut size = 0;
    let status = unsafe { RegGetKeySecurity(HKEY(key.raw_handle()), flags, None, &mut size) };
    if status != ERROR_INSUFFICIENT_BUFFER || size == 0 || size > 65536 {
        return Err(refused());
    }
    let mut bytes = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    let descriptor = PSECURITY_DESCRIPTOR(bytes.as_mut_ptr().cast());
    unsafe { RegGetKeySecurity(HKEY(key.raw_handle()), flags, Some(descriptor), &mut size) }.ok().map_err(io::Error::other)?;
    check_descriptor(descriptor)
}

pub(in crate::engine) fn receipt(file: &Path, bytes: &[u8]) -> io::Result<String> {
    let id = file.parent().and_then(Path::file_name).ok_or_else(refused)?;
    let name = file.file_name().ok_or_else(refused)?;
    let mut digest = blake3::Hasher::new();
    digest.update(b"Wisping privileged registry backup v1\0");
    for part in [id, name] {
        let wide: Vec<u16> = part.encode_wide().collect();
        digest.update(&(wide.len() as u64).to_le_bytes());
        for unit in wide {
            digest.update(&unit.to_le_bytes());
        }
    }
    digest.update(bytes);
    Ok(digest.finalize().to_hex().to_string())
}

pub(in crate::engine) fn protected_store(store: &str) -> io::Result<RegKey> {
    let descriptor = Descriptor::new(SECURITY)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.0,
        bInheritHandle: BOOL(0),
    };
    let path: Vec<u16> = store.encode_utf16().chain([0]).collect();
    let mut handle = HKEY::default();
    unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_SET_VALUE | KEY_WOW64_64KEY, Some(&attributes), &mut handle, None)
    }.ok().map_err(io::Error::other)?;
    let key = RegKey::predef(handle.0);
    check_store(&key)?;
    Ok(key)
}

pub(in crate::engine) fn open_protected(store: &str, write: bool) -> io::Result<Option<RegKey>> {
    let access = winreg::enums::KEY_READ | winreg::enums::KEY_WOW64_64KEY | if write { winreg::enums::KEY_SET_VALUE } else { 0 };
    let key = match RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey_with_flags(store, access) {
        Ok(key) => key,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    check_store(&key)?;
    Ok(Some(key))
}

pub(super) fn record(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let key = protected_store(STORE)?;
    key.set_value(receipt(file, bytes)?, &1u32)
}

pub(in crate::engine) fn forget(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(key) = open_protected(STORE, true)? else { return Ok(()) };
    match key.delete_value(receipt(file, bytes)?) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

pub(in crate::engine) fn names() -> io::Result<Vec<String>> {
    let Some(key) = open_protected(STORE, false)? else { return Ok(Vec::new()) };
    key.enum_values().map(|value| value.map(|(name, _)| name)).collect()
}

pub(in crate::engine) fn forget_names(names: &[String]) -> io::Result<()> {
    let Some(key) = open_protected(STORE, true)? else { return Ok(()) };
    for name in names {
        match key.delete_value(name) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

pub(in crate::engine) fn verify(file: &Path, bytes: &[u8]) -> io::Result<bool> {
    let Some(key) = open_protected(STORE, false)? else { return Ok(false) };
    match key.get_value::<u32, _>(receipt(file, bytes)?) {
        Ok(value) => Ok(value == 1),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub(in crate::engine) fn has_records() -> io::Result<bool> {
    let Some(key) = open_protected(STORE, false)? else { return Ok(false) };
    Ok(key.enum_values().next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_backup_receipt_binds_bytes_and_identity_but_allows_relocation() {
        let path = Path::new(r"C:\data\20261023-123456\0001.reg");
        let original = receipt(path, b"original").unwrap();
        assert_ne!(original, receipt(path, b"tampered").unwrap());
        assert_ne!(original, receipt(Path::new(r"C:\data\20261023-123456\0002.reg"), b"original").unwrap());
        assert_ne!(original, receipt(Path::new(r"C:\data\20261023-123457\0001.reg"), b"original").unwrap());
        assert_eq!(original, receipt(Path::new(r"D:\moved\20261023-123456\0001.reg"), b"original").unwrap());
    }

    #[test]
    fn security_backup_receipt_store_requires_privileged_owner_and_writers() {
        let protected = Descriptor::new(SECURITY).unwrap();
        check_descriptor(protected.0).unwrap();
        for text in [
            w!("O:BAG:BAD:P(A;;KA;;;WD)"),
            w!("O:BAG:BAD:P(A;;KW;;;BU)"),
            w!("O:BAG:BAD:P(A;;WD;;;BU)"),
            w!("O:BAG:BAD:P(A;;WO;;;BU)"),
            w!("O:BAG:BAD:P(A;;SD;;;BU)"),
            w!("O:BUG:BUD:P(A;;KR;;;BU)"),
            w!("O:BAG:BAD:PNO_ACCESS_CONTROL"),
            w!("O:BAG:BAD:(A;;KA;;;BA)(A;;KR;;;BU)"),
        ] {
            let unsafe_store = Descriptor::new(text).unwrap();
            assert!(check_descriptor(unsafe_store.0).is_err());
        }
    }

    #[test]
    fn security_backup_rejects_a_user_owned_registry_store() {
        let base = crate::engine::fsx::security_tests::fixture("restore-store");
        let path = format!("Software\\Classes\\{}", base.file_name().unwrap().to_string_lossy());
        let root = RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let (key, _) = root.create_subkey(&path).unwrap();
        let result = check_store(&key);
        drop(key);
        root.delete_subkey_all(&path).unwrap();
        assert!(result.is_err());
    }
}
