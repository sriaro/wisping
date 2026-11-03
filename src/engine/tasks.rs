use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::data;
use super::fileinfo;
use super::fsx;
use super::programs;
use super::registry;
use super::startup::{StartupEntry, StartupKind};
use super::sys;
use super::regbackup::Hive;

const MAX_DEPTH: usize = 5;
const MAX_FILE: u64 = 1024 * 1024;

pub fn tasks_root() -> PathBuf {
    super::winenv::system_dir().join("Tasks")
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

struct TaskFile {
    task: String,
    dir: Arc<fsx::PinnedDir>,
    name: OsString,
}

fn walk(dir: Arc<fsx::PinnedDir>, rel: &str, depth: usize, out: &mut Vec<TaskFile>) {
    let Ok(rd) = fs::read_dir(dir.path()) else { return };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if fsx::attrs_reparse(meta.file_attributes()) {
            continue;
        }
        let name = e.file_name();
        let task = format!("{}\\{}", rel, name.to_string_lossy());
        if meta.is_dir() {
            if depth < MAX_DEPTH
                && let Ok(sub) = dir.child(&name, false) {
                    walk(Arc::new(sub), &task, depth + 1, out);
            }
        } else if meta.is_file() && meta.len() <= MAX_FILE {
            out.push(TaskFile { task, dir: dir.clone(), name });
        }
    }
}

fn is_system_path(task: &str) -> bool {
    task.to_lowercase().starts_with("\\microsoft\\windows\\")
}

const HOSTS: [&str; 14] = [
    "wmic.exe", "msiexec.exe", "explorer.exe", "cmstp.exe", "control.exe", "msdt.exe", "installutil.exe", "regasm.exe", "regsvcs.exe", "msbuild.exe", "dllhost.exe", "odbcconf.exe", "presentationhost.exe", "wuauclt.exe",
];

fn windows_component(exe: &str) -> bool {
    let path = Path::new(exe);
    let leaf = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    fsx::under_windir(exe) && fileinfo::admin_only(path) && !programs::SHELLS.contains(&leaf.as_str()) && !HOSTS.contains(&leaf.as_str())
}

fn entry(file: &TaskFile) -> Option<StartupEntry> {
    let xml = decode(&read_pinned(&file.dir, &file.name).ok()?);
    if !xml.contains("<Task") {
        return None;
    }
    let task = file.task.as_str();
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
        file_path: Some(file.dir.path().join(&file.name)),
        publisher,
        system: is_system_path(task) || exe.as_deref().is_some_and(windows_component),
        ..Default::default()
    })
}

pub fn list() -> Vec<StartupEntry> {
    let Ok(root) = fsx::PinnedDir::open(&tasks_root()) else { return Vec::new() };
    let mut files = Vec::new();
    walk(Arc::new(root), "", 0, &mut files);
    sys::par_map(&files, entry)
}

fn valid_path(p: &str) -> bool {
    p.starts_with('\\') && p.len() >= 2 && !p.contains("..") && !p.contains(['"', '\n', '\r', '/'])
}

pub fn task_path(xml: &str) -> Option<String> {
    let path = block(xml, "RegistrationInfo").and_then(|r| text(r, "URI"))?;
    (valid_path(&path) && !is_system_path(&path)).then_some(path)
}

fn valid_task(e: &StartupEntry) -> io::Result<PathBuf> {
    let p = e.key_path.as_str();
    if !valid_path(p) {
        return Err(io::Error::other("bad task"));
    }
    let file = tasks_root().join(p.trim_start_matches('\\'));
    let expected = e.file_path.as_ref().ok_or_else(|| io::Error::other("bad task"))?;
    if fsx::norm_key(&file) != fsx::norm_key(expected) || fsx::plain_file(&file) != Some(true) {
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
    Err(sys::command_failure(&out.stderr))
}

pub fn set_enabled(e: &mut StartupEntry, enable: bool) -> io::Result<()> {
    let file = valid_task(e)?;
    schtasks(&["/Change", "/TN", &e.key_path, if enable { "/ENABLE" } else { "/DISABLE" }])?;
    let now = read_task(&file).map(|b| parse(&decode(&b)).enabled).unwrap_or(enable);
    if now != enable {
        return Err(io::Error::other("the task did not change"));
    }
    e.enabled = enable;
    data::log(&format!("task {} {}", if enable { "enabled" } else { "disabled" }, e.key_path));
    Ok(())
}

fn read_task(file: &Path) -> io::Result<Vec<u8>> {
    let (Some(parent), Some(name)) = (file.parent(), file.file_name()) else { return Err(io::Error::other("bad task")) };
    read_pinned(&fsx::PinnedDir::open(parent)?, name)
}

fn read_pinned(dir: &fsx::PinnedDir, name: &OsStr) -> io::Result<Vec<u8>> {
    let mut source = dir.read_file(name)?;
    let len = source.metadata()?.len();
    if len > MAX_FILE {
        return Err(io::Error::other("bad task"));
    }
    let mut bytes = Vec::new();
    (&mut source).take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(io::Error::other("backup copy failed"));
    }
    Ok(bytes)
}

fn backup(file: &Path, backup_dir: &Path, name: &OsStr) -> io::Result<(fsx::PinnedDir, File)> {
    let bytes = read_task(file)?;
    let target = fsx::PinnedDir::open(backup_dir)?;
    let mut copy = target.create_file(name)?;
    copy.write_all(&bytes)?;
    copy.sync_all()?;
    if copy.metadata()?.len() != bytes.len() as u64 {
        return Err(io::Error::other("backup copy failed"));
    }
    if sys::elevation()? {
        super::regbackup::integrity::record(&target.path().join(name), &bytes)?;
    }
    Ok((target, copy))
}

pub fn restore(xml: &Path, task: &str) -> io::Result<()> {
    if !valid_path(task) || is_system_path(task) {
        return Err(io::Error::other("bad task"));
    }
    if fsx::plain_exists(&tasks_root().join(task.trim_start_matches('\\'))) != Some(false) {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "exists"));
    }
    let xml = xml.to_str().ok_or_else(|| io::Error::other("bad task"))?;
    schtasks(&["/Create", "/TN", task, "/XML", xml])?;
    data::log(&format!("task restored {}", task));
    Ok(())
}

pub fn delete(e: &StartupEntry, backup_dir: &Path) -> io::Result<()> {
    let file = valid_task(e)?;
    let name = format!("task-{}.xml", e.value_name.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_"));
    let _held = backup(&file, backup_dir, name.as_ref())?;
    schtasks(&["/Delete", "/TN", &e.key_path, "/F"])?;
    data::log(&format!("task deleted {}", e.key_path));
    Ok(())
}
