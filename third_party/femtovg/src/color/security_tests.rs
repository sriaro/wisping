use super::*;

#[test]
fn security_hex_non_ascii_is_safe() {
    for text in ["€000", "00€0", "0000€0", "💥00", "00💥00"] {
        assert!(std::panic::catch_unwind(|| Color::hex(text)).is_ok(), "{text}");
    }
    assert_eq!(Color::hex("éabcd"), Color::rgb(0, 0xab, 0xcd));
}

#[test]
fn security_hex_preserves_existing_channel_parsing() {
    assert_eq!(Color::hex("##fF0102"), Color::rgb(255, 1, 2));
    assert_eq!(Color::hex("ff010280"), Color::rgba(255, 1, 2, 128));
    assert_eq!(Color::hex("+f0102"), Color::rgb(15, 1, 2));
    assert_eq!(Color::hex("ffzz02zz"), Color::rgba(255, 0, 2, 0));
    assert_eq!(Color::hex("bad"), Color::black());
}
