use std::path::PathBuf;

use super::{Scratch, make_tree};
use crate::{ArchiveError, CancelToken, Progress, add, create, extract, list, remove};

#[test]
fn create_then_list_and_extract_round_trip_for_each_format() {
    for name in ["rt.zip", "rt.tar", "rt.tar.gz"] {
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
    }
}

#[test]
fn unix_mode_survives_round_trip_for_zip_and_tar_gz() {
    use std::os::unix::fs::PermissionsExt;
    for name in ["mode.zip", "mode.tar.gz"] {
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

#[test]
fn cancel_mid_create_removes_partial_output() {
    let dir = Scratch::new("cancel-create");
    make_tree(dir.path());
    std::fs::write(dir.path().join("tree/big.bin"), vec![1u8; 4 * 1024 * 1024]).unwrap();
    let archive = dir.path().join("c.tar.gz");
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let mut sink = |p: Progress| {
        if p.bytes_done >= 1024 * 1024 {
            trigger.cancel();
        }
    };
    let err = create(&archive, &[dir.path().join("tree")], &cancel, &mut sink).unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "got {err}");
    assert!(!archive.exists());
    assert!(!dir.path().join(".c.tar.gz.partial").exists());
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
