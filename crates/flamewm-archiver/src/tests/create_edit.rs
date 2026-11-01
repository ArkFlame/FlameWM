use std::path::PathBuf;

use super::{Scratch, make_tree};
use crate::{ArchiveError, CancelToken, Format, Progress, add, create, extract, list, remove};

/// Every format `create` can write; used by the round-trip and mode tests so a new format cannot
/// be added to `Format` without being exercised here.
const CREATE_FORMATS: [&str; 5] = ["rt.zip", "rt.tar", "rt.tar.gz", "rt.tar.xz", "rt.tar.bz2"];

#[test]
fn create_then_list_and_extract_round_trip_for_each_format() {
    for name in CREATE_FORMATS {
        let dir = Scratch::new(&format!("create-{name}"));
        make_tree(dir.path());
        let archive = dir.path().join(name);
        let inputs = vec![dir.path().join("tree")];
        let summary = create(&archive, &inputs, &CancelToken::new(), &mut |_| {}).expect("create");
        assert_eq!(summary.files, 2, "{name}");
        assert!(!dir.path().join(format!(".{name}.partial")).exists());

        let names: Vec<_> = list(&archive)
            .expect("list")
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert!(
            names.contains(&PathBuf::from("tree/sub/b.txt")),
            "{name}: {names:?}"
        );

        let out = dir.path().join("out");
        extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).expect("extract");
        assert_eq!(
            std::fs::read(out.join("tree/sub/b.txt")).unwrap(),
            b"bravo",
            "{name}"
        );
        assert_eq!(
            std::fs::read(out.join("tree/a.txt")).unwrap(),
            b"alpha",
            "{name}"
        );
    }
}

/// The short spellings must resolve to the same format as the long ones, or an archive the user
/// named `x.txz` would be rejected while `x.tar.xz` works.
#[test]
fn every_accepted_extension_detects() {
    for (name, expected) in [
        ("a.zip", Format::Zip),
        ("a.tar", Format::Tar),
        ("a.tar.gz", Format::TarGz),
        ("a.tgz", Format::TarGz),
        ("a.tar.xz", Format::TarXz),
        ("a.txz", Format::TarXz),
        ("a.tar.bz2", Format::TarBz2),
        ("a.tbz2", Format::TarBz2),
        ("a.tbz", Format::TarBz2),
        ("A.TAR.XZ", Format::TarXz),
        ("A.TBZ2", Format::TarBz2),
    ] {
        assert_eq!(
            Format::detect(&PathBuf::from(name)).ok(),
            Some(expected),
            "{name} must detect"
        );
    }
    for name in ["a.7z", "a.rar", "a.tar.zst", "a.xz", "a.bz2", "a"] {
        assert!(
            matches!(
                Format::detect(&PathBuf::from(name)),
                Err(ArchiveError::Unsupported(_))
            ),
            "{name} must stay unsupported"
        );
    }
}

/// A compressed tar must not be read as a plain tar: the suffix list is ordered longest-first.
#[test]
fn suffix_stripping_prefers_the_longest_extension() {
    assert_eq!(Format::strip_suffix("a.tar.gz"), Some("a"));
    assert_eq!(Format::strip_suffix("a.tar.xz"), Some("a"));
    assert_eq!(Format::strip_suffix("a.tar.bz2"), Some("a"));
    assert_eq!(Format::strip_suffix("a.tbz2"), Some("a"));
    assert_eq!(Format::strip_suffix("a.tar"), Some("a"));
    assert_eq!(Format::strip_suffix("a.zip"), Some("a"));
    assert_eq!(Format::strip_suffix("a.txt"), None);
}

