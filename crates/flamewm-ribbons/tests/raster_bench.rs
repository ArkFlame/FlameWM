//! Ignored raster benchmark at 1280x720 and 1920x1080 (N=6, seed 42, 240 frames per mode).
//! Run: `cargo test --release -p flamewm-ribbons --test raster_bench -- --ignored --nocapture`.

use std::time::Instant;

use flamewm_ribbons::{Config, Quality, Rasterizer, Rect, Ribbons};

fn bench(w: u32, h: u32, quality: Quality) {
    let config = Config {
        num: 6,
        width_px: 11.0,
        span_rects: vec![Rect { x: 0, y: 0, w, h }],
        seed: 42,
    };
    let mut r = Ribbons::new(config, 0.0);
    for k in 1..=60 * 9 {
        r.step_to(f64::from(k) / 60.0);
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
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
        "raster_bench {quality:?} {w}x{h} N=6: p50={:.2} ms p95={:.2} ms",
        pct(0.50),
        pct(0.95)
    );
}

#[test]
#[ignore = "benchmark; run with --release --ignored --nocapture"]
fn raster_bench_1280x720_and_1920x1080() {
    for quality in [Quality::Full, Quality::Half] {
        bench(1280, 720, quality);
        bench(1920, 1080, quality);
    }
}
