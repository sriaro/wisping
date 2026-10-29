use std::io;

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::System::Registry::{KeyNameInformation, NtCreateKey, NtDeleteKey, NtOpenKey, NtQueryKey};
use windows::Win32::Foundation::{HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, OBJ_OPENLINK, RtlNtStatusToDosError, UNICODE_STRING};
use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
use windows::Win32::System::Registry::{HKEY, RegOpenCurrentUser};
use windows::core::PWSTR;
use winreg::RegKey;
use winreg::enums::{KEY_CREATE_SUB_KEY, KEY_READ, KEY_WOW64_64KEY, REG_LINK};

pub(in crate::engine) const DELETE: u32 = 0x0001_0000;

#[cfg(test)]
mod tests;

fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "A registry target is a link or has changed.")
}

fn nt_result(status: NTSTATUS) -> io::Result<()> {
    if status.0 < 0 {
        Err(io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32))
    } else {
        Ok(())
    }
}

pub(in crate::engine) fn open_root(machine: bool, flags: u32) -> io::Result<RegKey> {
    if !machine {
        let mut key = HKEY::default();
        let status = unsafe { RegOpenCurrentUser(flags, &mut key) };
        return if status.0 == 0 { Ok(RegKey::predef(key.0)) } else { Err(io::Error::from_raw_os_error(status.0 as i32)) };
    }
    let mut wide: Vec<u16> = "\\Registry\\Machine".encode_utf16().collect();
    let length = (wide.len() * 2) as u16;
    let name = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: PWSTR(wide.as_mut_ptr()) };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let mut key = HANDLE::default();
    let status = unsafe { NtOpenKey(&mut key, flags, &attributes) };
    nt_result(status)?;
    let key = RegKey::predef(key.0);
    if !same(&native_name(&key)?, "\\Registry\\Machine") {
        return Err(refused());
    }
    Ok(key)
}

pub(in crate::engine) fn same(a: &str, b: &str) -> bool {
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    unsafe { CompareStringOrdinal(&a, &b, true) == CSTR_EQUAL }
}

pub(in crate::engine) fn native_name(key: &RegKey) -> io::Result<String> {
    let mut words = vec![0u32; 128];
    loop {
        let mut needed = 0;
        let status = unsafe { NtQueryKey(HANDLE(key.raw_handle()), KeyNameInformation,
            Some(words.as_mut_ptr().cast()), (words.len() * 4) as u32, &mut needed) };
        if status.0 >= 0 {
            let length = words[0] as usize;
            if !length.is_multiple_of(2) || length > words.len() * 4 - 4 {
                return Err(refused());
            }
            let name = unsafe { std::slice::from_raw_parts(words.as_ptr().add(1).cast::<u16>(), length / 2) };
            return String::from_utf16(name).map_err(io::Error::other);
        }
        if needed as usize <= words.len() * 4 || needed > 65536 {
            return Err(refused());
        }
        words.resize((needed as usize).div_ceil(4), 0);
    }
}

pub(in crate::engine) fn open_child(parent: &RegKey, part: &str, flags: u32) -> io::Result<RegKey> {
    child(parent, part, flags, false)
}

fn child(parent: &RegKey, part: &str, flags: u32, create: bool) -> io::Result<RegKey> {
    if part.is_empty() || part.contains(['\\', '\0']) || part == "." || part == ".." {
        return Err(refused());
    }
    let expected = format!("{}\\{part}", native_name(parent)?);
    let mut wide: Vec<u16> = part.encode_utf16().collect();
    let length = u16::try_from(wide.len() * 2).map_err(|_| refused())?;
    let name = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: PWSTR(wide.as_mut_ptr()) };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: HANDLE(parent.raw_handle()),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_OPENLINK,
        ..Default::default()
    };
    let mut handle = HANDLE::default();
    let status = unsafe {
        if create { NtCreateKey(&mut handle, flags, &attributes, None, None, 0, None) }
        else { NtOpenKey(&mut handle, flags, &attributes) }
    };
    nt_result(status)?;
    let child = RegKey::predef(handle.0);
    if !same(&native_name(&child)?, &expected) {
        return Err(refused());
    }
    match child.get_raw_value("SymbolicLinkValue") {
        Ok(value) if value.vtype == REG_LINK => return Err(refused()),
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    Ok(child)
}

