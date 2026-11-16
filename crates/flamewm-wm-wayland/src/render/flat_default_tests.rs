use super::*;

/// Process-wide statics: this test restores the flat default it changes.
#[test]
fn flat_glass_commits_opaque_fill_and_no_desktop_blur() {
    let _guard = GLASS_STATICS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    set_glass_fill(0x0017_2135, 255, 0, true);
    assert!(glass_flat());
    assert_eq!(titlebar_fill(), (0x0017_2135, 255));
    assert_eq!(
        DESKTOP_BLUR_PERCENT.load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    set_glass_fill(0x0033_3333, 165, 80, false);
    assert!(!glass_flat());
    assert_eq!(titlebar_fill(), (0x0033_3333, 165));
    set_glass_fill(0x0017_2135, 255, 0, true);
}
