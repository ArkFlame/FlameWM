//! Password-protected zip: AES-256 write, unlock on extract, and the same traversal rules.
//!
//! Encrypted entries must never be written without their password, and a hostile name inside an
//! encrypted archive must be rejected exactly like one in a plain archive.

use std::fs;
use std::io::Write;
use std::path::Path;

use super::{Scratch, make_tree};
use crate::{ArchiveError, CancelToken, create_encrypted, extract, extract_with_password, list};

const PASSWORD: &str = "correct horse";

/// Writes a zip whose every entry is AES-256 encrypted with `PASSWORD`, names taken verbatim.
fn make_encrypted_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).expect("zip file");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .with_aes_encryption(zip::AesMode::Aes256, PASSWORD);
    for (name, data) in entries {
        zip.start_file(*name, options).expect("start");
        zip.write_all(data).expect("write");
    }
    zip.finish().expect("finish");
}

#[test]
fn encrypted_zip_round_trips_with_the_right_password() {
    let dir = Scratch::new("aes-round-trip");
    make_tree(dir.path());
    let archive = dir.path().join("secret.zip");
    create_encrypted(
        &archive,
        &[dir.path().join("tree")],
        PASSWORD,
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("create");

    let out = dir.path().join("out");
    extract_with_password(
        &archive,
        &out,
        &[],
        Some(PASSWORD),
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("extract");
    assert_eq!(fs::read(out.join("tree/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(out.join("tree/sub/b.txt")).unwrap(), b"bravo");
}

#[test]
fn listing_works_without_a_password_and_marks_entries_encrypted() {
    let dir = Scratch::new("aes-list");
    let archive = dir.path().join("l.zip");
    make_encrypted_zip(&archive, &[("doc.txt", b"hidden")]);
    let entries = list(&archive).expect("list without password");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].encrypted);
    assert_eq!(entries[0].path, Path::new("doc.txt"));
}

#[test]
fn encrypted_entry_without_password_writes_nothing() {
    let dir = Scratch::new("aes-no-pw");
    let archive = dir.path().join("locked.zip");
    make_encrypted_zip(&archive, &[("doc.txt", b"hidden")]);
    let out = dir.path().join("out");
    let err = extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, ArchiveError::PasswordRequired), "got {err}");
    assert!(!out.exists(), "destination created before password check");
}

#[test]
fn wrong_password_is_reported_and_leaves_no_file() {
    let dir = Scratch::new("aes-wrong-pw");
    let archive = dir.path().join("locked.zip");
    make_encrypted_zip(&archive, &[("doc.txt", b"hidden")]);
    let out = dir.path().join("out");
    let err = extract_with_password(
        &archive,
        &out,
        &[],
        Some("wrong"),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::WrongPassword), "got {err}");
    assert!(!out.join("doc.txt").exists());
}

#[test]
fn zip_slip_inside_an_encrypted_archive_is_rejected_before_any_write() {
    let dir = Scratch::new("aes-zip-slip");
    let archive = dir.path().join("evil.zip");
    make_encrypted_zip(&archive, &[("ok.txt", b"fine"), ("../escape.txt", b"x")]);
    let out = dir.path().join("out");
    let err = extract_with_password(
        &archive,
        &out,
        &[],
        Some(PASSWORD),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert!(
        !dir.path().join("escape.txt").exists(),
        "escaped destination"
    );
    assert!(!out.join("ok.txt").exists(), "wrote before rejecting");
}

#[test]
fn password_is_refused_for_non_zip_names() {
    let dir = Scratch::new("aes-non-zip");
    make_tree(dir.path());
    let err = create_encrypted(
        &dir.path().join("x.tar.gz"),
        &[dir.path().join("tree")],
        PASSWORD,
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::Unsupported(_)), "got {err}");
    let err = create_encrypted(
        &dir.path().join("e.zip"),
        &[dir.path().join("tree")],
        "",
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::Unsupported(_)), "got {err}");
}
