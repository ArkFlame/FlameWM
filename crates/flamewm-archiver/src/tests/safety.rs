//! Path traversal, zip-slip, symlink escape and cancel behaviour.

use std::fs;
use std::path::PathBuf;

use super::{Scratch, make_tar_gz, make_zip};
use crate::{ArchiveError, CancelToken, Progress, extract, sanitize};

#[test]
fn sanitize_rejects_parent_and_absolute_names() {
    assert!(matches!(
        sanitize("../x".as_ref()),
        Err(ArchiveError::UnsafePath(_))
    ));
    assert!(matches!(
        sanitize("a/../../x".as_ref()),
        Err(ArchiveError::UnsafePath(_))
    ));
    assert!(matches!(
        sanitize("/etc/x".as_ref()),
        Err(ArchiveError::UnsafePath(_))
    ));
    assert_eq!(
        sanitize("./a/b".as_ref()).unwrap(),
        Some(PathBuf::from("a/b"))
    );
    assert_eq!(sanitize("./".as_ref()).unwrap(), None);
}

#[test]
fn zip_slip_is_rejected_before_any_write() {
    let dir = Scratch::new("zip-slip");
    let archive = dir.path().join("evil.zip");
    make_zip(
        &archive,
        &[
            ("ok.txt", Some(b"fine")),
            ("../escaped.txt", Some(b"pwned")),
        ],
    );
    let out = dir.path().join("out");
    let err = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert!(!dir.path().join("escaped.txt").exists());
    assert!(
        !out.join("ok.txt").exists(),
        "nothing written before validation"
    );
}

#[test]
fn tar_parent_and_absolute_names_are_rejected() {
    let dir = Scratch::new("tar-traversal");
    let archive = dir.path().join("evil.tar.gz");
    make_tar_gz(&archive, &[("../up.txt", b'0', b"x")]);
    let err = extract(
        &archive,
        &dir.path().join("out"),
        &[],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)));

    let archive = dir.path().join("abs.tar.gz");
    make_tar_gz(&archive, &[("/tmp/flamewm-abs-probe", b'0', b"x")]);
    let err = extract(
        &archive,
        &dir.path().join("out2"),
        &[],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)));
    assert!(!PathBuf::from("/tmp/flamewm-abs-probe").exists());
}

#[test]
fn symlink_entry_is_skipped_and_never_created() {
    let dir = Scratch::new("symlink-entry");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let archive = dir.path().join("link.tar.gz");
    let target = outside.to_string_lossy().into_owned();
    make_tar_gz(
        &archive,
        &[
            ("esc", b'2', target.as_bytes()),
            ("esc/pwned.txt", b'0', b"pwned"),
        ],
    );
    let out = dir.path().join("out");
    let summary = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).expect("extract");
    assert_eq!(summary.skipped, 1);
    assert!(
        !fs::symlink_metadata(out.join("esc"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !outside.join("pwned.txt").exists(),
        "write escaped through symlink"
    );
}

#[test]
fn write_through_preexisting_symlink_is_rejected() {
    let dir = Scratch::new("symlink-preexisting");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("esc")).unwrap();
    let archive = dir.path().join("w.zip");
    make_zip(&archive, &[("esc/pwned.txt", Some(b"pwned"))]);
    let err = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert!(!outside.join("pwned.txt").exists());
}

#[test]
fn cancel_mid_extract_stops_and_removes_partial_file() {
    let dir = Scratch::new("cancel-extract");
    let archive = dir.path().join("big.zip");
    let payload = vec![7u8; 4 * 1024 * 1024];
    make_zip(&archive, &[("big.bin", Some(&payload))]);
    let out = dir.path().join("out");
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let mut sink = |p: Progress| {
        if p.bytes_done >= 1024 * 1024 {
            trigger.cancel();
        }
    };
    let err = extract(&archive, &out, &[], &cancel, &mut sink).unwrap_err();
    assert!(matches!(err, ArchiveError::Cancelled), "got {err}");
    assert!(!out.join("big.bin").exists(), "truncated file left behind");
}
