use std::io;
use std::path::PathBuf;

use winreg::enums::*;

use super::regbackup::{self, Hive};
use super::registry;

pub const ROOT: &str = "Control Panel\\NotifyIconSettings";
const VALUE: &str = "IsPromoted";

pub struct Group {
    pub id: &'static str,
    pub needles: &'static [&'static str],
}

pub const GROUPS: [Group; 1] = [Group { id: "nvtray", needles: &["\\nvidia corporation\\", "\\display.nvcontainer\\"] }];

pub fn group(id: &str) -> Option<&'static Group> {
    GROUPS.iter().find(|g| g.id == id)
}

fn root_allowed(root: &str) -> bool {
    root.eq_ignore_ascii_case(ROOT) || {
        let test = registry::TEST_KEY.to_lowercase();
        let lower = root.to_lowercase();
        registry::allow_test_key() && (lower == test || lower.starts_with(&format!("{}\\", test)))
    }
}

fn needle_allowed(needles: &[&str]) -> bool {
    !needles.is_empty() && needles.iter().all(|n| GROUPS.iter().any(|g| g.needles.contains(n)))
}

fn entries(root: &str, needles: &[&str]) -> Vec<String> {
    let Ok(key) = Hive::User.open(root) else { return Vec::new() };
    let mut ids: Vec<String> = key
        .enum_keys()
        .flatten()
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .filter(|id| {
            key.open_subkey_with_flags(id, KEY_READ | KEY_WOW64_64KEY)
                .ok()
                .and_then(|k| regbackup::read_string(&k, "ExecutablePath"))
                .is_some_and(|p| {
                    let path = format!("\\{}", p).to_lowercase();
                    needles.iter().any(|n| path.contains(n))
                })
        })
        .collect();
    ids.sort();
    ids
}

fn promoted(root: &str, id: &str) -> Option<u32> {
    Hive::User.open(&format!("{}\\{}", root, id)).ok().and_then(|k| regbackup::read_dword(&k, VALUE))
}

pub fn count(root: &str, needles: &[&str]) -> usize {
    entries(root, needles).len()
}

pub fn is_hidden(root: &str, needles: &[&str]) -> bool {
    let ids = entries(root, needles);
    !ids.is_empty() && ids.iter().all(|id| promoted(root, id) == Some(0))
}

pub fn set_hidden(root: &str, needles: &[&str], hidden: bool, new_backup: &dyn Fn() -> io::Result<PathBuf>) -> io::Result<()> {
    if !root_allowed(root) || !needle_allowed(needles) {
        return Err(io::Error::other("refused"));
    }
    let ids = entries(root, needles);
    if ids.is_empty() {
        return Err(io::Error::other("Windows does not know this tray icon yet. Let it appear once, then try again."));
    }
    let changes: Vec<&String> = ids.iter().filter(|id| if hidden { promoted(root, id) != Some(0) } else { promoted(root, id).is_some() }).collect();
    if changes.is_empty() {
        return Ok(());
    }
    let present: Vec<&&String> = changes.iter().filter(|id| Hive::User.open(&format!("{}\\{}", root, id)).is_ok_and(|k| regbackup::has_value(&k, VALUE))).collect();
    if !present.is_empty() {
        let dir = new_backup()?;
        for id in &present {
            regbackup::export_value(Hive::User, &format!("{}\\{}", root, id), VALUE, &dir)?;
        }
    }
    for id in changes {
        let path = format!("{}\\{}", root, id);
        let key = Hive::User.open_write(&path)?;
        if hidden {
            key.set_value(VALUE, &0u32)?;
        } else if regbackup::has_value(&key, VALUE) {
            key.delete_value(VALUE)?;
        }
    }
    Ok(())
}
