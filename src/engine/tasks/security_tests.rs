use super::*;
use fsx::security_tests::{fixture, junction};

fn task_source(base: &Path) -> PathBuf {
    let tasks = base.join("Tasks");
    fs::create_dir(&tasks).unwrap();
    let file = tasks.join("Updater");
    fs::write(&file, b"<Task>original</Task>").unwrap();
    file
}

#[test]
fn security_task_backup_holds_its_pinned_destination() {
    let base = fixture("task-backup-normal");
    let file = task_source(&base);
    let backup_dir = base.join("data\\backup\\stamp");
    fs::create_dir_all(&backup_dir).unwrap();
    let held = backup(&file, &backup_dir, OsStr::new("task-Updater.xml")).unwrap();
    assert!(fs::rename(&backup_dir, base.join("moved")).is_err());
    assert!(fs::rename(base.join("data\\backup"), base.join("moved")).is_err());
    assert!(fs::write(backup_dir.join("task-Updater.xml"), b"changed").is_err());
    drop(held);
    assert_eq!(fs::read(backup_dir.join("task-Updater.xml")).unwrap(), b"<Task>original</Task>");
}

#[test]
fn security_task_backup_rejects_linked_destinations_and_ancestors() {
    let base = fixture("task-backup-links");
    let file = task_source(&base);
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    let backup_root = base.join("data\\backup");
    fs::create_dir_all(&backup_root).unwrap();
    let linked = backup_root.join("stamp");
    junction(&linked, &outside);
    assert!(backup(&file, &linked, OsStr::new("task-Updater.xml")).is_err());
    let moved = base.join("moved-data");
    fs::create_dir_all(moved.join("backup\\stamp")).unwrap();
    junction(&base.join("linked-data"), &moved);
    assert!(backup(&file, &base.join("linked-data\\backup\\stamp"), OsStr::new("task-Updater.xml")).is_err());
    assert!(!outside.join("task-Updater.xml").exists());
    assert!(!moved.join("backup\\stamp\\task-Updater.xml").exists());
}

#[test]
fn security_task_backup_refuses_existing_destinations_and_non_files() {
    let base = fixture("task-backup-existing");
    let file = task_source(&base);
    let backup_dir = base.join("backup");
    fs::create_dir(&backup_dir).unwrap();
    fs::write(backup_dir.join("task-Updater.xml"), b"existing").unwrap();
    assert!(backup(&file, &backup_dir, OsStr::new("task-Updater.xml")).is_err_and(|e| e.kind() == io::ErrorKind::AlreadyExists));
    assert_eq!(fs::read(backup_dir.join("task-Updater.xml")).unwrap(), b"existing");
    let folder = base.join("Tasks\\Folder");
    fs::create_dir(&folder).unwrap();
    assert!(backup(&folder, &backup_dir, OsStr::new("task-Folder.xml")).is_err());
    junction(&base.join("Tasks\\Linked"), &folder);
    assert!(backup(&base.join("Tasks\\Linked"), &backup_dir, OsStr::new("task-Linked.xml")).is_err());
    assert!(!backup_dir.join("task-Folder.xml").exists());
    assert!(!backup_dir.join("task-Linked.xml").exists());
}

#[test]
fn security_task_backup_refuses_sources_behind_linked_folders() {
    let base = fixture("task-backup-source");
    let backup_dir = base.join("backup");
    fs::create_dir(&backup_dir).unwrap();
    let secret = base.join("secret");
    fs::create_dir(&secret).unwrap();
    fs::write(secret.join("Updater"), b"<Task>secret</Task>").unwrap();
    fs::create_dir(base.join("Tasks")).unwrap();
    junction(&base.join("Tasks\\Evil"), &secret);
    let file = base.join("Tasks\\Evil\\Updater");
    assert!(file.is_file() && !fsx::is_reparse(&file));
    assert!(read_task(&file).is_err());
    assert!(backup(&file, &backup_dir, OsStr::new("task-Updater.xml")).is_err());
    assert!(!backup_dir.join("task-Updater.xml").exists());
    junction(&base.join("LinkedTasks"), &base.join("Tasks"));
    fs::create_dir(base.join("Tasks\\Plain")).unwrap();
    fs::write(base.join("Tasks\\Plain\\Updater"), b"<Task>plain</Task>").unwrap();
    assert!(read_task(&base.join("LinkedTasks\\Plain\\Updater")).is_err());
    assert_eq!(read_task(&base.join("Tasks\\Plain\\Updater")).unwrap(), b"<Task>plain</Task>");
}
