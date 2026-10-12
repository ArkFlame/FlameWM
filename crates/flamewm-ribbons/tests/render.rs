use std::time::Instant;

use flamewm_ribbons::{Config, Quality, Rasterizer, Rect, Ribbons};

const W: u32 = 1280;
const H: u32 = 720;

fn config(num: usize, width_px: f64, seed: u64) -> Config {
    Config {
        num,
        width_px,
        span_rects: vec![Rect { x: 0, y: 0, w: W, h: H }],
        seed,
    }
}

/// Steps `state` to absolute time `t` in 1/60 s frames, the way a lock-screen loop would.
fn advance(state: &mut Ribbons, from_frame: u64, to_frame: u64) {
    for k in from_frame..=to_frame {
        state.step_to(k as f64 / 60.0);
    }
}

fn frame(state: &Ribbons, quality: Quality) -> Vec<u8> {
    let mut out = vec![0u8; (state.width() * state.height() * 4) as usize];
    Rasterizer::new(quality).render(state, &mut out);
    out
}

/// Pixels whose blue, green or red channel is above `threshold`.
fn lit_pixels(buf: &[u8], threshold: u8) -> usize {
    buf.chunks_exact(4)
        .filter(|p| p[0].max(p[1]).max(p[2]) > threshold)
        .count()
}

#[test]
fn ribbon_width_setting_changes_rendered_stroke_area() {
    let ink = |width| {
        let mut r = Ribbons::new(config(1, width, 21), 0.0);
        advance(&mut r, 1, 60 * 10);
        lit_pixels(&frame(&r, Quality::Full), 24)
    };
    let (thin, mid, wide) = (ink(2.0), ink(11.0), ink(28.0));
    assert!(thin > 0, "thin ribbon must be visible");
    assert!(mid as f64 > thin as f64 * 1.5, "11 px ({mid}) vs 2 px ({thin})");
    assert!(wide as f64 > mid as f64 * 1.5, "28 px ({wide}) vs 11 px ({mid})");
}

fn changed_pixels(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(p, q)| p != q).count()
}

#[test]
fn frames_visibly_change_over_time_and_are_not_black() {
    // Trails are static samples, so only heads, tails and fades change between frames.
    let mut r = Ribbons::new(config(6, 11.0, 4), 0.0);
    advance(&mut r, 1, 180);
    let f0 = frame(&r, Quality::Half);
    advance(&mut r, 181, 186);
    let f_100ms = frame(&r, Quality::Half);
    advance(&mut r, 187, 195);
    let f_250ms = frame(&r, Quality::Half);

    let total = (W * H) as usize;
    let at_100 = changed_pixels(&f0, &f_100ms);
    let at_250 = changed_pixels(&f0, &f_250ms);
    assert!(at_100 * 100 > total * 2, "only {at_100} of {total} pixels changed at 100 ms");
    assert!(at_250 * 100 > total * 3, "only {at_250} of {total} pixels changed at 250 ms");
    assert!(lit_pixels(&f0, 0) > total / 20, "frame must not be black");
    assert!(lit_pixels(&f_250ms, 0) > total / 20, "frame must not be black");
}

#[test]
fn identical_time_renders_identical_frames() {
    let mut r = Ribbons::new(config(6, 11.0, 4), 0.0);
    advance(&mut r, 1, 180);
    let a = frame(&r, Quality::Half);
    let b = frame(&r, Quality::Half);
    assert_eq!(changed_pixels(&a, &b), 0, "negative control: same time must change 0%");
}

#[test]
fn gaps_of_an_l_shaped_span_stay_black() {
    // Two rects forming an L: the union bounds are 2000x1000 and the top-right corner is a gap.
    let rects = vec![
        Rect { x: 0, y: 0, w: 1000, h: 1000 },
        Rect { x: 1000, y: 0, w: 1000, h: 400 },
    ];
    let mut r = Ribbons::new(Config { num: 6, width_px: 11.0, span_rects: rects, seed: 8 }, 0.0);
    advance(&mut r, 1, 60 * 6);
    assert!(r.is_masked());
    assert!(!r.covers(1900, 900));
    assert!(r.covers(1900, 100));
    let out = frame(&r, Quality::Half);
    let at = |x: usize, y: usize| &out[(y * 2000 + x) * 4..(y * 2000 + x) * 4 + 3];
    for y in (600..1000).step_by(37) {
        for x in (1000..2000).step_by(37) {
            assert_eq!(at(x, y), &[0, 0, 0], "gap pixel ({x},{y}) must be black");
        }
    }
}

#[test]
fn render_is_deterministic_for_same_seed_and_frame_times() {
    let mut a = Ribbons::new(config(6, 11.0, 13), 0.0);
    let mut b = Ribbons::new(config(6, 11.0, 13), 0.0);
    advance(&mut a, 1, 300);
    advance(&mut b, 1, 300);
    assert!(frame(&a, Quality::Half) == frame(&b, Quality::Half));
}

/// Timing probe: `cargo test -p flamewm-ribbons --release -- --ignored --nocapture render_p95`.
#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn render_p95_1280x720_n6() {
    let mut r = Ribbons::new(config(6, 11.0, 42), 0.0);
    advance(&mut r, 1, 60 * 9);
    let mut out = vec![0u8; (W * H * 4) as usize];
    for quality in [Quality::Half, Quality::Full, Quality::Quarter] {
        let mut rast = Rasterizer::new(quality);
        let mut times = Vec::new();
        for k in 0..240u64 {
            r.step_to((60 * 9 + 1 + k) as f64 / 60.0);
            let t0 = Instant::now();
            rast.render(&r, &mut out);
            times.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| times[((times.len() - 1) as f64 * p).round() as usize];
        println!(
            "render_p95 {quality:?} 1280x720 N=6: p50={:.2} ms p95={:.2} ms max={:.2} ms",
            pct(0.50),
            pct(0.95),
            times[times.len() - 1]
        );
    }
}
