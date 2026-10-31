use std::fs;
use std::path::PathBuf;

use super::{Scratch, make_tar_gz, make_zip};
use crate::{CancelToken, Kind, Progress, extract, list};

#[test]
fn lists_zip_entries_with_kinds_and_sizes() {
    let dir = Scratch::new("list-zip");
    let archive = dir.path().join("a.zip");
    make_zip(
        &archive,
        &[("docs/", None), ("docs/note.txt", Some(b"hello"))],
    );
    let entries = list(&archive).expect("list");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].kind, Kind::Dir);
    assert_eq!(entries[1].kind, Kind::File);
    assert_eq!(entries[1].size, 5);
    assert_eq!(entries[1].path, PathBuf::from("docs/note.txt"));
}

#[test]
fn lists_tar_gz_entries() {
    let dir = Scratch::new("list-tgz");
    let archive = dir.path().join("a.tar.gz");
    make_tar_gz(
        &archive,
        &[("d", b'5', b""), ("d/x.bin", b'0', b"12345678")],
    );
    let entries = list(&archive).expect("list");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].size, 8);
    assert_eq!(entries[1].kind, Kind::File);
}

#[test]
fn extracts_all_zip_entries_with_progress() {
    let dir = Scratch::new("extract-zip");
    let archive = dir.path().join("a.zip");
    make_zip(
        &archive,
        &[
            ("top/", None),
            ("top/one.txt", Some(b"one")),
            ("two.txt", Some(b"22")),
        ],
    );
    let out = dir.path().join("out");
    let cancel = CancelToken::new();
    let mut last = Progress::default();
    let mut sink = |p: Progress| last = p;
    let summary = extract(&archive, &out, &[], &cancel, &mut sink).expect("extract");
    assert_eq!(fs::read(out.join("top/one.txt")).unwrap(), b"one");
    assert_eq!(fs::read(out.join("two.txt")).unwrap(), b"22");
    assert_eq!(summary.files, 2);
    assert_eq!(summary.bytes, 5);
    assert_eq!(last.bytes_done, last.bytes_total);
    assert_eq!(last.bytes_total, 5);
    assert_eq!(last.entries_done, last.entries_total);
    assert_eq!(last.entries_total, 3);
}

#[test]
fn extracts_only_selected_directory() {
    let dir = Scratch::new("extract-select");
    let archive = dir.path().join("a.tar.gz");
    make_tar_gz(
        &archive,
        &[
            ("keep", b'5', b""),
            ("keep/a.txt", b'0', b"A"),
            ("skip.txt", b'0', b"S"),
        ],
    );
    let out = dir.path().join("out");
    let cancel = CancelToken::new();
    extract(
        &archive,
        &out,
        &[PathBuf::from("keep")],
        &cancel,
        &mut |_| {},
    )
    .expect("extract");
    assert_eq!(fs::read(out.join("keep/a.txt")).unwrap(), b"A");
    assert!(!out.join("skip.txt").exists());
}
