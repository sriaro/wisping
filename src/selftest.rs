use std::fs::{self, FileTimes, OpenOptions};
use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use winreg::RegKey;
use winreg::RegValue;
use winreg::enums::*;

use crate::engine::cleaner::{self, CleanTarget, Source};
use crate::engine::cpu;
use crate::engine::data;
use crate::engine::regbackup::Hive;
use crate::engine::registry::{self, RegIssue};
use crate::engine::startup;
use crate::engine::sys;

struct Log {
    text: String,
    ok: bool,
}

impl Log {
    fn check(&mut self, name: &str, pass: bool) {
        self.text.push_str(&format!("{} {}\n", if pass { "PASS" } else { "FAIL" }, name));
        if !pass {
            self.ok = false;
        }
    }
}

fn set_times(path: &Path, when: SystemTime) {
    let file = OpenOptions::new().write(true).access_mode(0x100).share_mode(7).custom_flags(0x0200_0000).open(path);
    if let Ok(f) = file {
        let _ = f.set_times(FileTimes::new().set_accessed(when).set_modified(when).set_created(when));
    }
}

fn unique(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("{}{}-{}", prefix, std::process::id(), nanos))
}

fn target_for(sources: Vec<Source>) -> CleanTarget {
    let mut t = CleanTarget::custom("usertemp");
    t.sources = sources;
    t
}

fn wipe(dir: &Path) {
    fn walk(d: &Path) {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if let Ok(m) = fs::symlink_metadata(&p) {
                    if m.is_dir() && !crate::engine::fsx::is_reparse(&p) {
                        walk(&p);
                        let _ = fs::remove_dir(&p);
                    } else {
                        let mut perm = m.permissions();
                        #[allow(clippy::permissions_set_readonly_false)]
                        perm.set_readonly(false);
                        let _ = fs::set_permissions(&p, perm);
                        let _ = fs::remove_file(&p);
                    }
                }
            }
        }
    }
    walk(dir);
    let _ = fs::remove_dir(dir);
}

pub fn run(out_file: &str) -> bool {
    let mut log = Log { text: String::new(), ok: true };
    paths(&mut log);
    files(&mut log);
    registry_tests(&mut log);
    age_and_value_tests(&mut log);
    startup_tests(&mut log);
    startup_ext_tests(&mut log);
    regscan_tests(&mut log);
    leftover_tests(&mut log);
    diskmap_tests(&mut log);
    tools_tests(&mut log);
    programs_tests(&mut log);
    classify_tests(&mut log);
    policy_tests(&mut log);
    engine_ext_tests(&mut log);
    data_tests(&mut log);
    backup_check_tests(&mut log);
    single_instance_tests(&mut log);
    i18n_tests(&mut log);
    log.text.push_str(if log.ok { "ALL PASS\n" } else { "SOME FAILED\n" });
    let _ = fs::write(out_file, &log.text);
    log.ok
}

fn paths(log: &mut Log) {
    log.check("first_path quoted", registry::first_path("\"C:\\Program Files\\X\\a.exe\" /s").as_deref() == Some("C:\\Program Files\\X\\a.exe"));
    log.check("first_path unquoted with space", registry::first_path("C:\\Program Files\\X Y\\a.exe /silent").as_deref() == Some("C:\\Program Files\\X Y\\a.exe"));
    log.check("first_path keeps dotted folders", registry::first_path("C:\\Tools\\app.exe.d\\run.exe /x").as_deref() == Some("C:\\Tools\\app.exe.d\\run.exe"));
    let odd = unique("pc-selftest-odd-");
    let odd_dir = odd.join("X.exe Files");
    let odd_file = odd_dir.join("app.exe");
    let made = fs::create_dir_all(&odd_dir).and_then(|_| fs::write(&odd_file, b"x")).is_ok();
    log.check("first_path keeps a real path whose folder contains .exe and a space", made && registry::first_path(&odd_file.to_string_lossy()).as_deref() == Some(odd_file.to_string_lossy().as_ref()));
    wipe(&odd);
    log.check("extension and openwith names must be plain", !crate::engine::regscan::allowed("extension", Hive::User, "software\\classes\\.") && !crate::engine::regscan::allowed("openwith", Hive::User, "software\\classes\\applications\\a/b.exe"));
    log.check(
        "cut_at_ext survives characters that change length when lowercased",
        registry::cut_at_ext("C:\\\u{23a}\\a.exe\u{e9}\u{1e9e}", &[".exe"]).as_deref() == Some("C:\\\u{23a}\\a.exe") && registry::cut_at_ext("C:\\\u{23a}\\a.exe /x\u{1e9e}", &[".exe"]).as_deref() == Some("C:\\\u{23a}\\a.exe"),
    );
    log.check("first_path rundll32",registry::first_path("rundll32.exe a.dll,Run").as_deref() == Some("rundll32.exe"));
    log.check("missing unrooted is unknown", !registry::missing("rundll32.exe"));
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    log.check("missing existing file", !registry::missing(&format!("{}\\System32\\cmd.exe", windir)));
    log.check("missing gone file", registry::missing("C:\\NoSuchFolder-pc-test\\x.exe"));
    log.check("missing absent drive is unknown", !registry::missing("Q:\\x.exe") || Path::new("Q:\\").is_dir());
    log.check("missing UNC is unknown", !registry::missing("\\\\server\\share\\x.exe"));
}

fn files(log: &mut Log) {
    let boxdir = unique("pc-selftest-");
    let work = boxdir.join("work");
    let old_a = work.join("old.txt");
    let old_b = work.join("sub").join("deep").join("old2.txt");
    let old_ro = work.join("readonly.txt");
    let fresh = work.join("keep").join("fresh.txt");
    for d in [work.join("keep"), work.join("sub").join("deep"), work.join("emptydir"), work.join("freshempty")] {
        fs::create_dir_all(&d).unwrap();
    }
    for f in [&old_a, &old_b, &old_ro, &fresh] {
        fs::write(f, b"0123456789").unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(3 * 86400);
    for f in [&old_a, &old_b, &old_ro] {
        set_times(f, old);
    }
    let mut perm = fs::metadata(&old_ro).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&old_ro, perm).unwrap();
    for d in [work.join("sub").join("deep"), work.join("sub"), work.join("emptydir")] {
        set_times(&d, old);
    }

    let mut t = target_for(vec![Source::new(work.to_str().unwrap(), "*", true, 24)]);
    cleaner::scan(&mut t);
    log.check("scan counts only old files", t.count == 3 && t.bytes == 30);
    cleaner::clean(&mut t);
    log.check("old files deleted incl. read-only", !old_a.exists() && !old_b.exists() && !old_ro.exists());
    log.check("fresh file kept", fresh.exists());
    log.check("emptied subfolders removed", !work.join("sub").exists() && !work.join("emptydir").exists());
    log.check("root kept", work.is_dir());
    log.check("fresh empty folder kept", work.join("freshempty").is_dir());
    log.check("freed counted", t.freed == 30 && t.failed == 0);

    let mask_dir = boxdir.join("mask");
    fs::create_dir_all(&mask_dir).unwrap();
    fs::write(mask_dir.join("a.lnk"), b"x").unwrap();
    fs::write(mask_dir.join("b.lnkx"), b"x").unwrap();
    let mut mt = target_for(vec![Source::new(mask_dir.to_str().unwrap(), "*.lnk", false, 0)]);
    cleaner::scan(&mut mt);
    log.check("extension mask is exact", mt.count == 1);
    cleaner::clean(&mut mt);
    log.check("longer extension survives", !mask_dir.join("a.lnk").exists() && mask_dir.join("b.lnkx").exists());

    let det = boxdir.join("det");
    fs::create_dir_all(det.join("subA")).unwrap();
    fs::write(det.join("subA").join("x.bin"), b"12345").unwrap();
    fs::write(det.join("subA").join("y.bin"), b"1234567").unwrap();
    fs::write(det.join("top.txt"), b"abc").unwrap();
    let dt = target_for(vec![Source::new(det.to_str().unwrap(), "*", true, 0)]);
    let rows = cleaner::details(&dt);
    log.check(
        "details group by top entry, biggest first",
        rows.len() == 2 && rows[0].name == "subA" && rows[0].bytes == 12 && rows[0].count == 2 && rows[1].name == "top.txt" && rows[1].bytes == 3,
    );
    log.check(
        "details carry the real location of each entry",
        rows.iter().all(|r| std::path::Path::new(&r.path).exists()) && rows[0].path.ends_with("subA") && rows[1].path.ends_with("top.txt"),
    );
    log.check("details change nothing",det.join("subA").join("x.bin").exists() && det.join("top.txt").exists());

    let uni = boxdir.join("uni");
    fs::create_dir_all(&uni).unwrap();
    let names = ["吉 日本語.txt", "Привет мир.txt", "مرحبا بالعالم.txt", "😀 👍 𝕏.txt", "नमस्ते दुनिया.txt", "สวัสดีชาวโลก.txt", "café résumé.txt", "안녕하세요.txt"];
    let made = names.iter().filter(|n| fs::write(uni.join(n), b"12345").is_ok()).count();
    let mut ut = target_for(vec![Source::new(uni.to_str().unwrap(), "*", true, 0)]);
    cleaner::scan(&mut ut);
    let urows = cleaner::details(&ut);
    log.check(
        "names in every script are scanned and listed with real paths",
        made == names.len() && ut.count == names.len() as u64 && urows.len() == names.len() && urows.iter().all(|r| std::path::Path::new(&r.path).exists() && names.contains(&r.name.as_str())),
    );
    cleaner::clean(&mut ut);
    log.check("names in every script are cleaned", ut.freed == 5 * names.len() as u64 && ut.failed == 0 && fs::read_dir(&uni).map(|d| d.count()).unwrap_or(1) == 0);
    log.check("oem text decoding keeps ascii", sys::oem_text(b"Access is denied.\r\n") == "Access is denied.");
    log.check("empty tool output still gives a failure message", sys::command_failure(b"", Some(5)) == "failed (exit code 5)" && sys::command_failure(b"x\r\n", Some(5)) == "x");
    log.check(
        "bidi control characters are neutralised in listed names",
        crate::views::ltr("a\u{202E}txt.exe") == "a\u{FFFD}txt.exe" && crate::views::ltr("plain.txt") == "plain.txt" && crate::views::ltr("\u{05D0}.txt").starts_with('\u{200E}') && crate::views::ltr("\u{10800}.txt").starts_with('\u{200E}'),
    );

    let models = boxdir.join("models");
    let digest = "a".repeat(64);
    fs::create_dir_all(models.join("blobs")).unwrap();
    fs::create_dir_all(models.join("manifests").join("registry.ollama.ai").join("library").join("qwen3")).unwrap();
    fs::write(models.join("manifests").join("registry.ollama.ai").join("library").join("qwen3").join("8b"), format!("{{\"layers\":[{{\"digest\":\"sha256:{}\"}}]}}", digest)).unwrap();
    fs::write(models.join("blobs").join(format!("sha256-{}", digest)), b"0123456789").unwrap();
    fs::write(models.join("blobs").join(format!("sha256-{}", "b".repeat(64))), b"0123").unwrap();
    let mut mt2 = CleanTarget::custom("aimodels");
    mt2.sources = vec![Source::new(models.to_str().unwrap(), "*", true, 0)];
    let mrows = cleaner::details(&mt2);
    log.check(
        "details name ollama models from manifests",
        mrows.iter().any(|d| d.name == "qwen3:8b" && d.bytes == 10) && mrows.iter().any(|d| d.name.starts_with("blobs\\sha256-b") && d.bytes == 4) && mrows.iter().any(|d| d.name == "Model list"),
    );

    let jroot = boxdir.join("jroot");
    let jtarget = boxdir.join("jtarget");
    fs::create_dir_all(&jroot).unwrap();
    fs::create_dir_all(&jtarget).unwrap();
    let victim = jtarget.join("victim.txt");
    fs::write(&victim, b"keep me").unwrap();
    let link = jroot.join("link");
    let made = sys::hidden("cmd.exe").arg("/c").arg("mklink").arg("/J").arg(&link).arg(&jtarget).output().map(|o| o.status.success()).unwrap_or(false);
    if made && link.is_dir() {
        let mut jt = target_for(vec![Source::new(jroot.to_str().unwrap(), "*", true, 0)]);
        cleaner::scan(&mut jt);
        cleaner::clean(&mut jt);
        log.check("junction inside is not followed", victim.exists() && link.is_dir());
        let mut jr = target_for(vec![Source::new(link.to_str().unwrap(), "*", true, 0)]);
        cleaner::scan(&mut jr);
        cleaner::clean(&mut jr);
        log.check("junction as root is refused", jr.count == 0 && victim.exists());
        let _ = fs::remove_dir(&link);
    } else {
        log.check("junction test could not create a junction", false);
    }

    data::ensure_root();
    let mut own = target_for(vec![Source::new(data::root().to_str().unwrap(), "*", true, 0)]);
    cleaner::scan(&mut own);
    log.check("own data folder is refused", own.count == 0);
    log.check("own data is protected by path", data::is_own(&data::backup_root().join("x").join("1.reg")));
    log.check("exe folder is protected", data::is_own(&data::exe_dir().join("Wisp.exe")));
    log.check("other temp is not protected", !data::is_own(&old_a));

    let mut drive = target_for(vec![Source::new("C:\\", "*", true, 0)]);
    cleaner::scan(&mut drive);
    log.check("drive root is refused", drive.count == 0);
    let mut user = target_for(vec![Source::new("%USERPROFILE%", "*", true, 0)]);
    cleaner::scan(&mut user);
    log.check("profile root is refused", user.count == 0);
    let mut win = target_for(vec![Source::new("%WINDIR%", "*", true, 0)]);
    cleaner::scan(&mut win);
    log.check("windows folder is refused", win.count == 0);
    let mut unc = target_for(vec![Source::new("\\\\localhost\\c$\\Users", "*", true, 0)]);
    cleaner::scan(&mut unc);
    log.check("network path is refused", unc.count == 0);

    wipe(&boxdir);
}

