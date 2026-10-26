use super::*;

fn payload(state: u32, symbol: u64) -> String {
    let mut out = String::new();
    typed_key_payload(state, symbol, &mut out);
    out
}

#[test]
fn plain_altgr_and_alt_keys_carry_text_while_ctrl_and_super_do_not() {
    assert!(inserts_text(0));
    assert!(inserts_text(SHIFT_MASK));
    assert!(inserts_text(LOCK_MASK));
    assert!(inserts_text(MOD5_MASK));
    assert!(inserts_text(MOD5_MASK | MOD1_MASK));
    assert!(inserts_text(MOD1_MASK));
    assert!(inserts_text(MOD1_MASK | SHIFT_MASK));
    assert!(!inserts_text(CONTROL_MASK));
    assert!(!inserts_text(CONTROL_MASK | MOD5_MASK));
    assert!(!inserts_text(MOD4_MASK));
}

#[test]
fn ctrl_a_is_key_a_with_ctrl_and_no_text() {
    assert_eq!(payload(CONTROL_MASK, 'a' as u64), "Ctrl\u{1f}a\u{1f}");
    assert_eq!(
        payload(CONTROL_MASK | SHIFT_MASK, 'A' as u64),
        "Ctrl+Shift\u{1f}A\u{1f}"
    );
}

#[test]
fn plain_and_altgr_keys_carry_their_text() {
    assert_eq!(payload(0, 'q' as u64), "\u{1f}q\u{1f}q");
    assert_eq!(payload(MOD5_MASK, '@' as u64), "\u{1f}@\u{1f}@");
    assert_eq!(payload(MOD1_MASK, 'q' as u64), "Alt\u{1f}q\u{1f}q");
    assert_eq!(payload(MOD1_MASK, '.' as u64), "Alt\u{1f}.\u{1f}.");
    assert_eq!(
        payload(MOD1_MASK, XK_BACK_SPACE),
        "Alt\u{1f}BackSpace\u{1f}"
    );
}
