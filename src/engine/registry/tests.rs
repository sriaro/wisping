use super::*;

fn issue(hive: Hive, path: &str) -> RegIssue {
    RegIssue::new(hive, "apppath", path, None, "")
}

#[test]
fn app_path_fixes_stay_inside_app_paths() {
    assert!(allowed(&issue(Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\a.exe")));
    assert!(allowed(&issue(Hive::Machine, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\a.exe")));
    assert!(!allowed(&issue(Hive::User, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\a.exe")));
    assert!(!allowed(&issue(Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths")));
    assert!(!allowed(&issue(Hive::Machine, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths\\a.exe\\sub")));
}

#[test]
fn inaccessible_files_are_not_missing() {
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    assert!(!missing(&format!("{windir}\\System32\\cmd.exe")));
    assert!(missing("C:\\NoSuchFolder-wisping-test\\x.exe"));
    assert!(!missing(&format!("{windir}\\System32\\config\\SAM")));
}