fn age_and_value_tests(log: &mut Log) {
    let boxdir = unique("pc-selftest-age-");
    fs::create_dir_all(&boxdir).unwrap();
    let new_file = boxdir.join("new.txt");
    let old_file = boxdir.join("old.txt");
    fs::write(&new_file, b"0123456789").unwrap();
    fs::write(&old_file, b"0123456789").unwrap();
    set_times(&old_file, SystemTime::now() - Duration::from_secs(3 * 86400));
    let mut keep = target_for(vec![Source::new(boxdir.to_str().unwrap(), "*", false, 24)]);
    cleaner::set_keep_recent(false);
    cleaner::scan(&mut keep);
    log.check("all-ages scan counts new and old files", keep.count == 2 && keep.bytes == 20);
    cleaner::clean(&mut keep);
    cleaner::set_keep_recent(true);
    log.check("all-ages clean deletes new and old files", !new_file.exists() && !old_file.exists() && keep.freed == 20 && keep.removed == 2);
    let _ = fs::remove_dir(&boxdir);

    let base = format!("{}\\RegClear", registry::TEST_KEY);
    let (key, _) = hkcu().create_subkey(&base).unwrap();
    key.set_value("One", &"a").unwrap();
    key.set_value("Two", &7u32).unwrap();
    let (sub, _) = hkcu().create_subkey(format!("{}\\Sub", base)).unwrap();
    sub.set_value("Three", &"c").unwrap();
    let mut rt = CleanTarget::custom("explorer");
    rt.reg.push(base.clone());
    registry::set_allow_test_key(true);
    cleaner::scan(&mut rt);
    log.check("registry target counts values", rt.count == 3);
    cleaner::clean(&mut rt);
    registry::set_allow_test_key(false);
    let after = hkcu().open_subkey(&base).unwrap();
    let sub_after = hkcu().open_subkey(format!("{}\\Sub", base)).unwrap();
    log.check(
        "registry target clears values and keeps keys",
        rt.removed == 3 && rt.failed == 0 && after.enum_values().count() == 0 && sub_after.enum_values().count() == 0,
    );

    let (guard, _) = hkcu().create_subkey(format!("{}Guard2", registry::TEST_KEY)).unwrap();
    guard.set_value("Keep", &"x").unwrap();
    let mut gt = CleanTarget::custom("explorer");
    gt.reg.push(format!("{}Guard2", registry::TEST_KEY));
    gt.reg.push("Software\\Microsoft".to_string());
    cleaner::scan(&mut gt);
    cleaner::clean(&mut gt);
    log.check("registry target refuses paths outside the allow-list", gt.count == 0 && gt.removed == 0 && guard.get_value::<String, _>("Keep").is_ok());
    let _ = hkcu().delete_subkey_all(format!("{}Guard2", registry::TEST_KEY));

    let vbase = format!("{}\\ValueOnly", registry::TEST_KEY);
    let (vk, _) = hkcu().create_subkey(&vbase).unwrap();
    vk.set_value("Target", &"a").unwrap();
    vk.set_value("Other", &"b").unwrap();
    let mut vt = CleanTarget::custom("wintools");
    vt.reg.push(format!("{}|Target", vbase));
    vt.reg.push(format!("{}|", vbase));
    vt.reg.push("Software\\Microsoft\\Windows\\CurrentVersion\\Run|Anything".to_string());
    registry::set_allow_test_key(true);
    cleaner::scan(&mut vt);
    log.check("single value entry counts one value", vt.count == 1);
    cleaner::clean(&mut vt);
    registry::set_allow_test_key(false);
    let vk = hkcu().open_subkey(&vbase).unwrap();
    log.check("single value entry removes only that value", vt.removed == 1 && vk.get_raw_value("Target").is_err() && vk.get_raw_value("Other").is_ok());
    let _ = hkcu().delete_subkey_all(registry::TEST_KEY);

    let rank = |g: &str| match g { "windows" => 0, "browsers" => 1, "apps" => 2, _ => 3 };
    let ranks: Vec<u8> = cleaner::targets().iter().map(|t| rank(cleaner::group_of(&t.id))).collect();
    log.check("cleaner targets are grouped in order", ranks.windows(2).all(|w| w[0] <= w[1]));
    let all = cleaner::targets();
    let ai: Vec<_> = all.iter().filter(|t| cleaner::group_of(&t.id) == "ai").collect();
    log.check("ai software targets exist", ai.len() == 6);
    log.check("high risk targets are never selected by default", all.iter().filter(|t| cleaner::risk_of(&t.id) == 2).all(|t| !t.selected) && ai.iter().any(|t| cleaner::risk_of(&t.id) == 2));
    log.check(
        "ai targets never reach chats, settings or credentials",
        ai.iter().flat_map(|t| t.sources.iter()).all(|s| {
            let d = s.dir.to_lowercase();
            !d.ends_with("\\.claude") && !d.contains("\\.claude\\projects") && !d.contains("settings") && !d.contains("local storage") && !d.contains("indexeddb") && !d.contains("\\network") && !d.contains("session storage")
        }),
    );
    log.check("every ai source stays inside a named subfolder", ai.iter().flat_map(|t| t.sources.iter()).all(|s| s.dir.matches('\\').count() >= 2 && s.mask == "*"));
    log.check(
        "every cleaner target has a title and description",
        cleaner::targets().iter().all(|t| crate::i18n::get(&format!("t.{}", t.id)) != format!("t.{}", t.id) && crate::i18n::get(&format!("d.{}", t.id)) != format!("d.{}", t.id)),
    );
}

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}

