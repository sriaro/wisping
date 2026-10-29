use super::*;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::FromRawHandle;
use windows::Win32::Storage::FileSystem::{
    CALLBACK_STREAM_SWITCH, COPY_FILE_COPY_SYMLINK,
    COPYPROGRESSROUTINE_PROGRESS, CopyFileExW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
    FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx, LPPROGRESS_ROUTINE_CALLBACK_REASON, PROGRESS_CANCEL,
    PROGRESS_CONTINUE, ReOpenFile,
};

fn windows_error(error: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0 & 0xffff)
}

fn identity(handle: HANDLE) -> io::Result<(u64, [u8; 16])> {
    let mut info = FILE_ID_INFO::default();
    unsafe { GetFileInformationByHandleEx(handle, FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(),
        std::mem::size_of::<FILE_ID_INFO>() as u32) }.map_err(windows_error)?;
    Ok((info.VolumeSerialNumber, info.FileId.Identifier))
}

fn reopen(file: &File, access: u32, share: u32, action: &str) -> io::Result<File> {
    let handle = unsafe { ReOpenFile(HANDLE(file.as_raw_handle()), access, FILE_SHARE_MODE(share),
        FILE_FLAGS_AND_ATTRIBUTES(OPEN_REPARSE_POINT)) }.map_err(|error| {
            let error = windows_error(error);
            io::Error::new(error.kind(), format!("{action}: {error}"))
        })?;
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

struct CopyState {
    source: (u64, [u8; 16]),
    destination: (u64, [u8; 16]),
    verified: bool,
    error: Option<io::Error>,
}

unsafe extern "system" fn copied_stream(
    _: i64, _: i64, _: i64, _: i64, _: u32, reason: LPPROGRESS_ROUTINE_CALLBACK_REASON,
    source: HANDLE, destination: HANDLE, context: *const c_void,
) -> COPYPROGRESSROUTINE_PROGRESS {
    let state = unsafe { &mut *context.cast_mut().cast::<CopyState>() };
    if reason != CALLBACK_STREAM_SWITCH { return PROGRESS_CONTINUE; }
    let result = (|| {
        if identity(source)? != state.source || identity(destination)? != state.destination {
            return Err(refused_path());
        }
        state.verified = true;
        Ok(())
    })();
    match result {
        Ok(()) => PROGRESS_CONTINUE,
        Err(error) => { state.error = Some(error); PROGRESS_CANCEL }
    }
}

fn same_contents(source: &mut File, backup: &mut File) -> io::Result<bool> {
    source.seek(SeekFrom::Start(0))?;
    backup.seek(SeekFrom::Start(0))?;
    if source.metadata()?.len() != backup.metadata()?.len() { return Ok(false); }
    let mut left = [0u8; 65536];
    let mut right = [0u8; 65536];
    loop {
        let count = source.read(&mut left)?;
        if count == 0 { return Ok(true); }
        backup.read_exact(&mut right[..count])?;
        if left[..count] != right[..count] { return Ok(false); }
    }
}

fn copy_path(directory: &PinnedDir, name: &OsStr) -> io::Result<Vec<u16>> {
    let mut path = final_path(directory.handle()).ok_or_else(refused_path)?;
    path.push(SEP);
    path.extend(name.encode_wide());
    path.push(0);
    Ok(path)
}

pub(super) fn run(source: &PinnedDir, name: &OsStr, target: &PinnedDir, before_delete: impl FnOnce()) -> io::Result<()> {
    child_name(name)?;
    let mut original = OpenOptions::new().read(true).share_mode(SHARE_READ | 4)
        .custom_flags(OPEN_REPARSE_POINT).open(source.path.join(name))?;
    source.plain_child(&original)?;
    let retained = OpenOptions::new().read(true).write(true).create_new(true).share_mode(SHARE_READ | 2)
        .custom_flags(OPEN_REPARSE_POINT).open(target.path.join(name))?;
    target.plain_child(&retained)?;
    let mut state = CopyState {
        source: identity(HANDLE(original.as_raw_handle()))?,
        destination: identity(HANDLE(retained.as_raw_handle()))?,
        verified: false,
        error: None,
    };
    let from = copy_path(source, name)?;
    let to = copy_path(target, name)?;
    let copied = unsafe { CopyFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), Some(copied_stream),
        Some((&mut state as *mut CopyState).cast()), None, COPY_FILE_COPY_SYMLINK) }.map_err(windows_error);
    if let Some(error) = state.error { return Err(error); }
    copied.map_err(|error| io::Error::new(error.kind(), format!("Backup copy failed: {error}")))?;
    if !state.verified { return Err(refused_path()); }
    retained.sync_all()?;
    let identity_pin = reopen(&retained, 0, SHARE_ALL, "Could not retain backup identity")?;
    drop(retained);
    let mut backup = reopen(&identity_pin, 0x8000_0000, SHARE_READ, "Could not protect backup")?;
    target.plain_child(&backup)?;
    let path = PathBuf::from(OsString::from_wide(&final_path(&backup).ok_or_else(refused_path)?));
    if path.file_name() != Some(name) { return Err(refused_path()); }
    let deletion = reopen(&original, DELETE | READ_ATTRIBUTES, SHARE_READ, "Could not protect startup file")?;
    source.plain_child(&deletion)?;
    if original.metadata()?.permissions().readonly() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "The startup file is read-only."));
    }
    if !same_contents(&mut original, &mut backup)
        .map_err(|error| io::Error::new(error.kind(), format!("Backup verification failed: {error}")))? {
        return Err(refused_path());
    }
    before_delete();
    if dispose(&deletion, false) { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fsx::security_tests::fixture;

    #[test]
    fn security_startup_holds_source_and_backup_until_deletion() {
        let base = fixture("startup-pinned-copy");
        let from = base.join("from");
        let to = base.join("to");
        fs::create_dir(&from).unwrap();
        fs::create_dir(&to).unwrap();
        let file = from.join("test.lnk");
        let backup = to.join("test.lnk");
        fs::write(&file, b"original").unwrap();
        let source = PinnedDir::open(&from).unwrap();
        let target = PinnedDir::open(&to).unwrap();
        run(&source, OsStr::new("test.lnk"), &target, || {
            assert!(fs::rename(&from, base.join("moved-source")).is_err());
            assert!(fs::rename(&to, base.join("moved-backup")).is_err());
            assert!(fs::rename(&file, from.join("replacement.lnk")).is_err());
            assert!(fs::write(&file, b"changed source").is_err());
            assert!(fs::rename(&backup, to.join("replacement.lnk")).is_err());
            assert!(fs::write(&backup, b"changed backup").is_err());
        }).unwrap();
        assert!(!file.exists());
        assert_eq!(fs::read(&backup).unwrap(), b"original");
    }

    #[test]
    fn security_startup_readonly_source_keeps_its_backup_and_file() {
        let base = fixture("startup-readonly-copy");
        let from = base.join("from");
        let to = base.join("to");
        fs::create_dir(&from).unwrap();
        fs::create_dir(&to).unwrap();
        let file = from.join("test.lnk");
        fs::write(&file, b"original").unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();
        let source = PinnedDir::open(&from).unwrap();
        let target = PinnedDir::open(&to).unwrap();
        assert!(source.backup_and_remove_file(OsStr::new("test.lnk"), &target).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(fs::read(to.join("test.lnk")).unwrap(), b"original");
    }
}
