use std::fs;
use std::io;
use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::Globalization::{CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::core::PCWSTR;

use super::fsx;

const HEADER: usize = 0x4C;
const CLSID: [u8; 16] = [0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];
const HAS_ID_LIST: u32 = 0x01;
const HAS_LINK_INFO: u32 = 0x02;
const STRINGS: [u32; 5] = [0x04, 0x08, 0x10, 0x20, 0x40];
const HAS_RELATIVE_PATH: u32 = 0x08;
const IS_UNICODE: u32 = 0x80;
const LOCAL_PATH: u32 = 0x01;
const ENV_BLOCK: u32 = 0xA000_0001;
const ENV_BLOCK_SIZE: usize = 0x314;
const DRIVE_FIXED: u32 = 3;
const MAX_LINK: u64 = 1 << 20;

struct Link {
    env: Option<String>,
    info: Option<(String, bool)>,
    relative: Option<String>,
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn until_nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
}

fn narrow(bytes: &[u8]) -> Option<String> {
    let bytes = until_nul(bytes);
    if bytes.is_empty() {
        return None;
    }
    if bytes.is_ascii() {
        return String::from_utf8(bytes.to_vec()).ok();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(8);
    let need = unsafe { MultiByteToWideChar(CP_ACP, flags, bytes, None) };
    if need <= 0 {
        return None;
    }
    let mut wide = vec![0u16; need as usize];
    let got = unsafe { MultiByteToWideChar(CP_ACP, flags, bytes, Some(&mut wide)) };
    (got > 0).then(|| String::from_utf16_lossy(&wide[..got as usize]))
}

fn wide(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    (!units.is_empty()).then(|| String::from_utf16_lossy(&units))
}

fn info_path(info: &[u8]) -> Option<(String, bool)> {
    let head = u32_at(info, 4)? as usize;
    if u32_at(info, 8)? & LOCAL_PATH == 0 {
        return None;
    }
    let field = |n: usize| {
        let off = u32_at(info, n)? as usize;
        (off >= head && off < info.len()).then(|| &info[off..])
    };
    if head >= 0x24
        && let Some(base) = field(28).and_then(wide)
    {
        return Some((base + &field(32).and_then(wide).unwrap_or_default(), true));
    }
    let base = field(16)?;
    let suffix = field(24)?;
    let exact = until_nul(base).is_ascii() && until_nul(suffix).is_ascii();
    Some((narrow(base)? + &narrow(suffix).unwrap_or_default(), exact))
}

fn parse(data: &[u8]) -> Option<Link> {
    if data.len() < HEADER || u32_at(data, 0)? != HEADER as u32 || data[4..20] != CLSID {
        return None;
    }
    let flags = u32_at(data, 20)?;
    let mut at = HEADER;
    if flags & HAS_ID_LIST != 0 {
        at += 2 + u16_at(data, at)?;
    }
    let mut link = Link { env: None, info: None, relative: None };
    if flags & HAS_LINK_INFO != 0 {
        let size = u32_at(data, at)? as usize;
        link.info = info_path(data.get(at..at + size)?);
        at += size;
    }
    let unicode = flags & IS_UNICODE != 0;
    for bit in STRINGS {
        if flags & bit == 0 {
            continue;
        }
        let count = u16_at(data, at)? * if unicode { 2 } else { 1 };
        let bytes = data.get(at + 2..at + 2 + count)?;
        if bit == HAS_RELATIVE_PATH {
            link.relative = if unicode { wide(bytes) } else { narrow(bytes) };
        }
        at += 2 + count;
    }
    while let Some(size) = u32_at(data, at).map(|s| s as usize).filter(|&s| s >= 4) {
        if u32_at(data, at + 4)? == ENV_BLOCK && size >= ENV_BLOCK_SIZE {
            let block = data.get(at + 8..at + ENV_BLOCK_SIZE)?;
            link.env = wide(&block[260..780]).or_else(|| narrow(&block[..260]));
        }
        at += size;
    }
    Some(link)
}

fn reparse_ancestor(target: &Path) -> bool {
    let mut chain: Vec<&Path> = target.ancestors().skip(1).take_while(|a| a.parent().is_some()).collect();
    chain.reverse();
    chain.into_iter().any(|a| fs::symlink_metadata(a).is_ok_and(|m| fsx::attrs_reparse(m.file_attributes())))
}

fn missing(target: &Path) -> bool {
    let text = target.to_string_lossy();
    let b = text.as_bytes();
    if b.len() <= 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || !matches!(b[2], b'\\' | b'/') || text.contains('%') {
        return false;
    }
    let root = [b[0] as u16, b':' as u16, b'\\' as u16, 0];
    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
        return false;
    }
    if reparse_ancestor(target) {
        return false;
    }
    matches!(fs::symlink_metadata(target), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

pub fn targets(data: &[u8], dir: &Path) -> Vec<(PathBuf, bool)> {
    let Some(link) = parse(data) else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(env) = link.env {
        let full = fsx::expand_env(&env);
        if !full.contains('%') {
            out.push((PathBuf::from(full), true));
        }
    }
    if let Some((path, exact)) = link.info {
        out.push((PathBuf::from(path), exact));
    }
    if let Some(rel) = link.relative
        && let Some(full) = fsx::absolute(&dir.join(rel))
    {
        out.push((full, true));
    }
    out
}

pub fn broken(link: &Path) -> bool {
    if link.ancestors().any(|a| a.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Startup"))) {
        return false;
    }
    let Ok(file) = fsx::open_plain(link) else { return false };
    let Ok(meta) = file.metadata() else { return false };
    if meta.len() > MAX_LINK {
        return false;
    }
    let Some(dir) = link.parent() else { return false };
    let mut data = Vec::new();
    if file.take(MAX_LINK + 1).read_to_end(&mut data).is_err() || data.len() as u64 > MAX_LINK {
        return false;
    }
    let found = targets(&data, dir);
    found.iter().any(|(_, exact)| *exact) && found.iter().all(|(p, _)| missing(p))
}
