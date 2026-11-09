//! Spec 8.1: the GPU helper renders the same scene as the CPU path, within calibrated thresholds.
//! Skips with a printed reason when no GPU device is active (no GL library, or a software
//! renderer that the backend rejects).

use flamewm_api::render_backend::RenderBackend;
use flamewm_render_core::{PixelOp, RoundedRect};
use flamewm_render_x11::{
    RenderAcceleration, blur_rgba, fill_rounded_rect, paint_batch, render_backend_status,
    set_render_acceleration,
};

const WIDTH: u32 = 800;
const HEIGHT: u32 = 500;
const COLOR: [u8; 4] = [200, 40, 40, 255];

// Calibrated on the helper against CPU, threshold = observed + 1; edit only in review.
// T_FILL: observed 0 on NVIDIA GeForce RTX 3050 Ti Laptop GPU/PCIe/SSE2.
const T_FILL: u8 = 1;
// T_BLUR: observed 16 (blur 12, batch) and 14 (blur 28) on NVIDIA GeForce RTX 3050 Ti Laptop GPU/PCIe/SSE2.
const T_BLUR: u8 = 17;
// M_BLUR: observed 0.7153 (blur 28) on NVIDIA GeForce RTX 3050 Ti Laptop GPU/PCIe/SSE2.
const M_BLUR: f64 = 1.7153;

struct Diff {
    max: u8,
    mean: f64,
}

fn diff(a: &[u8], b: &[u8]) -> Diff {
    let mut max = 0_u8;
    let mut sum = 0_u64;
    for (x, y) in a.iter().zip(b) {
        let d = x.abs_diff(*y);
        max = max.max(d);
        sum += u64::from(d);
    }
    Diff {
        max,
        mean: sum as f64 / a.len() as f64,
    }
}

fn scene() -> Vec<u8> {
    (0..(WIDTH * HEIGHT * 4) as usize)
        .map(|i| ((i * 7 + i / 5) % 256) as u8)
        .collect()
}

fn outer() -> RoundedRect {
    RoundedRect {
        x: 40,
        y: 30,
        width: 500,
        height: 300,
        radius: 24,
    }
}

fn hole() -> RoundedRect {
    RoundedRect {
        x: 44,
        y: 34,
        width: 492,
        height: 292,
        radius: 20,
    }
}

fn render(mode: RenderAcceleration) -> Vec<(&'static str, Vec<u8>)> {
    set_render_acceleration(mode);
    let mut fill = scene();
    fill_rounded_rect(&mut fill, WIDTH, HEIGHT, outer(), Some(hole()), COLOR).unwrap();
    let mut blur12 = scene();
    blur_rgba(&mut blur12, WIDTH, HEIGHT, 12.0);
    let mut blur28 = scene();
    blur_rgba(&mut blur28, WIDTH, HEIGHT, 28.0);
    let mut batch = scene();
    paint_batch(
        &mut batch,
        WIDTH,
        HEIGHT,
        &[
            PixelOp::FillRoundedRect {
                outer: outer(),
                hole: Some(hole()),
                color: COLOR,
            },
            PixelOp::Blur { radius: 12.0 },
        ],
    )
    .unwrap();
    vec![
        ("fill", fill),
        ("blur12", blur12),
        ("blur28", blur28),
        ("batch", batch),
    ]
}

#[test]
fn helper_matches_cpu_within_calibrated_thresholds() {
    // SAFETY: this integration test binary runs one test, so no other thread reads the environment.
    unsafe {
        std::env::set_var(
            "FLAMEWM_GPU_HELPER",
            env!("CARGO_BIN_EXE_flamewm-gpu-helper"),
        );
    }
    let cpu = render(RenderAcceleration::Cpu);
    let gpu = render(RenderAcceleration::Gpu);
    let status = render_backend_status();
    if status.active != RenderBackend::OpenGl {
        eprintln!("skipped: GPU not active ({:?})", status.fallback);
        return;
    }
    let device = status.device.clone().unwrap_or_default();
    for ((name, cpu_pixels), (_, gpu_pixels)) in cpu.iter().zip(&gpu) {
        let d = diff(cpu_pixels, gpu_pixels);
        println!(
            "observed {name} max={} mean={:.4} on {device}",
            d.max, d.mean
        );
        if *name == "fill" {
            assert!(d.max <= T_FILL, "{name}: max {} > {T_FILL}", d.max);
        } else {
            assert!(d.max <= T_BLUR, "{name}: max {} > {T_BLUR}", d.max);
            assert!(d.mean <= M_BLUR, "{name}: mean {:.4} > {M_BLUR}", d.mean);
        }
    }
}
