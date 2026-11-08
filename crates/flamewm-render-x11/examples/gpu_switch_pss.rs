//! Memory check for the free-on-switch rule (spec 8.3 at the pixel layer): CPU, then GPU, then
//! CPU again. Prints this process's `Pss` from `/proc/self/smaps_rollup` after each step.
//!
//! Run with `cargo run --release -p flamewm-render-x11 --example gpu_switch_pss`. A GPU step that
//! prints `status=Cpu` means the GPU could not be used and the numbers prove nothing.

use flamewm_render_x11::{
    RenderAcceleration, blur_rgba, release_if_stale, render_backend_status, set_render_acceleration,
};

const WIDTH: u32 = 800;
const HEIGHT: u32 = 500;

fn pss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("Pss:")
                    .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
            })
        })
        .unwrap_or(0)
}

fn report(label: &str) {
    let status = render_backend_status();
    println!(
        "{label}: pss_kib={} active={:?} device={:?} fallback={:?}",
        pss_kib(),
        status.active,
        status.device,
        status.fallback
    );
}

fn main() {
    let mut frame = vec![128_u8; (WIDTH * HEIGHT * 4) as usize];
    set_render_acceleration(RenderAcceleration::Cpu);
    for _ in 0..3 {
        blur_rgba(&mut frame, WIDTH, HEIGHT, 12.0);
    }
    report("cpu");
    set_render_acceleration(RenderAcceleration::Gpu);
    for _ in 0..5 {
        blur_rgba(&mut frame, WIDTH, HEIGHT, 12.0);
    }
    report("gpu");
    set_render_acceleration(RenderAcceleration::Cpu);
    report("cpu_after_switch");
    release_if_stale();
    for _ in 0..3 {
        blur_rgba(&mut frame, WIDTH, HEIGHT, 12.0);
    }
    report("cpu_after_work");
}