fn registry_tests(log: &mut Log) {
    let test_key = registry::TEST_KEY;
    let backup = unique("pc-selftest-backup-");
    fs::create_dir_all(&backup).unwrap();
    let (child, _) = hkcu().create_subkey(format!("{}\\Child", test_key)).unwrap();
    child.set_value("A", &"x\"y\\z").unwrap();
    let (k, _) = hkcu().create_subkey(test_key).unwrap();
    k.set_value("Dword", &305419896u32).unwrap();
    k.set_value("Str", &"C:\\a\\b \"q\"").unwrap();
    let mut expand = Vec::new();
    for u in "%WINDIR%\\x".encode_utf16().chain(std::iter::once(0u16)) {
        expand.extend_from_slice(&u.to_le_bytes());
    }
    k.set_raw_value("Exp", &RegValue { bytes: expand.into(), vtype: REG_EXPAND_SZ }).unwrap();
    k.set_raw_value("Bin", &RegValue { bytes: vec![1u8, 2].into(), vtype: REG_BINARY }).unwrap();

    let guard_key = format!("{}Guard", test_key);
    let _ = hkcu().create_subkey(&guard_key).unwrap();
    let mut refused = Vec::new();
    registry::set_allow_test_key(true);
    let guard_fixed = registry::fix(
        &[
            RegIssue::new(Hive::User, "apppath", &guard_key, None, ""),
            RegIssue::new(Hive::User, "uninstall", "SOFTWARE\\Microsoft", None, ""),
            RegIssue::new(Hive::Machine, "apppath", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths", None, ""),
            RegIssue::new(Hive::Machine, "apppath", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\a\\b", None, ""),
            RegIssue::new(Hive::User, "muicache", "Software\\Microsoft", Some("x"), ""),
        ],
        &backup,
        &mut refused,
    );
    registry::set_allow_test_key(false);
    log.check("registry guard refuses keys outside the allow-list", guard_fixed == 0 && refused.len() == 5 && hkcu().open_subkey(&guard_key).is_ok());
    let _ = hkcu().delete_subkey_all(&guard_key);

    let issues = vec![
        RegIssue::new(Hive::User, "muicache", test_key, Some("Dword"), ""),
        RegIssue::new(Hive::User, "muicache", test_key, Some("Str"), ""),
        RegIssue::new(Hive::User, "muicache", test_key, Some("Exp"), ""),
        RegIssue::new(Hive::User, "muicache", test_key, Some("Bin"), ""),
        RegIssue::new(Hive::User, "apppath", &format!("{}\\Child", test_key), None, ""),
    ];
    let mut errors = Vec::new();
    registry::set_allow_test_key(true);
    let fixed = registry::fix(&issues, &backup, &mut errors);
    registry::set_allow_test_key(false);
    log.check("registry fix: 4 fixed, binary refused", fixed == 4 && errors.len() == 1);
    let after = hkcu().open_subkey(test_key).unwrap();
    log.check(
        "registry values removed",
        after.get_raw_value("Dword").is_err() && after.get_raw_value("Str").is_err() && after.get_raw_value("Exp").is_err(),
    );
    log.check("binary value untouched", after.get_raw_value("Bin").is_ok());
    log.check("child key removed", after.open_subkey("Child").is_err());
    drop(after);

    let files: Vec<PathBuf> = fs::read_dir(&backup).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "reg").unwrap_or(false)).collect();
    log.check("backup files written", files.len() == 4);
    let mut restored = false;
    for f in &files {
        let bytes = fs::read(f).unwrap_or_default();
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let text = String::from_utf16_lossy(&units);
        if text.contains("\"Dword\"=dword:12345678") || text.contains("\"Str\"=\"C:\\\\a\\\\b \\\"q\\\"\"") || text.contains("hex(2):") {
            restored = true;
        }
    }
    log.check("backup content matches", restored);

    let mui_path = "Software\\Classes\\Local Settings\\Software\\Microsoft\\Windows\\Shell\\MuiCache";
    let gone = format!("{}.exe", unique("pc-mui-").display());
    let there = format!("{}.exe", unique("pc-mui-").display());
    fs::write(&there, b"x").unwrap();
    let gone_name = format!("{}.FriendlyAppName", gone);
    let there_name = format!("{}.FriendlyAppName", there);
    let (mui, _) = hkcu().create_subkey(mui_path).unwrap();
    mui.set_value(&gone_name, &"Gone").unwrap();
    mui.set_value(&there_name, &"There").unwrap();
    let live = registry::run();
    let gone_issue = live.iter().find(|x| x.value_name.as_deref() == Some(gone_name.as_str())).cloned();
    log.check(
        "scan lists a stale MUI entry, not a live one",
        gone_issue.is_some() && !live.iter().any(|x| x.value_name.as_deref() == Some(there_name.as_str())),
    );
    log.check("MUI entries are selected by default", gone_issue.as_ref().map(|g| g.selected).unwrap_or(false));
    let mut live_errors = Vec::new();
    let live_fixed = gone_issue.map(|g| registry::fix(&[g], &backup, &mut live_errors)).unwrap_or(0);
    let mui = hkcu().open_subkey(mui_path).unwrap();
    log.check("real scan result is fixed with a backup", live_fixed == 1 && mui.get_raw_value(&gone_name).is_err() && mui.get_raw_value(&there_name).is_ok());
    drop(mui);
    let _ = fs::remove_file(&there);
    let stale = registry::run().into_iter().find(|x| x.value_name.as_deref() == Some(there_name.as_str()));
    fs::write(&there, b"x").unwrap();
    let mut stale_errors = Vec::new();
    let stale_fixed = stale.as_ref().map(|s| registry::fix(std::slice::from_ref(s), &backup, &mut stale_errors)).unwrap_or(0);
    let mui = hkcu().open_subkey(mui_path).unwrap();
    log.check("an entry that became valid is not removed", stale.is_some() && stale_fixed == 0 && stale_errors.len() == 1 && mui.get_raw_value(&there_name).is_ok());
    drop(mui);

    if let Ok(w) = hkcu().open_subkey_with_flags(mui_path, KEY_WRITE) {
        let _ = w.delete_value(&gone_name);
        let _ = w.delete_value(&there_name);
    }
    let _ = fs::remove_file(&there);
    let _ = hkcu().delete_subkey_all(test_key);
    wipe(&backup);
}

fn startup_ext_tests(log: &mut Log) {
    use crate::engine::fsx;
    use crate::engine::shellmenu;
    use crate::engine::startup::{StartupEntry, StartupKind};
    use crate::engine::tasks;

    log.check("protected: bare system exe", fsx::is_protected("cmd.exe"));
    log.check("protected: windows folder file", fsx::is_protected("C:\\Windows\\System32\\drivers\\x.sys") || !Path::new("C:\\Windows").is_dir());
    log.check("protected: defender platform", fsx::is_protected("C:\\ProgramData\\Microsoft\\Windows Defender\\Platform\\1\\MsMpEng.exe"));
    log.check("not protected: program files app", !fsx::is_protected("C:\\Program Files\\Some App\\app.exe"));

    let xml = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task xmlns=\"x\"><RegistrationInfo><Author>Acme &amp; Co</Author></RegistrationInfo><Settings><Enabled>false</Enabled></Settings><Actions><Exec><Command>\"C:\\Acme\\a.exe\"</Command><Arguments>/quiet</Arguments></Exec></Actions></Task>";
    let p = tasks::parse(xml);
    log.check("task xml parsed", p.command == "\"C:\\Acme\\a.exe\" /quiet" && p.author == "Acme & Co" && !p.enabled);
    let mut utf16 = vec![0xFFu8, 0xFE];
    for u in xml.encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    log.check("task xml utf-16 decoded", tasks::parse(&tasks::decode(&utf16)).command == p.command);
    log.check("task defaults to enabled", tasks::parse("<Task><Actions><Exec><Command>a.exe</Command></Exec></Actions></Task>").enabled);

    let all = startup::list();
    log.check("tasks under Microsoft\\Windows are protected", all.iter().filter(|e| e.kind == StartupKind::Task && e.key_path.to_lowercase().starts_with("\\microsoft\\windows\\")).all(|e| e.system));
    log.check("services list has entries", all.iter().any(|e| e.kind == StartupKind::Service));
    log.check("core services are protected", all.iter().filter(|e| e.kind == StartupKind::Service && ["WinDefend", "RpcSs", "EventLog"].contains(&e.value_name.as_str())).all(|e| e.system));
    let mut protected = all.iter().find(|e| e.system && e.kind == StartupKind::Service).cloned();
    if let Some(e) = protected.as_mut() {
        let toggled = startup::set_enabled(e, !e.enabled);
        let deleted = startup::delete(e, Path::new("."));
        log.check("protected items refuse changes", toggled.is_err() && deleted.is_err());
    } else {
        log.check("protected items refuse changes", false);
    }

    registry::set_allow_test_key(true);
    let base = format!("{}\\CtxTest\\shell\\Verb", registry::TEST_KEY);
    let (k, _) = hkcu().create_subkey(format!("{}\\command", base)).unwrap();
    k.set_value("", &"\"C:\\NoSuchFolder-pc-test\\x.exe\" \"%1\"").unwrap();
    let mut e = StartupEntry { kind: StartupKind::Context, hive: Hive::User, key_path: base.clone(), name: "Verb".to_string(), enabled: true, ..Default::default() };
    let verb = || hkcu().open_subkey(&base);
    log.check("context verb disabled", startup::set_enabled(&mut e, false).is_ok() && verb().map(|k| k.get_raw_value("LegacyDisable").is_ok()).unwrap_or(false));
    log.check("context verb enabled again", startup::set_enabled(&mut e, true).is_ok() && verb().map(|k| k.get_raw_value("LegacyDisable").is_err()).unwrap_or(false));
    let mut outside = StartupEntry { kind: StartupKind::Context, hive: Hive::User, key_path: "Software\\Classes\\Directory\\shell".to_string(), ..Default::default() };
    log.check("context refuses keys outside shell verbs", startup::set_enabled(&mut outside, false).is_err() && startup::delete(&outside, Path::new(".")).is_err());
    let backup = unique("pc-selftest-ctx-");
    fs::create_dir_all(&backup).unwrap();
    let removed = startup::delete(&e, &backup).is_ok();
    let files = fs::read_dir(&backup).map(|rd| rd.flatten().count()).unwrap_or(0);
    log.check("context verb deleted with backup", removed && verb().is_err() && files == 1);
    wipe(&backup);

    let clsid = "{00000000-0000-4000-8000-0000504353F1}";
    log.check("handler block written", shellmenu::set_blocked(Hive::User, clsid, true).is_ok() && shellmenu::is_blocked(clsid));
    log.check("handler block removed", shellmenu::set_blocked(Hive::User, clsid, false).is_ok() && !shellmenu::is_blocked(clsid));
    log.check("handler block refuses bad id", shellmenu::set_blocked(Hive::User, "not-a-guid", true).is_err());
    registry::set_allow_test_key(false);
    let _ = hkcu().delete_subkey_all(registry::TEST_KEY);
}

fn startup_tests(log: &mut Log) {
    let run_key = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    let backup = unique("pc-selftest-startup-");
    fs::create_dir_all(&backup).unwrap();
    let name = "WispSelfTest";
    let (k, _) = hkcu().create_subkey(run_key).unwrap();
    k.set_value(name, &"\"C:\\NoSuchFolder-pc-test\\x.exe\" --go").unwrap();
    let find = || startup::list().into_iter().find(|x| x.name == name);
    let entry = find();
    log.check("startup entry listed and missing", entry.as_ref().map(|e| e.enabled && e.missing).unwrap_or(false));
    if let Some(mut e) = entry {
        let _ = startup::set_enabled(&mut e, false);
        log.check("startup disabled", find().map(|x| !x.enabled).unwrap_or(false));
        let mut again = find().unwrap();
        let _ = startup::set_enabled(&mut again, true);
        log.check("startup enabled again", find().map(|x| x.enabled).unwrap_or(false));
        let _ = startup::delete(&e, &backup);
        let count = fs::read_dir(&backup).map(|rd| rd.flatten().count()).unwrap_or(0);
        log.check("startup deleted with backup", find().is_none() && count == 1);
    }
    if let Ok(w) = hkcu().open_subkey_with_flags(run_key, KEY_WRITE) {
        let _ = w.delete_value(name);
    }
    if let Ok(w) = hkcu().open_subkey_with_flags("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run", KEY_WRITE) {
        let _ = w.delete_value(name);
    }
    wipe(&backup);
}

