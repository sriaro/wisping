use super::*;
use fsx::security_tests::{fixture, junction};

struct RestoreState {
    root: PathBuf,
    remember: bool,
}

impl Drop for RestoreState {
    fn drop(&mut self) {
        settings::set_remember(self.remember);
        set_root(self.root.clone());
    }
}

#[test]
fn security_data_dir_rejects_system_locations() {
    assert!(!data_dir_allowed(&crate::engine::winenv::windows_dir().join("System32")));
    assert!(!data_dir_allowed(Path::new("\\\\server\\share")));
    assert!(!data_dir_allowed(Path::new("//server/share")));
}

#[test]
fn security_settings_save_does_not_truncate_linked_external_file() {
    let _serial = fsx::security_tests::ROOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _restore = RestoreState { root: root().to_path_buf(), remember: settings::remember_on() };
    let base = fixture("settings-output");
    let data = base.join("data");
    fs::create_dir(&data).unwrap();
    set_root(data.clone());
    settings::set_remember(true);
    let external = base.join("installed-program.exe");
    fs::write(&external, b"external sentinel").unwrap();
    fs::hard_link(&external, data.join("theme.txt")).unwrap();
    write_setting("theme.txt", "dark");
    assert_eq!(fs::read(&external).unwrap(), b"external sentinel");
    fs::hard_link(&external, data.join("state.txt")).unwrap();
    settings::save_state(settings::WindowState { page: 1, width: 900, height: 600 });
    assert_eq!(fs::read(&external).unwrap(), b"external sentinel");
}

fn migrate(from: &Path, to: &Path) -> std::io::Result<()> {
    copy_missing(&fsx::PinnedDir::open(from)?, &fsx::PinnedDir::create(to)?)
}

#[test]
fn security_migration_rejects_root_and_nested_junctions() {
    let base = fixture("migration");
    let source = base.join("source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested\\payload"), b"payload").unwrap();
    let read_only = source.join("nested\\read-only");
    fs::write(&read_only, b"keep permissions").unwrap();
    let mut permissions = fs::metadata(&read_only).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&read_only, permissions).unwrap();
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    let linked_source = base.join("source-link");
    junction(&linked_source, &source);
    assert!(migrate(&linked_source, &base.join("new")).is_err());
    let linked_target = base.join("target-link");
    junction(&linked_target, &outside);
    assert!(migrate(&source, &linked_target).is_err());
    assert!(!writable(&linked_target));
    let target = base.join("target");
    fs::create_dir(&target).unwrap();
    junction(&target.join("nested"), &outside);
    assert!(migrate(&source, &target).is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    let normal = base.join("normal");
    migrate(&source, &normal).unwrap();
    assert!(fs::metadata(normal.join("nested\\read-only")).unwrap().permissions().readonly());
    assert_eq!(fs::read(normal.join("nested\\payload")).unwrap(), b"payload");
    fs::write(normal.join("nested\\payload"), b"existing").unwrap();
    migrate(&source, &normal).unwrap();
    assert_eq!(fs::read(normal.join("nested\\payload")).unwrap(), b"existing");
    fs::write(normal.join(".wisping-write-test"), b"keep").unwrap();
    assert!(writable(&normal));
    assert_eq!(fs::read(normal.join(".wisping-write-test")).unwrap(), b"keep");
    junction(&source.join("skip-link"), &outside);
    migrate(&source, &normal).unwrap();
    assert!(!normal.join("skip-link").exists());
    let file_link = normal.join("file-link");
    fs::write(source.join("file-link"), b"source").unwrap();
    junction(&file_link, &outside);
    assert!(migrate(&source, &normal).is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}

#[test]
fn security_migration_location_and_reset_preserve_data() {
    let _serial = fsx::security_tests::ROOT_LOCK.lock().unwrap();
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return };
    let Some(test_base) = std::env::var_os("WISPING_SECURITY_TEST_DIR").map(PathBuf::from) else { return };
    assert!(local.parent().is_some_and(|parent| test_base.starts_with(parent)));
    let base = fixture("location");
    let source = base.join("source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested\\keep"), b"data").unwrap();
    set_root(source.clone());
    let target = base.join("target");
    set_location(&target).unwrap();
    assert_eq!(root(), target);
    assert_eq!(fs::read(target.join("nested\\keep")).unwrap(), b"data");
    assert!(set_location(&target.join("inner")).is_err());
    reset_location().unwrap();
    assert_eq!(root(), default_root());
    assert_eq!(fs::read(root().join("nested\\keep")).unwrap(), b"data");
    assert!(source.join("nested\\keep").exists());
}
