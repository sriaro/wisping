use super::*;
use fsx::security_tests::{fixture, junction, ROOT_LOCK};

struct TestKey(String);

struct MachineTestKey(String);

impl Drop for MachineTestKey {
    fn drop(&mut self) {
        assert!(self.0.starts_with("SOFTWARE\\Classes\\wisping-restore-"));
        let _ = RegKey::predef(HKEY_LOCAL_MACHINE).delete_subkey_all(&self.0);
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        assert!(self.0.starts_with("Software\\Classes\\wisping-restore-"));
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
    }
}

#[test]
fn security_restore_rejects_untrusted_machine_data_before_any_write() {
    let _serial = ROOT_LOCK.lock().unwrap();
    let base = fixture("restore-untrusted");
    let unique = base.file_name().unwrap().to_string_lossy();
    let path = format!("Software\\Classes\\{unique}");
    let _cleanup = TestKey(path.clone());
    let _machine_cleanup = MachineTestKey(format!("SOFTWARE\\Classes\\{unique}"));
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path).unwrap();
    key.set_value("sentinel", &"original").unwrap();
    let dir = base.join("data\\backup\\20261023-123456");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("0001.reg"), format!(
        "{HEADER}\r\n[HKEY_CURRENT_USER\\{path}]\r\n\"sentinel\"=\"tampered\"\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\{unique}]\r\n\"sentinel\"=\"tampered\"\r\n"
    )).unwrap();
    data::set_test_root(base.join("data"));
    assert!(restore("20261023-123456").is_err());
    assert_eq!(key.get_value::<String, _>("sentinel").unwrap(), "original");
}

#[test]
fn security_restore_authenticates_the_whole_backup_before_writing() {
    use super::super::regbackup::{self, Hive, integrity};
    use winreg::{RegValue, enums::*};

    let _serial = ROOT_LOCK.lock().unwrap();
    let base = fixture("restore-integrity");
    let unique = base.file_name().unwrap().to_string_lossy();
    let path = format!("Software\\Classes\\{unique}");
    let _cleanup = TestKey(path.clone());
    let _machine_cleanup = MachineTestKey(format!("SOFTWARE\\Classes\\{unique}"));
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path).unwrap();
    key.set_value("number", &0x12345678u32).unwrap();
    key.set_value("", &"Unicode 日").unwrap();
    let (child, _) = key.create_subkey("child").unwrap();
    child.set_raw_value("binary", &RegValue { bytes: vec![0, 1, 255].into(), vtype: REG_BINARY }).unwrap();
    child.set_raw_value("multi", &RegValue { bytes: vec![65, 0, 0, 0, 0, 0].into(), vtype: REG_MULTI_SZ }).unwrap();
    drop(child);
    let root = base.join("data");
    let dir = root.join("backup\\20261023-123456");
    fs::create_dir_all(&dir).unwrap();
    data::set_test_root(root.clone());
    regbackup::export_key(Hive::User, &path, &dir).unwrap();
    let file = dir.join("0001.reg");
    let bytes = fs::read(&file).unwrap();
    let receipt = integrity::receipt(&file, &bytes).unwrap();
    let trusted = |file: &Path, bytes: &[u8]| Ok(integrity::receipt(file, bytes)? == receipt);

    key.set_value("number", &0u32).unwrap();
    key.delete_subkey_all("child").unwrap();
    assert_eq!(restore_with(&root.join("backup"), "20261023-123456", true, trusted).unwrap(), 1);
    assert_eq!(key.get_value::<u32, _>("number").unwrap(), 0x12345678);
    assert_eq!(key.get_value::<String, _>("").unwrap(), "Unicode 日");
    let restored_child = key.open_subkey("child").unwrap();
    assert_eq!(restored_child.get_raw_value("binary").unwrap().bytes.as_ref(), [0, 1, 255]);
    assert_eq!(restored_child.get_raw_value("multi").unwrap().bytes.as_ref(), [65, 0, 0, 0, 0, 0]);

    let text = decode(&bytes);
    assert!(text.contains("hex(4):78,56,34,12"));
    let tampered = [
        text.replace("hex(4):78,56,34,12", "hex(4):ef,be,ad,de"),
        text.replace("hex(4):78,56,34,12", "hex(b):78,56,34,12,00,00,00,00"),
        text.replace(&format!("HKEY_CURRENT_USER\\{path}"), &format!("HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\{unique}")),
    ];
    for text in tampered {
        key.set_value("number", &7u32).unwrap();
        let tampered: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        fs::write(&file, &tampered).unwrap();
        fs::write(dir.join("0001.reg.integrity"), integrity::receipt(&file, &tampered).unwrap()).unwrap();
        assert!(restore_with(&root.join("backup"), "20261023-123456", true, trusted).is_err());
        assert_eq!(key.get_value::<u32, _>("number").unwrap(), 7);
    }

    fs::write(&file, &bytes).unwrap();
    fs::write(dir.join("0002.reg"), &bytes).unwrap();
    assert!(restore_with(&root.join("backup"), "20261023-123456", true, trusted).is_err());
    assert_eq!(key.get_value::<u32, _>("number").unwrap(), 7);
    fs::remove_file(dir.join("0002.reg")).unwrap();

    let moved = base.join("moved");
    fs::rename(&root, &moved).unwrap();
    let replacing_verifier = |file: &Path, bytes: &[u8]| {
        let accepted = trusted(file, bytes)?;
        fs::write(file, b"replaced after verification").unwrap();
        Ok(accepted)
    };
    assert_eq!(restore_with(&moved.join("backup"), "20261023-123456", true, replacing_verifier).unwrap(), 1);
    assert_eq!(key.get_value::<u32, _>("number").unwrap(), 0x12345678);
}

