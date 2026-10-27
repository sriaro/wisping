use super::*;
use fsx::security_tests::{fixture, junction};

struct TestRoot;

impl TestRoot {
    fn set(dir: &Path) -> Self {
        set_test_root(Some(dir.to_path_buf()));
        Self
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        set_test_root(None);
    }
}

static SERIAL: Mutex<()> = Mutex::new(());

#[test]
fn security_leftovers_pin_the_approved_root_and_reject_linked_ancestors() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("leftover-ancestors");
    let real = base.join("real");
    let outside = base.join("outside");
    fs::create_dir_all(real.join("data\\OldApp")).unwrap();
    fs::create_dir_all(outside.join("data\\OldApp")).unwrap();
    fs::write(outside.join("data\\OldApp\\sentinel"), b"keep").unwrap();
    {
        let _root = TestRoot::set(&real);
        assert!(allowed(&real.join("data\\OldApp")));
        let held = pinned_parent(&real.join("data\\OldApp")).unwrap();
        assert!(fs::rename(real.join("data"), base.join("moved")).is_err());
        assert!(fs::rename(&real, base.join("moved")).is_err());
        fs::rename(real.join("data\\OldApp"), outside.join("Moved")).unwrap();
        drop(held);
        fs::rename(outside.join("Moved"), real.join("data\\OldApp")).unwrap();
        assert!(allowed(&real.join("data\\OldApp")));
    }
    let linked = base.join("linked");
    junction(&linked, &outside);
    {
        let _root = TestRoot::set(&linked);
        assert!(!allowed(&linked.join("data\\OldApp")));
        assert!(recycle(&linked.join("data\\OldApp")).is_err());
    }
    let swapped = base.join("swapped");
    fs::create_dir(&swapped).unwrap();
    junction(&swapped.join("data"), &outside.join("data"));
    {
        let _root = TestRoot::set(&swapped);
        assert!(!allowed(&swapped.join("data\\OldApp")));
        assert!(recycle(&swapped.join("data\\OldApp")).is_err());
    }
    assert_eq!(fs::read(outside.join("data\\OldApp\\sentinel")).unwrap(), b"keep");
}

#[test]
fn security_leftovers_refuse_linked_selections() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("leftover-selection");
    let outside = base.join("outside");
    fs::create_dir_all(base.join("data")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"keep").unwrap();
    junction(&base.join("data\\LinkedApp"), &outside);
    let _root = TestRoot::set(&base);
    assert!(!allowed(&base.join("data\\LinkedApp")));
    assert!(recycle(&base.join("data\\LinkedApp")).is_err());
    assert!(!allowed(&base.join("data\\Missing")));
    assert!(!allowed(&base.join("data")));
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn security_leftovers_scan_skips_roots_that_recycle_refuses() {
    use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = fixture("leftover-scan-roots");
    let real = base.join("real");
    let app = real.join("data\\OldApp");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("settings.ini"), b"0123456789").unwrap();
    let old = SystemTime::now() - Duration::from_secs(400 * 86_400);
    for path in [app.join("settings.ini"), app.clone()] {
        let f = fs::OpenOptions::new().write(true).access_mode(0x100).share_mode(7).custom_flags(0x0200_0000).open(&path).unwrap();
        f.set_times(fs::FileTimes::new().set_accessed(old).set_modified(old).set_created(old)).unwrap();
    }
    let linked = base.join("linked");
    junction(&linked, &real);
    let names = |root: &Path| {
        let _root = TestRoot::set(root);
        let found: Vec<PathBuf> = scan_with(&Links::default()).iter().map(|l| l.path.clone()).collect();
        (found.iter().all(|p| allowed(p)), found)
    };
    assert_eq!(names(&real), (true, vec![app.clone()]));
    assert_eq!(names(&linked), (true, Vec::new()));
}
