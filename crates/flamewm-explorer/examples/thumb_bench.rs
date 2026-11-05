//! Thumbnail scheduling bench for `scripts/perf/explorer_thumbs.sh`. Same process, A/B per iteration:
//! `old` = FIFO handoff, every window row requested each pass (pre-change policy);
//! `new` = pass-start cancellation, in-view requests only.
//! Usage: thumb_bench <fixture_dir> <iterations>
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use flamewm_explorer::thumbs::{render_thumbnail, Lookup, ThumbKey, ThumbQueue, Thumbnails};

const WINDOW: usize = 30;
const FRAME: Duration = Duration::from_millis(4);

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

struct Run {
    decodes: u64,
    settle_ms: f64,
    ready_visible: bool,
}

/// `passes` full projections, the window moving `step` rows per pass; `settle` waits for the last window.
fn run(files: &[PathBuf], new_policy: bool, passes: usize, step: usize) -> Run {
    let queue = Arc::new(ThumbQueue::default());
    let (tx, rx) = mpsc::channel::<(ThumbKey, Option<Vec<u8>>)>();
    let decodes = Arc::new(AtomicU64::new(0));
    {
        let (queue, decodes) = (Arc::clone(&queue), Arc::clone(&decodes));
        thread::spawn(move || loop {
            let key = queue.pop();
            let pixels = render_thumbnail(&key);
            decodes.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send((key, pixels));
        });
    }
    let key_of = |path: &PathBuf| ThumbKey { path: path.clone(), modified: None, size: 0 };
    let mut cache = Thumbnails::default();
    let max_start = files.len().saturating_sub(WINDOW);
    let mut last_window: Vec<ThumbKey> = Vec::new();
    let mut final_start = Instant::now();
    for pass in 0..passes {
        let origin = (pass * step).min(max_start);
        let window: Vec<ThumbKey> = files[origin..origin + WINDOW].iter().map(key_of).collect();
        if new_policy {
            cache.start_pass(&queue);
        }
        for key in &window {
            if let Lookup::Missing = cache.lookup(key) {
                if cache.begin(key) {
                    queue.push(key.clone());
                }
            }
        }
        last_window = window;
        final_start = Instant::now();
        thread::sleep(FRAME);
        while let Ok((key, pixels)) = rx.try_recv() {
            cache.complete(key, pixels);
        }
    }
    // Settle: keep the UI loop going until the final window is cached.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        while let Ok((key, pixels)) = rx.try_recv() {
            cache.complete(key, pixels);
        }
        let ready = last_window.iter().all(|key| !matches!(cache.lookup(key), Lookup::Missing));
        if ready || Instant::now() > deadline {
            let ready_visible = ready;
            return Run {
                decodes: decodes.load(Ordering::Relaxed),
                settle_ms: final_start.elapsed().as_secs_f64() * 1000.0,
                ready_visible,
            };
        }
        thread::sleep(FRAME);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("fixture dir"));
    let iterations: usize = args.next().unwrap_or_else(|| "3".into()).parse().expect("iterations");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("read fixture")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    files.sort();
    println!("fixture={} files={} window={WINDOW} iterations={iterations}", dir.display(), files.len());
    println!("load1={}", fs::read_to_string("/proc/loadavg").unwrap_or_default().split(' ').next().unwrap_or("?"));
    // Scenario A: fast scroll, 120 passes of 6 rows each (the window sweeps most of the folder).
    // Scenario B: steady view, one pass, time until its window is cached.
    for (name, passes, step) in [("scroll", 120usize, 2usize), ("steady", 1usize, 0usize)] {
        for (policy, new_policy) in [("old", false), ("new", true)] {
            let mut settle = Vec::new();
            let (mut decodes, mut visible) = (Vec::new(), true);
            for _ in 0..iterations {
                let r = run(&files, new_policy, passes, step);
                settle.push(r.settle_ms);
                decodes.push(r.decodes as f64);
                visible &= r.ready_visible;
            }
            println!(
                "{name}.{policy} settle_ms={:.1} decodes={:.0} final_window_ready={visible}",
                median(settle), median(decodes)
            );
        }
    }
}
