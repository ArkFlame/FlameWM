use flamewm_render_core::TextWrap;
use flamewm_render_core::text_layout::{
    layout_text, set_default_measure, unscoped_measure_count, with_measure,
};

fn ten_per_char(text: &str, _: f32, _: u16) -> f32 {
    text.chars().count() as f32 * 10.0
}

static DEFAULT: fn(&str, f32, u16) -> f32 = ten_per_char;

#[test]
fn installed_default_measure_drives_unscoped_layout_without_counting() {
    assert!(set_default_measure(&DEFAULT));
    let before = unscoped_measure_count();
    let width = layout_text("abc", 13.0, 1000.0, TextWrap::NoWrap, false).lines[0].width;
    assert!((width - 30.0).abs() < 1e-4, "width {width}");
    assert_eq!(unscoped_measure_count(), before, "default measure is not an unscoped measure");

    let scoped = |text: &str, _: f32, _: u16| text.chars().count() as f32;
    let width = with_measure(&scoped, || {
        layout_text("abc", 13.0, 1000.0, TextWrap::NoWrap, false).lines[0].width
    });
    assert!((width - 3.0).abs() < 1e-4, "scoped override must win, got {width}");
    assert!(!set_default_measure(&DEFAULT), "first install wins");
}