fn regscan_tests(log: &mut Log) {
    use crate::engine::regscan;
    log.check("guid accepted", regscan::is_guid("{00000100-0000-0010-8000-00AA006D2EA4}"));
    log.check("guid rejects other names", !regscan::is_guid("{not-a-guid}") && !regscan::is_guid("Photoshop.File") && !regscan::is_guid("{00000100-0000-0010-8000-00AA006D2EA}"));
    log.check("server path from quoted exe", regscan::server_path("\"C:\\A B\\x.exe\" /Embedding").as_deref() == Some("C:\\A B\\x.exe"));
    log.check("server path from dll", regscan::server_path("C:\\Dir\\lib.dll").as_deref() == Some("C:\\Dir\\lib.dll"));
    log.check("server path rejects unknown", regscan::server_path("thing").is_none());
    let com = "software\\classes\\clsid\\{00000100-0000-0010-8000-00aa006d2ea4}";
    log.check("com key allowed in classes", regscan::allowed("com", Hive::User, com));
    log.check("com refuses non guid and nested keys", !regscan::allowed("com", Hive::User, "software\\classes\\clsid\\other") && !regscan::allowed("com", Hive::User, &format!("{}\\inprocserver32", com)));
    log.check("com refuses wow64 path under user hive", !regscan::allowed("com", Hive::User, "software\\classes\\wow6432node\\clsid\\{00000100-0000-0010-8000-00aa006d2ea4}"));
    log.check(
        "extension allowed only for user dot keys",
        regscan::allowed("extension", Hive::User, "software\\classes\\.psdc") && !regscan::allowed("extension", Hive::Machine, "software\\classes\\.psdc") && !regscan::allowed("extension", Hive::User, "software\\classes\\clsid"),
    );
    log.check(
        "openwith allowed only for applications exe keys",
        regscan::allowed("openwith", Hive::User, "software\\classes\\applications\\x.exe")
            && !regscan::allowed("openwith", Hive::User, "software\\classes\\applications\\x.txt")
            && !regscan::allowed("openwith", Hive::User, "software\\classes\\applications"),
    );
}

fn diskmap_tests(log: &mut Log) {
    use crate::engine::diskmap::{self, dups, treemap, Kind};
    use std::sync::atomic::AtomicBool;

    let root = unique("pc-selftest-disk-");
    let a = root.join("a");
    let b = root.join("b").join("deep");
    let other = unique("pc-selftest-disk-target-");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::create_dir_all(&other).unwrap();
    let mut same = vec![7u8; 1_500_000];
    fs::write(a.join("one.bin"), &same).unwrap();
    fs::write(b.join("two.bin"), &same).unwrap();
    same[700_000] = 9;
    fs::write(a.join("three.bin"), &same).unwrap();
    fs::write(a.join("note.txt"), b"hello").unwrap();
    fs::write(a.join("note2.txt"), b"hello!").unwrap();
    fs::write(other.join("outside.bin"), vec![1u8; 500_000]).unwrap();
    let link = root.join("link");
    let linked = sys::hidden("cmd.exe").arg("/C").arg("mklink").arg("/J").arg(&link).arg(&other).output().map(|o| o.status.success()).unwrap_or(false);

    let progress = diskmap::Progress::default();
    let tree = diskmap::scan(&root, &progress);
    let expected = 3 * 1_500_000u64 + 5 + 6;
    log.check("disk scan totals bytes and files", tree.nodes[0].size == expected && tree.nodes[0].files == 5);
    log.check("disk scan skips junctions", !linked || tree.nodes[0].size == expected);
    log.check("disk scan folds small files", tree.nodes.iter().any(|n| n.kind == Kind::Small && n.files == 2 && n.size == 11));
    let a_idx = tree.children(0).find(|&i| tree.name_of(i) == "a").unwrap();
    log.check("disk folder size aggregates", tree.nodes[a_idx as usize].size == 2 * 1_500_000 + 11);
    log.check("disk path round trip", tree.path_of(a_idx) == a);
    log.check("disk largest files sorted", {
        let l = tree.largest_files(10);
        l.len() == 3 && l.windows(2).all(|w| tree.nodes[w[0] as usize].size >= tree.nodes[w[1] as usize].size)
    });
    let groups = dups::find(&tree, &AtomicBool::new(false));
    log.check("disk finds identical files and ignores near copies", groups.len() == 1 && groups[0].members.len() == 2 && groups[0].wasted() == 1_500_000);

    let sizes = [500u64, 300, 120, 50, 20, 10];
    let rects = treemap::layout(&sizes, 400.0, 250.0);
    let area: f64 = rects.iter().map(|r| r.w * r.h).sum();
    log.check("treemap fills the area", rects.len() == sizes.len() && (area - 100_000.0).abs() < 1.0);
    log.check("treemap stays inside bounds", rects.iter().all(|r| r.x >= -0.001 && r.y >= -0.001 && r.x + r.w <= 400.001 && r.y + r.h <= 250.001));
    log.check(
        "treemap areas are proportional",
        rects.iter().zip(sizes.iter()).all(|(r, s)| ((r.w * r.h) / 100_000.0 - *s as f64 / 1000.0).abs() < 0.001),
    );
    log.check("treemap handles empty input", treemap::layout(&[], 100.0, 100.0).is_empty() && treemap::layout(&[5], 0.0, 100.0).is_empty());

    let cancelled = diskmap::Progress::default();
    cancelled.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let partial = diskmap::scan(&root, &cancelled);
    log.check("disk scan honours cancel", partial.nodes[0].size == 0);

    if linked {
        let _ = fs::remove_dir(&link);
    }
    wipe(&root);
    wipe(&other);
}

