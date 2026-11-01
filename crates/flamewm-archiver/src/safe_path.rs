//! Path-safety rules applied to every archive entry name and every write target.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{ArchiveError, Result};

/// Normalises an entry name to a relative path.
///
/// Rejects `..`, root and drive prefixes. `Ok(None)` means the name names the destination itself
/// (for example `./`) and nothing should be written for it.
pub fn sanitize(entry: &Path) -> Result<Option<PathBuf>> {
    let mut out = PathBuf::new();
    for component in entry.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ArchiveError::UnsafePath(entry.to_path_buf()));
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// Fails when any existing component of `dest/rel` is a symlink, so a write cannot leave `dest`.
pub fn reject_symlinks(dest: &Path, rel: &Path) -> Result<()> {
    let mut current = dest.to_path_buf();
    for component in rel.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(ArchiveError::UnsafePath(rel.to_path_buf()));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}
