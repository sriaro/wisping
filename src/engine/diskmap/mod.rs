pub mod dups;
pub mod owners;
pub mod treemap;

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use super::fsx;
use super::lock::LockExt;

pub const NONE: u32 = u32::MAX;
pub const CATS: usize = 8;
const CAT_SPAN: u32 = 256;
const BIG_FILE: u64 = 128 * 1024;
const CHUNK_BITS: u32 = 20;
const CHUNK: usize = 1 << CHUNK_BITS;
const MAX_CHUNKS: usize = 1 << (32 - CHUNK_BITS);

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Dir,
    File,
    Small,
}

#[derive(Clone, Copy)]
pub struct Node {
    pub size: u64,
    pub off: u32,
    pub parent: u32,
    pub first: u32,
    pub count: u32,
    pub files: u32,
    pub cat: u32,
    pub len: u16,
    pub kind: Kind,
}

#[derive(Default)]
pub struct Names {
    chunks: Vec<Vec<u8>>,
}

impl Names {
    fn push(&mut self, s: &str) -> (u32, u16) {
        let bytes = s.as_bytes();
        let mut len = bytes.len().min(u16::MAX as usize);
        while !s.is_char_boundary(len) {
            len -= 1;
        }
        if self.chunks.last().is_none_or(|c| c.len() + len >= CHUNK) {
            if self.chunks.len() >= MAX_CHUNKS {
                return (0, 0);
            }
            self.chunks.push(Vec::with_capacity(CHUNK));
        }
        let ci = self.chunks.len() - 1;
        let chunk = &mut self.chunks[ci];
        let pos = chunk.len();
        chunk.extend_from_slice(&bytes[..len]);
        (((ci as u32) << CHUNK_BITS) | pos as u32, len as u16)
    }

    fn get(&self, off: u32, len: u16) -> &str {
        let ci = (off >> CHUNK_BITS) as usize;
        let pos = (off & (CHUNK as u32 - 1)) as usize;
        self.chunks.get(ci).and_then(|c| c.get(pos..pos + len as usize)).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }

    fn seal(&mut self) {
        if let Some(last) = self.chunks.last_mut() {
            last.shrink_to_fit();
        }
        self.chunks.shrink_to_fit();
    }
}

#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
}

pub struct Tree {
    pub nodes: Vec<Node>,
    dir_cats: Vec<[u64; CATS]>,
    names: Names,
    pub root: PathBuf,
}

pub fn category_of(name: &str) -> usize {
    let ext = match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => return 7,
    };
    match ext.as_str() {
        "rs" | "c" | "h" | "cpp" | "hpp" | "cs" | "py" | "js" | "ts" | "tsx" | "jsx" | "java" | "go" | "json" | "xml" | "html" | "css" | "pdb" | "obj" | "lib" | "o" | "class" | "map" | "pyc" | "rlib" | "rmeta" => 0,
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "pdf" | "txt" | "md" | "csv" | "rtf" | "odt" | "epub" | "log" => 1,
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "wma" | "opus" | "mid" | "bank" | "wem" => 2,
        "zip" | "rar" | "7z" | "gz" | "tar" | "xz" | "bz2" | "iso" | "cab" | "msi" | "wim" | "zst" | "vhd" | "vhdx" | "img" => 3,
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "flv" | "m4v" | "ts_" | "bik" | "usm" => 4,
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "psd" | "svg" | "raw" | "heic" | "dds" | "tga" | "exr" => 5,
        "exe" | "dll" | "sys" | "ocx" | "so" | "dylib" | "node" | "ax" | "scr" | "drv" | "mui" | "winmd" => 6,
        _ => 7,
    }
}

impl Tree {
    pub fn name(&self, n: &Node) -> &str {
        self.names.get(n.off, n.len)
    }

    pub fn name_of(&self, idx: u32) -> &str {
        self.name(&self.nodes[idx as usize])
    }

    pub fn cats(&self, idx: u32) -> [u64; CATS] {
        let n = &self.nodes[idx as usize];
        if n.kind == Kind::Dir && n.cat != NONE {
            return self.dir_cats[n.cat as usize];
        }
        let mut c = [0u64; CATS];
        add_cats(&self.nodes, &self.names, idx, &mut c);
        c
    }

    pub fn path_of(&self, idx: u32) -> PathBuf {
        let mut parts: Vec<&str> = Vec::new();
        let mut i = idx;
        while i != NONE && i != 0 {
            let n = &self.nodes[i as usize];
            parts.push(self.name(n));
            i = n.parent;
        }
        let mut p = self.root.clone();
        for part in parts.iter().rev() {
            p.push(part);
        }
        p
    }

