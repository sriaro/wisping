use std::path::PathBuf;

use super::{data, fsx, settings, sys};

const SHOWN: usize = 200;
const KEPT: usize = 600;

pub struct Entry {
    pub when: String,
    pub kind: String,
    pub a: u64,
    pub b: u64,
    pub c: u64,
}

fn path() -> PathBuf {
    data::root().join("history.txt")
}

pub fn record(kind: &str, a: u64, b: u64, c: u64) {
    if !settings::history_on() {
        return;
    }
    let s = sys::now_local();
    let line = format!("{:04}-{:02}-{:02} {:02}:{:02}\t{}\t{}\t{}\t{}\n", s.year, s.month, s.day, s.hour, s.minute, kind, a, b, c);
    data::ensure_root();
    let _ = fsx::PinnedDir::create(&data::root()).and_then(|dir| dir.append_lines("history.txt".as_ref(), &line, KEPT));
}

pub fn parse(line: &str) -> Option<Entry> {
    let mut p = line.split('\t');
    let when = p.next()?.to_string();
    let kind = p.next()?.to_string();
    let a = p.next()?.trim().parse().ok()?;
    let b = p.next()?.trim().parse().ok()?;
    let c = p.next()?.trim().parse().ok()?;
    if when.is_empty() || kind.is_empty() {
        return None;
    }
    Some(Entry { when, kind, a, b, c })
}

fn all() -> Vec<Entry> {
    data::read_text(&path(), 256 * 1024).map(|t| t.lines().filter_map(parse).collect()).unwrap_or_default()
}

pub fn list() -> Vec<Entry> {
    let mut v = all();
    v.reverse();
    v.truncate(SHOWN);
    v
}

pub fn total_freed() -> u64 {
    all().iter().filter(|e| e.kind == "clean").fold(0, |total, e| total.saturating_add(e.a))
}

pub fn clear() {
    let _ = fsx::PinnedDir::open(&data::root()).and_then(|dir| dir.remove_file("history.txt".as_ref()));
}
