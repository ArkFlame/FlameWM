//! Safe startup load for the settings file.
//!
//! File IO only: parsing, validation and key tables stay with `ConfigStore`/`ProductSettings`.
//! Every read failure degrades to defaults; the original bytes are preserved under a
//! `settings.corrupt-<epoch>` name so a bad file is never silently destroyed. The caller receives
//! at most one notice string, already emitted once through the existing `flamewm:` error line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{ConfigStore, ProductSettings};

/// Result of a startup load: usable settings plus at most one user-visible notice.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadOutcome {
    pub(crate) settings: ProductSettings,
    pub(crate) notice: Option<String>,
}

/// Pure classification of a read that has already been attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadFailure {
    NotFound,
    Unreadable,
    NotUtf8,
}

/// Decode bytes that were already read. No IO side effects, so the mapping is unit-testable.
pub(crate) fn decode_read(bytes: io::Result<Vec<u8>>) -> Result<String, ReadFailure> {
    match bytes {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| ReadFailure::NotUtf8),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(ReadFailure::NotFound),
        Err(_) => Err(ReadFailure::Unreadable),
    }
}

/// Pure reason text for a read failure, used in the single notification line.
pub(crate) fn failure_reason(failure: ReadFailure) -> &'static str {
    match failure {
        ReadFailure::NotFound => "missing",
        ReadFailure::Unreadable => "unreadable",
        ReadFailure::NotUtf8 => "not UTF-8",
    }
}

/// Candidate quarantine path for a corrupt settings file.
///
/// `<stem>.corrupt-<epoch>[.<n>][.<ext>]` next to the original, e.g.
/// `settings.corrupt-1764200000`. `attempt` disambiguates same-second collisions without
/// overwriting an earlier quarantine. Pure: no filesystem access.
pub(crate) fn corrupt_name(path: &Path, epoch_secs: u64, attempt: u16) -> Option<PathBuf> {
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

/// Build the one user-visible notification. Pure: no printing, no IO.
pub(crate) fn notice_text(path: &Path, backup: Option<&Path>, reason: &str) -> String {
    match backup {
        Some(backup) => format!(
            "flamewm: settings {} {}; preserved at {}; using defaults",
            path.display(),
            reason,
            backup.display()
        ),
        None => format!(
            "flamewm: settings {} {}; using defaults (preservation failed)",
            path.display(),
            reason
        ),
    }
}

/// Move the unusable file aside without ever overwriting an earlier quarantine.
fn quarantine(path: &Path, epoch_secs: u64) -> io::Result<PathBuf> {
    for attempt in 0..=u16::MAX {
        let candidate = corrupt_name(path, epoch_secs, attempt).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "settings quarantine path must name a file",
            )
        })?;
        if fs::symlink_metadata(&candidate).is_ok() {
            continue;
        }
        match fs::rename(path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free corrupt settings quarantine name",
    ))
}

/// Emit exactly one error-line notice and return it for callers/tests that need the text.
fn report(notice: &str) -> String {
    eprintln!("{notice}");
    notice.to_owned()
}