    pub fn children(&self, idx: u32) -> impl Iterator<Item = u32> + '_ {
        let n = &self.nodes[idx as usize];
        n.first..n.first + n.count
    }

    pub fn sorted_children(&self, idx: u32) -> Vec<u32> {
        let mut v: Vec<u32> = self.children(idx).collect();
        v.sort_unstable_by(|a, b| self.nodes[*b as usize].size.cmp(&self.nodes[*a as usize].size).then(a.cmp(b)));
        v
    }

    pub fn ancestors(&self, idx: u32) -> Vec<u32> {
        let mut v = Vec::new();
        let mut i = idx;
        while i != NONE {
            v.push(i);
            i = self.nodes[i as usize].parent;
        }
        v.reverse();
        v
    }

    pub fn dominant(&self, idx: u32) -> usize {
        let c = self.cats(idx);
        (0..CATS).max_by_key(|&i| c[i]).unwrap_or(7)
    }

    pub fn largest_files(&self, limit: usize, cancel: &AtomicBool) -> Vec<u32> {
        if limit == 0 {
            return Vec::new();
        }
        let mut largest = BinaryHeap::with_capacity(limit);
        for (i, node) in self.nodes.iter().enumerate().skip(1) {
            if i.is_multiple_of(1024) && cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            if node.kind != Kind::File {
                continue;
            }
            let candidate = (Reverse(node.size), i as u32);
            if largest.len() < limit {
                largest.push(candidate);
            } else if let Some(mut smallest) = largest.peek_mut()
                && candidate < *smallest {
                    *smallest = candidate;
                }
        }
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let mut result: Vec<u32> = largest.into_iter().map(|(_, i)| i).collect();
        result.sort_unstable_by(|a, b| self.nodes[*b as usize].size.cmp(&self.nodes[*a as usize].size).then(a.cmp(b)));
        result
    }
}

struct Task {
    node: u32,
    path: PathBuf,
}

struct Shared<'a> {
    nodes: Mutex<(Vec<Node>, Names)>,
    queue: Mutex<Vec<Task>>,
    cv: Condvar,
    pending: AtomicUsize,
    progress: &'a Progress,
    linked: Mutex<HashSet<u128>>,
}

struct Listing {
    text: String,
    dirs: Vec<(u32, u32)>,
    big: Vec<(u32, u32, u64)>,
    small_files: u32,
    small_bytes: u64,
}

impl Listing {
    fn name(&self, start: u32, len: u32) -> &str {
        &self.text[start as usize..(start + len) as usize]
    }
}

fn first_link(path: &Path, linked: &Mutex<HashSet<u128>>) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
    let Ok(file) = std::fs::OpenOptions::new().access_mode(0x80).share_mode(7).custom_flags(0x0020_0000).open(path) else { return true };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.is_err() || info.nNumberOfLinks <= 1 {
        return true;
    }
    let id = ((info.dwVolumeSerialNumber as u128) << 64) | ((info.nFileIndexHigh as u128) << 32) | info.nFileIndexLow as u128;
    linked.locked().insert(id)
}

fn list_dir(path: &Path, progress: &Progress, linked: &Mutex<HashSet<u128>>) -> Option<Listing> {
    let rd = std::fs::read_dir(path).ok()?;
    let mut l = Listing { text: String::new(), dirs: Vec::new(), big: Vec::new(), small_files: 0, small_bytes: 0 };
    let (mut files, mut bytes) = (0u64, 0u64);
    for entry in rd.flatten() {
        if progress.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(md) = entry.metadata() else { continue };
        if md.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let start = l.text.len() as u32;
        l.text.push_str(&name);
        let len = name.len() as u32;
        if md.is_dir() {
            l.dirs.push((start, len));
        } else {
            let mut size = if fsx::attrs_cloud(md.file_attributes()) { 0 } else { md.len() };
            if size >= BIG_FILE && !first_link(&entry.path(), linked) {
                size = 0;
            }
            files += 1;
            bytes += size;
            if size >= BIG_FILE {
                l.big.push((start, len, size));
            } else {
                l.small_files += 1;
                l.small_bytes += size;
                l.text.truncate(start as usize);
            }
        }
    }
    progress.files.fetch_add(files, Ordering::Relaxed);
    progress.bytes.fetch_add(bytes, Ordering::Relaxed);
    l.text.shrink_to_fit();
    Some(l)
}

fn blank(off: u32, len: u16, parent: u32, kind: Kind) -> Node {
    Node { size: 0, off, parent, first: 0, count: 0, files: 0, cat: 0, len, kind }
}

fn small_label(count: u32) -> String {
    if count == 1 { "1 small file".to_string() } else { format!("{} small files", count) }
}

