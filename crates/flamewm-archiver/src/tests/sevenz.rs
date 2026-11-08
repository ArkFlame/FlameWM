//! 7z: listing parse, traversal rejection before the tool runs, symlink removal, selection,
//! cancel of a running child, and a real round trip when a 7z binary is installed.
//!
//! The traversal, symlink, selection and cancel tests use a fake tool script, so their behaviour
//! is pinned without depending on the installed 7z version.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::{Scratch, make_tree};
use crate::sevenz::{self, parse_listing};
use crate::{ArchiveError, CancelToken, Kind, Progress, create, extract, list};

const LISTING: &str = "\
7-Zip 26.00 (x64)

Listing archive: x.7z

--
Path = x.7z
Type = 7z

----------
Path = ok
Size = 0
Attributes = D drwxr-xr-x

Path = ok/a.txt
Size = 5
Attributes = A -rw-r--r--

Path = link
Size = 9
Attributes = A lrwxrwxrwx

Path = other/b.txt
Size = 3
Attributes = A -rw-r--r--
";

#[test]
fn listing_parses_dirs_files_and_symlinks() {
    let entries = parse_listing(LISTING);
    let summary: Vec<(String, Kind, u64)> = entries
        .iter()
        .map(|e| (e.path.display().to_string(), e.kind, e.size))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("ok".to_owned(), Kind::Dir, 0),
            ("ok/a.txt".to_owned(), Kind::File, 5),
            ("link".to_owned(), Kind::Symlink, 9),
            ("other/b.txt".to_owned(), Kind::File, 3),
        ]
    );
    assert_eq!(entries[1].mode, Some(0o644));
    assert!(!entries[0].encrypted);
}

/// Writes an executable `sh` script that stands in for `7z`.
fn fake_tool(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("fake7z");
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("tool");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Shell `case` body: `l` prints `listing`, `x` runs `on_extract` with `$out` set to the `-o` dir.
fn script(listing_file: &Path, on_extract: &str) -> String {
    format!(
        "case \"$1\" in\n  l) cat '{}' ;;\n  x)\n    for a in \"$@\"; do case \"$a\" in -o*) out=\"${{a#-o}}\" ;; esac; done\n    {on_extract}\n    ;;\nesac",
        listing_file.display()
    )
}

#[test]
fn traversal_name_in_listing_stops_before_the_tool_extracts() {
    let dir = Scratch::new("7z-traversal");
    let marker = dir.path().join("ran");
    let listing = dir.path().join("listing.txt");
    fs::write(
        &listing,
        "----------\nPath = ../escape.txt\nSize = 1\nAttributes = A -rw-r--r--\n",
    )
    .expect("listing");
    let tool = fake_tool(
        dir.path(),
        &script(&listing, &format!("touch '{}'", marker.display())),
    );
    let out = dir.path().join("out");
    let err = sevenz::extract_with(
        &tool,
        &dir.path().join("x.7z"),
        &out,
        &[],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::UnsafePath(_)), "got {err}");
    assert!(!marker.exists(), "tool extracted despite a hostile name");
    assert!(!out.exists(), "destination created before the check");
    assert!(!dir.path().join("escape.txt").exists());
}

#[test]
fn symlink_produced_by_the_tool_is_removed_not_moved() {
    let dir = Scratch::new("7z-symlink");
    let listing = dir.path().join("listing.txt");
    fs::write(&listing, LISTING).expect("listing");
    let tool = fake_tool(
        dir.path(),
        &script(
            &listing,
            "mkdir -p \"$out/ok\" \"$out/other\"; echo hi > \"$out/ok/a.txt\"; echo b > \"$out/other/b.txt\"; ln -s /etc/hostname \"$out/link\"",
        ),
    );
    let out = dir.path().join("out");
    let summary = sevenz::extract_with(
        &tool,
        &dir.path().join("x.7z"),
        &out,
        &[],
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("extract");
    assert!(out.join("ok/a.txt").is_file());
    assert!(fs::symlink_metadata(out.join("link")).is_err(), "symlink created");
    assert!(summary.skipped >= 1);
    let leftover = fs::read_dir(&out)
        .expect("out")
        .any(|e| e.expect("entry").file_name().to_string_lossy().starts_with(".flamewm-7z-stage"));
    assert!(!leftover, "staging directory left behind");
}

#[test]
fn selection_moves_only_the_selected_top_level_item() {
    let dir = Scratch::new("7z-select");
    let listing = dir.path().join("listing.txt");
    fs::write(&listing, LISTING).expect("listing");
    let tool = fake_tool(
        dir.path(),
        &script(
            &listing,
            "mkdir -p \"$out/ok\" \"$out/other\"; echo hi > \"$out/ok/a.txt\"; echo b > \"$out/other/b.txt\"",
        ),
    );
    let out = dir.path().join("out");
    sevenz::extract_with(
        &tool,
        &dir.path().join("x.7z"),
        &out,
        &[PathBuf::from("ok")],
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("extract");
    assert!(out.join("ok/a.txt").is_file());
    assert!(!out.join("other").exists());
}

#[test]
fn existing_target_is_refused_before_any_move() {
    let dir = Scratch::new("7z-clash");
    let listing = dir.path().join("listing.txt");
    fs::write(&listing, LISTING).expect("listing");
    let tool = fake_tool(
        dir.path(),
        &script(
            &listing,
            "mkdir -p \"$out/ok\" \"$out/other\"; echo hi > \"$out/ok/a.txt\"; echo b > \"$out/other/b.txt\"",
        ),
    );
    let out = dir.path().join("out");
    fs::create_dir_all(out.join("other")).expect("existing");
    let err = sevenz::extract_with(
        &tool,
        &dir.path().join("x.7z"),
        &out,
        &[],
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, ArchiveError::Io(_)), "got {err}");
    assert!(!out.join("ok").exists(), "partial move happened");
}

#[test]
fn cancel_kills_a_running_tool_promptly() {
    let dir = Scratch::new("7z-cancel");
    let listing = dir.path().join("listing.txt");
    fs::write(&listing, LISTING).expect("listing");
    let tool = fake_tool(dir.path(), &script(&listing, "sleep 30"));
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let timer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        trigger.cancel();
    });
    let started = Instant::now();
    let out = dir.path().join("out");
    let err = sevenz::extract_with(
        &tool,
        &dir.path().join("x.7z"),
        &out,
        &[],
        &cancel,
        &mut |_: Progress| {},
    )
    .unwrap_err();
    timer.join().expect("timer");
    assert!(matches!(err, ArchiveError::Cancelled), "got {err}");
    assert!(started.elapsed() < Duration::from_secs(10), "cancel too slow");
    assert!(!out.exists(), "cancelled run left a destination");
}

#[test]
fn real_7z_round_trip_when_installed() {
    if Command::new("7z").arg("-h").output().is_err() && Command::new("7zz").arg("-h").output().is_err() {
        eprintln!("SKIP: no 7z or 7zz on PATH");
        return;
    }
    let dir = Scratch::new("7z-real");
    make_tree(dir.path());
    let archive = dir.path().join("real.7z");
    create(&archive, &[dir.path().join("tree")], &CancelToken::new(), &mut |_| {})
        .expect("create");
    let entries = list(&archive).expect("list");
    assert!(entries.iter().any(|e| e.path == Path::new("tree/sub/b.txt")));
    let out = dir.path().join("out");
    extract(&archive, &out, &[], &CancelToken::new(), &mut |_| {}).expect("extract");
    assert_eq!(fs::read(out.join("tree/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(out.join("tree/sub/b.txt")).unwrap(), b"bravo");
}
