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

fn measure(name: &str, op: impl FnMut(&mut [u8])) {
    measure_in(name, WIDTH, HEIGHT, op);
}

/// Like [`measure`] on a `width` x `height` frame, for ops whose frame is not the bench frame.
fn measure_in(name: &str, width: u32, height: u32, mut op: impl FnMut(&mut [u8])) {
    let mut pixels: Vec<u8> = (0..width * height * 4).map(|index| (index % 251) as u8).collect();
    op(&mut pixels);
    let mut samples: Vec<Duration> = (0..ITERATIONS)
        .map(|_| {
            let start = Instant::now();
            op(&mut pixels);
            start.elapsed()
        })
        .collect();
    samples.sort();
    // Min is the noise-robust figure on a loaded machine; the median is kept beside it.
    let min = samples[0];
    let median = samples[ITERATIONS / 2];
    println!(
        "  {name:<34} min {:>9.3} ms  median {:>9.3} ms",
        min.as_secs_f64() * 1000.0,
        median.as_secs_f64() * 1000.0
    );
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
fn frame_batch() -> Vec<PixelOp<'static>> {
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
fn apply_per_op(pixels: &mut [u8], ops: &[PixelOp<'_>]) {
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
            blend @ (PixelOp::BlendMask { .. } | PixelOp::BlendImage { .. }) => {
                flamewm_render_core::composite_cpu(pixels, WIDTH, HEIGHT, blend).expect("blend")
            }
            // The bench batch has no node backdrop; the batch path covers it.
            PixelOp::BackdropBlur { .. } => {}
            // The bench batch has no clipped stroke; the batch path covers it.
            PixelOp::FillRoundedRectClipped { .. } => {}
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
        fill_rounded_rect(pixels, WIDTH, HEIGHT, panel, None, [30, 40, 50, 160]).expect("fill");
    });
    measure("stroke rounded 600x300 t2", |pixels| {
        stroke_rounded_rect(pixels, WIDTH, HEIGHT, panel, 2, [220, 220, 230, 255]).expect("stroke");
    });
    // Selection rubber band: a 320x210 translucent fill with a 1 px accent stroke.
    let selection = RoundedRect { x: 200, y: 120, width: 320, height: 210, radius: 0 };
    measure("selection fill+stroke 320x210", |pixels| {
        fill_rounded_rect(pixels, WIDTH, HEIGHT, selection, None, [60, 120, 220, 60]).expect("fill");
        stroke_rounded_rect(pixels, WIDTH, HEIGHT, selection, 1, [60, 120, 220, 255]).expect("stroke");
    });
    // Window move outline: a 2 px rounded stroke around a 800x500 window rect.
    let outline = RoundedRect { x: 150, y: 60, width: 800, height: 500, radius: 12 };
    measure("move outline 800x500 r12 t2", |pixels| {
        stroke_rounded_rect(pixels, WIDTH, HEIGHT, outline, 2, [255, 255, 255, 200]).expect("stroke");
    });
    // Alpha blend span: a 1000x700 full-coverage mask, source-over through composite_cpu.
    let mask = vec![200_u8; 1000 * 700];
    measure_in("alpha blend mask 1000x700", 1000, 700, |pixels| {
        flamewm_render_core::composite_cpu(
            pixels,
            1000,
            700,
            PixelOp::BlendMask { x: 0, y: 0, width: 1000, height: 700, mask: &mask, color: [30, 40, 50, 160] },
        )
        .expect("blend");
    });
    // Backdrop blur of a 800x500 node region, 28 px radius (the downscaled path).
    measure_in("blur radius 28 on 800x500", 800, 500, |pixels| {
        blur_rgba(pixels, 800, 500, 28.0);
    });
    let ops = frame_batch();
    measure("frame batch 60f+20s+2b per-op", |pixels| {
        apply_per_op(pixels, &ops);
    });
    measure("frame batch 60f+20s+2b paint_batch", |pixels| {
        paint_batch(pixels, WIDTH, HEIGHT, &ops).expect("batch");
    });
    let fills: Vec<PixelOp> = ops
        .iter()
        .copied()
        .filter(|op| !matches!(op, PixelOp::Blur { .. }))
        .collect();
    let blurs: Vec<PixelOp> = ops
        .iter()
        .copied()
        .filter(|op| matches!(op, PixelOp::Blur { .. }))
        .collect();
    measure("batch fills+strokes only (60f+20s)", |pixels| {
        paint_batch(pixels, WIDTH, HEIGHT, &fills).expect("fills");
    });
    measure("batch blurs only (2b)", |pixels| {
        paint_batch(pixels, WIDTH, HEIGHT, &blurs).expect("blurs");
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
