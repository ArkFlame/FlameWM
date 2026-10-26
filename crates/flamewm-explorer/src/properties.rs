//! Metadata and permission logic behind the Properties dialog.
//! Recursive size runs on a worker thread and honours a cancel flag.
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Properties {
    pub path: PathBuf,
    pub size: u64,
    pub uid: u32,
    pub gid: u32,
    /// Permission bits only (`0o7777` mask).
    pub mode: u32,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Symlink target, not followed. `None` for non-links.
    pub link_target: Option<PathBuf>,
}

/// Reads metadata without following symlinks, so a link describes itself.
pub fn read(path: &Path) -> io::Result<Properties> {
    let meta = fs::symlink_metadata(path)?;
    let link_target = if meta.file_type().is_symlink() {
        fs::read_link(path).ok()
    } else {
        None
    };
    Ok(Properties {
        path: path.to_path_buf(),
        size: meta.len(),
        uid: meta.uid(),
        gid: meta.gid(),
        mode: meta.mode() & 0o7777,
        modified: meta.modified().ok(),
        accessed: meta.accessed().ok(),
        link_target,
    })
}

/// Total bytes under `path` without following symlinks. Unreadable children are skipped.
/// Returns `ErrorKind::Interrupted` when `cancel` is set.
pub fn recursive_size(path: &Path, cancel: &AtomicBool) -> io::Result<u64> {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let meta = match fs::symlink_metadata(&dir) {
            Ok(m) => m,
            Err(_) => continue,
        };
        total += meta.len();
        if !meta.is_dir() {
            continue;
        }
        let Ok(children) = fs::read_dir(&dir) else {
            continue;
        };
        pending.extend(children.flatten().map(|c| c.path()));
    }
    Ok(total)
}

/// `rwxr-xr-x` style string for the nine permission bits.
pub fn mode_string(mode: u32) -> String {
    (0..9)
        .rev()
        .map(|bit| {
            let flag = match bit % 3 {
                2 => 'r',
                1 => 'w',
                _ => 'x',
            };
            if mode >> bit & 1 == 1 { flag } else { '-' }
        })
        .collect()
}

/// Parses an octal chmod value such as `644` or `4755`.
pub fn parse_mode(text: &str) -> Result<u32, String> {
    let text = text.trim();
    if !(3..=4).contains(&text.len()) || !text.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return Err("Enter 3 or 4 octal digits, such as 644 or 0755".into());
    }
    u32::from_str_radix(text, 8).map_err(|e| e.to_string())
}

/// Applies permission bits. Symlinks are refused: `chmod` would change the target instead.
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot change permissions of a symlink",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_string_matches_ls_layout() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o640), "rw-r-----");
        assert_eq!(mode_string(0), "---------");
    }

    #[test]
    fn parse_mode_accepts_only_octal_three_or_four_digits() {
        assert_eq!(parse_mode("644"), Ok(0o644));
        assert_eq!(parse_mode(" 4755 "), Ok(0o4755));
        assert!(parse_mode("788").is_err());
        assert!(parse_mode("+755").is_err());
        assert!(parse_mode("75").is_err());
        assert!(parse_mode("77777").is_err());
        assert!(parse_mode("").is_err());
    }

    #[test]
    fn recursive_size_sums_files_and_honours_cancel() {
        let dir = std::env::temp_dir().join(format!("flame-props-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a"), vec![0u8; 100]).unwrap();
        fs::write(dir.join("sub/b"), vec![0u8; 250]).unwrap();

        let idle = AtomicBool::new(false);
        let size = recursive_size(&dir, &idle).unwrap();
        // Directory entries also contribute their own metadata length on Linux, so only lower-bound the sum.
        assert!(size >= 350, "size was {size}");

        let cancelled = AtomicBool::new(true);
        let err = recursive_size(&dir, &cancelled).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_mode_round_trips_and_reads_back() {
        let file = std::env::temp_dir().join(format!("flame-mode-{}", std::process::id()));
        fs::write(&file, b"x").unwrap();
        set_mode(&file, 0o600).unwrap();
        let props = read(&file).unwrap();
        assert_eq!(props.mode, 0o600);
        assert_eq!(props.link_target, None);
        fs::remove_file(&file).unwrap();
    }
}
