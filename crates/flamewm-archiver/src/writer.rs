//! Format-agnostic writer used by create, add and remove.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use crate::error::{ArchiveError, Result};
use crate::format::Format;
use crate::progress::{Reporter, Tracked, copy_tracked};

pub enum Writer {
    Zip(zip::ZipWriter<File>),
    Tar(tar::Builder<File>),
    TarGz(tar::Builder<GzEncoder<File>>),
}

impl Writer {
    pub fn create(format: Format, path: &Path) -> Result<Self> {
        let file = File::create(path)?;
        Ok(match format {
            Format::Zip => Self::Zip(zip::ZipWriter::new(file)),
            Format::Tar => Self::Tar(tar::Builder::new(file)),
            Format::TarGz => Self::TarGz(tar::Builder::new(GzEncoder::new(
                file,
                Compression::default(),
            ))),
        })
    }

    pub fn add_dir(&mut self, name: &Path, mode: u32) -> Result<()> {
        match self {
            Self::Zip(zip) => {
                let options = SimpleFileOptions::default().unix_permissions(mode & 0o777);
                zip.add_directory(zip_name(name)?, options)?;
            }
            Self::Tar(tar) => tar_dir(tar, name, mode)?,
            Self::TarGz(tar) => tar_dir(tar, name, mode)?,
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
            Self::Zip(zip) => {
                let options = SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .unix_permissions(mode & 0o777);
                zip.start_file(zip_name(name)?, options)?;
                copy_tracked(data, zip, rep)?;
            }
            Self::Tar(tar) => tar_file(tar, name, data, size, mode, rep)?,
            Self::TarGz(tar) => tar_file(tar, name, data, size, mode, rep)?,
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        match self {
            Self::Zip(zip) => {
                zip.finish()?;
            }
            Self::Tar(mut tar) => {
                tar.finish()?;
                tar.into_inner()?;
            }
            Self::TarGz(mut tar) => {
                tar.finish()?;
                tar.into_inner()?.finish()?;
            }
        }
        Ok(())
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

fn tar_dir<W: Write>(tar: &mut tar::Builder<W>, name: &Path, mode: u32) -> Result<()> {
    let mut header = tar_header(tar::EntryType::Directory, 0, mode & 0o777);
    tar.append_data(&mut header, name, io::empty())?;
    Ok(())
}

fn tar_file<W: Write>(
    tar: &mut tar::Builder<W>,
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
