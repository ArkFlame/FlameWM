//! Median frame times of the shared pixel operations on the CPU and GPU backends at 1350x641.
//!
//! Run with `cargo run --release -p flamewm-render-x11 --example pixel_bench`. A GPU row that
//! matches the CPU row means the GPU request fell back to the CPU; the status line says why.
//!
//! The frame batch is 60 rounded fills, 20 strokes and 2 full-frame blurs. The per-op rows call the
//! single-op API, one GPU upload and readback per call. The batch rows call `paint_batch` once.

use std::time::{Duration, Instant};

use flamewm_render_core::{PixelOp, RoundedRect};
use flamewm_render_x11::{
    RenderAcceleration, blur_rgba, fill_rounded_rect, paint_batch, render_acceleration_status,
    set_render_acceleration, stroke_rounded_rect,
};

const WIDTH: u32 = 1350;
const HEIGHT: u32 = 641;
const ITERATIONS: usize = 25;

fn frame() -> Vec<u8> {
    (0..WIDTH * HEIGHT * 4)
        .map(|index| (index % 251) as u8)
        .collect()
}

fn measure(name: &str, mut op: impl FnMut(&mut [u8])) {
    let mut pixels = frame();
    op(&mut pixels);
    let mut samples: Vec<Duration> = (0..ITERATIONS)
        .map(|_| {
            let start = Instant::now();
            op(&mut pixels);
            start.elapsed()
        })
        .collect();
    samples.sort();
    let median = samples[ITERATIONS / 2];
    println!("  {name:<34} {:>9.3} ms", median.as_secs_f64() * 1000.0);
}

/// Deterministic panel positions spread over the frame, so every run measures the same batch.
fn panel(index: u32, width: u32, height: u32, radius: u32) -> RoundedRect {
    RoundedRect {
        x: (index * 97 % (WIDTH - width)) as i32,
        y: (index * 53 % (HEIGHT - height)) as i32,
        width,
        height,
        radius,
    }
}

/// 60 fills, then a blur, 20 strokes, then a second blur.
fn frame_batch() -> Vec<PixelOp> {
    let mut ops = Vec::new();
    for index in 0..60 {
        ops.push(PixelOp::FillRoundedRect {
            outer: panel(index, 140, 48, 12),
            hole: None,
            color: [30, 40, 50, 160],
        });
    }
    ops.push(PixelOp::Blur { radius: 12.0 });
    for index in 0..20 {
        let outer = panel(index + 60, 200, 80, 16);
        ops.push(PixelOp::FillRoundedRect {
            outer,
            hole: Some(outer.inset(2)),
            color: [220, 220, 230, 255],
        });
    }
    ops.push(PixelOp::Blur { radius: 12.0 });
    ops.into_iter().map(PixelOp::normalized).collect()
}

/// The same batch through the single-op API, one call per op.
fn apply_per_op(pixels: &mut [u8], ops: &[PixelOp]) {
    for op in ops {
        match *op {
            // The batch's strokes are the only ops with a hole, and they are 2 px rings.
            PixelOp::FillRoundedRect {
                outer,
                hole: Some(_),
                color,
            } => stroke_rounded_rect(pixels, WIDTH, HEIGHT, outer, 2, color).expect("stroke"),
            PixelOp::FillRoundedRect {
                outer,
                hole: None,
                color,
            } => fill_rounded_rect(pixels, WIDTH, HEIGHT, outer, None, color).expect("fill"),
            PixelOp::Blur { radius } => blur_rgba(pixels, WIDTH, HEIGHT, radius),
        }
    }
}

fn run(mode: RenderAcceleration) {
    set_render_acceleration(mode);
    println!("== renderAcceleration={} ==", mode.as_setting());
    let panel = RoundedRect {
        x: 300,
        y: 150,
        width: 600,
        height: 300,
        radius: 24,
    };
    measure("blur radius 12", |pixels| {
        blur_rgba(pixels, WIDTH, HEIGHT, 12.0);
    });
    measure("fill rounded 600x300 r24", |pixels| {
        fill_rounded_rect(pixels, WIDTH, HEIGHT, panel, None, [30, 40, 50, 160])
            .expect("fill");
    });
    measure("stroke rounded 600x300 t2", |pixels| {
        stroke_rounded_rect(pixels, WIDTH, HEIGHT, panel, 2, [220, 220, 230, 255])
            .expect("stroke");
    });
    let ops = frame_batch();
    measure("frame batch 60f+20s+2b per-op", |pixels| {
        apply_per_op(pixels, &ops);
    });
    measure("frame batch 60f+20s+2b paint_batch", |pixels| {
        paint_batch(pixels, WIDTH, HEIGHT, &ops).expect("batch");
    });
    let status = render_acceleration_status();
    println!(
        "  status: gpu_renderer={:?} gpu_error={:?}",
        status.gpu_renderer, status.gpu_error
    );
}

fn main() {
    run(RenderAcceleration::Cpu);
    run(RenderAcceleration::Gpu);
}
