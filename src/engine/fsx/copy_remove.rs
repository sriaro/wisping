use super::*;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::FromRawHandle;
use windows::Win32::Storage::FileSystem::{
    CALLBACK_STREAM_SWITCH, COPY_FILE_COPY_SYMLINK,
    COPYPROGRESSROUTINE_PROGRESS, CopyFileExW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE,
    LPPROGRESS_ROUTINE_CALLBACK_REASON, PROGRESS_CANCEL,
    PROGRESS_CONTINUE, ReOpenFile,
};

fn reopen(file: &File, access: u32, share: u32) -> io::Result<File> {
    let handle = unsafe { ReOpenFile(HANDLE(file.as_raw_handle()), access, FILE_SHARE_MODE(share),
        FILE_FLAGS_AND_ATTRIBUTES(OPEN_REPARSE_POINT)) }.map_err(windows_error)?;
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

pub(super) fn run(source: &PinnedDir, name: &OsStr, target: &PinnedDir, kept: &OsStr, seal: impl FnOnce(&Path, &mut File) -> io::Result<()>) -> io::Result<()> {
    child_name(name)?;
    child_name(kept)?;
    let mut original = OpenOptions::new().read(true).share_mode(SHARE_READ | 4)
        .custom_flags(OPEN_REPARSE_POINT).open(source.path.join(name))?;
    source.plain_child(&original)?;
    if original.metadata()?.permissions().readonly() {
        return Err(io::Error::other("readonly"));
    }
    let retained =OpenOptions::new().read(true).write(true).create_new(true).share_mode(SHARE_READ | 2)
        .custom_flags(OPEN_REPARSE_POINT).open(target.path.join(kept))?;
    target.plain_child(&retained)?;
    let mut state = CopyState {
        source: identity(HANDLE(original.as_raw_handle()))?,
        destination: identity(HANDLE(retained.as_raw_handle()))?,
        verified: false,
        error: None,
    };
    let from = copy_path(source, name)?;
    let to = copy_path(target, kept)?;
    let copied = unsafe { CopyFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), Some(copied_stream),
        Some((&mut state as *mut CopyState).cast()), None, COPY_FILE_COPY_SYMLINK) }.map_err(windows_error);
    if let Some(error) = state.error { return Err(error); }
    copied?;
    if !state.verified { return Err(refused_path()); }
    retained.sync_all()?;
    let identity_pin = reopen(&retained, 0, SHARE_ALL)?;
    drop(retained);
    let mut backup = reopen(&identity_pin, 0x8000_0000, SHARE_READ)?;
    target.plain_child(&backup)?;
    let path = PathBuf::from(OsString::from_wide(&final_path(&backup).ok_or_else(refused_path)?));
    if path.file_name() != Some(kept) { return Err(refused_path()); }
    let deletion = reopen(&original, DELETE | READ_ATTRIBUTES, SHARE_READ)?;
    source.plain_child(&deletion)?;
    if !same_contents(&mut original, &mut backup)? {
        return Err(refused_path());
    }
    backup.seek(SeekFrom::Start(0))?;
    seal(&path, &mut backup)?;
    if dispose(&deletion, false) { Ok(()) } else { Err(io::Error::last_os_error()) }
}