fn engine_ext_tests(log: &mut Log) {
    use crate::engine::fileinfo;
    use crate::engine::fsx::{self, Fence, Mask, Removal};
    use crate::engine::regbackup;
    use std::ffi::OsStr;

    let m = |mask: &str, name: &str| Mask::new(mask).matches(OsStr::new(name));
    log.check("mask any and exact", m("*", "x.bin") && m("MEMORY.DMP", "memory.dmp") && !m("MEMORY.DMP", "memory.dmp1"));
    log.check(
        "mask prefix, suffix and both",
        m("MP*.log", "mpLog-1.LOG") && !m("MP*.log", "xmp.log") && m("*.lnk", "a.LNK") && !m("*.lnk", "a.lnkx") && m("thumbcache_*.db", "thumbcache_256.db") && !m("thumbcache_*.db", "thumbcache_.dbx"),
    );
    log.check("mask general patterns", m("a?c*", "abcdef") && !m("a?c*", "acdef") && m("*.*", "x.y") && !m("*.*", "xy"));
    log.check("mask with non-ascii names", m("*", "\u{444}\u{430}\u{439}\u{43b}.txt") && m("*.txt", "\u{444}.TXT") && !m("Cookies", "Cooki\u{e9}s"));
    log.check("mask kinds", Mask::new("*") == Mask::Any && matches!(Mask::new("*.*"), Mask::General(_)) && matches!(Mask::new("cookies.sqlite*"), Mask::Prefix(_)));

    log.check("protected: bare windows exe outside system32", fsx::is_protected("explorer.exe") && fsx::is_protected("powershell.exe"));
    log.check("not protected: unknown bare exe", !fsx::is_protected("pc-no-such-tool.exe"));
    log.check("direct child", fsx::direct_child("a\\b\\c", "a\\b\\") && !fsx::direct_child("a\\b\\c\\d", "a\\b\\") && !fsx::direct_child("a\\b\\", "a\\b\\"));
    let reg = sys::system_tool("reg.exe");
    log.check(
        "system tools resolve to the system folder",
        reg.is_absolute() && fsx::under_windir(&reg.to_string_lossy()) && sys::system_tool("explorer.exe").is_absolute() && sys::system_tool("C:\\x\\y.exe") == *"C:\\x\\y.exe",
    );
    let items: Vec<u32> = (0..1000).collect();
    let topo = cpu::topology();
    log.check("cpu topology is consistent", topo.fast_cores >= 1 && topo.fast_cores <= topo.cores && topo.cores <= topo.logical);
    let hybrid = cpu::shape(32, &[[(1u8, 2usize); 8].as_slice(), [(0u8, 1usize); 16].as_slice()].concat(), 1);
    let amd = cpu::shape(32, &[(0u8, 2usize); 16], 2);
    let pinned = cpu::shape(8, &[(0u8, 2usize); 16], 2);
    log.check(
        "cpu shapes for hybrid, multi-ccd and pinned processors",
        (hybrid.cores, hybrid.fast_cores, hybrid.hybrid) == (24, 8, true)
            && (amd.cores, amd.fast_cores, amd.hybrid, amd.smt, amd.l3_groups) == (16, 16, false, true, 2)
            && (pinned.logical, pinned.cores) == (8, 4)
            && cpu::shape(6, &[], 0) == cpu::Topology { logical: 6, cores: 6, fast_cores: 6, hybrid: false, smt: false, l3_groups: 1 },
    );
    log.check("parallel map keeps order and filters", sys::par_map(&items, |x| (x % 3 != 0).then_some(x * 2)) == items.iter().filter(|x| *x % 3 != 0).map(|x| x * 2).collect::<Vec<_>>());
    log.check("publisher skips network paths", fileinfo::publisher("\\\\pc-no-such-server\\share\\x.exe").is_empty());
    let cmd = format!("{}\\System32\\cmd.exe", fsx::windir());
    log.check("publisher is read and cached", !fileinfo::publisher(&cmd).is_empty() && fileinfo::publisher(&cmd) == fileinfo::publisher(&cmd.to_uppercase()));

    let boxdir = unique("pc-selftest-fence-");
    let root = boxdir.join("root");
    let outside = boxdir.join("outside");
    let sub = root.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::create_dir_all(outside.join("empty")).unwrap();
    fs::write(sub.join("a.txt"), b"12345").unwrap();
    fs::write(outside.join("a.txt"), b"keep").unwrap();
    let fence = Fence::new(&root, &[]);
    log.check("fence opens on a plain folder", fence.is_some());
    if let Some(fence) = fence {
        log.check("guarded delete inside the root", fsx::remove_inside(&sub.join("a.txt"), &fence, 0, false) == Removal::Removed(5) && !sub.join("a.txt").exists());
        fs::write(sub.join("b.txt"), b"x").unwrap();
        fs::rename(&sub, root.join("moved")).unwrap();
        let made = sys::hidden("cmd.exe").arg("/c").arg("mklink").arg("/J").arg(&sub).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
        if made && sub.is_dir() {
            log.check("file behind a swapped junction is refused", fsx::remove_inside(&sub.join("a.txt"), &fence, 0, false) == Removal::Failed && outside.join("a.txt").exists());
            log.check("folder behind a swapped junction is refused", fsx::remove_inside(&sub.join("empty"), &fence, 0, true) == Removal::Failed && outside.join("empty").is_dir());
            log.check("junction itself is refused", fsx::remove_inside(&sub, &fence, 0, true) == Removal::Failed && sub.is_dir());
            let _ = fs::remove_dir(&sub);
        } else {
            log.check("fence test could not create a junction", false);
        }
        log.check("root itself is refused", fsx::remove_inside(&root, &fence, 0, true) == Removal::Failed && root.is_dir());
        log.check("missing file counts as gone", fsx::remove_inside(&root.join("nope.txt"), &fence, 0, false) == Removal::Gone);
        fs::write(root.join("young.txt"), b"x").unwrap();
        log.check("young file is kept by age", fsx::remove_inside(&root.join("young.txt"), &fence, 24, false) == Removal::Kept && root.join("young.txt").exists());
        let ro = root.join("ro.txt");
        fs::write(&ro, b"x").unwrap();
        let mut perm = fs::metadata(&ro).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&ro, perm).unwrap();
        log.check("read-only file is removed", fsx::remove_inside(&ro, &fence, 0, false) == Removal::Removed(1) && !ro.exists());
        let moved = root.join("moved");
        let own = Fence::new(&root, std::slice::from_ref(&moved));
        log.check("own folder inside the root is refused", own.is_some_and(|f| fsx::remove_inside(&moved.join("b.txt"), &f, 0, false) == Removal::Failed) && moved.join("b.txt").exists());
        log.check("fence refuses a root inside an own folder", Fence::new(&moved, std::slice::from_ref(&root)).is_none());
    }
    wipe(&boxdir);

    let tree = unique("pc-selftest-tree-");
    let (mut bytes, mut count) = (0u64, 0u64);
    for a in 0..6 {
        for b in 0..5 {
            let d = tree.join(format!("d{}", a)).join(format!("e{}", b));
            fs::create_dir_all(&d).unwrap();
            for f in 0..12 {
                let data = vec![b'x'; (a * 7 + b * 3 + f) % 50 + 1];
                fs::write(d.join(format!("f{}.tmp", f)), &data).unwrap();
                bytes += data.len() as u64;
                count += 1;
            }
        }
    }
    fs::write(tree.join("d0").join("keep.log"), b"log").unwrap();
    let tree_text = tree.to_str().unwrap();
    let mut tt = target_for(vec![Source::new(tree_text, "*.tmp", true, 0), Source::new(tree_text, "f1*", true, 0)]);
    cleaner::scan(&mut tt);
    log.check("parallel walker counts every file once", tt.count == count && tt.bytes == bytes);
    cleaner::clean(&mut tt);
    log.check(
        "parallel clean removes every match and prunes folders",
        tt.removed == count && tt.freed == bytes && tt.failed == 0 && tree.join("d0").join("keep.log").exists() && !tree.join("d1").exists() && !tree.join("d0").join("e0").exists(),
    );
    wipe(&tree);

    let profile = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    log.check("personal folders are refused for broad masks", !cleaner::is_safe(&profile.join("Documents"), "*") && !cleaner::is_safe(&profile.join("Desktop"), "*"));
    log.check("temp is allowed for broad masks", cleaner::is_safe(&std::env::temp_dir(), "*"));
    log.check("double star counts as broad", !cleaner::is_safe(&profile, "*.*") && cleaner::is_safe(&profile, "*.log"));

    let steam = unique("pc-selftest-steam-");
    fs::create_dir_all(&steam).unwrap();
    let steam_text = steam.to_string_lossy().into_owned();
    let without = cleaner::steam_ok(&steam_text);
    fs::write(steam.join("steam.exe"), b"x").unwrap();
    log.check("steam folder needs steam.exe and is never the windows folder", !without && cleaner::steam_ok(&steam_text) && !cleaner::steam_ok(fsx::windir()));
    wipe(&steam);

    let adm = unique("pc-selftest-admin-");
    fs::create_dir_all(&adm).unwrap();
    fs::write(adm.join("x.dmp"), b"1").unwrap();
    let mut at = CleanTarget::custom("crash");
    at.sources.push(Source { admin: true, ..Source::new(adm.to_str().unwrap(), "*", false, 0) });
    cleaner::scan(&mut at);
    log.check("admin-only locations are skipped without elevation", at.count == if sys::is_admin() { 1 } else { 0 });
    wipe(&adm);
    log.check(
        "system crash dumps need elevation",
        cleaner::targets().iter().filter(|t| t.id == "crash").flat_map(|t| t.sources.iter()).filter(|s| s.dir.starts_with("%WINDIR%") || s.dir.starts_with("%PROGRAMDATA%")).all(|s| s.admin),
    );

    let wide = |s: &str, nul: bool| -> Vec<u8> { s.encode_utf16().chain(nul.then_some(0)).flat_map(|u| u.to_le_bytes()).collect() };
    log.check("plain string backup is quoted", regbackup::value_line("V", &REG_SZ, &wide("C:\\a \"b\"", true)).ok().as_deref() == Some("\"V\"=\"C:\\\\a \\\"b\\\"\""));
    log.check("string with a line break is kept as hex", regbackup::value_line("V", &REG_SZ, &wide("a\nb", true)).is_ok_and(|l| l == "\"V\"=hex(1):61,00,0a,00,62,00,00,00"));
    log.check("string with data after a null is kept as hex", regbackup::value_line("V", &REG_SZ, &[97, 0, 0, 0, 98, 0, 0, 0]).is_ok_and(|l| l == "\"V\"=hex(1):61,00,00,00,62,00,00,00"));
    log.check("expand string keeps its exact bytes", regbackup::value_line("V", &REG_EXPAND_SZ, &wide("%x%", false)).is_ok_and(|l| l == "\"V\"=hex(2):25,00,78,00,25,00"));
    log.check("binary values are still refused", regbackup::value_line("V", &REG_BINARY, &[1]).is_err());
    let bad_dir = unique("pc-selftest-badname-");
    fs::create_dir_all(&bad_dir).unwrap();
    log.check(
        "backups refuse names with control characters",
        regbackup::value_line("a\u{0}b", &REG_SZ, &wide("x", true)).is_err()
            && regbackup::value_line("a\nb", &REG_DWORD, &[0, 0, 0, 0]).is_err()
            && regbackup::export_key(Hive::User, "Software\\x\u{0}y", &bad_dir).is_err()
            && regbackup::export_value(Hive::User, "Software\\x\ny", "v", &bad_dir).is_err()
            && fs::read_dir(&bad_dir).map(|rd| rd.count()).unwrap_or(1) == 0,
    );
    wipe(&bad_dir);

    log.check(
        "first path keeps network paths textual",
        registry::first_path("\\\\pc-no-such-server\\share\\tool.exe /x").as_deref() == Some("\\\\pc-no-such-server\\share\\tool.exe") && registry::first_path("\\\\pc-no-such-server\\share\\tool").is_none(),
    );
    let keep = RegIssue::new(Hive::User, "apppath", "Software\\pc-selftest\\App Paths\\x.exe", None, "");
    let mut gone = RegIssue::new(Hive::User, "muicache", "Software\\pc-selftest\\Mui", Some("C:\\pc-none\\x.exe.FriendlyAppName"), "");
    gone.selected = true;
    let merged = registry::rescan(&[gone.clone(), keep.clone()], std::slice::from_ref(&gone));
    let order = ["shareddll", "apppath", "uninstall", "muicache", "com", "extension", "openwith"];
    let rank = |k: &str| order.iter().position(|x| *x == k).unwrap_or(order.len());
    log.check(
        "rescan refreshes fixed kinds and keeps the rest",
        merged.iter().any(|i| i.id() == keep.id() && !i.selected) && !merged.iter().any(|i| i.id() == gone.id()) && merged.windows(2).all(|w| rank(&w[0].kind) <= rank(&w[1].kind)),
    );
}

