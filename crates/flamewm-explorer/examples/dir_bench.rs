//! Directory-listing bench for `scripts/perf/explorer_dir.sh`.
//! Usage: `dir_bench` <`fixture_dir`> <`file_count`> <iterations>
//! Prints one `key=value` line per metric (milliseconds, median over iterations).
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use flamewm_explorer::view::{self, Column, ColumnSort};
use flamewm_explorer::{SortBy, list_directory, list_directory_streamed, sort_listing};

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn ensure_fixture(dir: &PathBuf, count: usize) {
    fs::create_dir_all(dir).expect("create fixture dir");
    let existing = fs::read_dir(dir)
        .map(std::iter::Iterator::count)
        .unwrap_or(0);
    if existing >= count {
        return;
    }
    for index in 0..count {
        let name = format!("file-{index:05}-{}.txt", (index * 7919) % 100_003);
        fs::write(dir.join(name), b"x").expect("write fixture file");
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("fixture dir"));
    let count: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(20_000);
    let iterations: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(7);
    ensure_fixture(&dir, count);

    let mut listing_ms = Vec::new();
    let mut first_rows_ms = Vec::new();
    let mut stream_done_ms = Vec::new();
    let mut ui_sort_ms = Vec::new();
    let mut ui_check_ms = Vec::new();
    let mut rows = 0usize;
    let column = ColumnSort {
        column: Column::Name,
        descending: false,
    };
    for _ in 0..iterations {
        // Bulk listing: the rows of the old pipeline appear only when this returns.
        let start = Instant::now();
        let mut entries = list_directory(&dir, false, SortBy::Name).expect("list");
        listing_ms.push(start.elapsed().as_secs_f64() * 1e3);
        rows = entries.len();
        // Streamed listing: time to the first batch, and to the complete listing with the final sort.
        let start = Instant::now();
        let mut first: Option<f64> = None;
        let mut streamed = Vec::new();
        list_directory_streamed(&dir, false, |mut batch| {
            if first.is_none() {
                first = Some(start.elapsed().as_secs_f64() * 1e3);
            }
            sort_listing(&mut batch, SortBy::Name);
            streamed.extend(batch);
            true
        })
        .expect("stream");
        sort_listing(&mut streamed, SortBy::Name);
        stream_done_ms.push(start.elapsed().as_secs_f64() * 1e3);
        first_rows_ms.push(first.unwrap_or(f64::NAN));
        assert_eq!(streamed.len(), entries.len());
        // UI thread work per final listing: the old unconditional re-sort vs the new order check.
        let start = Instant::now();
        view::sort_entries(&mut entries, column);
        ui_sort_ms.push(start.elapsed().as_secs_f64() * 1e3);
        let start = Instant::now();
        let already = view::is_sorted_by(&streamed, column);
        ui_check_ms.push(start.elapsed().as_secs_f64() * 1e3);
        assert!(already);
    }
    println!("rows={rows}");
    println!("iterations={iterations}");
    println!(
        "time_to_first_rows_bulk_ms_median={:.3}",
        median(listing_ms.clone())
    );
    println!(
        "time_to_first_rows_streamed_ms_median={:.3}",
        median(first_rows_ms)
    );
    println!("streamed_complete_ms_median={:.3}", median(stream_done_ms));
    println!("bulk_listing_ms_median={:.3}", median(listing_ms));
    println!("ui_apply_resort_ms_median={:.3}", median(ui_sort_ms));
    println!("ui_apply_check_ms_median={:.3}", median(ui_check_ms));
}
