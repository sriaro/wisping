use super::*;
use fsx::security_tests::{fixture, junction, ROOT_LOCK};

struct TestRoot(PathBuf);

impl TestRoot {
    fn set(base: &Path) -> Self {
        let old = data::root().to_path_buf();
        data::set_test_root(base.join("data"));
        Self(old)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        data::set_test_root(self.0.clone());
    }
}

fn entry(file: &Path) -> StartupEntry {
    StartupEntry {
        kind: StartupKind::Folder,
        name: "test".into(),
        file_path: Some(file.to_path_buf()),
        approved_sub: "WispingSecurityTest-NotAnApprovalKey".into(),
        value_name: "test.lnk".into(),
        ..Default::default()
    }
}

#[test]
fn security_startup_preserves_native_backup_and_deletes_selected_file() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("startup-normal");
    let _data = TestRoot::set(&base);
    let source = base.join("Startup");
    let backup = base.join("backup");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir(&backup).unwrap();
    let file = source.join("应用.lnk");
    fs::write(&file, b"shortcut contents").unwrap();
    fs::write(file.with_file_name("应用.lnk:Zone.Identifier"), b"[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
    fs::write(source.join("keep.lnk"), b"keep").unwrap();
    delete(&entry(&file), &backup).unwrap();
    assert!(!file.exists());
    assert_eq!(fs::read(backup.join("应用.lnk")).unwrap(), b"shortcut contents");
    assert_eq!(fs::read(backup.join("应用.lnk:Zone.Identifier")).unwrap(), b"[ZoneTransfer]\r\nZoneId=3\r\n");
    assert_eq!(fs::read(source.join("keep.lnk")).unwrap(), b"keep");
    let empty = source.join("empty.lnk");
    fs::write(&empty, b"").unwrap();
    delete(&entry(&empty), &backup).unwrap();
    assert_eq!(fs::metadata(backup.join("empty.lnk")).unwrap().len(), 0);
}

#[test]
fn security_startup_rejects_source_junction_before_listing() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("startup-source-link");
    let _data = TestRoot::set(&base);
    let outside = base.join("outside");
    let backup = base.join("backup");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&backup).unwrap();
    fs::write(outside.join("test.lnk"), b"protected sentinel").unwrap();
    let linked = base.join("Startup");
    junction(&linked, &outside);
    assert!(delete(&entry(&linked.join("test.lnk")), &backup).is_err());
    assert_eq!(fs::read(outside.join("test.lnk")).unwrap(), b"protected sentinel");
    let mut listed = Vec::new();
    add_folder(&mut listed, &linked, Hive::User);
    assert!(listed.is_empty());
}

#[test]
fn security_startup_rejects_ancestor_swap_after_listing() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("startup-swapped-parent");
    let _data = TestRoot::set(&base);
    let parent = base.join("parent");
    let startup = parent.join("Startup");
    let outside = base.join("outside");
    let backup = base.join("backup");
    fs::create_dir_all(&startup).unwrap();
    fs::create_dir_all(outside.join("Startup")).unwrap();
    fs::create_dir(&backup).unwrap();
    fs::write(startup.join("test.lnk"), b"selected").unwrap();
    fs::write(outside.join("Startup\\test.lnk"), b"protected sentinel").unwrap();
    let mut listed = Vec::new();
    add_folder(&mut listed, &startup, Hive::User);
    assert_eq!(listed.len(), 1);
    fs::rename(&parent, base.join("original")).unwrap();
    junction(&parent, &outside);
    assert!(delete(&listed[0], &backup).is_err());
    assert_eq!(fs::read(outside.join("Startup\\test.lnk")).unwrap(), b"protected sentinel");
    assert_eq!(fs::read(base.join("original\\Startup\\test.lnk")).unwrap(), b"selected");
}

