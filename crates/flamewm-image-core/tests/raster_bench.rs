//! Timed image-core harness (blur, frosted backdrop, scale). Timing only: this crate forbids
//! `unsafe`, so the counting allocator lives in the render-core harness (`raster_bench.rs` there).
//! Ignored by default.
//!
//! `cargo test --release -p flamewm-image-core --test raster_bench -- --ignored --nocapture`

use flamewm_image_core::blur::{BlurScratch, frosted_backdrop_with, gaussian_blur_rgba_with};
use flamewm_image_core::scale::{ScaleFilter, scale_rgba};
use flamewm_image_core::{Rect, RgbaImage};
use std::hint::black_box;
use std::time::Instant;

fn bench(name: &str, pixels: usize, iters: usize, mut f: impl FnMut()) {
    for _ in 0..2 {
        f();
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ns = start.elapsed().as_nanos() as f64;
    println!(
        "BENCH {name} ns_px={:.4} allocs=n/a ns_call={:.0}",
        ns / (iters as f64 * pixels as f64),
        ns / iters as f64
    );
}

fn pattern(width: u32, height: u32, seed: u32) -> RgbaImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let pixels = (0..(width as usize * height as usize * 4))
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    RgbaImage {
        width,
        height,
        pixels,
    }
}

#[test]
#[ignore = "timed benchmark; run with --ignored --nocapture"]
fn raster_bench() {
    let src = pattern(1920, 1080, 5);
    for (name, filter) in [
        ("scale_area_1080p_to_540p", ScaleFilter::Area),
        ("scale_bilinear_1080p_to_540p", ScaleFilter::Bilinear),
        ("scale_nearest_1080p_to_540p", ScaleFilter::Nearest),
    ] {
        bench(name, 960 * 540, 5, || {
            black_box(scale_rgba(&src, 960, 540, filter));
        });
    }
    let mut scratch = BlurScratch::new();
    for radius in [3.0f32, 12.0, 33.0, 64.0] {
        let (w, h) = (480u32, 270u32);
        let mut px = pattern(w, h, 9).pixels;
        bench(
            &format!("blur_r{radius}_480x270"),
            (w * h) as usize,
            5,
            || {
                gaussian_blur_rgba_with(&mut scratch, &mut px, w, h, radius).unwrap();
                black_box(&px);
            },
        );
    }
    let crop = Rect {
        x: 200,
        y: 150,
        width: 600,
        height: 400,
    };
    bench("frosted_r24_600x400", 600 * 400, 5, || {
        black_box(frosted_backdrop_with(
            &mut scratch,
            &src,
            crop,
            24.0,
            Some([10, 20, 30, 96]),
        ));
    });
}
