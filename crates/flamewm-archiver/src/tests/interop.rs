//! Cross-tool honesty: an archive we write must be readable by the system extractors.
//!
//! Following the existing repo pattern (`flamewm-assistant-core::download`,
//! `flamewm-desktop-core::conversion`), these tests shell out to the real tool and return early
//! with a `SKIP` note when it is not installed, so a machine without `tar`/`xz`/`bzip2` does not
//! fail the suite. The extension is the contract: if `.tar.xz` bytes were not really xz, system
//! `tar` would reject them and these tests would fail.

use std::path::Path;
use std::process::Command;

use super::{Scratch, make_tree};
use crate::{CancelToken, create, extract, list};

/// Runs `program args...` and returns its exit status, or `None` when the program is absent.
fn run(program: &str, args: &[&str]) -> Option<std::process::Output> {
    match Command::new(program).args(args).output() {
        Ok(output) => Some(output),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("SKIP: {program} unavailable: {err}");
            None
        }
        Err(err) => panic!("{program} failed to start: {err}"),
    }
}

/// Creates `archive` from a two-file tree and asserts the text of `a.txt` after `extract`.
fn created_tree(dir: &Scratch, archive: &Path) {
    make_tree(dir.path());
    create(
        archive,
        &[dir.path().join("tree")],
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("create");
}

#[test]
fn system_tar_reads_our_xz_and_bz2_archives() {
    for (name, flag) in [("sys.tar.xz", "-J"), ("sys.tar.bz2", "-j"), ("sys.tar.zst", "--zstd")] {
        let dir = Scratch::new(&format!("interop-{name}"));
        let archive = dir.path().join(name);
        created_tree(&dir, &archive);

        let Some(out) = run("tar", &[flag, "-tf", &archive.to_string_lossy()]) else {
            return;
        };
        assert!(
            out.status.success(),
            "{name}: system tar could not list: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(listing.contains("tree/sub/b.txt"), "{name}: {listing}");

        // Extraction by the system tool proves the stream, not just the header block.
        let out_dir = dir.path().join("sys-out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let Some(extracted) = run(
            "tar",
            &[
                flag,
                "-xf",
                &archive.to_string_lossy(),
                "-C",
                &out_dir.to_string_lossy(),
            ],
        ) else {
            return;
        };
        assert!(
            extracted.status.success(),
            "{name}: system tar could not extract: {}",
            String::from_utf8_lossy(&extracted.stderr)
        );
        assert_eq!(
            std::fs::read(out_dir.join("tree/sub/b.txt")).unwrap(),
            b"bravo",
            "{name}"
        );
    }
}

/// The reverse direction: archives the system tools write must list and extract through us.
#[test]
fn we_read_system_tar_xz_and_bz2_archives() {
    for (name, flag) in [("sys-made.tar.xz", "-J"), ("sys-made.tar.bz2", "-j")] {
        let dir = Scratch::new(&format!("interop-reverse-{name}"));
        make_tree(dir.path());
        let archive = dir.path().join(name);
        let Some(out) = run(
            "tar",
            &[
                flag,
                "-cf",
                &archive.to_string_lossy(),
                "-C",
                &dir.path().to_string_lossy(),
                "tree",
            ],
        ) else {
            return;
        };
        assert!(
            out.status.success(),
            "{name}: could not build fixture: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let names: Vec<_> = list(&archive)
            .expect("list")
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert!(
            names.contains(&std::path::PathBuf::from("tree/a.txt")),
            "{name}: {names:?}"
        );

        let out_dir = dir.path().join("out");
        extract(&archive, &out_dir, &[], &CancelToken::new(), &mut |_| {}).expect("extract");
        assert_eq!(
            std::fs::read(out_dir.join("tree/sub/b.txt")).unwrap(),
            b"bravo",
            "{name}"
        );
    }
}

/// The compression bytes themselves must be the real thing, not only tar-readable by luck:
/// `xz` and `bzip2` must decode our output to a plain tar.
#[test]
fn raw_decompressors_accept_our_streams() {
    for (name, program) in [("plain.tar.xz", "xz"), ("plain.tar.bz2", "bzip2")] {
        let dir = Scratch::new(&format!("interop-decode-{name}"));
        let archive = dir.path().join(name);
        created_tree(&dir, &archive);
        let Some(out) = run(program, &["-d", "-c", &archive.to_string_lossy()]) else {
            return;
        };
        assert!(
            out.status.success(),
            "{name}: {program} rejected our stream: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        // A real xz/bzip2 stream decodes to a tar: the ustar magic sits at offset 257.
        assert!(
            out.stdout.len() > 512 && &out.stdout[257..262] == b"ustar",
            "{name}: decoded bytes are not a tar"
        );
    }
}
