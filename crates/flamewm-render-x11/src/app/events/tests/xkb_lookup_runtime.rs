use super::*;
use crate::xlib::{
    XCloseDisplay, XKeysymToKeycode, XMappingEvent, XOpenDisplay, XRefreshKeyboardMapping,
};
use std::ffi::CString;

#[test]
#[ignore = "needs FLAMEWM_KEY_TEST_DISPLAY set to a private display"]
fn xkb_lookup_resolves_levels_across_repeated_mapping_refreshes() {
    let name = std::env::var("FLAMEWM_KEY_TEST_DISPLAY")
        .expect("set FLAMEWM_KEY_TEST_DISPLAY to a private display");
    let name = CString::new(name).unwrap();
    // SAFETY: the display name is a valid C string; the display is closed below.
    let display = unsafe { XOpenDisplay(name.as_ptr()) };
    assert!(!display.is_null(), "XOpenDisplay failed");
    let keycode_of = |keysym: c_ulong| {
        // SAFETY: display is live.
        let code = unsafe { XKeysymToKeycode(display, keysym) };
        code as c_uint
    };
    let (q, two) = (keycode_of('q' as c_ulong), keycode_of('2' as c_ulong));
    assert!(q != 0 && two != 0);
    let event = |keycode: c_uint, state: u32| {
        // SAFETY: an all-zero XKeyEvent is a valid value; the fields used are set below.
        let mut key: XKeyEvent = unsafe { std::mem::zeroed() };
        key.type_ = KEY_PRESS;
        key.display = display;
        key.keycode = keycode;
        key.state = state;
        key
    };
    // FLAMEWM_KEY_TEST_ALTGR=1: the caller patched keycode of Q to `q Q at AT` (de AltGr+Q).
    let altgr_q = std::env::var("FLAMEWM_KEY_TEST_ALTGR").is_ok();
    for round in 0..2000 {
        if round % 100 == 0 {
            // SAFETY: display is live; this mirrors a MappingKeyboard notify.
            let mut mapping: XMappingEvent = unsafe { std::mem::zeroed() };
            mapping.request = 1;
            mapping.display = display;
            unsafe { XRefreshKeyboardMapping(&mut mapping) };
        }
        let (plain, typed) = unsafe { key_symbols(&event(q, 0)) };
        assert_eq!((plain, typed), ('q' as u64, 'q' as u64), "round {round}");
        let (_, shifted) = unsafe { key_symbols(&event(two, SHIFT_MASK)) };
        assert_eq!(shifted, '@' as u64, "Shift+2 must decode to @ (round {round})");
        assert!(inserts_text(SHIFT_MASK));
        if altgr_q {
            let (_, altgr) = unsafe { key_symbols(&event(q, MOD5_MASK)) };
            assert_eq!(altgr, '@' as u64, "AltGr+Q must decode to @ (round {round})");
            assert!(inserts_text(MOD5_MASK));
        }
        let (ctrl_plain, _) = unsafe { key_symbols(&event(q, CONTROL_MASK)) };
        assert_eq!(ctrl_plain, 'q' as u64, "Ctrl+Q keeps key name q");
        assert!(!inserts_text(CONTROL_MASK));
        let (_, alt_shifted) = unsafe { key_symbols(&event(two, MOD1_MASK | SHIFT_MASK)) };
        assert_eq!(alt_shifted, '@' as u64);
        assert!(inserts_text(MOD1_MASK | SHIFT_MASK), "Alt+Shift+2 carries @");
    }
    // SAFETY: display came from XOpenDisplay and is closed exactly once.
    unsafe { XCloseDisplay(display) };
}
