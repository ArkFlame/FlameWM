//! Memory check for the free-on-switch rule (spec 8.3 at the pixel layer): CPU, then GPU, then
//! CPU again. Prints this process's `Pss` from `/proc/self/smaps_rollup` after each step.
//!
//! Run with `cargo run --release -p flamewm-render-x11 --example gpu_switch_pss`. A GPU step that
//! prints `status=Cpu` means the GPU could not be used and the numbers prove nothing.
//!
//! `FLAMEWM_PSS_MODE` selects the experiment: `base` (GPU work on the main thread), `trim` (base plus
//! `malloc_trim(0)` after the switch back), `thread` (GPU work on a worker thread that exits
//! before the switch back, so its thread-local slot is dropped on thread exit).

use flamewm_render_x11::{
    RenderAcceleration, blur_rgba, release_if_stale, render_backend_status, set_render_acceleration,
};

const WIDTH: u32 = 800;
const HEIGHT: u32 = 500;

unsafe extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

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

fn gpu_work(frame: &mut [u8], passes: usize) {
    for _ in 0..passes {
        blur_rgba(frame, WIDTH, HEIGHT, 12.0);
    }
}

fn main() {
    if std::env::var_os("FLAMEWM_PSS_CHILD").is_some() {
        // Helper role: all GPU work happens here, and the process exits, taking the driver with it.
        set_render_acceleration(RenderAcceleration::Gpu);
        let mut frame = vec![128_u8; (WIDTH * HEIGHT * 4) as usize];
        gpu_work(&mut frame, 5);
        report("child_gpu");
        return;
    }
    let mode = std::env::var("FLAMEWM_PSS_MODE").unwrap_or_else(|_| "base".to_owned());
    if mode == "child" {
        let mut frame = vec![128_u8; (WIDTH * HEIGHT * 4) as usize];
        set_render_acceleration(RenderAcceleration::Cpu);
        for _ in 0..3 {
            blur_rgba(&mut frame, WIDTH, HEIGHT, 12.0);
        }
        report("cpu");
        let exe = std::env::current_exe().expect("current_exe");
        let status = std::process::Command::new(exe)
            .env("FLAMEWM_PSS_CHILD", "1")
            .status()
            .expect("spawn child");
        println!("child_exit={status}");
        report("parent_after_child");
        return;
    }
    let mut frame = vec![128_u8; (WIDTH * HEIGHT * 4) as usize];
    set_render_acceleration(RenderAcceleration::Cpu);
    for _ in 0..3 {
        blur_rgba(&mut frame, WIDTH, HEIGHT, 12.0);
    }
    report("cpu");
    set_render_acceleration(RenderAcceleration::Gpu);
    if mode == "thread" {
        let mut worker_frame = frame.clone();
        std::thread::spawn(move || {
            gpu_work(&mut worker_frame, 5);
            report("gpu_in_worker");
        })
        .join()
        .expect("worker panicked");
    } else {
        gpu_work(&mut frame, 5);
    }
    report("gpu");
    set_render_acceleration(RenderAcceleration::Cpu);
    report("cpu_after_switch");
    if mode == "trim" {
        // SAFETY: malloc_trim takes a pad size and has no pointer arguments.
        let released = unsafe { malloc_trim(0) };
        println!("malloc_trim returned {released}");
        report("cpu_after_trim");
    }
    release_if_stale();
    gpu_work(&mut frame, 3);
    report("cpu_after_work");
}