fn registry_link(parent: &RegKey, name: &str, target: &str) {
    use windows::Win32::System::Registry::{KEY_ALL_ACCESS, REG_OPTION_CREATE_LINK, RegCreateKeyExW};
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
}

#[test]
fn security_restore_rejects_registry_links_before_target_writes() {
    let _serial = ROOT_LOCK.lock().unwrap();
    let base = fixture("restore-registry-link");
    let path = format!("Software\\Classes\\{}", base.file_name().unwrap().to_string_lossy());
    let _cleanup = TestKey(path.clone());
    let (root, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path).unwrap();
    let (target, _) = root.create_subkey("target").unwrap();
    target.set_value("sentinel", &"original").unwrap();
    registry_link(&root, "alias", &registry_target::native_name(&target).unwrap());
    registry_link(&root, "dangling", &format!("{}\\missing", registry_target::native_name(&root).unwrap()));
    let backup_root = base.join("data\\backup");
    let dir = backup_root.join("20261023-123456");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("0001.reg");
    for suffix in ["alias", "alias\\child", "dangling", "dangling\\child"] {
        let text = format!("{HEADER}\r\n[HKEY_CURRENT_USER\\{path}\\{suffix}]\r\n\"sentinel\"=\"tampered\"\r\n");
        fs::write(&file, text.as_bytes()).unwrap();
        let receipt = super::super::regbackup::integrity::receipt(&file, text.as_bytes()).unwrap();
        let trusted = |file: &Path, bytes: &[u8]| Ok(super::super::regbackup::integrity::receipt(file, bytes)? == receipt);
        assert!(restore_with(&backup_root, "20261023-123456", true, trusted).is_err(), "{suffix}");
        assert_eq!(target.get_value::<String, _>("sentinel").unwrap(), "original");
        assert!(target.open_subkey("child").is_err());
        assert!(root.open_subkey("missing").is_err());
    }
}

#[test]
fn security_backup_delete_rejects_parent_links_and_preserves_nested_folders() {
    let _serial = ROOT_LOCK.lock().unwrap();
    let base = fixture("delete");
    let outside = base.join("outside");
    fs::create_dir_all(outside.join("20261023-123456")).unwrap();
    let victim = outside.join("20261023-123456\\victim");
    fs::write(&victim, b"keep").unwrap();
    let root = base.join("data");
    fs::create_dir(&root).unwrap();
    junction(&root.join("backup"), &outside);
    data::set_test_root(root);
    assert!(delete("20261023-123456").is_err());
    assert!(victim.exists());
    let backups = fsx::PinnedDir::create(&base.join("backups")).unwrap();
    fs::create_dir(backups.path().join("20261023-111111")).unwrap();
    fs::write(backups.path().join("20261023-111111\\1.reg"), b"backup").unwrap();
    let chosen = backups.deletable_child("20261023-111111".as_ref()).unwrap();
    assert!(fs::rename(backups.path(), base.join("swapped")).is_err());
    remove_one(chosen).unwrap();
    assert!(!backups.path().join("20261023-111111").exists());
    fs::create_dir_all(backups.path().join("20261023-222222\\nested")).unwrap();
    let chosen = backups.deletable_child("20261023-222222".as_ref()).unwrap();
    assert!(remove_one(chosen).is_err());
    assert!(backups.path().join("20261023-222222\\nested").is_dir());
}