/// Load settings from `path`, degrading to defaults on every failure.
///
/// A missing file is a normal fresh install and is silent. An unreadable, non-UTF-8 or unparsable
/// file is quarantined to `settings.corrupt-<epoch>` and reported once; defaults are used instead.
/// This function never returns an `Err`, so a bad file or a read-only home cannot abort startup.
pub(crate) fn load_settings(path: &Path, epoch_secs: u64) -> LoadOutcome {
    let defaults = ProductSettings::default();
    match decode_read(fs::read(path)) {
        Ok(text) => match ConfigStore::default().parse_snapshot(&text) {
            Ok(parsed) => {
                let mut settings = parsed;
                if settings.revision <= defaults.revision {
                    settings.revision = defaults.revision.saturating_add(1);
                }
                LoadOutcome {
                    settings,
                    notice: None,
                }
            }
            Err(error) => {
                let backup = quarantine(path, epoch_secs).ok();
                let notice = notice_text(
                    path,
                    backup.as_deref(),
                    &format!("unparsable ({})", error.message),
                );
                LoadOutcome {
                    settings: defaults,
                    notice: Some(report(&notice)),
                }
            }
        },
        Err(ReadFailure::NotFound) => LoadOutcome {
            settings: defaults,
            notice: None,
        },
        Err(failure) => {
            let backup = quarantine(path, epoch_secs).ok();
            let notice = notice_text(path, backup.as_deref(), failure_reason(failure));
            LoadOutcome {
                settings: defaults,
                notice: Some(report(&notice)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn decode_read_accepts_utf8() {
        assert_eq!(
            decode_read(Ok(b"revision=3\n".to_vec())).as_deref(),
            Ok("revision=3\n")
        );
    }

    #[test]
    fn decode_read_rejects_non_utf8() {
        assert_eq!(decode_read(Ok(vec![0xff, 0xfe])), Err(ReadFailure::NotUtf8));
    }

    #[test]
    fn decode_read_distinguishes_missing_from_unreadable() {
        let missing = io::Error::new(ErrorKind::NotFound, "gone");
        let denied = io::Error::new(ErrorKind::PermissionDenied, "denied");
        assert_eq!(decode_read(Err(missing)), Err(ReadFailure::NotFound));
        assert_eq!(decode_read(Err(denied)), Err(ReadFailure::Unreadable));
    }

    #[test]
    fn failure_reason_covers_every_read_failure() {
        assert_eq!(failure_reason(ReadFailure::NotFound), "missing");
        assert_eq!(failure_reason(ReadFailure::Unreadable), "unreadable");
        assert_eq!(failure_reason(ReadFailure::NotUtf8), "not UTF-8");
    }

    #[test]
    fn corrupt_name_uses_timestamp_and_preserves_extension() {
        let path = Path::new("/home/user/.config/flamewm/settings.toml");
        assert_eq!(
            corrupt_name(path, 1764200000, 0),
            Some(PathBuf::from(
                "/home/user/.config/flamewm/settings.corrupt-1764200000.toml"
            ))
        );
    }

    #[test]
    fn corrupt_name_without_extension_matches_documented_shape() {
        assert_eq!(
            corrupt_name(Path::new("settings"), 42, 0),
            Some(PathBuf::from("settings.corrupt-42"))
        );
    }

    #[test]
    fn corrupt_name_disambiguates_same_second_collisions() {
        let path = Path::new("settings");
        assert_eq!(
            corrupt_name(path, 7, 1),
            Some(PathBuf::from("settings.corrupt-7.1"))
        );
        assert_eq!(
            corrupt_name(path, 7, 2),
            Some(PathBuf::from("settings.corrupt-7.2"))
        );
    }

    #[test]
    fn corrupt_name_stays_in_the_same_directory() {
        let path = Path::new("settings");
        let backup = corrupt_name(path, 9, 0).expect("path names a file");
        assert_eq!(backup.parent(), path.parent());
    }

    #[test]
    fn corrupt_name_rejects_path_without_file_name() {
        assert_eq!(corrupt_name(Path::new(""), 1, 0), None);
        assert_eq!(corrupt_name(Path::new("/"), 1, 0), None);
    }

    #[test]
    fn notice_text_is_one_line_and_names_the_backup() {
        let notice = notice_text(
            Path::new("settings"),
            Some(Path::new("settings.corrupt-1")),
            "not UTF-8",
        );
        assert!(!notice.contains('\n'));
        assert!(notice.starts_with("flamewm: settings "));
        assert!(notice.contains("settings.corrupt-1"));
        assert!(notice.contains("not UTF-8"));
        assert!(notice.contains("using defaults"));
    }

    #[test]
    fn notice_text_reports_failed_preservation_once() {
        let notice = notice_text(Path::new("settings"), None, "unreadable");
        assert!(!notice.contains('\n'));
        assert!(notice.contains("preservation failed"));
        assert_eq!(notice.matches("flamewm:").count(), 1);
    }
}
