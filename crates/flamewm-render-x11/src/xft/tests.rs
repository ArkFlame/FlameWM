#[cfg(test)]
mod cases {
    use super::super::*;

    struct DisplayGuard(*mut crate::xlib::Display);

    impl Drop for DisplayGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the guard owns this display and closes it at most once.
                unsafe { crate::xlib::XCloseDisplay(self.0) };
            }
        }
    }

    /// Reproduce the init-error/display-close lifetime path without requiring fonts.
    /// Run: `FLAMEWM_TEST_DISPLAY=:91 cargo test -p flamewm-render-x11 --lib xft::tests::xft_init_failure_closes_display_safely -- --ignored --exact --test-threads=1`
    #[test]
    #[ignore = "requires a live FLAMEWM_TEST_DISPLAY with XRender but no usable font installed (Arch Xvfb package test environment)"]
    fn xft_init_failure_closes_display_safely() {
        use crate::xlib::{
            XCloseDisplay, XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay,
            XRootWindow,
        };

        let name = std::env::var_os("FLAMEWM_TEST_DISPLAY")
            .expect("requires FLAMEWM_TEST_DISPLAY pointing to the no-font Xvfb");
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        let display = unsafe { XOpenDisplay(name.as_ptr()) };
        let mut display_guard = DisplayGuard(display);
        assert!(!display.is_null(), "XOpenDisplay failed");

        let screen = unsafe { XDefaultScreen(display) };
        let root = unsafe { XRootWindow(display, screen) };
        let visual = unsafe { XDefaultVisual(display, screen) };
        let colormap = unsafe { XDefaultColormap(display, screen) };
        let error = match unsafe { XftBackend::new(display, screen, root, visual, colormap) } {
            Err(error) => error,
            Ok(backend) => {
                drop(backend);
                panic!("expected Xft initialization to fail because no usable font is installed");
            }
        };
        assert!(
            error.starts_with("Xft could not open a UI font"),
            "expected the no-font initialization error, got: {error}"
        );

        // The error is now gone; close the display after Xft's failed initialization path.
        drop(error);
        display_guard.0 = std::ptr::null_mut();
        unsafe { XCloseDisplay(display) };
    }

    /// Per-call cost of `XftBackend::measure_text` (XftTextExtentsUtf8) on a 40-char line.
    /// Run: `FLAMEWM_TEST_DISPLAY=:0 cargo test -p flamewm-render-x11 xft_text_extents_per_call_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "bench: needs FLAMEWM_TEST_DISPLAY"]
    fn xft_text_extents_per_call_bench() {
        use crate::xlib::{
            XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow,
        };
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let mut xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            let line = "Wi AVAST, Tofu: ilm 日本 Wa ff W./ VAT To ilm AVocado";
            const CALLS: u32 = 20_000;
            let started = std::time::Instant::now();
            let mut sink = 0.0f32;
            for _ in 0..CALLS {
                sink += xft.measure_text(13.0, 400, std::hint::black_box(line)).0;
            }
            let per_call = started.elapsed().as_nanos() as f64 / f64::from(CALLS);
            std::hint::black_box(sink);
            eprintln!("XftTextExtentsUtf8 bench: {per_call:.0} ns/call over {CALLS} calls");
            assert!(per_call.is_finite());
        }
    }

    /// Caret-vs-paint probe on a live display (`FLAMEWM_TEST_DISPLAY`). For each prefix length the
    /// caret is `measure_text(prefix)`; the painted edge is the rightmost inked column of the real
    /// Xft text mask of the same prefix. A growing `advance - ink_end` means the caret drifts.
    #[test]
    #[ignore = "live-display probe: FLAMEWM_TEST_DISPLAY=:99 cargo test -p flamewm-render-x11 --lib caret_x_tracks -- --ignored"]
    fn caret_x_tracks_painted_glyph_edge_on_live_display() {
        use crate::xlib::{
            XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow,
        };
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let mut xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            let text = &"I".repeat(60);
            let pad = 8u32;
            let mut worst_drift = f32::MIN;
            let mut best_drift = f32::MAX;
            for n in [10usize, 20, 30, 40, 50, 60] {
                let prefix: String = text.chars().take(n).collect();
                let caret = xft.measure_text(13.0, 400, &prefix).0;
                let mut mask = Vec::new();
                let (width, height) = xft
                    .text_alpha_mask(13.0, 400, &prefix, pad, &mut mask)
                    .expect("mask");
                let mut ink_end: Option<u32> = None;
                for row in 0..height as usize {
                    let line = &mask[row * width as usize..(row + 1) * width as usize];
                    if let Some(col) = line.iter().rposition(|&c| c > 0) {
                        ink_end = Some(ink_end.map_or(col as u32, |e| e.max(col as u32)));
                    }
                }
                let ink_end = ink_end.expect("prefix must ink") as f32 + 1.0 - pad as f32;
                let drift = caret - ink_end;
                eprintln!(
                    "caret probe n={n:2} caret={caret:7.2} ink_end={ink_end:7.2} gap={drift:6.2}"
                );
                worst_drift = worst_drift.max(drift);
                best_drift = best_drift.min(drift);
            }
            // The last glyph's right side bearing is the only legitimate gap: it is at most a few px
            // and must not grow with prefix length.
            assert!(
                worst_drift - best_drift < 4.0,
                "caret-to-ink gap varies by {} px across prefix lengths",
                worst_drift - best_drift
            );
        }
    }

    #[test]
    fn xglyph_info_matches_xrender_abi() {
        assert_eq!(std::mem::size_of::<XGlyphInfo>(), 12);
        assert_eq!(std::mem::align_of::<XGlyphInfo>(), 2);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, width), 0);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, height), 2);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, x), 4);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, y), 6);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, x_off), 8);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, y_off), 10);
    }

    #[test]
    fn css_weights_map_to_available_plex_faces() {
        assert_eq!(normalize_weight(400), 400);
        assert_eq!(normalize_weight(500), 500);
        assert_eq!(normalize_weight(600), 600);
        assert_eq!(normalize_weight(700), 700);
    }

    #[test]
    fn xft_measure_estimate_matches_layout() {
        assert_eq!(xft_measure_estimate("hello", 13.0), (38.0, 17.0));
        assert_eq!(xft_measure_estimate("", 13.0), (0.0, 17.0));
    }

    #[test]
    fn xft_color_expands_straight_alpha() {
        assert_eq!(
            xrender_color_for(Color {
                r: 255,
                g: 0,
                b: 0,
                a: 128
            }),
            (65535, 0, 0, 32896)
        );
    }

    /// Opens a private test display with Xft, or returns None (test skipped) without one.
    #[cfg(test)]
    fn open_test_xft() -> Option<(*mut crate::xlib::Display, XftBackend)> {
        use crate::xlib::{
            XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow,
        };
        let name = std::env::var_os("FLAMEWM_TEST_DISPLAY")?;
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            Some((display, xft))
        }
    }

    /// Batched shadow masks must be byte-identical to the original single-pixmap render, for
    /// every run, including when the batch mixes sizes, weights and pads.
    #[cfg(test)]
    #[test]
    fn batched_masks_match_reference_render_pixel_for_pixel() {
        let Some((display, mut xft)) = open_test_xft() else {
            return;
        };
        let runs: [(f32, u16, u32, &str); 6] = [
            (14.0, 400, 4, "Shadow"),
            (11.0, 400, 10, "0 processes \u{b7} 0 threads"),
            (13.0, 700, 6, "/usr/lib/jvm/zulu21/bin/java -Xmx1096m"),
            (9.0, 400, 3, "CPU 12.5%"),
            (14.0, 400, 4, "Shadow"),
            (16.0, 400, 8, "Wi\u{66}i \u{2014} Aa"),
        ];
        let mut reference = Vec::new();
        for &(size, weight, pad, text) in &runs {
            xft.text_masks.clear();
            let mut bytes = Vec::new();
            let dims = unsafe { xft.reference_alpha_mask(size, weight, text, pad, &mut bytes) }
                .expect("reference mask");
            reference.push((dims, bytes));
        }
        xft.text_masks.clear();
        let specs: Vec<AlphaMaskSpec<'_>> = runs
            .iter()
            .map(|&(size, weight, pad, text)| AlphaMaskSpec {
                size,
                weight,
                text,
                pad,
            })
            .collect();
        unsafe { xft.prefetch_text_masks(&specs) };
        for (index, &(size, weight, pad, text)) in runs.iter().enumerate() {
            let mut batched = Vec::new();
            let dims = unsafe { xft.text_alpha_mask(size, weight, text, pad, &mut batched) }
                .expect("batched mask");
            assert_eq!(dims, reference[index].0, "dims differ for {text:?}");
            assert!(batched == reference[index].1, "pixels differ for {text:?}");
        }
        let libraries = unsafe { xft.into_deferred_libraries() };
        unsafe { crate::xlib::XCloseDisplay(display) };
        drop(libraries);
    }

    /// A paint's worth of uncached shadow runs costs one readback; the follow-up per-run calls
    /// and a repeated prefetch cost none.
    #[cfg(test)]
    #[test]
    fn prefetch_costs_one_readback_per_batch() {
        let Some((display, mut xft)) = open_test_xft() else {
            return;
        };
        let texts = ["one", "two two", "three", "four 4%", "five", "six 6.0%"];
        let specs: Vec<AlphaMaskSpec<'_>> = texts
            .iter()
            .map(|text| AlphaMaskSpec {
                size: 12.0,
                weight: 400,
                text,
                pad: 6,
            })
            .collect();
        let before = xft.readbacks;
        unsafe { xft.prefetch_text_masks(&specs) };
        assert_eq!(
            xft.readbacks - before,
            1,
            "six short runs must share one readback"
        );
        let mut out = Vec::new();
        for text in texts {
            unsafe { xft.text_alpha_mask(12.0, 400, text, 6, &mut out) }.expect("cached");
        }
        assert_eq!(xft.readbacks - before, 1, "cached runs must not read back");
        unsafe { xft.prefetch_text_masks(&specs) };
        assert_eq!(
            xft.readbacks - before,
            1,
            "repeated prefetch must hit the cache"
        );
        let libraries = unsafe { xft.into_deferred_libraries() };
        unsafe { crate::xlib::XCloseDisplay(display) };
        drop(libraries);
    }

    /// The text-mask cache must return the same coverage as a fresh A8 render, and
    /// a second identical request must be a hit (no new entry). Needs
    /// `FLAMEWM_TEST_DISPLAY` with Xft fonts; skipped otherwise.
    #[test]
    fn text_mask_cache_hit_matches_fresh_render() {
        use crate::xlib::{
            XCloseDisplay, XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay,
            XRootWindow,
        };
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let mut xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            let (mut fresh, mut hit) = (Vec::new(), Vec::new());
            let dims = xft
                .text_alpha_mask(14.0, 400, "Shadow", 4, &mut fresh)
                .expect("fresh mask");
            assert_eq!(xft.text_masks.len(), 1);
            let hit_dims = xft
                .text_alpha_mask(14.0, 400, "Shadow", 4, &mut hit)
                .expect("cached mask");
            assert_eq!((dims, fresh.as_slice()), (hit_dims, hit.as_slice()));
            assert_eq!(xft.text_masks.len(), 1, "identical request must hit");
            xft.text_alpha_mask(15.0, 400, "Shadow", 4, &mut hit)
                .expect("other size");
            assert_eq!(xft.text_masks.len(), 2, "size is part of the key");
            // Libraries stay loaded through XCloseDisplay, as in production.
            let libraries = xft.into_deferred_libraries();
            XCloseDisplay(display);
            drop(libraries);
        }
    }
}
