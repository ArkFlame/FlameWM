//! Path traversal, zip-slip, symlink escape and cancel behaviour.

use std::fs;
use std::path::PathBuf;

use super::{Scratch, make_tar, make_zip};
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
    for name in ["evil.tar.gz", "evil.tar.xz", "evil.tar.bz2"] {
        let archive = dir.path().join(name);
        make_tar(&archive, &[("../up.txt", b'0', b"x")]);
        let err = extract(
            &archive,
            &dir.path().join(format!("out-{name}")),
            &[],
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, ArchiveError::UnsafePath(_)), "{name}: {err}");
    }

    for name in ["abs.tar.gz", "abs.tar.xz", "abs.tar.bz2"] {
        let archive = dir.path().join(name);
        make_tar(&archive, &[("/tmp/flamewm-abs-probe", b'0', b"x")]);
        let err = extract(
            &archive,
            &dir.path().join(format!("out2-{name}")),
            &[],
            &CancelToken::new(),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, ArchiveError::UnsafePath(_)), "{name}: {err}");
    }
    assert!(!PathBuf::from("/tmp/flamewm-abs-probe").exists());
}

#[test]
fn symlink_entry_is_skipped_and_never_created() {
    let dir = Scratch::new("symlink-entry");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let target = outside.to_string_lossy().into_owned();
    for name in ["link.tar.gz", "link.tar.xz", "link.tar.bz2"] {
        let archive = dir.path().join(name);
        make_tar(
            &archive,
            &[
                ("esc", b'2', target.as_bytes()),
                ("esc/pwned.txt", b'0', b"pwned"),
            ],
        );
        let out = dir.path().join(format!("out-{name}"));
        let summary =
            extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).expect("extract");
        assert_eq!(summary.skipped, 1, "{name}");
        assert!(
            !fs::symlink_metadata(out.join("esc"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{name}"
        );
        assert!(
            !outside.join("pwned.txt").exists(),
            "{name}: write escaped through symlink"
        );
    }
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
fn symlink_destination_is_rejected() {
    let dir = Scratch::new("symlink-destination");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let out = dir.path().join("out");
    std::os::unix::fs::symlink(&outside, &out).unwrap();
    let archive = dir.path().join("w.zip");
    make_zip(&archive, &[("pwned.txt", Some(b"pwned"))]);
    let err = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert!(!outside.join("pwned.txt").exists());
}

#[test]
fn symlink_destination_ancestor_is_rejected_before_creation() {
    let dir = Scratch::new("symlink-destination-ancestor");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let archive = dir.path().join("w.zip");
    make_zip(&archive, &[("pwned.txt", Some(b"pwned"))]);
    let err = extract(
        &archive,
        &link.join("new"),
        &[],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert!(!outside.join("new").exists());
}

#[test]
fn preexisting_hardlink_is_not_overwritten() {
    let dir = Scratch::new("hardlink-preexisting");
    let outside = dir.path().join("outside");
    fs::write(&outside, b"original").unwrap();
    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::hard_link(&outside, out.join("file.txt")).unwrap();
    let archive = dir.path().join("w.zip");
    make_zip(&archive, &[("file.txt", Some(b"changed"))]);
    let err = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert_eq!(fs::read(&outside).unwrap(), b"original");
    assert_eq!(fs::read(out.join("file.txt")).unwrap(), b"original");
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

#[test]
fn zip_symlink_entry_cannot_redirect_a_later_write() {
    let dir = Scratch::new("zip-symlink-escape");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let archive = dir.path().join("esc.zip");
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.add_symlink("esc", outside.to_string_lossy(), options)
            .unwrap();
        zip.start_file("esc/pwned.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();
    }
    let out = dir.path().join("out");
    let summary = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).expect("extract");
    assert_eq!(summary.skipped, 1);
    assert!(
        !outside.join("pwned.txt").exists(),
        "write escaped via zip symlink"
    );
    assert!(
        !fs::symlink_metadata(out.join("esc"))
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    );
}

#[test]
fn cancel_mid_extract_removes_partial_compressed_tar_output() {
    let dir = Scratch::new("cancel-tar-xz");
    let archive = dir.path().join("big.tar.xz");
    let payload = vec![9u8; 4 * 1024 * 1024];
    make_tar(&archive, &[("big.bin", b'0', &payload)]);
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

#[test]
fn cancel_before_create_writes_no_archive() {
    let dir = Scratch::new("cancel-create");
    let src = dir.path().join("src.bin");
    fs::write(&src, vec![3u8; 1024 * 1024]).unwrap();
    let out = dir.path().join("out.tar.gz");
    let cancel = CancelToken::new();
    cancel.cancel();
    let result = crate::create(&out, &[src], &cancel, &mut |_| {});
    assert!(matches!(result, Err(ArchiveError::Cancelled)), "{result:?}");
    assert!(!out.exists(), "archive left behind after cancel");
    assert!(
        !crate::writer::partial_path(&out).exists(),
        "partial left behind"
    );
}