fn policy_tests(log: &mut Log) {
    use crate::engine::tools;

    registry::set_allow_test_key(true);
    let policy = format!("{}\\PolicyTest\\Policy", registry::TEST_KEY);
    let user = format!("{}\\PolicyTest\\User", registry::TEST_KEY);
    let backup = unique("pc-selftest-policy-");
    fs::create_dir_all(&backup).unwrap();
    let dir = backup.clone();
    let make_dir = move || -> std::io::Result<PathBuf> { Ok(dir.clone()) };

    let on = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, true, &make_dir);
    log.check("policy written to both keys", on.is_ok() && tools::read_policy(Hive::User, &policy, "Block", 1) && hkcu().open_subkey(&user).and_then(|k| k.get_value::<u32, _>("Block")).ok() == Some(1));
    let again = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, true, &make_dir);
    let files = fs::read_dir(&backup).map(|rd| rd.flatten().count()).unwrap_or(0);
    log.check("policy rewrite backs up the old policy and mirror values", again.is_ok() && files == 2);
    let off = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, false, &make_dir);
    log.check("policy removed with backup and empty key cleaned", off.is_ok() && !tools::read_policy(Hive::User, &policy, "Block", 1) && hkcu().open_subkey(&policy).is_err() && hkcu().open_subkey(&user).is_ok());
    log.check("policy off also removes the mirror value it set", hkcu().open_subkey(&user).map(|k| k.get_raw_value("Block").is_err()).unwrap_or(false));
    let _ = hkcu().create_subkey(&user).map(|(k, _)| k.set_value("Block", &0u32));
    let keep_mirror = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, false, &make_dir);
    log.check("policy off keeps a mirror value it did not set", keep_mirror.is_ok() && hkcu().open_subkey(&user).and_then(|k| k.get_value::<u32, _>("Block")).ok() == Some(0));
    let (bad, _) = hkcu().create_subkey(&policy).unwrap();
    bad.set_raw_value("Block", &RegValue { bytes: vec![9u8].into(), vtype: REG_BINARY }).unwrap();
    let refused = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, true, &make_dir);
    log.check(
        "policy is not written when its backup fails",
        refused.is_err() && bad.get_raw_value("Block").map(|v| v.vtype == REG_BINARY).unwrap_or(false) && hkcu().open_subkey(&user).and_then(|k| k.get_value::<u32, _>("Block")).ok() == Some(0),
    );
    let _ = hkcu().delete_subkey_all(&policy);
    log.check("policy off without a value is a no-op", tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, false, &make_dir).is_ok());
    let solo = format!("{}\\PolicyTest\\Solo", registry::TEST_KEY);
    let zero = tools::set_policy(Hive::User, &solo, None, "Level", 0, true, &make_dir);
    log.check("policy with value zero and no mirror", zero.is_ok() && tools::read_policy(Hive::User, &solo, "Level", 0) && !tools::read_policy(Hive::User, &solo, "Level", 1) && hkcu().open_subkey(&user).is_ok());
    let zero_off = tools::set_policy(Hive::User, &solo, None, "Level", 0, false, &make_dir);
    log.check("zero policy removed and empty key cleaned", zero_off.is_ok() && !tools::read_policy(Hive::User, &solo, "Level", 0) && hkcu().open_subkey(&solo).is_err());
    let (pk, _) = hkcu().create_subkey(&policy).unwrap();
    pk.set_value("Block", &5u32).unwrap();
    let pval = || hkcu().open_subkey(&policy).and_then(|k| k.get_value::<u32, _>("Block")).ok();
    let mval = || hkcu().open_subkey(&user).and_then(|k| k.get_value::<u32, _>("Block")).ok();
    let restored_on = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, true, &make_dir);
    let was_on = pval() == Some(1) && mval() == Some(1);
    let restored_off = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, false, &make_dir);
    log.check("policy off restores the values that were there before", restored_on.is_ok() && was_on && restored_off.is_ok() && pval() == Some(5) && mval() == Some(0));
    pk.set_value("Block", &7u32).unwrap();
    let other = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, false, &make_dir);
    log.check("policy off leaves a different value it did not set", other.is_ok() && pval() == Some(7) && mval() == Some(0));
    pk.set_value("Block", &1u32).unwrap();
    let legacy = tools::set_policy(Hive::User, &policy, Some(&user), "Block", 1, false, &make_dir);
    log.check("policy off without a record removes only the tool's own value", legacy.is_ok() && pval().is_none() && mval() == Some(0));
    drop(pk);
    let _ = hkcu().delete_subkey_all(&policy);
    let run = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
    log.check(
        "policy refuses other keys and values on the machine hive",
        tools::set_policy(Hive::Machine, run, Some(run), "x", 1, true, &make_dir).is_err()
            && tools::set_policy(Hive::Machine, "SOFTWARE\\Policies\\Microsoft\\Windows\\Device Metadata", Some("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Device Metadata"), "Other", 1, true, &make_dir).is_err()
            && tools::set_policy(Hive::Machine, "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", None, "AllowTelemetry", 3, true, &make_dir).is_err()
            && tools::set_policy(Hive::Machine, "SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", Some(run), "AllowTelemetry", 0, true, &make_dir).is_err(),
    );
    let tray = format!("{}{}Tray", registry::TEST_KEY, "\\");
    let needle: &[&str] = &["\\nvidia corporation\\", "\\display.nvcontainer\\"];
    for (id, exe) in [("1001", "{{AAAA}}\\NVIDIA Corporation\\App\\a.exe"), ("1002", "{{AAAA}}\\NVIDIA Corporation\\App\\b.exe"), ("1004", "{{BBBB}}\\DriverStore\\nv.inf\\Display.NvContainer\\d.exe"), ("1003", "{{AAAA}}\\Other Corp\\c.exe")] {
        let (k, _) = hkcu().create_subkey(format!("{}{}{}", tray, "\\", id)).unwrap();
        k.set_value("ExecutablePath", &exe).unwrap();
    }
    hkcu().open_subkey_with_flags(format!("{}{}1002", tray, "\\"), winreg::enums::KEY_WRITE).unwrap().set_value("IsPromoted", &1u32).unwrap();
    let before = fs::read_dir(&backup).map(|rd| rd.flatten().count()).unwrap_or(0);
    let hid = crate::engine::trayicons::set_hidden(&tray, needle, true, &make_dir);
    let val = |id: &str| hkcu().open_subkey(format!("{}{}{}", tray, "\\", id)).ok().and_then(|k| k.get_value::<u32, _>("IsPromoted").ok());
    log.check("tray icons hidden only for the matching programs", hid.is_ok() && val("1001") == Some(0) && val("1002") == Some(0) && val("1004") == Some(0) && val("1003").is_none() && crate::engine::trayicons::is_hidden(&tray, needle) && crate::engine::trayicons::count(&tray, needle) == 3);
    log.check("tray icon change backs up an existing value", fs::read_dir(&backup).map(|rd| rd.flatten().count()).unwrap_or(0) == before + 1);
    let shown = crate::engine::trayicons::set_hidden(&tray, needle, false, &make_dir);
    log.check("tray icons shown again by removing the value", shown.is_ok() && val("1001").is_none() && val("1002").is_none() && !crate::engine::trayicons::is_hidden(&tray, needle));
    log.check("tray icons refuse other keys and patterns", crate::engine::trayicons::set_hidden("Software\\Microsoft\\Windows\\CurrentVersion\\Run", needle, true, &make_dir).is_err() && crate::engine::trayicons::set_hidden(&tray, &["\\other corp\\"], true, &make_dir).is_err() && val("1003").is_none());
    log.check("tray icons report a missing icon", crate::engine::trayicons::set_hidden(&tray, &["\\nvidia corporation\\"], true, &|| Ok(PathBuf::new())).is_ok() && crate::engine::trayicons::set_hidden(&format!("{}{}None", registry::TEST_KEY, "\\"), needle, true, &make_dir).is_err());

    log.check("policy refuses real user keys", tools::set_policy(Hive::User, "Software\\Microsoft\\Windows\\CurrentVersion\\Run", Some("Software\\Microsoft\\Windows\\CurrentVersion\\Run"), "x", 1, true, &make_dir).is_err());

    registry::set_allow_test_key(false);
    let _ = hkcu().delete_subkey_all(registry::TEST_KEY);
    wipe(&backup);
}

fn classify_tests(log: &mut Log) {
    use crate::engine::classify::{classify, Advice, Role};
    use crate::engine::startup::{StartupEntry, StartupKind};

    let make = |kind: StartupKind, name: &str, publisher: &str, command: &str, system: bool| StartupEntry { kind, name: name.to_string(), publisher: publisher.to_string(), command: command.to_string(), system, ..Default::default() };
    log.check("classify system items as keep", classify(&make(StartupKind::Service, "Spooler", "Microsoft Windows", "", true)) == (Role::Windows, Advice::Keep));
    log.check("classify anti-cheat as security", classify(&make(StartupKind::Service, "AntiCheatExpert Protection", "ANTICHEATEXPERT.COM", "", false)) == (Role::Security, Advice::Keep));
    log.check("classify updaters as safe", classify(&make(StartupKind::Service, "Microsoft Edge Update Service", "Microsoft Corporation", "", false)) == (Role::Updater, Advice::Safe));
    log.check("classify graphics vendor as hardware caution", classify(&make(StartupKind::Service, "NVIDIA LocalSystem Container", "NVIDIA Corporation", "", false)) == (Role::Hardware, Advice::Caution));
    log.check("classify plain startup app as safe", classify(&make(StartupKind::Run, "Spotify", "Spotify AB", "", false)) == (Role::App, Advice::Safe));
    log.check("classify third-party service as caution", classify(&make(StartupKind::Service, "Steam Client Service", "Valve Corporation", "", false)) == (Role::App, Advice::Caution));
    log.check("known list: edge update service", classify(&make(StartupKind::Service, "Microsoft Edge Update Service (edgeupdate)", "Microsoft Corporation", "", false)) == (Role::Updater, Advice::Safe));
    log.check("known list: nvidia display container keeps", classify(&make(StartupKind::Service, "NVIDIA Display Container LS", "NVIDIA Corporation", "", false)) == (Role::Hardware, Advice::Keep));
    log.check("known list: frameview service is safe", classify(&make(StartupKind::Service, "NVIDIA FrameView SDK service", "NVIDIA", "", false)) == (Role::Hardware, Advice::Safe));
    log.check("known list: steam client service caution", classify(&make(StartupKind::Service, "Steam Client Service", "Valve Corporation", "", false)) == (Role::App, Advice::Caution));
    log.check("known list: steam startup is safe, not the service", classify(&make(StartupKind::Run, "Steam", "Valve", "", false)) == (Role::App, Advice::Safe));
    log.check("known list: packaged task parts", classify(&make(StartupKind::Packaged, "Microsoft.WindowsTerminal / StartTerminalOnLoginTask", "", "", false)) == (Role::Microsoft, Advice::Safe));
    log.check("known list: unknown names fall back to keywords", classify(&make(StartupKind::Service, "Acme Audio Recorder Service", "Acme", "", false)).0 == Role::Hardware);
    log.check("classify menu entries as safe", classify(&make(StartupKind::Context, "Edit with Paint", "", "", false)).1 == Advice::Safe);
}

fn programs_tests(log: &mut Log) {
    use crate::engine::programs::split_for;
    let split_command = |c: &str| split_for(c, true);

    let plain = split_command("\"C:\\Program Files\\X\\unins000.exe\" /SILENT");
    log.check("uninstall command quoted", plain == Some(("C:\\Program Files\\X\\unins000.exe".to_string(), "/SILENT".to_string())));
    let bare = split_command("C:\\Tools\\un.exe /x");
    log.check("uninstall command unquoted", bare == Some(("C:\\Tools\\un.exe".to_string(), "/x".to_string())));
    log.check("uninstall command keeps metacharacters as arguments", split_command("C:\\Tools\\un.exe & calc.exe").map(|c| c.1) == Some("& calc.exe".to_string()));
    log.check("uninstall command refuses a shell wrapper", split_command("cmd.exe /c del x").is_none() && split_command("calc & C:\\a.exe").is_none());
    log.check("uninstall command refuses relative or odd paths", split_command("tool.exe").is_none() && split_command("..\\a.exe").is_none() && split_command("\"C:\\a|b.exe\"").is_none() && split_command("").is_none());
    let guid = "{12345678-ABCD-4EF0-9876-0123456789AB}";
    log.check(
        "uninstall command maps msiexec to system folder",
        split_command(&format!("MsiExec.exe /X{}", guid)).is_some_and(|c| c.0.to_lowercase().ends_with("\\msiexec.exe") && c.0.contains(':') && c.1 == format!("/X{}", guid)),
    );
    log.check(
        "uninstall command allows only product-code msiexec calls",
        split_command(&format!("MsiExec.exe /I {} /qn /norestart", guid)).is_some()
            && split_command("MsiExec.exe /i https://example.invalid/x.msi").is_none()
            && split_command("\"C:\\Windows\\System32\\msiexec.exe\" /i C:\\x.msi").is_none()
            && split_command(&format!("MsiExec.exe /X{} TRANSFORMS=x.mst", guid)).is_none()
            && split_command("MsiExec.exe").is_none(),
    );
    log.check(
        "uninstall command refuses shells and script hosts by full path",
        split_command("\"C:\\Windows\\System32\\cmd.exe\" /c del x").is_none()
            && split_command("\"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -c x").is_none()
            && split_command("C:\\Windows\\System32\\mshta.exe x").is_none()
            && split_command("\"C:\\Program Files\\App\\unins000.exe\" /SILENT").is_some(),
    );
    let vendor = "\"C:\\WINDOWS\\SysWOW64\\RunDll32.EXE\" \"C:\\Program Files\\Vendor\\Core.dll\",UninstallPackage Driver";
    log.check(
        "machine uninstallers keep vendor rundll32 and msiexec forms",
        crate::engine::programs::split_for(vendor, false).is_some_and(|c| c.0.to_lowercase().ends_with("rundll32.exe"))
            && crate::engine::programs::split_for(vendor, true).is_none()
            && crate::engine::programs::split_for("MsiExec.exe /X{ABC} /L*v x.log", false).is_some(),
    );
    let mine = crate::engine::programs::Program { name: "Per User App".to_string(), user: true, ..Default::default() };
    let shared = crate::engine::programs::Program { name: "Machine App".to_string(), user: false, ..Default::default() };
    log.check(
        "per-user uninstallers are refused only while elevated",
        crate::engine::programs::refused_elevated(&mine, true) && !crate::engine::programs::refused_elevated(&mine, false) && !crate::engine::programs::refused_elevated(&shared, true),
    );
    let note = crate::i18n::fmt("uninst.elevated", &["Per User App"]);
    log.check("elevated refusal has a clear message", note.starts_with("Per User App") && note.contains("not as administrator"));
    let sec = format!("{}\\System32\\SecurityHealthSystray.exe", crate::engine::fsx::windir());
    log.check(
        "only the real Windows SecurityHealth entry is protected",
        startup::windows_run_entry(Hive::Machine, "SecurityHealth", Some(&sec))
            && !startup::windows_run_entry(Hive::User, "SecurityHealth", Some(&sec))
            && !startup::windows_run_entry(Hive::Machine, "SecurityHealthUpdate", Some("C:\\Users\\x\\AppData\\evil.exe"))
            && !startup::windows_run_entry(Hive::Machine, "SecurityHealth", None),
    );
}

