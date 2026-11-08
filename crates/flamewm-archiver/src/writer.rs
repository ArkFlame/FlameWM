//! Format-agnostic writer used by create, add and remove.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use bzip2::Compression as BzLevel;
use bzip2::write::BzEncoder;
use flate2::Compression;
use flate2::write::GzEncoder;
use xz2::write::XzEncoder;
use zstd::stream::write::Encoder as ZstdEncoder;
use zip::{AesMode, CompressionMethod};
use zip::write::SimpleFileOptions;

use crate::error::{ArchiveError, Result};
use crate::format::Format;
use crate::progress::{Reporter, Tracked, copy_tracked};

pub enum Writer {
    /// Zip with an optional AES-256 password applied to every file entry.
    Zip(zip::ZipWriter<File>, Option<String>),
    /// Every tar variant shares this one builder; only `Sink` differs by compression.
    Tar(tar::Builder<Sink>),
}

impl Writer {
    pub fn create(format: Format, path: &Path, password: Option<&str>) -> Result<Self> {
        let file = File::create(path)?;
        Ok(match format {
            Format::Zip => Self::Zip(zip::ZipWriter::new(file), password.map(str::to_owned)),
            other => Self::Tar(tar::Builder::new(Sink::create(other, file)?)),
        })
    }

    pub fn add_dir(&mut self, name: &Path, mode: u32) -> Result<()> {
        match self {
            Self::Zip(zip, _) => {
                let options = SimpleFileOptions::default().unix_permissions(mode & 0o777);
                zip.add_directory(zip_name(name)?, options)?;
            }
            Self::Tar(tar) => tar_dir(tar, name, mode)?,
        }
        Ok(())
    }

    pub fn add_file(
        &mut self,
        name: &Path,
        data: &mut dyn Read,
        size: u64,
        mode: u32,
        rep: &mut Reporter<'_>,
    ) -> Result<()> {
        match self {
            Self::Zip(zip, password) => {
                let mut options = SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .unix_permissions(mode & 0o777);
                if let Some(password) = password.as_deref() {
                    options = options.with_aes_encryption(AesMode::Aes256, password);
                }
                zip.start_file(zip_name(name)?, options)?;
                copy_tracked(data, zip, rep)?;
            }
            Self::Tar(tar) => tar_file(tar, name, data, size, mode, rep)?,
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        match self {
            Self::Zip(zip, _) => {
                zip.finish()?;
            }
            Self::Tar(mut tar) => {
                tar.finish()?;
                tar.into_inner()?.finish()?;
            }
        }
        Ok(())
    }
}

/// zstd level 3: the library default, a good ratio at a speed close to gzip.
const ZSTD_LEVEL: i32 = 3;

/// The byte stream under a tar builder: raw or one of the supported compressors.
///
/// Compressors are finished explicitly so the stream trailer (xz index, bzip2 end marker) is
/// always written; a dropped encoder would leave a file that system `tar` cannot read.
pub enum Sink {
    Raw(File),
    Gz(GzEncoder<File>),
    Xz(XzEncoder<File>),
    Bz2(BzEncoder<File>),
    Zst(ZstdEncoder<'static, File>),
}

impl Sink {
    /// Builds the tar byte sink for `format`. Only `Zip` fails here; zip has its own writer.
    pub fn create(format: Format, file: File) -> Result<Self> {
        Ok(match format {
            Format::Tar => Self::Raw(file),
            Format::TarGz => Self::Gz(GzEncoder::new(file, Compression::default())),
            Format::TarXz => Self::Xz(XzEncoder::new(file, 6)),
            Format::TarBz2 => Self::Bz2(BzEncoder::new(file, BzLevel::default())),
            Format::TarZst => Self::Zst(ZstdEncoder::new(file, ZSTD_LEVEL)?),
            Format::Zip | Format::SevenZ => {
                return Err(ArchiveError::Unsupported(format!(
                    "{} is not a tar format",
                    format_name(format)
                )));
            }
        })
    }

    /// Flushes the compressor trailer. `Raw` just flushes.
    pub fn finish(self) -> Result<()> {
        match self {
            Self::Raw(mut file) => file.flush()?,
            Self::Gz(encoder) => {
                encoder.finish()?;
            }
            Self::Xz(encoder) => {
                encoder.finish()?;
            }
            Self::Bz2(encoder) => {
                encoder.finish()?;
            }
            Self::Zst(encoder) => {
                encoder.finish()?;
            }
        }
        Ok(())
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Raw(w) => w.write(buf),
            Self::Gz(w) => w.write(buf),
            Self::Xz(w) => w.write(buf),
            Self::Bz2(w) => w.write(buf),
            Self::Zst(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Raw(w) => w.flush(),
            Self::Gz(w) => w.flush(),
            Self::Xz(w) => w.flush(),
            Self::Bz2(w) => w.flush(),
            Self::Zst(w) => w.flush(),
        }
    }
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Zip => "zip",
        Format::Tar => "tar",
        Format::TarGz => "tar.gz",
        Format::TarXz => "tar.xz",
        Format::TarBz2 => "tar.bz2",
        Format::TarZst => "tar.zst",
        Format::SevenZ => "7z",
    }
}

/// Sibling temp path that is renamed over the target only after a complete write.
pub fn partial_path(out: &Path) -> PathBuf {
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    out.with_file_name(format!(".{name}.partial"))
}

fn tar_header(kind: tar::EntryType, size: u64, mode: u32) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(size);
    header.set_mode(mode);
    header
}

fn tar_dir(tar: &mut tar::Builder<Sink>, name: &Path, mode: u32) -> Result<()> {
    let mut header = tar_header(tar::EntryType::Directory, 0, mode & 0o777);
    tar.append_data(&mut header, name, io::empty())?;
    Ok(())
}

fn tar_file(
    tar: &mut tar::Builder<Sink>,
    name: &Path,
    data: &mut dyn Read,
    size: u64,
    mode: u32,
    rep: &mut Reporter<'_>,
) -> Result<()> {
    let mut header = tar_header(tar::EntryType::Regular, size, mode & 0o777);
    let tracked = Tracked::new(data, rep);
    tar.append_data(&mut header, name, tracked)
        .map_err(|err| rep.fail(err))?;
    Ok(())
}

/// Forward-slash name as zip requires, built from normal components only.
fn zip_name(name: &Path) -> Result<String> {
    let mut out = String::new();
    for component in name.components() {
        if let Component::Normal(part) = component {
            if !out.is_empty() {
                out.push('/');
            }
            let text = part
                .to_str()
                .ok_or_else(|| ArchiveError::Unsupported(name.display().to_string()))?;
            out.push_str(text);
        }
    }
    Ok(out)
}
