//! Single streaming walk over zip, tar and tar.gz archives.
//!
//! Every operation (list, extract, rewrite) goes through `visit`, so format handling lives here
//! only. The data reader passed to the callback is streamed; nothing is buffered per archive.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

use crate::error::Result;
use crate::format::Format;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    /// Hard links, devices, FIFOs and other entries `FlameWM` never materialises.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Raw, unsanitised entry name. Pass through `safe_path::sanitize` before use.
    pub path: PathBuf,
    pub size: u64,
    pub kind: Kind,
}

/// Streams each entry of `archive` through `f` together with its data reader.
pub fn visit<F>(archive: &Path, mut f: F) -> Result<()>
where
    F: FnMut(Entry, &mut dyn Read) -> Result<()>,
{
    let format = Format::detect(archive)?;
    let file = BufReader::new(File::open(archive)?);
    match format {
        Format::Zip => visit_zip(file, &mut f),
        Format::Tar => visit_tar(file, &mut f),
        Format::TarGz => visit_tar(GzDecoder::new(file), &mut f),
    }
}

/// Entries of `archive` in archive order, without data.
pub fn list(archive: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    visit(archive, |entry, _| {
        entries.push(entry);
        Ok(())
    })?;
    Ok(entries)
}

/// True when `path` equals or lies under one of `roots`.
pub fn under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn visit_zip<R, F>(reader: R, f: &mut F) -> Result<()>
where
    R: Read + Seek,
    F: FnMut(Entry, &mut dyn Read) -> Result<()>,
{
    let mut zip = zip::ZipArchive::new(reader)?;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index)?;
        let kind = if file.is_dir() {
            Kind::Dir
        } else if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
        {
            Kind::Symlink
        } else {
            Kind::File
        };
        let entry = Entry {
            path: PathBuf::from(file.name()?.as_ref()),
            size: file.size(),
            kind,
        };
        f(entry, &mut file)?;
    }
    Ok(())
}

fn visit_tar<R, F>(reader: R, f: &mut F) -> Result<()>
where
    R: Read,
    F: FnMut(Entry, &mut dyn Read) -> Result<()>,
{
    let mut archive = tar::Archive::new(reader);
    for item in archive.entries()? {
        let mut item = item?;
        let kind = match item.header().entry_type() {
            tar::EntryType::Regular | tar::EntryType::Continuous => Kind::File,
            tar::EntryType::Directory => Kind::Dir,
            tar::EntryType::Symlink => Kind::Symlink,
            _ => Kind::Other,
        };
        let entry = Entry {
            path: item.path()?.into_owned(),
            size: item.size(),
            kind,
        };
        f(entry, &mut item)?;
    }
    Ok(())
}