#[test]
fn unix_mode_survives_round_trip_for_each_format() {
    use std::os::unix::fs::PermissionsExt;
    for name in CREATE_FORMATS {
        let dir = Scratch::new(&format!("mode-{name}"));
        make_tree(dir.path());
        let script = dir.path().join("tree/run.sh");
        std::fs::write(&script, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let archive = dir.path().join(name);
        create(
            &archive,
            &[dir.path().join("tree")],
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
        let out = dir.path().join("out");
        extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap();
        let mode = std::fs::metadata(out.join("tree/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "{name}");
    }
}

/// Selection and progress must behave the same for the newly supported tars as for tar.gz.
#[test]
fn select_and_progress_work_for_xz_and_bz2() {
    for name in ["sel.tar.xz", "sel.tar.bz2"] {
        let dir = Scratch::new(&format!("select-{name}"));
        make_tree(dir.path());
        let archive = dir.path().join(name);
        create(
            &archive,
            &[dir.path().join("tree")],
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
        let out = dir.path().join("out");
        let mut last = Progress::default();
        let mut sink = |p: Progress| last = p;
        extract(
            &archive,
            &out,
            &[PathBuf::from("./tree/sub")],
            &CancelToken::new(),
            &mut sink,
        )
        .expect("extract");
        assert_eq!(std::fs::read(out.join("tree/sub/b.txt")).unwrap(), b"bravo");
        assert!(!out.join("tree/a.txt").exists(), "{name}");
        assert_eq!(last.bytes_done, last.bytes_total, "{name}");
        assert_eq!(last.bytes_total, 5, "{name}: only b.txt is in the job");
        assert_eq!(last.entries_total, 2, "{name}: dir entry plus one file");
    }
}

#[test]
fn cancel_mid_create_removes_partial_output() {
    for name in ["c.tar.xz", "c.tar.bz2"] {
        let dir = Scratch::new(&format!("cancel-create-{name}"));
        make_tree(dir.path());
        std::fs::write(dir.path().join("tree/big.bin"), vec![1u8; 4 * 1024 * 1024]).unwrap();
        let archive = dir.path().join(name);
        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let mut sink = |p: Progress| {
            if p.bytes_done >= 1024 * 1024 {
                trigger.cancel();
            }
        };
        let err = create(&archive, &[dir.path().join("tree")], &cancel, &mut sink).unwrap_err();
        assert!(matches!(err, ArchiveError::Cancelled), "{name}: got {err}");
        assert!(!archive.exists(), "{name}");
        assert!(
            !dir.path().join(format!(".{name}.partial")).exists(),
            "{name}: partial removed"
        );
    }
}

#[test]
fn remove_and_add_rewrite_archive() {
    let dir = Scratch::new("edit");
    make_tree(dir.path());
    let archive = dir.path().join("e.zip");
    create(
        &archive,
        &[dir.path().join("tree")],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();

    remove(
        &archive,
        &[PathBuf::from("tree/a.txt")],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    let names: Vec<_> = list(&archive)
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert!(!names.contains(&PathBuf::from("tree/a.txt")));
    assert!(names.contains(&PathBuf::from("tree/sub/b.txt")));

    let extra = dir.path().join("extra.txt");
    std::fs::write(&extra, b"new").unwrap();
    add(&archive, &[extra], &CancelToken::new(), &mut |_| {}).unwrap();
    let out = dir.path().join("out");
    extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap();
    assert_eq!(std::fs::read(out.join("extra.txt")).unwrap(), b"new");
    assert_eq!(std::fs::read(out.join("tree/sub/b.txt")).unwrap(), b"bravo");
    assert!(!out.join("tree/a.txt").exists());
}

/// Add/remove rewrite streams every kept entry through a fresh encoder, so the new compressed tars
/// must be readable after a rewrite, not only straight after create.
#[test]
fn remove_and_add_rewrite_compressed_tars() {
    for name in ["e.tar.xz", "e.tar.bz2"] {
        let dir = Scratch::new(&format!("edit-{name}"));
        make_tree(dir.path());
        let archive = dir.path().join(name);
        create(
            &archive,
            &[dir.path().join("tree")],
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();

        remove(
            &archive,
            &[PathBuf::from("tree/a.txt")],
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap();
        let names: Vec<_> = list(&archive)
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert!(!names.contains(&PathBuf::from("tree/a.txt")), "{name}");
        assert!(names.contains(&PathBuf::from("tree/sub/b.txt")), "{name}");

        let extra = dir.path().join("extra.txt");
        std::fs::write(&extra, b"new").unwrap();
        add(&archive, &[extra], &CancelToken::new(), &mut |_| {}).unwrap();
        let out = dir.path().join("out");
        extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(out.join("extra.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(out.join("tree/sub/b.txt")).unwrap(), b"bravo");
        assert!(!out.join("tree/a.txt").exists());
    }
}

/// Names the archive stores must reach the extractor as a real xz/bzip2 stream: a plain tar with a
/// `.tar.xz` name must fail to decode rather than silently succeed.
#[test]
fn compressed_suffix_with_uncompressed_bytes_is_an_error() {
    for name in ["fake.tar.xz", "fake.tar.bz2"] {
        let dir = Scratch::new(&format!("fake-{name}"));
        let archive = dir.path().join(name);
        let file = std::fs::File::create(&archive).unwrap();
        let mut tar = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "a.txt", &b"x"[..]).unwrap();
        tar.finish().unwrap();
        assert!(
            list(&archive).is_err(),
            "{name}: plain tar bytes must not decode"
        );
    }
}
