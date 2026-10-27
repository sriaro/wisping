use super::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::{data, regbackup::{self, Hive}, registry, tools};
use crate::engine::fsx::security_tests::{fixture, ROOT_LOCK};
use windows::Wdk::System::Registry::NtRenameKey;
use windows::Win32::System::Registry::{KEY_ALL_ACCESS, REG_OPTION_CREATE_LINK, REG_VALUE_TYPE, RegCreateKeyExW, RegSetValueExW};
use windows::core::PCWSTR;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TestRegistry {
    path: String,
    root: RegKey,
    links: RefCell<Vec<RegKey>>,
}

impl TestRegistry {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = format!("{}\\registry-security-{}-{id}", registry::TEST_KEY, std::process::id());
        let (root, _) = RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(&path).unwrap();
        Self { path, root, links: RefCell::new(Vec::new()) }
    }

    fn path(&self, relative: &str) -> String { format!("{}\\{relative}", self.path) }

    fn key(&self, relative: &str) -> RegKey { self.root.create_subkey(relative).unwrap().0 }

    fn link(&self, parent: &RegKey, name: &str, target: &str) {
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mut handle = HKEY::default();
        let status = unsafe { RegCreateKeyExW(HKEY(parent.raw_handle()), PCWSTR(name.as_ptr()), None, PCWSTR::null(),
            REG_OPTION_CREATE_LINK, KEY_ALL_ACCESS, None, &mut handle, None) };
        assert_eq!(status.0, 0);
        let link = RegKey::predef(handle.0);
        let target: Vec<u8> = target.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let name: Vec<u16> = "SymbolicLinkValue".encode_utf16().chain([0]).collect();
        let status = unsafe { RegSetValueExW(HKEY(link.raw_handle()), PCWSTR(name.as_ptr()), None, REG_VALUE_TYPE(6), Some(&target)) };
        assert_eq!(status.0, 0);
        self.links.borrow_mut().push(link);
    }
}

impl Drop for TestRegistry {
    fn drop(&mut self) {
        for link in self.links.get_mut().drain(..) { let _ = delete_key(&link); }
        if let Ok(root) = self.root.open_subkey_with_flags("", winreg::enums::KEY_ALL_ACCESS)
            && let Ok(tree) = Tree::open(root, winreg::enums::KEY_ALL_ACCESS) {
                let _ = tree.delete();
        }
        let _ = RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey(registry::TEST_KEY);
    }
}

struct TestState {
    root: PathBuf,
    allow_test: bool,
}

impl TestState {
    fn set(path: &Path) -> Self {
        let state = Self { root: data::root().to_path_buf(), allow_test: registry::allow_test_key() };
        data::set_test_root(path.to_path_buf());
        registry::set_allow_test_key(true);
        state
    }
}

impl Drop for TestState {
    fn drop(&mut self) {
        data::set_test_root(self.root.clone());
        registry::set_allow_test_key(self.allow_test);
    }
}

fn rename(key: &RegKey, name: &str) {
    let mut wide: Vec<u16> = name.encode_utf16().collect();
    let length = (wide.len() * 2) as u16;
    let name = UNICODE_STRING { Length: length, MaximumLength: length, Buffer: PWSTR(wide.as_mut_ptr()) };
    nt_result(unsafe { NtRenameKey(HANDLE(key.raw_handle()), &name) }).unwrap();
}

#[test]
fn security_registry_open_rejects_root_ancestor_and_leaf_links() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let test = TestRegistry::new();
    let outside = test.key("outside");
    outside.set_value("sentinel", &"outside").unwrap();
    outside.create_subkey("child").unwrap().0.set_value("sentinel", &"outside child").unwrap();
    let selected = test.key("selected");
    let destination = native_name(&outside).unwrap();
    test.link(&test.root, "linked-root", &destination);
    test.link(&selected, "ancestor", &destination);
    test.link(&selected, "leaf", &destination);

    for relative in ["linked-root\\child", "selected\\ancestor\\child", "selected\\leaf"] {
        let path = test.path(relative);
        assert!(Hive::User.open(&path).is_err(), "read {relative}");
        assert!(Hive::User.open_write(&path).is_err(), "write {relative}");
        assert!(Hive::User.create(&path).is_err(), "create {relative}");
    }
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "outside");
    assert_eq!(outside.open_subkey("child").unwrap().get_value::<String, _>("sentinel").unwrap(), "outside child");
}

