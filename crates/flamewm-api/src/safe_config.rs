//! Bounded reads and quarantine for small `FlameWM` config files.
//!
//! An unreadable, oversized, non-UTF-8 or unparsable file is renamed aside to
//! `<stem>.corrupt-<epoch>[.<n>].<ext>` and the caller runs on defaults with one stderr notice.
//! Bad bytes are never overwritten: a save quarantines an invalid file before it writes.
//! Only the newest [`KEEP_QUARANTINED`] copies per file are kept.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound for a config snapshot. A larger file is a fault, never read into memory whole.
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// Quarantine copies kept per config file; older copies are removed after a new quarantine.
pub const KEEP_QUARANTINED: usize = 4;

/// Why a config file could not be used as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailure {
    NotFound,
    Unreadable,
    NotUtf8,
    TooLarge,
}

/// Human reason text for a read failure, used in the notice line.
#[must_use]
pub fn failure_reason(failure: ReadFailure) -> &'static str {
    match failure {
        ReadFailure::NotFound => "missing",
        ReadFailure::Unreadable => "unreadable",
        ReadFailure::NotUtf8 => "not UTF-8",
        ReadFailure::TooLarge => "too large",
    }
}

/// Read at most one byte past the limit so an oversized file is detected without loading it whole.
pub fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Decode bytes that were already read. No IO side effects, so the mapping is unit-testable.
pub fn decode_read(bytes: io::Result<Vec<u8>>) -> Result<String, ReadFailure> {
    match bytes {
        Ok(bytes) if bytes.len() as u64 > MAX_CONFIG_BYTES => Err(ReadFailure::TooLarge),
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| ReadFailure::NotUtf8),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(ReadFailure::NotFound),
        Err(_) => Err(ReadFailure::Unreadable),
    }
}

/// Read a config file as bounded UTF-8 text.
pub fn read_text(path: &Path) -> Result<String, ReadFailure> {
    decode_read(read_bounded(path))
}

/// Current Unix time in whole seconds; `0` if the clock is before the epoch.
#[must_use]
pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Candidate quarantine path for a corrupt file, in the same directory as the original.
///
/// `attempt` disambiguates same-second collisions without overwriting an earlier quarantine.
/// Pure: no filesystem access.
pub fn corrupt_name(path: &Path, epoch_secs: u64, attempt: u16) -> Option<PathBuf> {
    let stem = path.file_stem()?;
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let mut name = stem.to_os_string();
    name.push(format!(".corrupt-{epoch_secs}"));
    if attempt > 0 {
        name.push(format!(".{attempt}"));
    }
    if let Some(extension) = path.extension() {
        name.push(".");
        name.push(extension);
    }
    Some(parent.join(name))
}

/// Move the unusable file aside without ever overwriting an earlier quarantine.
pub fn quarantine(path: &Path, epoch_secs: u64) -> io::Result<PathBuf> {
    for attempt in 0..=u16::MAX {
        let candidate = corrupt_name(path, epoch_secs, attempt).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "config quarantine path must name a file",
            )
        })?;
        if fs::symlink_metadata(&candidate).is_ok() {
            continue;
        }
        match fs::rename(path, &candidate) {
            Ok(()) => {
                prune_quarantine(path);
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free config quarantine name",
    ))
}

/// Keep the newest [`KEEP_QUARANTINED`] `<stem>.corrupt-*` siblings by (epoch, attempt). Deletion
/// failures are ignored: retention must never fail a load or a save.
fn prune_quarantine(path: &Path) {
    let Some(stem) = path.file_stem() else {
        return;
    };
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let prefix = format!("{}.corrupt-", stem.to_string_lossy());
    let suffix = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()));
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut copies: Vec<(u64, u16, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix(&prefix)?;
            let rest = match &suffix {
                Some(suffix) => rest.strip_suffix(suffix.as_str())?,
                None => rest,
            };
            let mut parts = rest.splitn(2, '.');
            let epoch = parts.next()?.parse::<u64>().ok()?;
            let attempt = match parts.next() {
                Some(attempt) => attempt.parse::<u16>().ok()?,
                None => 0,
            };
            Some((epoch, attempt, entry.path()))
        })
        .collect();
    copies.sort_unstable_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for (_, _, old) in copies.into_iter().skip(KEEP_QUARANTINED) {
        let _ = fs::remove_file(old);
    }
}

