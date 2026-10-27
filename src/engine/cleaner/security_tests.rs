use super::*;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::System::Registry::{HKEY, KEY_ALL_ACCESS, REG_OPTION_CREATE_LINK, REG_VALUE_TYPE, RegCreateKeyExW, RegSetValueExW};

struct Fixture {
    path: String,
    key: RegKey,
    links: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = format!("Software\\wisping-cleaner-security-{}-{stamp}", std::process::id());
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path).unwrap();
        Self { path, key, links: Vec::new() }
    }

    fn link(&mut self, name: &str, target: &str) {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let mut handle = HKEY::default();
        let status = unsafe { RegCreateKeyExW(HKEY(self.key.raw_handle()), PCWSTR(wide.as_ptr()), None,
            PCWSTR::null(), REG_OPTION_CREATE_LINK, KEY_ALL_ACCESS, None, &mut handle, None) };
        assert_eq!(status.0, 0);
        self.links.push(name.to_string());
        let link = RegKey::predef(handle.0);
        let value: Vec<u16> = "SymbolicLinkValue".encode_utf16().chain([0]).collect();
        let bytes: Vec<u8> = target.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let status = unsafe { RegSetValueExW(HKEY(link.raw_handle()), PCWSTR(value.as_ptr()), None,
            REG_VALUE_TYPE(6), Some(&bytes)) };
        assert_eq!(status.0, 0);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for link in self.links.iter().rev() {
            let _ = self.key.delete_subkey(link);
        }
        assert!(self.path.starts_with("Software\\wisping-cleaner-security-"));
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.path);
    }
}

#[test]
fn security_cleaner_fences_the_pinned_root_after_ancestor_swaps() {
    use fsx::security_tests::{fixture, junction, ROOT_LOCK};
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("cleaner-pinned-root");
    let parent = base.join("parent");
    let root = parent.join("cache");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(outside.join("cache")).unwrap();
    fs::write(root.join("a.tmp"), b"x").unwrap();
    fs::write(outside.join("cache\\a.tmp"), b"keep").unwrap();
    let mut t = CleanTarget::custom("security-test");
    t.sources.push(Source::new(&root.to_string_lossy(), "*.tmp", false, 0));
    let (planned, refused) = plans(&t, true);
    assert!(refused.is_empty());
    assert_eq!(planned.len(), 1);
    let fence = planned[0].fence.as_ref().unwrap();
    let opened = |p: &Path| fsx::final_path(&fs::File::open(p).unwrap()).unwrap();
    assert!(fence.admits(&opened(&root.join("a.tmp"))));
    assert!(!fence.admits(&opened(&outside.join("cache\\a.tmp"))));
    fs::rename(&parent, base.join("old")).unwrap();
    junction(&parent, &outside);
    assert_eq!(fsx::remove_inside(&root.join("a.tmp"), fence, 0, false), Removal::Failed);
    assert!(plans(&t, true).0.is_empty());
    assert_eq!(fs::read(outside.join("cache\\a.tmp")).unwrap(), b"keep");
    fs::remove_dir(&parent).unwrap();
    junction(&parent, &base.join("old"));
    assert!(plans(&t, true).0.is_empty());
}

#[test]
fn security_cleaner_reports_roots_it_cannot_pin() {
    use fsx::security_tests::{fixture, ROOT_LOCK};
    use std::os::windows::fs::OpenOptionsExt;
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("cleaner-unpinned-root");
    let parent = base.join("parent");
    let root = parent.join("cache");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.tmp"), b"x").unwrap();
    let mut t = CleanTarget::custom("security-test");
    t.sources.push(Source::new(&root.to_string_lossy(), "*.tmp", false, 0));
    let busy = fs::OpenOptions::new().access_mode(0x0001_0000).share_mode(7).custom_flags(0x0200_0000).open(&parent).unwrap();
    let (planned, refused) = plans(&t, true);
    assert!(planned.is_empty());
    assert_eq!(refused, vec![root.clone()]);
    assert_eq!(plans(&t, false).0.len(), 1);
    drop(busy);
    let (planned, refused) = plans(&t, true);
    assert_eq!(planned.len(), 1);
    assert!(refused.is_empty());
    assert!(root.join("a.tmp").exists());
}

