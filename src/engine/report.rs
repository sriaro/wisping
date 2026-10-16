use std::fs;
use std::path::PathBuf;

use super::cleaner::CleanTarget;
use super::{data, sys};

pub struct Line<'a> {
    pub name: String,
    pub before: &'a CleanTarget,
    pub after: &'a CleanTarget,
}

pub fn report_root() -> PathBuf {
    data::root().join("reports")
}

pub fn write_clean(lines: &[Line]) -> Option<PathBuf> {
    let s = sys::now_local();
    data::ensure_root();
    fs::create_dir_all(report_root()).ok()?;
    let path = report_root().join(format!("clean-{}.txt", sys::stamp_name()));
    let mut out = String::new();
    out.push_str(&format!("Wisp - clean report {:04}-{:02}-{:02} {:02}:{:02}:{:02}\r\n\r\n", s.year, s.month, s.day, s.hour, s.minute, s.second));
    out.push_str("How cleaning works\r\n");
    out.push_str("- Only files inside the locations listed below are deleted, with the masks shown.\r\n");
    out.push_str("- Temp locations keep every file newer than the age shown. Junctions and symbolic links are never followed.\r\n");
    out.push_str("- Files are deleted for good (not moved to the Recycle Bin). Files in use are skipped.\r\n");
    out.push_str("- After cleaning, every selected location is scanned again; the remaining size below is that new scan, not an estimate.\r\n\r\n");
    let before_bytes: u64 = lines.iter().map(|l| l.before.bytes).sum();
    let before_count: u64 = lines.iter().map(|l| l.before.count).sum();
    let freed: u64 = lines.iter().map(|l| l.after.freed).sum();
    let failed: u64 = lines.iter().map(|l| l.after.failed).sum();
    let left: u64 = lines.iter().map(|l| l.after.bytes).sum();
    let left_count: u64 = lines.iter().map(|l| l.after.count).sum();
    out.push_str(&format!("Totals\r\n  Before: {} in {} files\r\n  Deleted: {}\r\n  Skipped or failed: {} files\r\n  Left after a new scan: {} in {} files\r\n\r\n", data::format_size(before_bytes), before_count, data::format_size(freed), failed, data::format_size(left), left_count));
    for l in lines {
        out.push_str(&format!("[{}]\r\n", l.name));
        if l.before.recycle {
            out.push_str("  Location: Recycle Bin of every drive (emptied through Windows)\r\n");
        }
        for key in &l.before.reg {
            match key.split_once('|') {
                Some((k, v)) => out.push_str(&format!("  Location: HKCU\\{}  (only the value \"{}\", the key is kept)\r\n", k, v)),
                None => out.push_str(&format!("  Location: HKCU\\{}  (values only, keys are kept)\r\n", key)),
            }
        }
        let admin = sys::is_admin();
        for src in &l.before.sources {
            let hours = super::cleaner::effective_age(src);
            let age = if hours > 0 { format!(", keeps files newer than {} h", hours) } else { String::new() };
            let skipped = if src.admin && !admin { ", skipped without administrator rights" } else { "" };
            out.push_str(&format!("  Location: {}  mask {}{}{}{}\r\n", src.dir, src.mask, if src.recurse { ", subfolders included" } else { "" }, age, skipped));
        }
        out.push_str(&format!("  Before: {} in {} files\r\n  Deleted: {} in {} items\r\n  Skipped or failed: {}\r\n  Left after a new scan: {} in {} items\r\n", data::format_size(l.before.bytes), l.before.count, data::format_size(l.after.freed), l.after.removed, l.after.failed, data::format_size(l.after.bytes), l.after.count));
        for p in &l.after.fail_samples {
            out.push_str(&format!("    skipped: {}\r\n", p));
        }
        out.push_str("\r\n");
    }
    fs::write(&path, out).ok()?;
    Some(path)
}
