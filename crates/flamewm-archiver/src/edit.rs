//! Add and remove entries by rewriting the archive.
//!
//! Tar cannot be edited in place and zip re-indexing would need the same copy, so both edits
//! stream every kept entry into a sibling `.partial` file and rename it over the original only on
//! success. An added name replaces an existing entry of the same top-level name.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::create::{Source, collect, write_sources_into};
use crate::error::Result;
use crate::format::Format;
use crate::progress::{CancelToken, Progress, Reporter, Summary};
use crate::safe_path::sanitize;
use crate::visit::{Entry, Kind, list, under_any, visit};
use crate::writer::{Writer, partial_path};

/// Removes every entry equal to or under one of `names` (archive-relative paths).
pub fn remove(
    archive: &Path,
    names: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    rewrite(archive, names, Vec::new(), 0, cancel, sink)
}

/// Adds `inputs` to the archive, replacing existing entries with the same top-level name.
pub fn add(
    archive: &Path,
    inputs: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let (sources, skipped) = collect(inputs)?;
    let replaced: Vec<PathBuf> = inputs
        .iter()
        .filter_map(|input| input.file_name().map(PathBuf::from))
        .collect();
    rewrite(archive, &replaced, sources, skipped, cancel, sink)
}

fn rewrite(
    archive: &Path,
    removed: &[PathBuf],
    sources: Vec<Source>,
    skipped: usize,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let format = Format::detect(archive)?;

    // Pass 1: validate every existing name and size the job. Unsafe names abort before any write.
    let mut entries_total = sources.len();
    let mut bytes_total: u64 = sources.iter().filter(|s| !s.is_dir).map(|s| s.size).sum();
    for entry in list(archive)? {
        let Some(rel) = sanitize(&entry.path)? else {
            continue;
        };
        if is_kept(&rel, removed) && matches!(entry.kind, Kind::File | Kind::Dir) {
            entries_total += 1;
            if entry.kind == Kind::File {
                bytes_total += entry.size;
            }
        }
    }

    let mut rep = Reporter::new(bytes_total, entries_total, cancel, sink);
    let partial = partial_path(archive);
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    match write_rewrite(
        format,
        archive,
        &partial,
        removed,
        &sources,
        &mut rep,
        &mut summary,
    ) {
        Ok(()) => {
            fs::rename(&partial, archive)?;
            Ok(summary)
        }
        Err(err) => {
            let _ = fs::remove_file(&partial);
            Err(err)
        }
    }
}

fn is_kept(rel: &Path, removed: &[PathBuf]) -> bool {
    !under_any(rel, removed)
}

fn write_rewrite(
    format: Format,
    archive: &Path,
    partial: &Path,
    removed: &[PathBuf],
    sources: &[Source],
    rep: &mut Reporter<'_>,
    summary: &mut Summary,
) -> Result<()> {
    let mut writer = Writer::create(format, partial)?;
    visit(archive, |entry: Entry, data: &mut dyn Read| {
        rep.check()?;
        let Some(rel) = sanitize(&entry.path)? else {
            return Ok(());
        };
        if !is_kept(&rel, removed) {
            return Ok(());
        }
        match entry.kind {
            Kind::Dir => {
                writer.add_dir(&rel)?;
                summary.dirs += 1;
                rep.finish_entry();
            }
            Kind::File => {
                writer.add_file(&rel, data, entry.size, rep)?;
                summary.files += 1;
                summary.bytes += entry.size;
                rep.finish_entry();
            }
            Kind::Symlink | Kind::Other => summary.skipped += 1,
        }
        Ok(())
    })?;
    write_sources_into(&mut writer, sources, rep, summary)?;
    writer.finish()
}
