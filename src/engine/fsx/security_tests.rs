use super::*;

pub static ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn fixture(label: &str) -> PathBuf {
    let base = std::env::var_os("WISPING_SECURITY_TEST_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
    let path = base.join(format!("wisping-{label}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

pub fn junction(link: &Path, target: &Path) {
    use std::os::windows::process::CommandExt;
    let cmd = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32\\cmd.exe");
    let out = std::process::Command::new(cmd).creation_flags(0x0800_0000).args(["/C", "mklink", "/J"]).arg(link).arg(target).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn security_local_drive_path_rejects_network_and_unc() {
    assert!(local_drive_path("C:\\"));
    assert!(!local_drive_path("\\\\server\\share"));
    assert!(!local_drive_path("//server/share"));
    assert!(!local_drive_path("relative"));
}

#[test]
fn security_pins_hold_ancestors_and_reject_links() {
    let base = fixture("pins");
    let parent = base.join("parent");
    let root = parent.join("root");
    let guard = PinnedDir::create(&root).unwrap();
    assert!(fs::rename(&root, parent.join("moved")).is_err());
    assert!(fs::rename(&parent, base.join("moved-parent")).is_err());
    let write_dir = |path: &Path| OpenOptions::new().access_mode(0x4000_0000).share_mode(SHARE_ALL).custom_flags(OPEN_REPARSE_POINT | BACKUP_SEMANTICS).open(path);
    assert_eq!(write_dir(&root).unwrap_err().raw_os_error(), Some(32));
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    junction(&root.join("link"), &outside);
    assert!(guard.child(OsStr::new("link"), false).is_err());
    assert!(PinnedDir::open(&root.join("link")).is_err());
    assert!(PinnedDir::create(&root.join("link\\new")).is_err());
    assert!(!outside.join("new").exists());
    let mut file = guard.create_file(OsStr::new("plain")).unwrap();
    std::io::Write::write_all(&mut file, b"valid").unwrap();
    assert!(fs::write(root.join("plain"), b"replacement").is_err());
    drop(file);
    let mut read = guard.read_file(OsStr::new("plain")).unwrap();
    assert!(fs::write(root.join("plain"), b"replacement").is_err());
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut read, &mut bytes).unwrap();
    assert_eq!(bytes, b"valid");
    drop(read);
    let target = PinnedDir::open(&outside).unwrap();
    fs::write(outside.join("plain"), b"existing").unwrap();
    assert_eq!(guard.copy_file_to(OsStr::new("plain"), &target).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(outside.join("plain")).unwrap(), b"existing");
    fs::write(root.join("linked-file"), b"source").unwrap();
    junction(&outside.join("linked-file"), &parent);
    assert!(guard.copy_file_to(OsStr::new("linked-file"), &target).is_err());
    assert!(!parent.join("plain").exists());
    drop(guard);
    fs::rename(&root, parent.join("moved")).unwrap();
    let held = base.join("held-writer");
    fs::create_dir(&held).unwrap();
    let writer = write_dir(&held).unwrap();
    assert!(PinnedDir::open(&held).is_err());
    drop(writer);
    assert!(PinnedDir::open(&held).is_ok());
}

#[test]
fn security_output_rejects_links_before_truncating() {
    let base = fixture("output-links");
    let root = base.join("data");
    let outside = base.join("outside.txt");
    fs::create_dir(&root).unwrap();
    fs::write(&outside, b"outside sentinel").unwrap();
    let output = root.join("state.txt");
    fs::hard_link(&outside, &output).unwrap();
    let dir = PinnedDir::open(&root).unwrap();
    assert!(dir.write_file(OsStr::new("state.txt"), b"replace").is_err());
    assert!(dir.append_file(OsStr::new("state.txt"), b"append").is_err());
    assert!(dir.append_lines(OsStr::new("state.txt"), "record\n", 2).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"outside sentinel");
    fs::remove_file(&output).unwrap();
    match std::os::windows::fs::symlink_file(&outside, &output) {
        Ok(()) => {
            assert!(dir.write_file(OsStr::new("state.txt"), b"replace").is_err());
            assert!(dir.append_file(OsStr::new("state.txt"), b"append").is_err());
            assert_eq!(fs::read(&outside).unwrap(), b"outside sentinel");
        }
        Err(error) if error.raw_os_error() == Some(1314) => {
            eprintln!("File symlink case unavailable without Developer Mode; hardlink case passed.");
        }
        Err(error) => panic!("create file symlink: {error}"),
    }
}

#[test]
fn security_output_rejects_parent_junctions_and_unsafe_names() {
    let base = fixture("output-paths");
    let root = base.join("data");
    let outside = base.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    junction(&root.join("reports"), &outside);
    assert!(write_file(&root.join("reports\\new.txt"), b"escape").is_err());
    assert!(!outside.join("new.txt").exists());
    let dir = PinnedDir::open(&root).unwrap();
    for name in ["..\\escape", "nested\\file", "state.txt:stream", "C:\\escape"] {
        assert!(dir.write_file(OsStr::new(name), b"escape").is_err(), "{name}");
    }
}

#[test]
fn security_output_retains_identity_and_preserves_normal_writes() {
    let base = fixture("output-normal");
    let root = base.join("data");
    let dir = PinnedDir::create(&root).unwrap();
    dir.write_file(OsStr::new("state.txt"), b"long old state").unwrap();
    dir.write_file(OsStr::new("state.txt"), b"new").unwrap();
    dir.append_file(OsStr::new("state.txt"), b" state").unwrap();
    assert_eq!(fs::read(root.join("state.txt")).unwrap(), b"new state");
    dir.update_file(OsStr::new("state.txt"), |file| {
        assert!(fs::rename(root.join("state.txt"), root.join("moved.txt")).is_err());
        assert!(fs::write(root.join("state.txt"), b"changed").is_err());
        assert!(fs::rename(&root, base.join("moved-root")).is_err());
        file.seek(SeekFrom::End(0))?;
        file.write_all(b" locked")
    }).unwrap();
    assert_eq!(fs::read(root.join("state.txt")).unwrap(), b"new state locked");
    for line in ["one\n", "two\n", "three\n"] {
        dir.append_lines(OsStr::new("history.txt"), line, 2).unwrap();
    }
    assert_eq!(fs::read_to_string(root.join("history.txt")).unwrap(), "two\nthree\n");
}

#[test]
fn security_pins_descend_under_a_shared_prefix() {
    let base = fixture("pins-under");
    let nested = base.join("a").join("b");
    fs::create_dir_all(&nested).unwrap();
    let root = PinnedDir::open(&base).unwrap();
    assert_eq!(root.under(&nested).unwrap().path(), nested.as_path());
    assert_eq!(root.under(&base).unwrap().path(), base.as_path());
    assert!(root.under(&base.join("missing")).is_err());
    assert!(root.under(Path::new("C:\\")).is_err());
    junction(&base.join("link"), &nested);
    assert!(root.under(&base.join("link")).is_err());
    assert_eq!(fixed_prefix("C:\\Users\\*\\AppData"), Some(PathBuf::from("C:\\Users")));
    assert_eq!(fixed_prefix("C:\\Users\\Public"), Some(PathBuf::from("C:\\Users\\Public")));
    assert_eq!(fixed_prefix("relative\\*"), None);
}

#[test]
fn security_history_append_reads_only_the_tail() {
    let base = fixture("append-tail");
    let dir = PinnedDir::open(&base).unwrap();
    let big: String = (0..100_000).map(|i| format!("line {i:08}\n")).collect();
    fs::write(base.join("history.txt"), &big).unwrap();
    dir.append_lines(OsStr::new("history.txt"), "last\n", 600).unwrap();
    let text = fs::read_to_string(base.join("history.txt")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 600);
    assert_eq!(lines[0], "line 00099401");
    assert_eq!(lines[599], "last");
}

#[test]
fn security_copy_refuses_a_dangling_destination_link() {
    let base = fixture("copy-dangling");
    let from = base.join("from");
    let to = base.join("to");
    let outside = base.join("outside");
    for directory in [&from, &to, &outside] {
        fs::create_dir(directory).unwrap();
    }
    fs::write(from.join("settings.txt"), b"settings").unwrap();
    let redirected = outside.join("planted.txt");
    if std::os::windows::fs::symlink_file(&redirected, to.join("settings.txt")).is_err() {
        return;
    }
    let source = PinnedDir::open(&from).unwrap();
    let target = PinnedDir::open(&to).unwrap();
    assert!(source.copy_file_to(OsStr::new("settings.txt"), &target).is_err());
    assert!(!redirected.exists());
    assert_eq!(fs::read(from.join("settings.txt")).unwrap(), b"settings");
}
