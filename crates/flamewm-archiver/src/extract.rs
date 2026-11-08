//! Extraction with path-safety, selection and progress.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{ArchiveError, Result};
use crate::format::Format;
use crate::progress::{CancelToken, Progress, Reporter, Summary, copy_tracked};
use crate::safe_path;
use crate::visit::{Entry, Kind, list, under_any, visit_with_password};

/// Extracts `archive` into `dest`. Encrypted zip entries need `extract_with_password`.
///
/// An empty `selection` extracts everything. A selected directory includes its contents.
/// Every listed entry name is validated before the first write. If the archive stays unchanged,
/// one unsafe name aborts with nothing written. Symlink, hard link and special entries are skipped.
/// Filesystem checks are not atomic with writes; do not extract into a tree that another process
/// can modify concurrently. Archive validation is also not atomic with a changing archive.
pub fn extract(
    archive: &Path,
    dest: &Path,
    selection: &[PathBuf],
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    extract_with_password(archive, dest, selection, None, cancel, sink)
}

/// Extracts `archive` like `extract`, unlocking encrypted zip entries with `password`.
///
/// A selected encrypted entry without a password fails before any write.
pub fn extract_with_password(
    archive: &Path,
    dest: &Path,
    selection: &[PathBuf],
    password: Option<&str>,
    cancel: &CancelToken,
    sink: &mut dyn FnMut(Progress),
) -> Result<Summary> {
    if Format::detect(archive)? == Format::SevenZ {
        if password.is_some() {
            return Err(ArchiveError::Unsupported(
                "passwords apply to zip only".to_owned(),
            ));
        }
        return crate::sevenz::extract(archive, dest, selection, cancel, sink);
    }
    // Selected roots are compared against sanitised entry names, so they are sanitised the same
    // way: `./docs` and `docs` both match an entry stored as `./docs/a.txt`. A non-empty selection
    // that sanitises away entirely selects nothing, never everything.
    let roots = selection
        .iter()
        .map(|root| safe_path::sanitize(root))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let nothing_selected = roots.is_empty() && !selection.is_empty();

    let mut entries_total = 0usize;
    let mut bytes_total = 0u64;
    for entry in list(archive)? {
        let Some(rel) = safe_path::sanitize(&entry.path)? else {
            continue;
        };
        if is_selected(&rel, &roots, nothing_selected) {
            if entry.encrypted && password.is_none() {
                return Err(ArchiveError::PasswordRequired);
            }
            entries_total += 1;
            if entry.kind == Kind::File {
                bytes_total += entry.size;
            }
        }
    }

    // Only a destination this call creates is removed again when the job does not finish, so a
    // cancel never deletes a directory the user already had.
    let created = !dest.exists();
    if !nothing_selected {
        safe_path::reject_symlinks(dest, Path::new(""))?;
        fs::create_dir_all(dest)?;
    }
    let mut rep = Reporter::new(bytes_total, entries_total, cancel, sink);
    let mut summary = Summary::default();
    let outcome = visit_with_password(archive, password, |entry: Entry, data: &mut dyn Read| {
        rep.check()?;
        let Some(rel) = safe_path::sanitize(&entry.path)? else {
            return Ok(());
        };
        if !is_selected(&rel, &roots, nothing_selected) {
            return Ok(());
        }
        extract_one(dest, &rel, &entry, data, &mut rep, &mut summary)?;
        rep.finish_entry();
        Ok(())
    });
    match outcome {
        Ok(()) => Ok(summary),
        Err(err) => {
            if created {
                let _ = fs::remove_dir_all(dest);
            }
            Err(err)
        }
    }
}

fn is_selected(rel: &Path, roots: &[PathBuf], nothing_selected: bool) -> bool {
    if nothing_selected {
        false
    } else {
        roots.is_empty() || under_any(rel, roots)
    }
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
            summary.bytes += write_file(dest, rel, data, entry.mode, rep)?;
            summary.files += 1;
        }
        Kind::Symlink | Kind::Other => summary.skipped += 1,
    }
    Ok(())
}

fn write_file(
    dest: &Path,
    rel: &Path,
    data: &mut dyn Read,
    mode: Option<u32>,
    rep: &mut Reporter<'_>,
) -> Result<u64> {
    safe_path::reject_symlinks(dest, rel)?;
    let target = dest.join(rel);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    // Do not truncate a shared inode or open a device, FIFO, or other special node.
    // This check does not protect against concurrent replacement or hard-link creation.
    match fs::symlink_metadata(&target) {
        Ok(meta) if !meta.is_file() || meta.nlink() > 1 => {
            return Err(crate::error::ArchiveError::UnsafePath(rel.to_path_buf()));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let mut out = File::create(&target)?;
    match copy_tracked(data, &mut out, rep) {
        Ok(n) => {
            // Permission bits only: setuid/setgid/sticky are never restored from an archive.
            if let Some(bits) = mode {
                out.set_permissions(fs::Permissions::from_mode(bits & 0o777))?;
            }
            Ok(n)
        }
        Err(err) => {
            // Never leave a truncated file behind after a failure or cancel.
            drop(out);
            let _ = fs::remove_file(&target);
            Err(err)
        }
    }
}
