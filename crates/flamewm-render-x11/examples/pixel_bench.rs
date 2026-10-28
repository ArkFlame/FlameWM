//! Median frame time of the shared pixel operations on the CPU and GPU backends at 1350x641.
//!
//! Run with `cargo run --release -p flamewm-render-x11 --example pixel_bench`. A GPU row that
//! matches the CPU row means the GPU request fell back to the CPU; the status line says why.

use std::time::{Duration, Instant};

use flamewm_render_core::RoundedRect;
use flamewm_render_x11::{
    RenderAcceleration, blur_rgba, fill_rounded_rect, render_acceleration_status,
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
    println!("  {name:<30} {:>9.3} ms", median.as_secs_f64() * 1000.0);
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
