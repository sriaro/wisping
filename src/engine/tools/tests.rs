use super::*;
use std::fs;
use crate::engine::fsx::security_tests::{fixture, junction};

#[test]
fn empty_backup_is_removed_after_success_or_failure() {
    let base = fixture("tool-empty-backup");
    for fail in [false, true] {
        let dir = base.join(if fail { "failure" } else { "success" });
        fs::create_dir(&dir).unwrap();
        let result = with_backup_cleanup(&dir, || {
            if fail { Err(io::Error::from_raw_os_error(5)) } else { Ok(()) }
        });
        assert_eq!(result.err().and_then(|e| e.raw_os_error()), fail.then_some(5));
        assert!(!dir.exists());
    }
    fs::remove_dir(base).unwrap();
}

#[test]
fn backup_contents_are_preserved_after_success_or_failure() {
    let base = fixture("tool-kept-backup");
    for fail in [false, true] {
        let dir = base.join(if fail { "failure" } else { "success" });
        fs::create_dir(&dir).unwrap();
        let file = dir.join("0001.reg");
        let result = with_backup_cleanup(&dir, || {
            fs::write(&file, b"backup contents")?;
            if fail { Err(io::Error::from_raw_os_error(5)) } else { Ok(()) }
        });
        assert_eq!(result.err().and_then(|e| e.raw_os_error()), fail.then_some(5));
        assert_eq!(fs::read(&file).unwrap(), b"backup contents");
        fs::remove_file(file).unwrap();
        let child = dir.join("nested");
        fs::create_dir(&child).unwrap();
        assert!(with_backup_cleanup(&dir, || Ok(())).is_ok());
        assert!(child.is_dir());
        fs::remove_dir(child).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    fs::remove_dir(base).unwrap();
}

#[test]
fn cleanup_rejects_linked_backup_directories_and_ancestors() {
    let base = fixture("tool-linked-backup");
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    let link = base.join("link");
    junction(&link, &outside);
    assert!(with_backup_cleanup(&link, || Ok(())).is_ok());
    assert!(link.is_dir());
    assert!(outside.is_dir());
    let child = outside.join("backup");
    fs::create_dir(&child).unwrap();
    assert!(with_backup_cleanup(&link.join("backup"), || Ok(())).is_ok());
    assert!(child.is_dir());
    fs::remove_dir(link).unwrap();
    fs::remove_dir(child).unwrap();
    fs::remove_dir(outside).unwrap();
    fs::remove_dir(base).unwrap();
}

#[test]
fn unknown_tool_removes_its_empty_backup_and_keeps_the_error() {
    let base = fixture("tool-unknown");
    let dir = base.join("backup");
    fs::create_dir(&dir).unwrap();
    assert_eq!(run("unknown", &dir).unwrap_err().to_string(), "unknown tool");
    assert!(!dir.exists());
    fs::remove_dir(base).unwrap();
}

#[test]
fn missing_backup_directory_keeps_the_operation_result() {
    let base = fixture("tool-missing-backup");
    let dir = base.join("missing");
    assert!(with_backup_cleanup(&dir, || Ok(())).is_ok());
    let result = with_backup_cleanup(&dir, || Err(io::Error::from_raw_os_error(5)));
    assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
    assert!(!dir.exists());
    fs::remove_dir(base).unwrap();
}