/// Quarantine an unusable file and print the one stderr notice. Returns the notice text.
pub fn quarantine_and_warn(path: &Path, cause: &str, blast_radius: &str) -> String {
    let preserved = match quarantine(path, now_epoch_secs()) {
        Ok(backup) => format!("preserved at {}", backup.display()),
        Err(_) => "preservation failed".to_owned(),
    };
    let notice = format!(
        "flamewm: {} {cause}; {preserved}; using defaults; blast radius: {blast_radius}",
        path.display()
    );
    eprintln!("{notice}");
    notice
}

/// Load a config file, degrading to `T::default()` on every failure.
///
/// A missing file is silent. Any other read failure, or a `parse` error, quarantines the file
/// and prints one notice naming `blast_radius`.
pub fn load_or_default<T: Default, E>(
    path: &Path,
    parse: impl FnOnce(&str) -> Result<T, E>,
    blast_radius: &str,
) -> T {
    match read_text(path) {
        Ok(text) => match parse(&text) {
            Ok(value) => value,
            Err(_) => {
                quarantine_and_warn(path, "unparsable", blast_radius);
                T::default()
            }
        },
        Err(ReadFailure::NotFound) => T::default(),
        Err(failure) => {
            quarantine_and_warn(path, failure_reason(failure), blast_radius);
            T::default()
        }
    }
}

/// Call before writing a config file. A missing or valid file passes. An unreadable or invalid
/// file is quarantined first, so the save never overwrites the bad bytes. If quarantine fails the
/// error is returned and the caller must not write.
pub fn quarantine_before_save(path: &Path, is_valid: impl FnOnce(&str) -> bool) -> io::Result<()> {
    match read_text(path) {
        Err(ReadFailure::NotFound) => Ok(()),
        Ok(text) if is_valid(&text) => Ok(()),
        _ => quarantine(path, now_epoch_secs()).map(|_| ()),
    }
}

/// Collision counter for unique sibling temps owned by this process.
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// Write `text` to `path` atomically: a unique sibling temp is created with mode `0600`, fully
/// written and `sync_all`ed, then renamed over `path`; the parent directory is `sync_all`ed on a
/// best-effort basis so the rename entry is durable.
///
/// `create_new` means a competing writer's temp is never reused and readers only ever observe a
/// whole file. Directory fsync is deliberately best effort: some filesystems reject it, and a
/// rejection must not fail an otherwise committed write. The write-temp-rename and fsync policy is
/// ported from `.vendor/kensaku-master/src-tauri/src/config.rs:500-526`.
pub fn atomic_write_text(path: &Path, text: &str) -> io::Result<()> {
    let Some(file_name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic write path must name a file",
        ));
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    for _ in 0..16 {
        let mut temp_name = file_name.to_os_string();
        temp_name.push(format!(
            ".{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let temp = parent.join(temp_name);
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a unique atomic-write temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "flamewm-safe-config-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn quarantine_keeps_only_newest_copies() {
        let dir = temp_dir("retention");
        let path = dir.join("widget.conf");
        for step in 0..6u64 {
            fs::write(&path, b"\xff\xfe junk").expect("write bad file");
            quarantine(&path, 100 + step).expect("quarantine");
        }
        let mut kept: Vec<String> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("widget.corrupt-"))
            .collect();
        kept.sort();
        assert_eq!(KEEP_QUARANTINED, 4);
        assert_eq!(kept.len(), KEEP_QUARANTINED);
        assert!(kept.contains(&"widget.corrupt-105.conf".to_owned()));
        assert!(kept.contains(&"widget.corrupt-102.conf".to_owned()));
        assert!(!kept.contains(&"widget.corrupt-101.conf".to_owned()));
        assert!(!kept.contains(&"widget.corrupt-100.conf".to_owned()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn atomic_write_replaces_whole_file_with_0600_and_no_temp_left() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("atomic-write");
        let path = dir.join("settings.conf");
        fs::write(&path, b"old contents that must disappear").expect("old file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("old mode");

        atomic_write_text(&path, "new contents").expect("atomic write");

        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            "new contents"
        );
        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the private temp must become the file");
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no temp may survive: {leftovers:?}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn atomic_write_rejects_path_without_a_file_name() {
        let error = atomic_write_text(Path::new("/"), "x").expect_err("no file name");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
