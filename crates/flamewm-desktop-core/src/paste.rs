//! Copy or move of pasted files into a folder, with non-destructive name choice.
//!
//! Pure naming ([`paste_destination`]) is separate from the filesystem work ([`paste_into`]) so
//! collision rules are unit-tested without touching disk.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// Treat dangling symlinks as occupied entries too.
fn entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Splits `name` into stem and extension at the last dot. Dotfiles such as `.env` and names
/// without a dot keep the whole name as the stem.
fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    }
}

/// Name for a pasted entry inside `directory`. Keeps `name` when free; otherwise `stem (copy).ext`,
/// then `stem (copy N).ext` for N from 2. Never returns an existing entry.
#[must_use]
pub fn paste_destination(directory: &Path, name: &OsStr) -> PathBuf {
    let first = directory.join(name);
    if !entry_exists(&first) {
        return first;
    }
    let text = name.to_string_lossy();
    let (stem, extension) = split_name(&text);
    let candidate = directory.join(format!("{stem} (copy){extension}"));
    if !entry_exists(&candidate) {
        return candidate;
    }
    for number in 2_u32..=u32::MAX {
        let candidate = directory.join(format!("{stem} (copy {number}){extension}"));
        if !entry_exists(&candidate) {
            return candidate;
        }
    }
    directory.join(format!("{stem} (overflow){extension}"))
}

/// Copies `source` (file, symlink or directory tree) to `destination`, which must not exist.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(source).map_err(|error| format!("{}: {error}", source.display()))?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(source).map_err(|error| format!("{}: {error}", source.display()))?;
        return std::os::unix::fs::symlink(target, destination)
            .map_err(|error| format!("{}: {error}", destination.display()));
    }
    if meta.is_dir() {
        fs::create_dir(destination).map_err(|error| format!("{}: {error}", destination.display()))?;
        let entries = fs::read_dir(source).map_err(|error| format!("{}: {error}", source.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("{}: {error}", source.display()))?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| format!("{}: {error}", source.display()))
}

/// Removes a source after a successful cross-device move fallback.
fn remove_tree(source: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(source).map_err(|error| format!("{}: {error}", source.display()))?;
    let result = if meta.is_dir() {
        fs::remove_dir_all(source)
    } else {
        fs::remove_file(source)
    };
    result.map_err(|error| format!("{}: {error}", source.display()))
}

/// Pastes each source into `directory`. A move renames when possible and falls back to copy then
/// remove across devices. Copying a folder into itself is refused. Returns one outcome per source,
/// holding the new path or the reason it failed.
pub fn paste_into(directory: &Path, sources: &[PathBuf], move_it: bool) -> Vec<Result<PathBuf, String>> {
    sources
        .iter()
        .map(|source| paste_one(directory, source, move_it))
        .collect()
}

fn paste_one(directory: &Path, source: &Path, move_it: bool) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .ok_or_else(|| format!("{}: no file name", source.display()))?;
    if source.is_dir() && directory.starts_with(source) {
        return Err(format!("{}: cannot paste a folder into itself", source.display()));
    }
    let destination = paste_destination(directory, name);
    if move_it {
        match fs::rename(source, &destination) {
            Ok(()) => return Ok(destination),
            // Cross-device or otherwise refused: copy first, delete the source only after success.
            Err(_) => {}
        }
    }
    copy_tree(source, &destination)?;
    if move_it {
        remove_tree(source)?;
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-paste-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn free_name_is_kept_and_collisions_get_copy_suffix() {
        let dir = temp_dir("names");
        assert_eq!(paste_destination(&dir, OsStr::new("a.png")), dir.join("a.png"));
        fs::write(dir.join("a.png"), b"x").unwrap();
        assert_eq!(paste_destination(&dir, OsStr::new("a.png")), dir.join("a (copy).png"));
        fs::write(dir.join("a (copy).png"), b"x").unwrap();
        assert_eq!(paste_destination(&dir, OsStr::new("a.png")), dir.join("a (copy 2).png"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extensionless_and_dotfile_names_keep_their_shape() {
        let dir = temp_dir("shape");
        fs::write(dir.join("Notes"), b"x").unwrap();
        fs::write(dir.join(".env"), b"x").unwrap();
        assert_eq!(paste_destination(&dir, OsStr::new("Notes")), dir.join("Notes (copy)"));
        assert_eq!(paste_destination(&dir, OsStr::new(".env")), dir.join(".env (copy)"));
        assert_eq!(split_name("archive.tar.gz"), ("archive.tar", ".gz"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_keeps_source_and_move_removes_it() {
        let root = temp_dir("ops");
        let src = root.join("src");
        let dst = root.join("dst");
        fs::create_dir_all(src.join("tree/inner")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("tree/inner/f.txt"), b"hello").unwrap();
        fs::write(src.join("solo.txt"), b"one").unwrap();

        let copied = paste_into(&dst, &[src.join("solo.txt"), src.join("tree")], false);
        assert!(copied.iter().all(Result::is_ok), "{copied:?}");
        assert!(src.join("solo.txt").exists());
        assert_eq!(fs::read(dst.join("tree/inner/f.txt")).unwrap(), b"hello");

        let moved = paste_into(&dst, &[src.join("solo.txt")], true);
        // `solo.txt` already sits in dst from the copy above, so the move lands on the suffixed name.
        assert_eq!(moved[0].as_ref().unwrap(), &dst.join("solo (copy).txt"));
        assert!(!src.join("solo.txt").exists());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn folder_cannot_be_pasted_into_itself() {
        let root = temp_dir("self");
        let folder = root.join("folder");
        fs::create_dir_all(folder.join("child")).unwrap();
        let outcome = paste_into(&folder.join("child"), &[folder.clone()], false);
        assert!(outcome[0].is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