pub(in crate::engine) fn open(machine: bool, path: &str, flags: u32, create: bool) -> io::Result<RegKey> {
    let parts: Vec<&str> = path.split('\\').collect();
    if parts.iter().any(|part| part.is_empty() || part.contains('\0') || *part == "." || *part == "..") { return Err(refused()); }
    let traversal = KEY_READ | KEY_WOW64_64KEY | if create { KEY_CREATE_SUB_KEY } else { 0 };
    let mut key = open_root(machine, traversal)?;
    let root_name = native_name(&key)?;
    let mut logical = String::new();
    for (index, part) in parts.iter().copied().enumerate() {
        let access = if index + 1 == parts.len() { flags } else { traversal };
        if !logical.is_empty() { logical.push('\\'); }
        logical.push_str(part);
        let alias = if machine && logical.eq_ignore_ascii_case("SYSTEM\\CurrentControlSet") {
            let system = open_child(&open_root(true, KEY_READ | KEY_WOW64_64KEY)?, "SYSTEM", KEY_READ | KEY_WOW64_64KEY)?;
            let select = open_child(&system, "Select", KEY_READ | KEY_WOW64_64KEY)?;
            let current: u32 = select.get_value("Current")?;
            Some(format!("{root_name}\\SYSTEM\\ControlSet{current:03}"))
        } else if machine && ["AppID", "PROTOCOLS", "TypeLib"].iter().any(|alias|
            logical.eq_ignore_ascii_case(&format!("SOFTWARE\\Classes\\Wow6432Node\\{alias}"))) {
            Some(format!("{root_name}\\SOFTWARE\\Classes\\{part}"))
        } else if !machine && logical.eq_ignore_ascii_case("Software\\Classes") {
            Some(format!("{root_name}_Classes"))
        } else {
            None
        };
        key = if let Some(alias) = alias {
            match key.open_subkey_with_flags(part, access) {
                Ok(next) => {
                    let actual = native_name(&next)?;
                    let direct = format!("{}\\{part}", native_name(&key)?);
                    if !same(&actual, &alias) && (machine || !same(&actual, &direct)) { return Err(refused()); }
                    next
                }
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => child(&key, part, access, true)?,
                Err(error) => return Err(error),
            }
        } else {
            child(&key, part, access, create)?
        };
    }
    Ok(key)
}

pub(in crate::engine) struct Tree {
    entries: Vec<(String, RegKey)>,
}

impl Tree {
    pub(in crate::engine) fn open(key: RegKey, flags: u32) -> io::Result<Self> {
        let mut entries = vec![(String::new(), key)];
        let mut index = 0;
        while index < entries.len() {
            let (path, key) = &entries[index];
            let mut children = Vec::new();
            for name in key.enum_keys() {
                let name = name?;
                let child = open_child(key, &name, flags)?;
                let path = if path.is_empty() { name } else { format!("{path}\\{name}") };
                children.push((path, child));
                if entries.len() + children.len() > 65536 { return Err(refused()); }
            }
            entries.extend(children);
            index += 1;
        }
        Ok(Self { entries })
    }

    pub(in crate::engine) fn entries(&self) -> &[(String, RegKey)] { &self.entries }

    pub(in crate::engine) fn delete(self) -> io::Result<()> {
        for (_, key) in self.entries.into_iter().rev() {
            delete_key(&key)?;
        }
        Ok(())
    }
}

pub(in crate::engine) fn delete_key(key: &RegKey) -> io::Result<()> {
    nt_result(unsafe { NtDeleteKey(HANDLE(key.raw_handle())) })
}