#[test]
fn security_registry_create_rejects_dangling_links_without_creating_targets() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let test = TestRegistry::new();
    let outside = test.key("outside");
    test.link(&test.root, "dangling", &format!("{}\\missing", native_name(&outside).unwrap()));
    for relative in ["dangling", "dangling\\nested"] {
        assert!(Hive::User.create(&test.path(relative)).is_err());
        assert!(outside.open_subkey("missing").is_err());
    }
}

#[test]
fn security_registry_snapshot_and_delete_reject_descendant_links_before_mutation() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let base = fixture("registry-tree-links");
    let _state = TestState::set(&base.join("data"));
    let test = TestRegistry::new();
    let outside = test.key("outside");
    outside.set_value("sentinel", &"outside").unwrap();
    let selected = test.key("selected");
    selected.create_subkey("ordinary").unwrap().0.set_value("keep", &1u32).unwrap();
    let nested = selected.create_subkey("nested").unwrap().0;
    test.link(&nested, "linked", &native_name(&outside).unwrap());
    let backup = base.join("backup");
    std::fs::create_dir(&backup).unwrap();
    let path = test.path("selected");
    assert!(Hive::User.tree(&path).is_err());
    assert!(regbackup::export_key(Hive::User, &path, &backup).is_err());
    assert!(tools::delete_tree(&path, &backup).is_err());
    let issue = registry::RegIssue::new(Hive::User, "apppath", &path, None, "test");
    assert!(registry::fix_one(&issue, &backup).is_err());
    assert!(selected.open_subkey("ordinary").is_ok());
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "outside");
    assert_eq!(std::fs::read_dir(&backup).unwrap().count(), 0);
}

#[test]
fn security_registry_backup_and_delete_use_retained_handles_after_rename() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let base = fixture("registry-retained-tree");
    let test = TestRegistry::new();
    let outside = test.key("outside");
    outside.set_value("sentinel", &"outside").unwrap();
    let selected = test.key("selected");
    selected.create_subkey("child").unwrap().0.set_value("sentinel", &"selected").unwrap();
    let path = test.path("selected");
    let tree = Hive::User.tree(&path).unwrap();
    rename(&selected, "original");
    test.link(&test.root, "selected", &native_name(&outside).unwrap());
    let backup = base.join("backup");
    std::fs::create_dir(&backup).unwrap();
    match regbackup::export_tree(Hive::User, &path, &tree, &backup) {
        Ok(()) => {
            let bytes = std::fs::read(backup.join("0001.reg")).unwrap();
            let text = regbackup::raw_string(&bytes[2..]);
            assert!(text.contains(&format!("[HKEY_CURRENT_USER\\{path}\\child]")));
            assert!(!text.contains("6f,00,75,00,74,00,73,00,69,00,64,00,65,00"));
            tree.delete().unwrap();
            assert!(test.root.open_subkey("original").is_err());
        }
        Err(error) => {
            let native = error.get_ref().and_then(|error| error.downcast_ref::<windows::core::Error>());
            assert_eq!(native.map(|error| error.code()), Some(windows::core::HRESULT::from_win32(
                windows::Win32::Foundation::ERROR_KEY_DELETED.0)));
            assert_eq!(std::fs::read_dir(&backup).unwrap().count(), 0);
            assert_eq!(test.root.open_subkey("original\\child").unwrap().get_value::<String, _>("sentinel").unwrap(), "selected");
        }
    }
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "outside");
}

