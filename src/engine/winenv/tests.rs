use super::*;

#[test]
fn user_folder_outside_profile_falls_back() {
    let Some(home) = profile() else { return };
    let program_files = machine("programfiles", 0).unwrap();
    let redirected = user("temp", Some(format!("{}\\Victim", program_files))).unwrap();
    assert_eq!(PathBuf::from(redirected), home.join("AppData\\Local\\Temp"));
    let unc = user("localappdata", Some("\\\\server\\share\\x".to_string())).unwrap();
    assert_eq!(PathBuf::from(unc), home.join("AppData\\Local"));
    let escaped = user("appdata", Some(format!("{}\\..\\..\\Windows", home.display()))).unwrap();
    assert_eq!(PathBuf::from(escaped), home.join("AppData\\Roaming"));
    let kept = home.join("AppData\\Local\\Temp\\Custom");
    assert_eq!(PathBuf::from(user("tmp", Some(kept.to_string_lossy().into_owned())).unwrap()), kept);
}

#[test]
fn machine_folders_ignore_process_environment() {
    let windows = windows_dir();
    assert!(windows.is_absolute() && windows.join("System32").is_dir());
    assert!(system_dir().starts_with(windows));
    let data = PathBuf::from(machine("programdata", 0).unwrap());
    assert!(data.is_absolute() && !data.to_string_lossy().contains('%'));
    assert_eq!(var("WINDIR").map(PathBuf::from).as_deref(), Some(windows));
}

#[test]
fn inside_rejects_relative_and_variable_paths() {
    let base = windows_dir();
    assert!(inside(base, base));
    assert!(inside(&base.join("Temp"), base));
    assert!(!inside(Path::new("Temp"), base));
    assert!(!inside(Path::new("%TEMP%\\x"), base));
    assert!(!inside(&base.join("..\\Users"), base));
}
