use flamewm_ribbons::{Config, Quality, Rasterizer, Rect, Ribbons};

/// Same config and canvas as the lock screensaver test (`flamewm-lock` ribbons::tests): 320x180,
/// 6 ribbons, 11 px width, Half quality.
/// Mirrors `RibbonFrames::frame`: one `step_to` call straight to `t`, not frame-by-frame.
fn lock_frame(state: &mut Ribbons, t: f64) -> Vec<u8> {
    let (w, h) = (320u32, 180u32);
    let r = state;
    r.step_to(t);
    let mut out = vec![0u8; (w * h * 4) as usize];
    Rasterizer::new(Quality::Half).render(&r, &mut out);
    out
}

#[test]
fn lock_canvas_frames_show_ribbons_at_1s_and_3s() {
    let config = || Config {
        num: 6,
        width_px: 11.0,
        span_rects: vec![Rect {
            x: 0,
            y: 0,
            w: 320,
            h: 180,
        }],
        seed: 0x464c_414d,
    };
    let mut r = Ribbons::new(config(), 0.0);
    let a = lock_frame(&mut r, 1.0);
    let b = lock_frame(&mut r, 3.0);
    let lit = |buf: &[u8]| {
        buf.chunks_exact(4)
            .filter(|p| p[0].max(p[1]).max(p[2]) > 0)
            .count()
    };
    assert!(lit(&a) > 0, "frame at 1 s must not be black");
    assert!(lit(&b) > 0, "frame at 3 s must not be black");
    assert_ne!(a, b, "frames must differ between 1 s and 3 s");
}