fn tools_tests(log: &mut Log) {
    use crate::engine::tools;

    registry::set_allow_test_key(true);
    let base = format!("{}\\ToolsTest", registry::TEST_KEY);
    let (k, _) = hkcu().create_subkey(format!("{}\\child", base)).unwrap();
    k.set_value("v", &1u32).unwrap();
    let backup = unique("pc-selftest-tools-");
    fs::create_dir_all(&backup).unwrap();
    let removed = tools::delete_tree(&base, &backup);
    let files = fs::read_dir(&backup).map(|rd| rd.flatten().count()).unwrap_or(0);
    log.check("tool key deleted with backup", matches!(removed, Ok(true)) && hkcu().open_subkey(&base).is_err() && files == 1);
    log.check("tool key missing is a no-op", matches!(tools::delete_tree(&base, &backup), Ok(false)));
    log.check("tool refuses keys outside the list", tools::delete_tree("Software\\Microsoft\\Windows\\CurrentVersion\\Run", &backup).is_err() && tools::delete_value("Software\\Microsoft\\Windows\\CurrentVersion\\Run", "x", &backup).is_err());

    let (vk, _) = hkcu().create_subkey(&base).unwrap();
    vk.set_value("keep", &1u32).unwrap();
    vk.set_value("drop", &2u32).unwrap();
    let dropped = tools::delete_value(&base, "drop", &backup);
    log.check("tool value deleted and sibling kept", matches!(dropped, Ok(true)) && vk.get_raw_value("drop").is_err() && vk.get_raw_value("keep").is_ok());

    let pairs = [(base.as_str(), "FlagA"), (base.as_str(), "FlagB")];
    log.check("tool flags write off", tools::write_flags(&pairs, false).is_ok() && !tools::read_flags(&pairs) && vk.get_value::<u32, _>("FlagA").ok() == Some(0));
    log.check("tool flags write on", tools::write_flags(&pairs, true).is_ok() && tools::read_flags(&pairs));
    log.check("tool flags refuse other values", tools::write_flags(&[("Software\\Microsoft\\Windows\\CurrentVersion\\Run", "x")], false).is_err());
    log.check("tool flags default to on when missing", tools::read_flags(&[("Software\\WispSelfTest\\Nope", "x")]));

    registry::set_allow_test_key(false);
    log.check("tool refuses test keys when not testing", tools::delete_tree(&base, &backup).is_err());
    let _ = hkcu().delete_subkey_all(registry::TEST_KEY);
    wipe(&backup);
}

fn data_tests(log: &mut Log) {
    use crate::engine::{backups, history, settings};
    log.check("settings flag parse", settings::flag("log=0\r\nremember=1\r\n", "log") == Some(false) && settings::flag("log=0\r\nremember=1\r\n", "remember") == Some(true) && settings::flag("x=1", "log").is_none());
    let good = settings::parse_state("page=3\r\nwidth=1200\r\nheight=800\r\n");
    log.check("state parse accepts valid", good == Some(settings::WindowState { page: 3, width: 1200, height: 800 }));
    log.check("state parse rejects page out of range", settings::parse_state("page=99\r\nwidth=1200\r\nheight=800").is_none());
    log.check("state parse rejects absurd size", settings::parse_state("page=1\r\nwidth=5\r\nheight=800").is_none() && settings::parse_state("page=1\r\nwidth=1200\r\nheight=99999").is_none());
    log.check("state parse rejects garbage", settings::parse_state("hello").is_none() && settings::parse_state("page=a\r\nwidth=1\r\nheight=2").is_none());
    log.check(
        "data root is never inside a cleaned temp folder",
        data::cleaned_root(&std::env::temp_dir().join("Wisp-unzipped")) && data::cleaned_root(&std::env::temp_dir()) && !data::cleaned_root(Path::new("C:\\Tools\\Wisp")),
    );
    log.check("backup delete refuses path-like ids", backups::delete("..\\x").is_err() && backups::delete("../x").is_err() && backups::delete("").is_err());
    log.check("backup restore refuses path-like ids", backups::restore("..\\x").is_err() && backups::restore("../x").is_err() && backups::restore("").is_err() && backups::restore("C:\\Windows").is_err());

    log.check("history parses a good line", history::parse("2026-10-15 22:10	clean	1024	2	5").is_some_and(|e| e.kind == "clean" && e.a == 1024 && e.b == 2 && e.c == 5));
    log.check("history rejects bad lines", history::parse("").is_none() && history::parse("a	b").is_none() && history::parse("x	clean	q	1	2").is_none() && history::parse("	clean	1	1	1").is_none());
    let long: String = "x".repeat(200);
    let desc = crate::engine::restorepoint::description(&long);
    log.check("restore point description is cut and terminated", desc[62] == b'x' as u16 && desc[63] == 0 && desc[255] == 0);

    if data::is_custom() {
        log.check("data location tests skipped because a custom folder is set", true);
        return;
    }
    let flags = (settings::log_on(), settings::remember_on());
    let history_flag = settings::history_on();
    let target = unique("pc-selftest-data-");
    let made = fs::create_dir_all(&target).is_ok();
    let old_root = data::root().to_path_buf();
    log.check("data root defaults next to the exe or to the local fallback", old_root.ends_with("data") || old_root.ends_with("Wisp"));
    log.check("move refuses a relative path", data::set_location(Path::new("relative")).is_err());
    log.check("move refuses a folder inside the current data folder", data::set_location(&old_root.join("inner")).is_err());
    let moved = made && data::set_location(&target).is_ok();
    log.check("move to a new data folder works", moved && data::root() == target.as_path() && data::is_custom());
    let mut test_id: Option<String> = None;
    if moved {
        let dir = data::new_backup_dir();
        let dir_ok = dir.as_ref().is_ok_and(|d| d.starts_with(&target));
        log.check("new backups follow the data folder", dir_ok);
        settings::set_history(true);
        history::record("clean", 1000, 5, 3);
        history::record("fix", 4, 5, 0);
        history::record("clean", 500, 0, 2);
        let listed = history::list();
        log.check("history lists newest first", listed.len() == 3 && listed[0].a == 500 && listed[2].a == 1000);
        log.check("history total only counts cleans", history::total_freed() == 1500);
        settings::set_history(false);
        history::record("clean", 7, 0, 1);
        log.check("history writes nothing when history is off", history::list().len() == 3);
        history::clear();
        log.check("history clear empties the list", history::list().is_empty() && history::total_freed() == 0);
        if let Ok(dir) = dir {
            let key = registry::TEST_KEY.to_string();
            registry::set_allow_test_key(true);
            let reg = format!("Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\{}]\r\n\"v\"=\"restored\"\r\n", key);
            let wrote = fs::write(dir.join("a.reg"), reg).is_ok();
            let listed = backups::list();
            log.check("backup list shows the new backup", wrote && listed.iter().any(|b| Some(b.id.as_str()) == dir.file_name().and_then(|n| n.to_str()) && b.files == 1 && b.keys.first().is_some_and(|k| k.starts_with("HKCU"))));
            let id = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            test_id = Some(id.clone());
            let done = backups::restore(&id);
            let back: Option<String> = hkcu().open_subkey(&key).ok().and_then(|k| k.get_value("v").ok());
            log.check("backup restore writes the saved value", done.is_ok() && back.as_deref() == Some("restored"));
            let _ = hkcu().delete_subkey_all(&key);
            registry::set_allow_test_key(false);
        }
        if let Ok(extra) = data::new_backup_dir() {
            let _ = fs::write(extra.join("a.reg"), "x");
            let extra_id = extra.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            log.check("backup delete removes the folder", backups::delete(&extra_id).is_ok() && !extra.exists());
            let a = data::new_backup_dir();
            let b = data::new_backup_dir();
            if let (Ok(a), Ok(b)) = (a, b) {
                let _ = fs::write(a.join("a.reg"), "x");
                let _ = fs::write(b.join("a.reg"), "x");
                let before = backups::list().len();
                let n = backups::delete_all();
                log.check("delete all removes every backup", n == before && backups::list().is_empty());
            }
        }
        let _ = fs::remove_file(data::log_path());
        settings::set_log(false);
        data::log("selftest line");
        log.check("log off writes nothing", !data::log_path().exists());
        settings::set_log(true);
        data::log("selftest line");
        log.check("log on writes a line", fs::read_to_string(data::log_path()).is_ok_and(|t| t.contains("selftest line")));
        settings::set_remember(false);
        settings::save_state(settings::WindowState { page: 2, width: 900, height: 700 });
        log.check("remember off saves no window state", settings::load_state().is_none() && !target.join("state.txt").exists());
        settings::set_remember(true);
        settings::save_state(settings::WindowState { page: 2, width: 900, height: 700 });
        log.check("remember on round-trips window state", settings::load_state() == Some(settings::WindowState { page: 2, width: 900, height: 700 }));
        log.check("custom data folder counts as own", data::is_own(&target.join("backup")));
        log.check("reset returns to the default folder", data::reset_location().is_ok() && !data::is_custom() && data::root() == old_root.as_path());
        log.check("reset copied the settings back", old_root.join("settings.txt").is_file());
        if let Some(id) = test_id {
            wipe(&old_root.join("backup").join(id));
        }
    }
    settings::set_history(history_flag);
    settings::set_log(flags.0);
    settings::set_remember(flags.1);
    wipe(&target);
}

