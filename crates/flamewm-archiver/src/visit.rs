//! Single streaming walk over zip and every tar variant.
//!
//! Every operation (list, extract, rewrite) goes through `visit`, so format handling lives here
//! only. The data reader passed to the callback is streamed; nothing is buffered per archive.
//! Compressed tars share one tar path: only the decoder placed under `tar::Archive` differs.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use xz2::read::XzDecoder;
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::error::{ArchiveError, Result};
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
    /// Unix permission bits when the archive records them.
    pub mode: Option<u32>,
    /// True for a zip entry stored with encryption. Its data needs a password to read.
    pub encrypted: bool,
}

/// Streams each entry of `archive` through `f` together with its data reader.
pub fn visit<F>(archive: &Path, f: F) -> Result<()>
where
    F: FnMut(Entry, &mut dyn Read) -> Result<()>,
{
    visit_with_password(archive, None, f)
}

/// Like `visit`, with the password used for encrypted zip entries.
///
/// Without a password, an encrypted entry is still reported, but its data reader fails with
/// `ArchiveError::Io`, so listing works and nothing reads ciphertext as plaintext.
pub fn visit_with_password<F>(archive: &Path, password: Option<&str>, mut f: F) -> Result<()>
where
    F: FnMut(Entry, &mut dyn Read) -> Result<()>,
{
    let format = Format::detect(archive)?;
    let file = BufReader::new(File::open(archive)?);
    match format {
        Format::Zip => visit_zip(file, password, &mut f),
        Format::Tar => visit_tar(file, &mut f),
        Format::TarGz => visit_tar(GzDecoder::new(file), &mut f),
        Format::TarXz => visit_tar(XzDecoder::new(file), &mut f),
        Format::TarBz2 => visit_tar(BzDecoder::new(file), &mut f),
        Format::TarZst => visit_tar(ZstdDecoder::new(file)?, &mut f),
        Format::SevenZ => Err(ArchiveError::Unsupported(
            "7z entries are read by the 7z tool, not streamed".to_owned(),
        )),
    }
}

/// Entries of `archive` in archive order, without data.
pub fn list(archive: &Path) -> Result<Vec<Entry>> {
    if Format::detect(archive)? == Format::SevenZ {
        return crate::sevenz::list(archive);
    }
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

/// Reader that stands in for the data of an encrypted entry when no password was given.
struct Locked;

impl Read for Locked {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("archive entry is encrypted; a password is required"))
    }
}

fn visit_zip<R, F>(reader: R, password: Option<&str>, f: &mut F) -> Result<()>
where
    R: Read + Seek,
    F: FnMut(Entry, &mut dyn Read) -> Result<()>,
{
    let mut zip = zip::ZipArchive::new(reader)?;
    for index in 0..zip.len() {
        let probe = zip.by_index_raw(index)?;
        let encrypted = probe.encrypted();
        let entry = zip_entry(&probe, encrypted)?;
        drop(probe);
        if encrypted && password.is_none() {
            f(entry, &mut Locked)?;
            continue;
        }
        match password {
            Some(password) => {
                let mut file = zip.by_index_decrypt(index, password.as_bytes()).map_err(|err| {
                    match err {
                        zip::result::ZipError::InvalidPassword => ArchiveError::WrongPassword,
                        other => other.into(),
                    }
                })?;
                f(entry, &mut file)?;
            }
            None => {
                let mut file = zip.by_index(index)?;
                f(entry, &mut file)?;
            }
        }
    }
    Ok(())
}

fn zip_entry<R: Read>(file: &zip::read::ZipFile<'_, R>, encrypted: bool) -> Result<Entry> {
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
    Ok(Entry {
        path: PathBuf::from(file.name()?.as_ref()),
        size: file.size(),
        kind,
        mode: file.unix_mode(),
        encrypted,
    })
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
            mode: item.header().mode().ok(),
            encrypted: false,
        };
        f(entry, &mut item)?;
    }
    Ok(())
}
