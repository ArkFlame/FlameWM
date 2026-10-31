//! Shared fixtures for archive tests. Scratch dirs follow `TMPDIR`; run tests with
//! `TMPDIR` on the SDD disk because `/tmp` is nearly full.

mod create_edit;
mod listing_extract;
mod safety;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Scratch directory removed on drop.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("flamewm-archiver-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes a zip; `None` data means a directory entry.
pub fn make_zip(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
    let file = File::create(path).expect("zip file");
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        match data {
            None => zip.add_directory(*name, options).expect("dir"),
            Some(bytes) => {
                zip.start_file(*name, options).expect("start");
                zip.write_all(bytes).expect("write");
            }
        }
    }
    zip.finish().expect("finish");
}

/// Writes a tar.gz from raw header fields, so hostile names can be stored verbatim.
/// `kind` is `b'0'` (regular), `b'5'` (directory) or `b'2'` (symlink to `data` as UTF-8).
pub fn make_tar_gz(path: &Path, entries: &[(&str, u8, &[u8])]) {
    let file = File::create(path).expect("tar file");
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    for (name, kind, data) in entries {
        if *kind == b'2' {
            // `append_link` writes name, target and checksum consistently.
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            let target = std::str::from_utf8(data).expect("utf8");
            tar.append_link(&mut header, *name, target).expect("link");
            continue;
        }
        let mut header = tar::Header::new_old();
        {
            let raw = header.as_old_mut();
            raw.name[..name.len()].copy_from_slice(name.as_bytes());
        }
        header.set_mode(0o644);
        if *kind == b'5' {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
        }
        header.set_cksum();
        tar.append(&header, *data).expect("append");
    }
    tar.into_inner().expect("tar").finish().expect("gz");
}

/// Builds a file tree under `root` for create tests.
pub fn make_tree(root: &Path) {
    fs::create_dir_all(root.join("tree/sub")).expect("tree");
    fs::write(root.join("tree/a.txt"), b"alpha").expect("a");
    fs::write(root.join("tree/sub/b.txt"), b"bravo").expect("b");
}