#[test]
fn security_startup_rejects_backup_junction_and_collision() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("startup-backup-link");
    let _data = TestRoot::set(&base);
    let startup = base.join("Startup");
    let outside = base.join("outside");
    fs::create_dir(&startup).unwrap();
    fs::create_dir(&outside).unwrap();
    let file = startup.join("test.lnk");
    fs::write(&file, b"selected").unwrap();
    let linked = base.join("backup");
    junction(&linked, &outside);
    assert!(delete(&entry(&file), &linked).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"selected");
    assert!(!outside.join("test.lnk").exists());
    let backup = base.join("ordinary-backup");
    fs::create_dir(&backup).unwrap();
    fs::write(backup.join("test.lnk"), b"existing backup").unwrap();
    assert!(delete(&entry(&file), &backup).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"selected");
    assert_eq!(fs::read(backup.join("test.lnk")).unwrap(), b"existing backup");
}

#[test]
fn security_startup_rejects_dangling_backup_symlink() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("startup-dangling-backup");
    let _data = TestRoot::set(&base);
    let startup = base.join("Startup");
    let backup = base.join("backup");
    let outside = base.join("outside");
    for directory in [&startup, &backup, &outside] { fs::create_dir(directory).unwrap(); }
    let file = startup.join("test.lnk");
    let redirected = outside.join("new-file.lnk");
    fs::write(&file, b"selected").unwrap();
    std::os::windows::fs::symlink_file(&redirected, backup.join("test.lnk")).unwrap();
    let result = delete(&entry(&file), &backup);
    assert!(!redirected.exists());
    assert!(result.is_err());
    assert_eq!(fs::read(&file).unwrap(), b"selected");
}

#[test]
fn security_startup_preserves_long_path_support() {
    let _serial = ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("startup-long-path");
    let _data = TestRoot::set(&base);
    let mut parent = base.join("long");
    for _ in 0..12 { parent.push("ordinary-folder-name"); }
    let startup = parent.join("Startup");
    let backup = parent.join("backup");
    fs::create_dir_all(&startup).unwrap();
    fs::create_dir(&backup).unwrap();
    let file = startup.join("test.lnk");
    assert!(file.as_os_str().len() > 260);
    fs::write(&file, b"long-path shortcut").unwrap();
    delete(&entry(&file), &backup).unwrap();
    assert!(!file.exists());
    assert_eq!(fs::read(backup.join("test.lnk")).unwrap(), b"long-path shortcut");
}

#[test]
fn security_startup_recognises_default_winlogon_values() {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    assert!(only_windows("explorer.exe", "explorer.exe"));
    assert!(only_windows(&format!("{windir}\\system32\\userinit.exe,"), "userinit.exe"));
    assert!(only_windows("%SystemRoot%\\system32\\userinit.exe,", "userinit.exe"));
    assert!(!only_windows("explorer.exe, C:\\Users\\Public\\evil.exe", "explorer.exe"));
    assert!(!only_windows(&format!("{windir}\\system32\\userinit.exe,C:\\ProgramData\\x\\userinit.exe"), "userinit.exe"));
    assert!(!only_windows("C:\\Tools\\explorer.exe", "explorer.exe"));
    assert!(!only_windows("", "explorer.exe"));
}

#[test]
fn security_startup_refuses_changes_to_view_only_and_run_once_entries() {
    let mut view = StartupEntry { kind: StartupKind::Advanced, hive: Hive::Machine, key_path: WINLOGON.into(), value_name: "Shell".into(), ..Default::default() };
    assert_eq!(set_enabled(&mut view, false).unwrap_err().to_string(), "locked");
    assert_eq!(delete(&view, Path::new("C:\\")).unwrap_err().to_string(), "locked");
    let mut once = StartupEntry { kind: StartupKind::Run, hive: Hive::User, key_path: format!("{}\\RunOnce", CV), value_name: "x".into(), ..Default::default() };
    assert_eq!(set_enabled(&mut once, false).unwrap_err().to_string(), "nodisable");
}
