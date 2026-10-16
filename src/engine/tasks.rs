use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::data;
use super::fileinfo;
use super::fsx;
use super::registry;
use super::startup::{StartupEntry, StartupKind};
use super::sys;
use super::regbackup::Hive;

const MAX_DEPTH: usize = 5;
const MAX_FILE: u64 = 1024 * 1024;

pub fn tasks_root() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string())).join("System32\\Tasks")
}

pub fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(body).into_owned()
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn block<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{}", name);
    let mut from = 0;
    while let Some(rel) = xml[from..].find(&open) {
        let start = from + rel;
        let after = start + open.len();
        match xml[after..].chars().next() {
            Some('>') | Some(' ') | Some('/') | Some('\r') | Some('\n') | Some('\t') => {}
            _ => {
                from = after;
                continue;
            }
        }
        let gt = xml[start..].find('>')? + start;
        if xml[start..gt].ends_with('/') {
            return Some("");
        }
        let close = format!("</{}>", name);
        let end = xml[gt..].find(&close)? + gt;
        return Some(&xml[gt + 1..end]);
    }
    None
}

fn text(xml: &str, name: &str) -> Option<String> {
    let t = unescape(block(xml, name)?.trim());
    if t.is_empty() { None } else { Some(t) }
}

pub struct Parsed {
    pub command: String,
    pub author: String,
    pub enabled: bool,
}

pub fn parse(xml: &str) -> Parsed {
    let exec = block(xml, "Exec").unwrap_or("");
    let mut command = text(exec, "Command").unwrap_or_default();
    if let Some(args) = text(exec, "Arguments") {
        command = format!("{} {}", command, args).trim().to_string();
    }
    if command.is_empty()
        && let Some(handler) = block(xml, "ComHandler") {
            command = text(handler, "ClassId").map(|c| format!("COM {}", c)).unwrap_or_default();
        }
    let author = block(xml, "RegistrationInfo").and_then(|r| text(r, "Author")).unwrap_or_default();
    let enabled = block(xml, "Settings").and_then(|s| text(s, "Enabled")).map(|v| !v.eq_ignore_ascii_case("false")).unwrap_or(true);
    Parsed { command, author, enabled }
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if fsx::attrs_reparse(meta.file_attributes()) {
            continue;
        }
        let task = format!("{}\\{}", rel, e.file_name().to_string_lossy());
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                walk(&e.path(), &task, depth + 1, out);
            }
        } else if meta.is_file() && meta.len() <= MAX_FILE {
            out.push((task, e.path()));
        }
    }
}

fn is_system_path(task: &str) -> bool {
    task.to_lowercase().starts_with("\\microsoft\\windows\\")
}

fn entry(task: &str, path: &Path) -> Option<StartupEntry> {
    let xml = decode(&fs::read(path).ok()?);
    if !xml.contains("<Task") {
        return None;
    }
    let parsed = parse(&xml);
    let exe = registry::first_path(&parsed.command);
    let publisher = exe.as_deref().map(fileinfo::publisher).filter(|p| !p.is_empty()).unwrap_or(parsed.author);
    let leaf = task.rsplit('\\').next().unwrap_or(task).to_string();
    Some(StartupEntry {
        kind: StartupKind::Task,
        name: leaf.clone(),
        command: parsed.command,
        enabled: parsed.enabled,
        missing: exe.as_deref().is_some_and(registry::missing),
        hive: Hive::Machine,
        key_path: task.to_string(),
        value_name: leaf,
        file_path: Some(path.to_path_buf()),
        publisher,
        system: is_system_path(task),
        ..Default::default()
    })
}

pub fn list() -> Vec<StartupEntry> {
    let mut files = Vec::new();
    walk(&tasks_root(), "", 0, &mut files);
    sys::par_map(&files, |(task, path)| entry(task, path))
}

fn valid_task(e: &StartupEntry) -> io::Result<PathBuf> {
    let p = e.key_path.as_str();
    if !p.starts_with('\\') || p.len() < 2 || p.contains("..") || p.contains('"') || p.contains('\n') || p.contains('\r') || p.contains('/') {
        return Err(io::Error::other("bad task"));
    }
    let file = tasks_root().join(p.trim_start_matches('\\'));
    let expected = e.file_path.as_ref().ok_or_else(|| io::Error::other("bad task"))?;
    if fsx::norm_key(&file) != fsx::norm_key(expected) || !file.is_file() || fsx::is_reparse(&file) {
        return Err(io::Error::other("bad task"));
    }
    Ok(file)
}

fn schtasks(args: &[&str]) -> io::Result<()> {
    let out = sys::hidden("schtasks.exe").args(args).output()?;
    if out.status.success() {
        return Ok(());
    }
    if !sys::is_admin() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "access denied"));
    }
    Err(io::Error::other(sys::command_failure(&out.stderr, out.status.code())))
}

pub fn set_enabled(e: &mut StartupEntry, enable: bool) -> io::Result<()> {
    let file = valid_task(e)?;
    schtasks(&["/Change", "/TN", &e.key_path, if enable { "/ENABLE" } else { "/DISABLE" }])?;
    let now = fs::read(&file).map(|b| parse(&decode(&b)).enabled).unwrap_or(enable);
    if now != enable {
        return Err(io::Error::other("the task did not change"));
    }
    e.enabled = enable;
    data::log(&format!("task {} {}", if enable { "enabled" } else { "disabled" }, e.key_path));
    Ok(())
}

pub fn delete(e: &StartupEntry, backup_dir: &Path) -> io::Result<()> {
    let file = valid_task(e)?;
    let copy = backup_dir.join(format!("task-{}.xml", e.value_name.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_")));
    fs::copy(&file, &copy)?;
    if fs::metadata(&copy)?.len() != fs::metadata(&file)?.len() {
        return Err(io::Error::other("backup copy failed"));
    }
    schtasks(&["/Delete", "/TN", &e.key_path, "/F"])?;
    data::log(&format!("task deleted {}", e.key_path));
    Ok(())
}