#[test]
fn security_registry_delete_does_not_traverse_a_link_added_after_verification() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let test = TestRegistry::new();
    let outside = test.key("outside");
    outside.set_value("sentinel", &"outside").unwrap();
    let selected = test.key("selected");
    let tree = Hive::User.tree(&test.path("selected")).unwrap();
    test.link(&selected, "late-link", &native_name(&outside).unwrap());
    assert!(tree.delete().is_err());
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "outside");
    assert!(test.root.open_subkey("selected").is_ok());
}

#[test]
fn security_registry_policy_keeps_verified_handle_through_backup() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let base = fixture("registry-policy-handles");
    let _state = TestState::set(&base.join("data"));
    let test = TestRegistry::new();
    let outside = test.key("outside");
    outside.set_value("value", &90u32).unwrap();
    let selected = test.key("selected");
    selected.set_value("value", &10u32).unwrap();
    let backup = base.join("backup");
    std::fs::create_dir(&backup).unwrap();
    tools::set_policy(Hive::User, &test.path("selected"), None, "value", 1, true, &|| {
        rename(&selected, "original");
        test.link(&test.root, "selected", &native_name(&outside).unwrap());
        Ok(backup.clone())
    }).unwrap();
    assert_eq!(selected.get_value::<u32, _>("value").unwrap(), 1);
    assert_eq!(outside.get_value::<u32, _>("value").unwrap(), 90);
    let bytes = std::fs::read(backup.join("0001.reg")).unwrap();
    assert!(regbackup::raw_string(&bytes[2..]).contains("dword:0000000a"));
}

#[test]
fn security_registry_tools_reject_linked_flags_and_policy_targets() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let base = fixture("registry-tool-links");
    let _state = TestState::set(&base.join("data"));
    let test = TestRegistry::new();
    let outside = test.key("outside");
    outside.set_value("value", &90u32).unwrap();
    test.link(&test.root, "linked", &native_name(&outside).unwrap());
    let path = test.path("linked");
    assert!(tools::write_flags(&[(&path, "value")], false).is_err());
    let backup = base.join("backup");
    std::fs::create_dir(&backup).unwrap();
    let issue = registry::RegIssue::new(Hive::User, "shareddll", &path, Some("value"), "test");
    assert!(registry::fix_one(&issue, &backup).is_err());
    let startup = crate::engine::startup::StartupEntry {
        kind: crate::engine::startup::StartupKind::Run,
        hive: Hive::User,
        key_path: path.clone(),
        value_name: "value".into(),
        ..Default::default()
    };
    assert!(crate::engine::startup::delete(&startup, &backup).is_err());
    let mut packaged = startup.clone();
    packaged.kind = crate::engine::startup::StartupKind::Packaged;
    assert!(crate::engine::startup::set_enabled(&mut packaged, true).is_err());
    assert!(outside.get_raw_value("State").is_err());
    for enabled in [false, true] {
        assert!(tools::set_policy(Hive::User, &path, None, "value", 1, enabled, &|| panic!("backup must not start")).is_err());
    }
    assert_eq!(outside.get_value::<u32, _>("value").unwrap(), 90);
}

#[test]
fn security_registry_creates_normal_nested_keys_and_accepts_windows_aliases() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let test = TestRegistry::new();
    let path = test.path("ordinary\\nested\\日");
    Hive::User.create(&path).unwrap().set_value("value", &7u32).unwrap();
    assert_eq!(Hive::User.open(&path).unwrap().get_value::<u32, _>("value").unwrap(), 7);
    Hive::User.open("Software\\Classes").unwrap();
    for path in ["SYSTEM\\CurrentControlSet", "SOFTWARE\\Classes\\Wow6432Node\\AppID",
        "SOFTWARE\\Classes\\Wow6432Node\\PROTOCOLS", "SOFTWARE\\Classes\\Wow6432Node\\TypeLib"] {
        Hive::Machine.open(path).unwrap();
    }
}
