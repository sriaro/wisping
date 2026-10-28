use super::*;

#[test]
fn security_publisher_cache_round_trips_and_drops_bad_lines() {
    let mut map = HashMap::new();
    map.insert("c:\\apps\\a.exe".to_string(), (10u64, 20u64, "Contoso".to_string()));
    map.insert("c:\\apps\\b.exe".to_string(), (11u64, 21u64, String::new()));
    let back = parse_cache(&render_cache(&map));
    assert_eq!(back, map);
    let bad = "x\t1\tName\tc:\\a.exe\n1\t2\tName\t\\\\server\\share\\a.exe\n1\t2\tNa\u{1}me\tc:\\b.exe\n1\t2\tName\trelative.exe\n1\t2\n3\t4\tOk\tC:\\Good.exe\n";
    let parsed = parse_cache(bad);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed.get("c:\\good.exe"), Some(&(3, 4, "Ok".to_string())));
}
