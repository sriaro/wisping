use slint::fontique_011::{fontique, shared_collection};
use windows::Win32::Globalization::GetUserDefaultLocaleName;

use fontique::{FallbackKey, FamilyId, Script};

const YAHEI: &str = "Microsoft YaHei UI";
const JHENGHEI: &str = "Microsoft JhengHei UI";
const YUGOTHIC: &str = "Yu Gothic UI";
const MALGUN: &str = "Malgun Gothic";

const TAIL: [&str; 25] = [
    "Segoe UI Symbol",
    "Segoe UI Emoji",
    "Cambria Math",
    "Nirmala UI",
    "Leelawadee UI",
    "Segoe UI Historic",
    "Ebrima",
    "Gadugi",
    "Myanmar Text",
    "Microsoft Himalaya",
    "Mongolian Baiti",
    "Microsoft Yi Baiti",
    "Javanese Text",
    "Sylfaen",
    "Microsoft PhagsPa",
    "Microsoft New Tai Lue",
    "Microsoft Tai Le",
    "MV Boli",
    "Segoe UI",
    "MS Gothic",
    "SimSun-ExtB",
    "MingLiU-ExtB",
    "Malgun Gothic",
    "Yu Gothic UI",
    "Microsoft YaHei UI",
];

const SCRIPTS: [&[u8; 4]; 36] = [
    b"Latn", b"Cyrl", b"Grek", b"Arab", b"Hebr", b"Deva", b"Thai", b"Hani", b"Hira", b"Kana", b"Hang", b"Zyyy", b"Zinh", b"Armn", b"Geor", b"Beng", b"Taml", b"Telu", b"Knda", b"Mlym",
    b"Gujr", b"Guru", b"Sinh", b"Khmr", b"Laoo", b"Mymr", b"Tibt", b"Ethi", b"Orya", b"Thaa", b"Bopo", b"Syrc", b"Mong", b"Yiii", b"Tfng", b"Cher",
];

pub fn locale() -> String {
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]).to_ascii_lowercase()
}

fn han_order(locale: &str) -> [&'static str; 4] {
    if ["zh-tw", "zh-hk", "zh-mo", "zh-hant"].iter().any(|p| locale.starts_with(p)) {
        [JHENGHEI, YAHEI, YUGOTHIC, MALGUN]
    } else if locale.starts_with("ja") {
        [YUGOTHIC, YAHEI, JHENGHEI, MALGUN]
    } else if locale.starts_with("ko") {
        [MALGUN, YAHEI, JHENGHEI, YUGOTHIC]
    } else {
        [YAHEI, JHENGHEI, YUGOTHIC, MALGUN]
    }
}

pub fn install() {
    let mut collection = shared_collection();
    let mut tail: Vec<FamilyId> = Vec::new();
    for name in TAIL.iter().chain(&[JHENGHEI]) {
        if let Some(id) = collection.family_id(name)
            && !tail.contains(&id) {
                tail.push(id);
            }
    }
    let system_locale = locale();
    let traditional = ["zh-tw", "zh-hk", "zh-mo", "zh-hant"].iter().any(|p| system_locale.starts_with(p));
    let han_locale = if crate::i18n::zh() && !traditional { "zh-cn".to_string() } else { system_locale };
    let han: Vec<FamilyId> = han_order(&han_locale).iter().filter_map(|n| collection.family_id(n)).collect();
    for script in SCRIPTS {
        let key = FallbackKey::new(Script::from_bytes(*script), None);
        let mut chain: Vec<FamilyId> = Vec::new();
        let head: Vec<FamilyId> = if [b"Hani", b"Hira", b"Kana", b"Bopo"].contains(&script) { han.clone() } else { Vec::new() };
        let system: Vec<FamilyId> = collection.fallback_families(key).collect();
        for id in head.into_iter().chain(system).chain(tail.iter().copied()) {
            if !chain.contains(&id) {
                chain.push(id);
            }
        }
        collection.set_fallbacks(key, chain.into_iter());
    }
}
