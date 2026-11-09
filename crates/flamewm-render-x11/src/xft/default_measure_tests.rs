#[cfg(test)]
mod cases {
    use super::super::*;

    /// Layout width from the process-wide default must equal the scoped Xft measure (the paint
    /// measure) for caret-length prefixes 1..=60. Needs a live display: run with `--ignored`.
    #[test]
    #[ignore = "needs a live X display (DISPLAY)"]
    fn default_measure_matches_scoped_xft_for_caret_prefixes() {
        let (Some(default), Some(scoped_owner)) = (open_default_xft(), open_default_xft()) else {
            panic!("Xft must open on $DISPLAY");
        };
        let mut scoped_slot = scoped_owner.backend.borrow_mut();
        let backend = scoped_slot.as_mut().expect("backend");
        for chars in 1..=60 {
            let text = "M".repeat(chars);
            let scoped = scoped_text_measure(Some(&mut *backend), || {
                flamewm_render_core::text_layout::layout_text(
                    &text,
                    13.0,
                    1.0e6,
                    flamewm_render_core::TextWrap::NoWrap,
                    false,
                )
                .lines[0]
                    .width
            });
            let painted = default.measure(13.0, 400, &text);
            assert!(
                (scoped - painted).abs() < 0.01,
                "{chars} chars: {scoped} vs {painted}"
            );
        }
    }
}
