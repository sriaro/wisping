use std::fs;
use std::io::Write;
use std::path::PathBuf;

use super::{data, settings, sys};

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
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path()) {
        let _ = f.write_all(line.as_bytes());
    }
    trim();
}

fn trim() {
    let Ok(text) = fs::read_to_string(path()) else { return };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > KEPT {
        let kept = lines[lines.len() - KEPT..].join("\n") + "\n";
        let _ = fs::write(path(), kept);
    }
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
    fs::read_to_string(path()).map(|t| t.lines().filter_map(parse).collect()).unwrap_or_default()
}

pub fn list() -> Vec<Entry> {
    let mut v = all();
    v.reverse();
    v.truncate(SHOWN);
    v
}

pub fn total_freed() -> u64 {
    all().iter().filter(|e| e.kind == "clean").map(|e| e.a).sum()
}

pub fn clear() {
    let _ = fs::remove_file(path());
}
