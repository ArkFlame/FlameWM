//! Streamed local listings must match the bulk listing exactly, and a superseded stream must stop early.
use std::fs;
use std::path::{Path, PathBuf};

use crate::view::{self, Column, ColumnSort};
use crate::{Entry, FIRST_BATCH, SortBy, list_directory, list_directory_streamed, sort_listing};

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "flamewm-listing-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Mixed folders, hidden files, case variants and sizes so every sort key is exercised.
fn populate(dir: &Path, files: usize) {
    fs::create_dir_all(dir.join("Folder-B")).unwrap();
    fs::create_dir_all(dir.join("folder-a")).unwrap();
    fs::write(dir.join(".hidden"), b"h").unwrap();
    for index in 0..files {
        let name = if index % 3 == 0 {
            format!("File-{index:05}.TXT")
        } else {
            format!("file-{index:05}.log")
        };
        fs::write(dir.join(name), vec![b'x'; (index * 37) % 911]).unwrap();
    }
}

fn paths(entries: &[Entry]) -> Vec<PathBuf> {
    entries.iter().map(|e| e.path.clone()).collect()
}

#[test]
fn streamed_batches_sort_to_the_bulk_listing_order() {
    let scratch = Scratch::new("equiv");
    populate(scratch.path(), 3_000);
    for sort in [SortBy::Name, SortBy::Size, SortBy::Modified] {
        for show_hidden in [false, true] {
            let bulk = list_directory(scratch.path(), show_hidden, sort).unwrap();
            let mut streamed = Vec::new();
            let mut sizes = Vec::new();
            let finished = list_directory_streamed(scratch.path(), show_hidden, |batch| {
                sizes.push(batch.len());
                streamed.extend(batch);
                true
            })
            .unwrap();
            assert!(finished);
            assert_eq!(sizes.iter().sum::<usize>(), bulk.len());
            assert_eq!(
                sizes[0], FIRST_BATCH,
                "first rows must come from a small first batch"
            );
            sort_listing(&mut streamed, sort);
            assert_eq!(
                paths(&streamed),
                paths(&bulk),
                "sort={sort:?} hidden={show_hidden}"
            );
            assert_eq!(streamed, bulk, "sort={sort:?} hidden={show_hidden}");
        }
    }
}

#[test]
fn small_directory_is_one_batch_and_empty_directory_emits_nothing() {
    let scratch = Scratch::new("small");
    populate(scratch.path(), 5);
    let mut calls = 0;
    list_directory_streamed(scratch.path(), false, |batch| {
        calls += 1;
        assert_eq!(batch.len(), 7);
        true
    })
    .unwrap();
    assert_eq!(calls, 1);

    let empty = Scratch::new("empty");
    let mut emitted = 0;
    assert!(
        list_directory_streamed(empty.path(), false, |_| {
            emitted += 1;
            true
        })
        .unwrap()
    );
    assert_eq!(emitted, 0);
}

#[test]
fn superseded_stream_stops_at_the_next_batch() {
    let scratch = Scratch::new("cancel");
    populate(scratch.path(), 3_000);
    let mut calls = 0;
    let finished = list_directory_streamed(scratch.path(), true, |_| {
        calls += 1;
        false
    })
    .unwrap();
    assert!(!finished, "a stopped stream reports it did not finish");
    assert_eq!(calls, 1, "no batch is read after the stop request");
}

#[test]
fn ui_sort_check_agrees_with_sort_entries() {
    let scratch = Scratch::new("check");
    populate(scratch.path(), 400);
    let mut entries = list_directory(scratch.path(), true, SortBy::Name).unwrap();
    let by_name = ColumnSort {
        column: Column::Name,
        descending: false,
    };
    let by_size = ColumnSort {
        column: Column::Size,
        descending: false,
    };
    assert!(
        view::is_sorted_by(&entries, by_name),
        "worker order is the default column order"
    );
    view::sort_entries(&mut entries, by_size);
    assert!(view::is_sorted_by(&entries, by_size));
}
