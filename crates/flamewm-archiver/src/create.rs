//! Archive creation from files and directory trees.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{ArchiveError, Result};
use crate::format::Format;
use crate::progress::{CancelToken, Progress, Reporter, Summary};
use crate::writer::{Writer, partial_path};

/// One item to store: where it lives on disk and the name it gets inside the archive.
#[derive(Clone, Debug)]
pub struct Source {
    pub disk: PathBuf,
    pub name: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub mode: u32,
}

/// Creates `out` from `inputs`. The format comes from the `out` file name.
///
/// Writes to a sibling `.partial` file and renames it into place only on success.
pub fn create(
    out: &Path,
    inputs: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let format = Format::detect(out)?;
    let (sources, skipped) = collect(inputs)?;
    let bytes_total = sources.iter().filter(|s| !s.is_dir).map(|s| s.size).sum();
    let mut rep = Reporter::new(bytes_total, sources.len(), cancel, sink);
    let partial = partial_path(out);
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    match write_sources(format, &partial, &sources, &mut rep, &mut summary) {
        Ok(()) => {
            fs::rename(&partial, out)?;
            Ok(summary)
        }
        Err(err) => {
            let _ = fs::remove_file(&partial);
            Err(err)
        }
    }
}

/// Walks `inputs` without following symlinks. Returns the sources and the skipped-entry count.
pub fn collect(inputs: &[PathBuf]) -> Result<(Vec<Source>, usize)> {
    let mut sources = Vec::new();
    let mut skipped = 0usize;
    let mut stack: Vec<(PathBuf, PathBuf)> = Vec::new();
    for input in inputs {
        let name = input
            .file_name()
            .ok_or_else(|| ArchiveError::Unsupported(input.display().to_string()))?;
        stack.push((input.clone(), PathBuf::from(name)));
    }
    while let Some((disk, name)) = stack.pop() {
        let meta = fs::symlink_metadata(&disk)?;
        let file_type = meta.file_type();
        let mode = meta.permissions().mode();
        if file_type.is_dir() {
            sources.push(Source {
                disk: disk.clone(),
                name: name.clone(),
                is_dir: true,
                size: 0,
                mode,
            });
            let mut children = fs::read_dir(&disk)?.collect::<std::io::Result<Vec<_>>>()?;
            children.sort_by_key(fs::DirEntry::file_name);
            for child in children {
                stack.push((child.path(), name.join(child.file_name())));
            }
        } else if file_type.is_file() {
            sources.push(Source {
                disk,
                name,
                is_dir: false,
                size: meta.len(),
                mode,
            });
        } else {
            skipped += 1;
        }
    }
    Ok((sources, skipped))
}

fn write_sources(
    format: Format,
    partial: &Path,
    sources: &[Source],
    rep: &mut Reporter<'_>,
    summary: &mut Summary,
) -> Result<()> {
    let mut writer = Writer::create(format, partial)?;
    write_sources_into(&mut writer, sources, rep, summary)?;
    writer.finish()
}

/// Streams `sources` into an open writer. Shared by create and by the add step of an edit.
pub(crate) fn write_sources_into(
    writer: &mut Writer,
    sources: &[Source],
    rep: &mut Reporter<'_>,
    summary: &mut Summary,
) -> Result<()> {
    for source in sources {
        rep.check()?;
        if source.is_dir {
            writer.add_dir(&source.name, source.mode)?;
            summary.dirs += 1;
        } else {
            let mut file = File::open(&source.disk)?;
            writer.add_file(&source.name, &mut file, source.size, source.mode, rep)?;
            summary.files += 1;
            summary.bytes += source.size;
        }
        rep.finish_entry();
    }
    Ok(())
}
