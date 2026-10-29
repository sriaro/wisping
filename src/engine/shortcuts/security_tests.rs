use super::*;
use crate::engine::fsx::security_tests::fixture;

const LINK_INFO: u32 = 0x02;
const RELATIVE: u32 = 0x08;
const UNICODE: u32 = 0x80;
const EXP_STRING: u32 = 0x200;

fn header(flags: u32) -> Vec<u8> {
    let mut out = (HEADER as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&CLSID);
    out.extend_from_slice(&flags.to_le_bytes());
    out.resize(HEADER, 0);
    out
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn link_info(ansi: &[u8], wide: Option<&str>) -> Vec<u8> {
    let head = if wide.is_some() { 0x24 } else { 0x1C };
    let mut volume = [0u8; 17];
    volume[..4].copy_from_slice(&17u32.to_le_bytes());
    volume[4..8].copy_from_slice(&3u32.to_le_bytes());
    volume[12..16].copy_from_slice(&16u32.to_le_bytes());
    let base_off = head + volume.len();
    let suffix_off = base_off + ansi.len() + 1;
    let wide_off = suffix_off + 1;
    let wide_bytes = wide.map(utf16).unwrap_or_default();
    let wide_suffix_off = wide_off + wide_bytes.len();
    let size = if wide.is_some() { wide_suffix_off + 2 } else { wide_off };
    let mut fields = vec![size as u32, head as u32, 1, head as u32, base_off as u32, 0, suffix_off as u32];
    if wide.is_some() {
        fields.extend([wide_off as u32, wide_suffix_off as u32]);
    }
    let mut out: Vec<u8> = fields.iter().flat_map(|v| v.to_le_bytes()).collect();
    out.extend_from_slice(&volume);
    out.extend_from_slice(ansi);
    out.extend_from_slice(&[0, 0]);
    if wide.is_some() {
        out.extend_from_slice(&wide_bytes);
        out.extend_from_slice(&[0, 0]);
    }
    out
}

fn string_data(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = (units.len() as u16).to_le_bytes().to_vec();
    out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    out
}

fn env_block(text: &str) -> Vec<u8> {
    let mut out = (ENV_BLOCK_SIZE as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&ENV_BLOCK.to_le_bytes());
    let mut ansi = text.as_bytes().to_vec();
    ansi.resize(260, 0);
    out.extend_from_slice(&ansi);
    let mut wide = utf16(text);
    wide.resize(520, 0);
    out.extend_from_slice(&wide);
    out
}

fn file_link(target: &Path) -> Vec<u8> {
    let text = target.to_string_lossy();
    let mut out = header(LINK_INFO | UNICODE);
    out.extend(link_info(text.as_bytes(), Some(&text)));
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

fn free_drive() -> Option<char> {
    ('A'..='Z').rev().find(|c| {
        let root = [*c as u16, b':' as u16, b'\\' as u16, 0];
        (unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }) == 1
    })
}

#[test]
fn security_broken_shortcuts_need_a_missing_target_on_a_fixed_drive() {
    let base = fixture("shortcuts");
    let present = base.join("present.txt");
    fs::write(&present, b"x").unwrap();
    let gone = base.join("gone.exe");
    let good = base.join("good.lnk");
    fs::write(&good, file_link(&present)).unwrap();
    assert!(!broken(&good));
    let bad = base.join("bad.lnk");
    fs::write(&bad, file_link(&gone)).unwrap();
    assert!(broken(&bad));
    let saved = base.join("relative.lnk");
    let mut data = header(LINK_INFO | RELATIVE | UNICODE);
    data.extend(link_info(gone.to_string_lossy().as_bytes(), None));
    data.extend(string_data("present.txt"));
    data.extend_from_slice(&0u32.to_le_bytes());
    fs::write(&saved, data).unwrap();
    assert!(!broken(&saved));
    if let Some(letter) = free_drive() {
        let offline = base.join("offline.lnk");
        fs::write(&offline, file_link(Path::new(&format!("{letter}:\\missing\\app.exe")))).unwrap();
        assert!(!broken(&offline));
    }
    let bare = base.join("bare.lnk");
    fs::write(&bare, header(UNICODE)).unwrap();
    assert!(!broken(&bare));
    let startup = base.join("Startup");
    fs::create_dir_all(&startup).unwrap();
    let kept = startup.join("bad.lnk");
    fs::write(&kept, file_link(&gone)).unwrap();
    assert!(!broken(&kept));
    let text = base.join("notes.lnk");
    fs::write(&text, b"not a shell link").unwrap();
    assert!(!broken(&text));
    let fuzzy = base.join("fuzzy.lnk");
    let mut data = header(LINK_INFO);
    let mut path = base.join("x").to_string_lossy().into_owned().into_bytes();
    path.extend_from_slice(&[0xC4, 0xE3]);
    data.extend(link_info(&path, None));
    data.extend_from_slice(&0u32.to_le_bytes());
    fs::write(&fuzzy, data).unwrap();
    assert!(!broken(&fuzzy));
    let env_ok = base.join("env-ok.lnk");
    let mut data = header(EXP_STRING | UNICODE);
    data.extend(env_block("%SystemRoot%\\notepad.exe"));
    data.extend_from_slice(&0u32.to_le_bytes());
    fs::write(&env_ok, data).unwrap();
    assert!(!broken(&env_ok));
    let env_bad = base.join("env-bad.lnk");
    let mut data = header(EXP_STRING | UNICODE);
    data.extend(env_block("%SystemRoot%\\wisping-missing-9f3a.exe"));
    data.extend_from_slice(&0u32.to_le_bytes());
    fs::write(&env_bad, data).unwrap();
    assert!(broken(&env_bad));
    fs::remove_dir_all(&base).unwrap();
}
