//! Extraction with path-safety, selection and progress.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::progress::{CancelToken, Progress, Reporter, Summary, copy_tracked};
use crate::safe_path;
use crate::visit::{Entry, Kind, list, under_any, visit};

/// Extracts `archive` into `dest`.
///
/// An empty `selection` extracts everything. A selected directory includes its contents.
/// Every entry name is validated before the first write, so one unsafe name aborts with nothing
/// written. Symlink, hard link and special entries are counted as skipped and never created.
pub fn extract(
    archive: &Path,
    dest: &Path,
    selection: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    let mut entries_total = 0usize;
    let mut bytes_total = 0u64;
    for entry in list(archive)? {
        let Some(rel) = safe_path::sanitize(&entry.path)? else {
            continue;
        };
        if is_selected(&rel, selection) {
            entries_total += 1;
            if entry.kind == Kind::File {
                bytes_total += entry.size;
            }
        }
    }

    fs::create_dir_all(dest)?;
    let mut rep = Reporter::new(bytes_total, entries_total, cancel, sink);
    let mut summary = Summary::default();
    visit(archive, |entry: Entry, data: &mut dyn Read| {
        rep.check()?;
        let Some(rel) = safe_path::sanitize(&entry.path)? else {
            return Ok(());
        };
        if !is_selected(&rel, selection) {
            return Ok(());
        }
        extract_one(dest, &rel, &entry, data, &mut rep, &mut summary)?;
        rep.finish_entry();
        Ok(())
    })?;
    Ok(summary)
}

fn is_selected(rel: &Path, selection: &[PathBuf]) -> bool {
    selection.is_empty() || under_any(rel, selection)
}

fn extract_one(
    dest: &Path,
    rel: &Path,
    entry: &Entry,
    data: &mut dyn Read,
    rep: &mut Reporter<'_>,
    summary: &mut Summary,
) -> Result<()> {
    match entry.kind {
        Kind::Dir => {
            safe_path::reject_symlinks(dest, rel)?;
            fs::create_dir_all(dest.join(rel))?;
            summary.dirs += 1;
        }
        Kind::File => {
            summary.bytes += write_file(dest, rel, data, rep)?;
            summary.files += 1;
        }
        Kind::Symlink | Kind::Other => summary.skipped += 1,
    }
    Ok(())
}

fn write_file(dest: &Path, rel: &Path, data: &mut dyn Read, rep: &mut Reporter<'_>) -> Result<u64> {
    safe_path::reject_symlinks(dest, rel)?;
    let target = dest.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = File::create(&target)?;
    match copy_tracked(data, &mut out, rep) {
        Ok(n) => Ok(n),
        Err(err) => {
            // Never leave a truncated file behind after a failure or cancel.
            drop(out);
            let _ = fs::remove_file(&target);
            Err(err)
        }
    }
}