fn work(sh: &Shared) {
    loop {
        let task = {
            let mut q = sh.queue.locked();
            loop {
                if sh.progress.cancel.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(t) = q.pop() {
                    break t;
                }
                if sh.pending.load(Ordering::SeqCst) == 0 {
                    return;
                }
                q = sh.cv.wait_timeout(q, std::time::Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        let listing = list_dir(&task.path, sh.progress, &sh.linked);
        let mut next = Vec::new();
        if let Some(l) = listing {
            let mut guard = sh.nodes.locked();
            let (nodes, names) = &mut *guard;
            let first = nodes.len() as u32;
            nodes.reserve(l.dirs.len() + l.big.len() + 1);
            for &(s, len) in &l.dirs {
                let (off, n) = names.push(l.name(s, len));
                nodes.push(blank(off, n, task.node, Kind::Dir));
            }
            for &(s, len, size) in &l.big {
                let (off, n) = names.push(l.name(s, len));
                let mut node = blank(off, n, task.node, Kind::File);
                node.size = size;
                node.files = 1;
                nodes.push(node);
            }
            if l.small_files > 0 {
                let (off, n) = names.push(&small_label(l.small_files));
                let mut node = blank(off, n, task.node, Kind::Small);
                node.size = l.small_bytes;
                node.files = l.small_files;
                nodes.push(node);
            }
            let count = nodes.len() as u32 - first;
            let parent = &mut nodes[task.node as usize];
            parent.first = first;
            parent.count = count;
            drop(guard);
            next.reserve(l.dirs.len());
            for (i, &(s, len)) in l.dirs.iter().enumerate() {
                next.push(Task { node: first + i as u32, path: task.path.join(l.name(s, len)) });
            }
        }
        if !next.is_empty() {
            sh.pending.fetch_add(next.len(), Ordering::SeqCst);
            sh.queue.locked().extend(next);
            sh.cv.notify_all();
        }
        if sh.pending.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _guard = sh.queue.locked();
            sh.cv.notify_all();
        }
    }
}

pub fn scan(root: &Path, progress: &Progress) -> Option<Tree> {
    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut names = Names::default();
    let root_name = root.to_string_lossy();
    let (off, len) = names.push(&root_name);
    let sh = Shared {
        nodes: Mutex::new((vec![blank(off, len, NONE, Kind::Dir)], names)),
        queue: Mutex::new(vec![Task { node: 0, path: root.to_path_buf() }]),
        cv: Condvar::new(),
        pending: AtomicUsize::new(1),
        progress,
        linked: Mutex::new(HashSet::new()),
    };
    let workers = (super::cleaner::worker_count() * 2).min(crate::engine::pool::BULK_WORKERS);
    crate::engine::pool::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                crate::engine::cpu::urgent();
                work(&sh)
            });
        }
    });
    if progress.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let (mut nodes, mut names) = sh.nodes.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    nodes.shrink_to_fit();
    names.seal();
    let mut weight = vec![1u32; nodes.len()];
    for i in (1..nodes.len()).rev() {
        if i.is_multiple_of(1024) && progress.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (parent, size, files) = (nodes[i].parent as usize, nodes[i].size, nodes[i].files);
        nodes[parent].size += size;
        nodes[parent].files += files;
        weight[parent] = weight[parent].saturating_add(weight[i]);
    }
    let mut big = 0u32;
    for (i, n) in nodes.iter_mut().enumerate() {
        if i.is_multiple_of(1024) && progress.cancel.load(Ordering::Relaxed) {
            return None;
        }
        n.cat = if n.kind == Kind::Dir && (i == 0 || weight[i] >= CAT_SPAN) {
            big += 1;
            big - 1
        } else {
            NONE
        };
    }
    drop(weight);
    let mut dir_cats = vec![[0u64; CATS]; big as usize];
    for i in (0..nodes.len()).rev() {
        if i.is_multiple_of(1024) && progress.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = nodes[i];
        if n.cat == NONE {
            continue;
        }
        let mut c = [0u64; CATS];
        for child in n.first..n.first + n.count {
            let m = &nodes[child as usize];
            match m.kind {
                Kind::Dir if m.cat != NONE => {
                    for (sum, part) in c.iter_mut().zip(dir_cats[m.cat as usize]) {
                        *sum += part;
                    }
                }
                _ => add_cats(&nodes, &names, child, &mut c),
            }
        }
        dir_cats[n.cat as usize] = c;
    }
    (!progress.cancel.load(Ordering::Relaxed)).then_some(Tree { nodes, dir_cats, names, root: root.to_path_buf() })
}

fn add_cats(nodes: &[Node], names: &Names, idx: u32, c: &mut [u64; CATS]) {
    let mut stack = vec![idx];
    while let Some(i) = stack.pop() {
        let n = &nodes[i as usize];
        match n.kind {
            Kind::File => c[category_of(names.get(n.off, n.len))] += n.size,
            Kind::Small => c[7] += n.size,
            Kind::Dir => stack.extend(n.first..n.first + n.count),
        }
    }
}