fn backup_check_tests(log: &mut Log) {
    use crate::engine::backups;

    let root = unique("pc-selftest-backups-");
    let good_dir = root.join("20260101-120000");
    fs::create_dir_all(&good_dir).unwrap();
    let utf16 = |text: &str| -> Vec<u8> { [0xFFu8, 0xFE].into_iter().chain(text.encode_utf16().flat_map(|u| u.to_le_bytes())).collect() };
    let header = "Windows Registry Editor Version 5.00\r\n\r\n";
    let run_key = "[HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run]\r\n";
    let write = |name: &str, text: String| -> PathBuf {
        let p = good_dir.join(name);
        fs::write(&p, utf16(&text)).unwrap();
        p
    };
    let good = write("0001.reg", format!("{}{}\"App\"=\"C:\\\\App\\\\app.exe\"\r\n", header, run_key));
    log.check("backup check accepts a well-formed file", backups::check_file(&good).is_ok());
    let no_header = write("0002.reg", format!("{}\"App\"=\"x\"\r\n", run_key));
    log.check("backup check needs the registry file header", backups::check_file(&no_header).is_err());
    let other_key = write("0003.reg", format!("{}[HKEY_CURRENT_USER\\Software\\Other]\r\n\"App\"=\"x\"\r\n", header));
    log.check("backup check accepts only keys Wisp backs up", backups::check_file(&other_key).is_err());
    let removes = write("0004.reg", format!("{}{}\"App\"=-\r\n", header, run_key));
    let removes_key = write("0005.reg", format!("{}[-HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run]\r\n", header));
    log.check("backup check refuses delete lines", backups::check_file(&removes).is_err() && backups::check_file(&removes_key).is_err());
    let test_file = write("0006.reg", format!("{}[HKEY_CURRENT_USER\\{}\\Restore]\r\n\"V\"=\"x\"\r\n", header, registry::TEST_KEY));
    let refused_outside_tests = backups::check_file(&test_file).is_err();
    registry::set_allow_test_key(true);
    let allowed_in_tests = backups::check_file(&test_file).is_ok();
    registry::set_allow_test_key(false);
    log.check("backup check allows the test key only during tests", refused_outside_tests && allowed_in_tests);
    let stray = write("0007.reg", format!("{}{}\"App\"=\"x\"\r\nHello\r\n", header, run_key));
    let glued = write("0008.reg", format!("{}[HKEY_CURRENT_USER\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run]junk\r\n", header));
    let spaced = write("0009.reg", format!("{}{}\"App\" = -\r\n", header, run_key));
    let nul = write("0010.reg", format!("{}{}\"App\"=\"x\0y\"\r\n", header, run_key));
    let lone = write("0011.reg", format!("{}{}\"App\"=\"x\ry\"\r\n", header, run_key));
    log.check("backup check refuses unrecognised lines, stray text, control characters and spaced deletes", [&stray, &glued, &spaced, &nul, &lone].iter().all(|f| backups::check_file(f).is_err()));
    let hex = write("0012.reg", format!("{}{}\"App\"=hex:41,00,\\\r\n  42,00,00,00\r\n@=\"d\"\r\n", header, run_key));
    log.check("backup check accepts continued hex values", backups::check_file(&hex).is_ok());
    let classes_root = write("0013.reg", format!("{}[HKEY_CURRENT_USER\\Software\\Classes]\r\n\"A\"=\"x\"\r\n", header));
    let classes_child = write("0014.reg", format!("{}[HKEY_CURRENT_USER\\Software\\Classes\\.abc]\r\n\"A\"=\"x\"\r\n", header));
    let ifeo = write("0015.reg", format!("{}[HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options\\NVIDIA App.exe]\r\n\"A\"=\"x\"\r\n", header));
    log.check("backup check narrows Classes and Services and drops unused keys", backups::check_file(&classes_root).is_err() && backups::check_file(&classes_child).is_ok() && backups::check_file(&ifeo).is_err());

    let elsewhere = unique("pc-selftest-backups-target-");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("0001.reg"), utf16(&format!("{}{}", header, run_key))).unwrap();
    let link = root.join("20260101-130000");
    let made = sys::hidden("cmd.exe").arg("/c").arg("mklink").arg("/J").arg(&link).arg(&elsewhere).output().map(|o| o.status.success()).unwrap_or(false);
    let ids: Vec<String> = backups::list_in(&root).into_iter().map(|b| b.id).collect();
    log.check("backup list skips folders that are links", made && ids == vec!["20260101-120000".to_string()]);
    if made {
        let _ = fs::remove_dir(&link);
    }
    wipe(&elsewhere);
    wipe(&root);
}

fn single_instance_tests(log: &mut Log) {
    log.check("first claim of the instance lock succeeds", crate::winui::claim_instance(0));
    log.check("second claim is refused while the first is held", !crate::winui::claim_instance(0));
    let started = std::time::Instant::now();
    let waited = crate::winui::claim_instance(300);
    log.check("a waiting claim gives up after its time", !waited && started.elapsed() >= std::time::Duration::from_millis(250));
}

fn leftover_tests(log: &mut Log) {
    use crate::engine::leftovers::{self, Links};
    let base = unique("wisp-leftover-");
    let old = SystemTime::now() - Duration::from_secs(400 * 86_400);
    let make = |rel: &str, file: &str, aged: bool| {
        let dir = base.join(rel);
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join(file);
        fs::write(&f, b"0123456789").unwrap();
        if aged {
            set_times(&f, old);
            set_times(&dir, old);
        }
        dir
    };
    let orphan = make("data\\OldApp", "settings.ini", true);
    make("data\\FreshApp", "settings.ini", false);
    make("data\\Linked App", "settings.ini", true);
    make("data\\Microsoft", "settings.ini", true);
    make("data\\VendorOnly", "settings.ini", true);
    make("data\\vlc", "settings.ini", true);
    make("data\\ab", "settings.ini", true);
    make("data\\Located", "settings.ini", true);
    make("data\\CodeApp", "tool.exe", true);
    let pathy = make("data\\pathy", "x.exe", true);
    make("data\\Nested\\Inner", "a.txt", true);
    let cfg = make("program\\EmptyCfg", "config.txt", true);
    make("program\\HasExe", "app.exe", true);
    make("program\\LibOnly", "core.dll", true);
    set_times(&base.join("data\\Nested"), old);
    leftovers::set_test_root(Some(base.clone()));
    let mut links = Links::default();
    links.add_name("LinkedApp Pro");
    links.add_vendor("VendorOnly Suite");
    links.add_name("VLC media player");
    links.add_path(&base.join("data\\Located").to_string_lossy());
    links.add_path(&format!("\"{}\" --run", pathy.join("x.exe").display()));
    let found: Vec<String> = leftovers::scan_with(&links).iter().map(|l| l.path.file_name().unwrap().to_string_lossy().to_string()).collect();
    let mut sorted = found.clone();
    sorted.sort();
    log.check("leftovers list only unlinked, old, code-free folders", sorted == ["CodeApp", "EmptyCfg", "Nested", "OldApp", "VendorOnly"]);
    let listed = leftovers::scan_with(&links);
    log.check("leftovers linked only by a vendor key are marked high risk", leftovers::scan_with(&links).iter().all(|l| l.risky == (l.path.file_name().unwrap() == "VendorOnly" || l.path.file_name().unwrap() == "CodeApp")));
    log.check("leftovers carry size, admin flag and month", listed.iter().find(|l| l.path == orphan).is_some_and(|l| l.size == 10 && !l.admin && leftovers::display(l).ends_with(&leftovers::month_text(Some(old)))) && listed.iter().find(|l| l.path == cfg).is_some_and(|l| l.admin));
    log.check("leftovers refuse protected, nested, root and outside paths", !leftovers::allowed(&base.join("data\\Microsoft")) && !leftovers::allowed(&base.join("data\\Nested\\Inner")) && !leftovers::allowed(&base.join("data")) && !leftovers::allowed(&std::env::temp_dir()) && !leftovers::allowed(&base.join("data\\Missing")));
    log.check("leftovers month text is a calendar month", leftovers::month_text(Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_704_067_200))) == "2024-01" && leftovers::month_text(Some(SystemTime::UNIX_EPOCH)) == "1970-01");
    let removed = leftovers::recycle(&orphan);
    log.check("leftover moves to the Recycle Bin and keeps the others", removed.is_ok() && !orphan.exists() && base.join("data\\FreshApp").is_dir() && leftovers::recycle(&base.join("data\\Microsoft")).is_err() && base.join("data\\Microsoft").is_dir());
    leftovers::set_test_root(None);
    let guarded = registry::RegIssue::new(Hive::User, "leftover", &base.join("data\\FreshApp").to_string_lossy(), None, "");
    let mut errors = Vec::new();
    let none = registry::fix(std::slice::from_ref(&guarded), &base, &mut errors);
    log.check("leftover fix refuses a folder outside the scanned roots", none == 0 && errors.len() == 1 && base.join("data\\FreshApp").is_dir());
    wipe(&base);
    let win = std::env::var("WINDIR").unwrap_or_default();
    let exe = registry::tray_exe("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\DriverStore\\a.exe");
    log.check("tray icon paths resolve known folders and skip unknown ones", exe.as_deref() == Some(format!("{}\\System32\\DriverStore\\a.exe", win).as_str()) && registry::tray_exe("{00000000-0000-0000-0000-000000000000}\\a.exe").is_none() && registry::tray_exe("a.exe").is_none() && registry::tray_exe("C:\\Tools\\a.exe").as_deref() == Some("C:\\Tools\\a.exe"));
}

fn i18n_tests(log: &mut Log) {
    use std::collections::{BTreeSet, HashMap};
    let en: HashMap<&str, &str> = crate::i18n::english().iter().copied().collect();
    let zh: HashMap<&str, &str> = crate::i18n_zh::TABLE.iter().copied().collect();
    let missing: Vec<&str> = en.keys().filter(|k| !zh.contains_key(*k)).copied().collect();
    log.check(&format!("every English text has a Chinese text ({} missing: {:?})", missing.len(), missing.iter().take(5).collect::<Vec<_>>()), missing.is_empty());
    let extra: Vec<&str> = zh.keys().filter(|k| !en.contains_key(*k)).copied().collect();
    log.check(&format!("no Chinese text lacks an English key ({:?})", extra.iter().take(5).collect::<Vec<_>>()), extra.is_empty());
    let holes = |text: &str| -> BTreeSet<usize> { (0..6).filter(|n| text.contains(&format!("{{{}}}", n))).collect() };
    let bad: Vec<&str> = en.iter().filter(|(k, v)| zh.get(*k).is_some_and(|z| holes(v) != holes(z))).map(|(k, _)| *k).collect();
    log.check(&format!("Chinese texts keep the same placeholders ({:?})", bad.iter().take(5).collect::<Vec<_>>()), bad.is_empty());
    let empty = crate::i18n_zh::TABLE.iter().filter(|(_, v)| v.trim().is_empty()).count();
    log.check("no Chinese text is empty", empty == 0);
}
