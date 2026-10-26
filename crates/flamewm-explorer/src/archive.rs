//! Archive actions delegated to the `ark` CLI. Actions are offered only when `ark` is executable on PATH.
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Finds an executable named `ark` in a PATH-style list. Relative PATH entries are ignored.
#[must_use]
pub fn find_ark_in(path_var: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("ark"))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

#[must_use]
pub fn find_ark() -> Option<PathBuf> {
    find_ark_in(&std::env::var_os("PATH").unwrap_or_default())
}

/// Arguments for "Extract Here": extract into the archive's directory, in a subfolder when needed.
#[must_use]
pub fn extract_here_args(archive: &Path) -> Vec<OsString> {
    let parent = archive
        .parent()
        .unwrap_or(Path::new("/"))
        .as_os_str()
        .to_owned();
    vec![
        "--batch".into(),
        "--autosubfolder".into(),
        "--destination".into(),
        parent,
        archive.as_os_str().to_owned(),
    ]
}

/// Arguments for compressing absolute `items` into `archive` (created when missing).
/// Refuses empty input and relative paths so that option-like names cannot be injected.
pub fn compress_args(archive: &Path, items: &[PathBuf]) -> Result<Vec<OsString>, String> {
    if items.is_empty() {
        return Err("Nothing selected to compress".into());
    }
    if items.iter().any(|item| !item.is_absolute()) || !archive.is_absolute() {
        return Err("Compress needs absolute paths".into());
    }
    let mut args: Vec<OsString> = vec![
        "--batch".into(),
        "--add-to".into(),
        archive.as_os_str().to_owned(),
    ];
    args.extend(items.iter().map(|item| item.as_os_str().to_owned()));
    Ok(args)
}

/// Picks a non-existing archive name in `dir`: `<base>.zip`, then `<base> (2).zip`, and so on.
#[must_use]
pub fn unique_archive_name(dir: &Path, base: &str, extension: &str) -> PathBuf {
    let first = dir.join(format!("{base}.{extension}"));
    if first.symlink_metadata().is_err() {
        return first;
    }
    (2_u32..=u32::MAX)
        .map(|n| dir.join(format!("{base} ({n}).{extension}")))
        .find(|candidate| candidate.symlink_metadata().is_err())
        .unwrap_or(first)
}

/// Default archive base name: a single item keeps its name; several items become "Archive".
#[must_use]
pub fn default_archive_base(items: &[PathBuf]) -> String {
    match items {
        [only] => only
            .file_name()
            .unwrap_or(OsStr::new("Archive"))
            .to_string_lossy()
            .into_owned(),
        _ => "Archive".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ark_is_found_only_as_executable_absolute_path_entry() {
        let dir = std::env::temp_dir().join(format!("flamewm-ark-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let ark = bin.join("ark");
        std::fs::write(&ark, b"#!/bin/sh\n").unwrap();
        let path_var = std::env::join_paths([PathBuf::from("relative"), bin.clone()]).unwrap();
        assert_eq!(
            find_ark_in(&path_var),
            None,
            "non-executable file must not count"
        );
        std::fs::set_permissions(&ark, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_ark_in(&path_var), Some(ark.clone()));
        assert_eq!(find_ark_in(OsStr::new("")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_and_compress_arguments_are_structured() {
        let archive = Path::new("/home/u/Docs/bundle.zip");
        let args = extract_here_args(archive);
        assert_eq!(args.last().unwrap(), archive.as_os_str());
        assert!(args.contains(&OsString::from("/home/u/Docs")));
        assert!(compress_args(Path::new("/x/out.zip"), &[]).is_err());
        assert!(compress_args(Path::new("/x/out.zip"), &[PathBuf::from("-rf")]).is_err());
        let args = compress_args(
            Path::new("/x/out.zip"),
            &[PathBuf::from("/x/a"), PathBuf::from("/x/b")],
        )
        .unwrap();
        assert_eq!(args.len(), 5);
        assert_eq!(args[1], OsString::from("--add-to"));
    }

    #[test]
    fn archive_names_do_not_overwrite_existing_entries() {
        let dir =
            std::env::temp_dir().join(format!("flamewm-archive-name-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            unique_archive_name(&dir, "Archive", "zip"),
            dir.join("Archive.zip")
        );
        std::fs::write(dir.join("Archive.zip"), b"").unwrap();
        assert_eq!(
            unique_archive_name(&dir, "Archive", "zip"),
            dir.join("Archive (2).zip")
        );
        assert_eq!(default_archive_base(&[PathBuf::from("/x/photo")]), "photo");
        assert_eq!(
            default_archive_base(&[PathBuf::from("/x/a"), PathBuf::from("/x/b")]),
            "Archive"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
