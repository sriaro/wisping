use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};

use crate::engine::lock::LockExt;
use super::{Kind, Tree};
use crate::engine::fsx;

const MIN_SIZE: u64 = 1024 * 1024;
const EDGE: usize = 64 * 1024;
const CHUNK: usize = 1024 * 1024;

pub struct Group {
    pub size: u64,
    pub members: Vec<u32>,
}

impl Group {
    pub fn wasted(&self) -> u64 {
        self.size * (self.members.len() as u64 - 1)
    }
}

fn file_id(f: &File) -> Option<u128> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info).ok()? };
    Some(((info.dwVolumeSerialNumber as u128) << 64) | ((info.nFileIndexHigh as u128) << 32) | info.nFileIndexLow as u128)
}

fn open(path: &std::path::Path, size: u64) -> Option<File> {
    let file = fsx::open_plain(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() != size || fsx::attrs_cloud(metadata.file_attributes()) || fsx::attrs_reparse(metadata.file_attributes()) {
        return None;
    }
    Some(file)
}

type Digest = [u8; 32];

fn partial_hash(path: &std::path::Path, size: u64, buf: &mut [u8]) -> Option<(Digest, u128)> {
    let mut f = open(path, size)?;
    let id = file_id(&f)?;
    let edge = &mut buf[..EDGE];
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    f.read_exact(edge).ok()?;
    h.update(edge);
    f.seek(SeekFrom::Start(size.saturating_sub(EDGE as u64))).ok()?;
    f.read_exact(edge).ok()?;
    h.update(edge);
    Some((*h.finalize().as_bytes(), id))
}

fn full_hash(path: &std::path::Path, size: u64, cancel: &AtomicBool, buf: &mut [u8]) -> Option<Digest> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut f = open(path, size)?;
    let mut h = blake3::Hasher::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = f.read(buf).ok()?;
        if n == 0 {
            return Some(*h.finalize().as_bytes());
        }
        h.update(&buf[..n]);
    }
}

fn split(tree: &Tree, ids: &[u32], cancel: &AtomicBool, mut hash: impl FnMut(&std::path::Path) -> Option<(Digest, u128)>) -> Vec<Vec<u32>> {
    let mut map: HashMap<Digest, Vec<u32>> = HashMap::new();
    let mut linked: HashSet<u128> = HashSet::new();
    for &id in ids {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Some((h, file)) = hash(&tree.path_of(id))
            && (file == 0 || linked.insert(file)) {
                map.entry(h).or_default().push(id);
            }
    }
    map.into_values().filter(|v| v.len() > 1).collect()
}

pub fn find(tree: &Tree, cancel: &AtomicBool) -> Vec<Group> {
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for (i, n) in tree.nodes.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        if n.kind == Kind::File && n.size >= MIN_SIZE {
            by_size.entry(n.size).or_default().push(i as u32);
        }
    }
    let mut candidates: Vec<(u64, Vec<u32>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    if candidates.is_empty() || cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }
    candidates.sort_by_key(|(s, v)| std::cmp::Reverse(s.saturating_mul(v.len() as u64)));
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Group>> = Mutex::new(Vec::new());
    let workers = crate::engine::cpu::compute_threads().div_ceil(2).min(candidates.len().max(1));
    crate::engine::pool::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                crate::engine::cpu::eco();
                let mut buf = vec![0u8; CHUNK];
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= candidates.len() || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let (size, ids) = &candidates[k];
                    for part in split(tree, ids, cancel, |p| partial_hash(p, *size, &mut buf)) {
                        for same in split(tree, &part, cancel, |p| full_hash(p, *size, cancel, &mut buf).map(|h| (h, 0))) {
                            out.locked().push(Group { size: *size, members: same });
                        }
                    }
                }
            });
        }
    });
    let mut groups = out.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    groups.sort_by_key(|g| std::cmp::Reverse(g.wasted()));
    groups
}