#[test]
fn security_cleaner_rejects_root_and_ancestor_links() {
    let mut fixture = Fixture::new();
    let (outside, _) = fixture.key.create_subkey("outside\\child").unwrap();
    outside.set_value("sentinel", &"keep-child").unwrap();
    let outside = fixture.key.open_subkey_with_flags("outside", KEY_ALL_ACCESS.0).unwrap();
    outside.set_value("sentinel", &"keep-root").unwrap();
    fixture.link("alias", &registry::target::native_name(&outside).unwrap());
    fixture.link("dangling", &format!("{}\\missing", registry::target::native_name(&fixture.key).unwrap()));
    for suffix in ["alias", "alias\\child", "dangling", "dangling\\child"] {
        let path = format!("{}\\{suffix}", fixture.path);
        assert_eq!(reg_entry_clear(&path).0, 0, "{suffix}");
        assert_eq!(reg_entry_count(&path), 0, "{suffix}");
        assert_eq!(reg_entry_clear(&format!("{path}|sentinel")).0, 0, "{suffix}");
    }
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "keep-root");
    assert_eq!(outside.open_subkey("child").unwrap().get_value::<String, _>("sentinel").unwrap(), "keep-child");
    assert!(fixture.key.open_subkey("missing").is_err());
}

#[test]
fn security_cleaner_rejects_linked_descendants() {
    let mut fixture = Fixture::new();
    let (outside, _) = fixture.key.create_subkey("outside").unwrap();
    outside.set_value("sentinel", &"keep").unwrap();
    let (history, _) = fixture.key.create_subkey("history\\ordinary").unwrap();
    history.set_value("remove", &1u32).unwrap();
    fixture.link("history\\redirect", &registry::target::native_name(&outside).unwrap());
    let path = format!("{}\\history", fixture.path);
    let count = reg_entry_count(&path);
    let (done, failed) = reg_entry_clear(&path);
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "keep");
    assert_eq!(count, 1);
    assert_eq!(done, 1);
    assert_eq!(failed, 1);
    assert!(history.get_raw_value("remove").is_err());
}

#[test]
fn security_cleaner_keeps_checked_handle_after_name_replacement() {
    let mut fixture = Fixture::new();
    let (history, _) = fixture.key.create_subkey("history").unwrap();
    history.set_value("remove", &1u32).unwrap();
    let (outside, _) = fixture.key.create_subkey("outside").unwrap();
    outside.set_value("sentinel", &"keep").unwrap();
    let checked = reg_open(&format!("{}\\history", fixture.path), true).unwrap();
    fixture.key.rename_subkey("history", "old").unwrap();
    fixture.link("history", &registry::target::native_name(&outside).unwrap());
    assert_eq!(reg_clear(&checked, 0), (1, 0));
    assert!(history.get_raw_value("remove").is_err());
    assert_eq!(outside.get_value::<String, _>("sentinel").unwrap(), "keep");
}

#[test]
fn security_cleaner_preserves_value_only_and_depth_limits() {
    let fixture = Fixture::new();
    let (root, _) = fixture.key.create_subkey("history").unwrap();
    root.set_value("remove", &1u32).unwrap();
    root.set_value("keep", &2u32).unwrap();
    let path = format!("{}\\history", fixture.path);
    assert_eq!(reg_entry_clear(&format!("{}|remove", path.to_uppercase())), (1, 0));
    assert_eq!(root.get_value::<u32, _>("keep").unwrap(), 2);
    let mut nested = String::new();
    for _ in 1..=5 {
        if !nested.is_empty() { nested.push('\\'); }
        nested.push_str("child");
        let (key, _) = root.create_subkey(&nested).unwrap();
        key.set_value("value", &3u32).unwrap();
    }
    assert_eq!(reg_entry_count(&path), 5);
    assert_eq!(reg_entry_clear(&path), (5, 0));
    assert!(root.get_raw_value("keep").is_err());
    assert_eq!(root.open_subkey(&nested).unwrap().get_value::<u32, _>("value").unwrap(), 3);
    assert_eq!(reg_entry_clear(&format!("{}\\absent", fixture.path)), (0, 0));
    assert!(fixture.key.open_subkey("absent").is_err());
}

#[test]
fn security_broad_roots_need_depth_unless_named_temp() {
    assert!(deep_enough("c:\\temp"));
    assert!(deep_enough("d:\\TMP"));
    assert!(deep_enough("c:\\users\\x"));
    assert!(!deep_enough("c:\\users"));
    assert!(!deep_enough("c:\\temporary"));
    assert!(!deep_enough("c:\\"));
    assert!(!deep_enough("c:"));
}
