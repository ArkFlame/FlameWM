use std::fs;
use std::path::PathBuf;

use super::{Scratch, make_tar, make_tar_gz, make_zip};
use crate::{ArchiveError, CancelToken, Kind, Progress, extract, list};

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

/// Every compressed tar must list through the same tar path, so a decoder mix-up shows up here.
#[test]
fn lists_entries_of_every_tar_variant() {
    for name in [
        "l.tar",
        "l.tar.gz",
        "l.tgz",
        "l.tar.xz",
        "l.txz",
        "l.tar.bz2",
        "l.tbz2",
        "l.tbz",
        "l.tar.zst",
        "l.tzst",
    ] {
        let dir = Scratch::new(&format!("list-{name}"));
        let archive = dir.path().join(name);
        make_tar(
            &archive,
            &[("d", b'5', b""), ("d/x.bin", b'0', b"12345678")],
        );
        let entries = list(&archive).expect("list");
        assert_eq!(entries.len(), 2, "{name}");
        assert_eq!(entries[0].kind, Kind::Dir, "{name}");
        assert_eq!(entries[1].kind, Kind::File, "{name}");
        assert_eq!(entries[1].size, 8, "{name}");
        assert_eq!(entries[1].path, PathBuf::from("d/x.bin"), "{name}");
    }
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

/// The window hands over raw entry names, which may carry a `./` prefix the archive itself does
/// not use. Selection must still match, or "Extract selected" silently extracts nothing.
#[test]
fn selected_dot_prefixed_root_matches_sanitised_entries() {
    let dir = Scratch::new("extract-select-dot");
    let archive = dir.path().join("a.zip");
    make_zip(
        &archive,
        &[("keep/a.txt", Some(b"A")), ("skip.txt", Some(b"S"))],
    );
    let out = dir.path().join("out");
    let mut seen = Progress::default();
    let mut sink = |p: Progress| seen = p;
    extract(
        &archive,
        &out,
        &[PathBuf::from("./keep/")],
        &CancelToken::new(),
        &mut sink,
    )
    .expect("extract");
    assert_eq!(fs::read(out.join("keep/a.txt")).unwrap(), b"A");
    assert!(!out.join("skip.txt").exists());
    // This archive carries no `keep/` directory entry, so the file is the only selected entry.
    assert_eq!(seen.entries_total, 1, "the selected file is in the job");
}

/// A selection that sanitises away names nothing, so it must extract nothing rather than fall
/// back to "empty selection means everything".
#[test]
fn selection_that_sanitises_away_extracts_nothing() {
    let dir = Scratch::new("extract-select-empty");
    let archive = dir.path().join("a.zip");
    make_zip(&archive, &[("only.txt", Some(b"O"))]);
    let out = dir.path().join("out");
    let summary = extract(
        &archive,
        &out,
        &[PathBuf::from("./")],
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("extract");
    assert_eq!(summary.files, 0);
    assert!(!out.join("only.txt").exists());
    assert!(!out.exists(), "nothing selected creates no destination");
}

/// A cancelled extraction must not leave the half-written tree behind, but must never delete a
/// destination the user already owned.
#[test]
fn cancel_removes_only_a_destination_it_created() {
    let dir = Scratch::new("cancel-tree");
    let archive = dir.path().join("big.zip");
    let first = vec![1u8; 4 * 1024 * 1024];
    let second = vec![2u8; 4 * 1024 * 1024];
    make_zip(
        &archive,
        &[("a.bin", Some(&first)), ("b.bin", Some(&second))],
    );

    let fresh = dir.path().join("fresh");
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let mut sink = |p: Progress| {
        if p.bytes_done >= 1024 * 1024 {
            trigger.cancel();
        }
    };
    let err = extract(&archive, &fresh, &[], &cancel, &mut sink).unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "got {err}");
    assert!(!fresh.exists(), "a cancelled tree it created is removed");

    let existing = dir.path().join("existing");
    fs::create_dir_all(&existing).unwrap();
    fs::write(existing.join("keep.txt"), b"mine").unwrap();
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let mut sink = |p: Progress| {
        if p.bytes_done >= 1024 * 1024 {
            trigger.cancel();
        }
    };
    let err = extract(&archive, &existing, &[], &cancel, &mut sink).unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "got {err}");
    assert_eq!(fs::read(existing.join("keep.txt")).unwrap(), b"mine");
    assert!(!existing.join("a.bin").exists(), "partial file removed");
}
