//! Timed CPU raster harness with a counting allocator. Ignored by default.
//!
//! `cargo test --release -p flamewm-render-core --test raster_bench -- --ignored --nocapture`

use flamewm_render_core::{PixelOp, RoundedRect, SoftPixels};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `f` for `iters` timed iterations after a warm-up; prints ns/pixel and allocs per call.
fn bench(name: &str, pixels: usize, iters: usize, mut f: impl FnMut()) {
    for _ in 0..2 {
        f();
    }
    let before = ALLOCS.load(Ordering::Relaxed);
    f();
    let allocs = ALLOCS.load(Ordering::Relaxed) - before;
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let ns = start.elapsed().as_nanos() as f64;
    println!(
        "BENCH {name} ns_px={:.4} allocs={allocs} ns_call={:.0}",
        ns / (iters as f64 * pixels as f64),
        ns / iters as f64
    );
}

const W: u32 = 1920;
const H: u32 = 1080;

#[test]
#[ignore = "timed benchmark; run with --ignored --nocapture"]
fn raster_bench() {
    let n = (W * H) as usize;
    let mut frame = vec![40u8; n * 4];
    let mut soft = SoftPixels::new();
    let full = RoundedRect {
        x: 0,
        y: 0,
        width: W,
        height: H,
        radius: 0,
    };
    bench("fill_rect_opaque_1080p", n, 20, || {
        soft.paint(
            &mut frame,
            W,
            H,
            &[PixelOp::FillRoundedRect {
                outer: full,
                hole: None,
                color: [200, 100, 50, 255],
            }],
        )
        .unwrap();
        black_box(&frame);
    });
    bench("fill_rect_alpha128_1080p", n, 20, || {
        soft.paint(
            &mut frame,
            W,
            H,
            &[PixelOp::FillRoundedRect {
                outer: full,
                hole: None,
                color: [200, 100, 50, 128],
            }],
        )
        .unwrap();
        black_box(&frame);
    });
    let rounded = RoundedRect {
        x: 100,
        y: 100,
        width: 1200,
        height: 700,
        radius: 24,
    };
    bench("rounded_rect_alpha128_1200x700", 1200 * 700, 20, || {
        soft.paint(
            &mut frame,
            W,
            H,
            &[PixelOp::FillRoundedRect {
                outer: rounded,
                hole: None,
                color: [200, 100, 50, 128],
            }],
        )
        .unwrap();
        black_box(&frame);
    });
    let layer = vec![180u8; 512 * 512 * 4];
    bench("blend_image_src_over_512", 512 * 512, 50, || {
        soft.paint(
            &mut frame,
            W,
            H,
            &[PixelOp::BlendImage {
                x: 300,
                y: 200,
                width: 512,
                height: 512,
                rgba: &layer,
            }],
        )
        .unwrap();
        black_box(&frame);
    });
    let dot = vec![180u8; 4];
    bench("batch_overhead_blend_1x1_1080p", 1, 200, || {
        soft.paint(
            &mut frame,
            W,
            H,
            &[PixelOp::BlendImage {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                rgba: &dot,
            }],
        )
        .unwrap();
        black_box(&frame);
    });
    let mask = vec![128u8; 512 * 512];
    bench("blend_mask_512", 512 * 512, 50, || {
        soft.paint(
            &mut frame,
            W,
            H,
            &[PixelOp::BlendMask {
                x: 300,
                y: 200,
                width: 512,
                height: 512,
                mask: &mask,
                color: [255, 255, 255, 255],
            }],
        )
        .unwrap();
        black_box(&frame);
    });
    for radius in [3.0f32, 12.0, 33.0] {
        let (bw, bh) = (480u32, 270u32);
        let mut small = vec![90u8; (bw * bh * 4) as usize];
        bench(
            &format!("blur_r{radius}_480x270"),
            (bw * bh) as usize,
            5,
            || {
                soft.paint(&mut small, bw, bh, &[PixelOp::Blur { radius }])
                    .unwrap();
                black_box(&small);
            },
        );
    }
}