#[test]
fn security_restore_uses_owned_plan_and_round_trips_registry_types() {
    let _serial = ROOT_LOCK.lock().unwrap();
    let base = fixture("restore");
    let unique = base.file_name().unwrap().to_string_lossy();
    let path = format!("Software\\Classes\\{unique}");
    let _cleanup = TestKey(path.clone());
    let full = format!("HKEY_CURRENT_USER\\{path}");
    let mut text = format!("{HEADER}\r\n\r\n[{full}]\r\n@=\"default\"\r\n\"quote\\\"\\\\name\"=\"Unicode 日 \\\\ \\\"\"\r\n\"number\"=dword:1234abcd\r\n\"empty\"=hex:\r\n\"continued\"=hex:01,02,\\\r\n  03,04\r\n\"expand\"=hex(2):25,00,58,00,25,00,00,00\r\n\"multi\"=hex(7):61,00,00,00,00,00\r\n\"qword\"=hex(b):01,02,03,04,05,06,07,08\r\n\"custom\"=hex(20):12,34\r\n");
    for kind in [0, 5, 6, 8, 9, 10] {
        text.push_str(&format!("\"kind-{kind}\"=hex({kind:x}):01,02,03,04\r\n"));
    }
    let plan = parse_import(&text).unwrap();
    assert_eq!(plan[0].values[1].name, "quote\"\\name");
    let backup_dir = base.join("data\\backup\\20261023-123456");
    fs::create_dir_all(&backup_dir).unwrap();
    let input = backup_dir.join("0001.reg");
    fs::write(&input, &text).unwrap();
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"keep").unwrap();
    junction(&base.join("data\\restore-work"), &outside);
    data::set_test_root(base.join("data"));
    assert_eq!(restore("20261023-123456").unwrap(), 1);
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(&path, winreg::enums::KEY_READ | KEY_WOW64_64KEY).unwrap();
    assert_eq!(key.get_value::<String, _>("").unwrap(), "default");
    assert_eq!(key.get_value::<u32, _>("number").unwrap(), 0x1234abcd);
    for value in &plan[0].values {
        let name: Vec<u16> = value.name.encode_utf16().chain([0]).collect();
        let mut kind = REG_VALUE_TYPE(0);
        let mut bytes = vec![0; value.bytes.len() + 16];
        let mut len = bytes.len() as u32;
        let status = unsafe { windows::Win32::System::Registry::RegQueryValueExW(HKEY(key.raw_handle()), PCWSTR(name.as_ptr()), None, Some(&mut kind), Some(bytes.as_mut_ptr()), Some(&mut len)) };
        assert_eq!(status.0, 0);
        bytes.truncate(len as usize);
        assert_eq!(kind.0, value.kind);
        assert_eq!(bytes, value.bytes);
    }
    let captured = base.join("captured");
    fs::create_dir(&captured).unwrap();
    super::super::regbackup::export_key(super::super::regbackup::Hive::User, &path, &captured).unwrap();
    let exported_plan = parse_import(&checked_text(&read_backup(&captured.join("0001.reg")).unwrap()).unwrap()).unwrap();
    assert_eq!(exported_plan[0].values.len(), plan[0].values.len());
    for expected in &plan[0].values {
        let actual = exported_plan[0].values.iter().find(|value| value.name == expected.name).unwrap();
        assert_eq!(actual.kind, expected.kind);
        assert_eq!(actual.bytes, expected.bytes);
    }
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"keep");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    let exported = base.join("windows-export.reg");
    let reg = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32\\reg.exe");
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(reg).creation_flags(0x0800_0000).arg("export").arg(&full).arg(&exported).args(["/y", "/reg:64"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    check_file(&exported).unwrap();
    fs::copy(&exported, &input).unwrap();
    assert_eq!(restore("20261023-123456").unwrap(), 1);
    fs::write(&input, text.replace("dword:1234abcd", "dword:00000000")).unwrap();
    fs::write(backup_dir.join("0002.reg"), format!("{HEADER}\r\n[{full}]\r\n\"bad\"=hex:00,zz\r\n")).unwrap();
    assert!(restore("20261023-123456").is_err());
    assert_eq!(key.get_value::<u32, _>("number").unwrap(), 0x1234abcd);
    drop(key);
}

#[test]
fn security_restore_parser_fails_closed() {
    let key = "HKEY_CURRENT_USER\\Software\\Classes\\WispingParserTest";
    for data in ["-", "hex:00,zz", "hex(100000000):00", "dword:100000000", "\"unterminated", "hex:00,\\", "\"ok\" trailing"] {
        assert!(parse_import(&format!("{HEADER}\r\n[{key}]\r\n\"v\"={data}\r\n")).is_err(), "{data}");
    }
    assert!(parse_import(&format!("{HEADER}\r\n[-{key}]\r\n")).is_err());
    assert!(parse_import(&format!("{HEADER}\r\n[HKEY_CURRENT_USER\\Software\\Outside]\r\n\"v\"=\"x\"\r\n")).is_err());
}

#[test]
fn security_restore_accepts_new_startup_and_app_path_keys_only() {
    for key in [
        "HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\RunOnce",
        "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\RunOnce",
        "HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer\\Run",
        "HKEY_LOCAL_MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\x.exe",
    ] {
        assert!(own_key(key), "{key}");
    }
    assert!(!own_key("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon"));
    assert!(!own_key("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options\\x.exe"));
}
