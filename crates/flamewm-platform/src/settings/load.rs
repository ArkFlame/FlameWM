//! Safe startup load for the settings file.
//!
//! File IO only: parsing, validation and key tables stay with `ConfigStore`/`ProductSettings`.
//! Every read failure degrades to defaults; the original bytes are preserved under a
//! `settings.corrupt-<epoch>` name so a bad file is never silently destroyed. The caller receives
//! at most one notice string, already emitted once through the existing `flamewm:` error line.

#[cfg(test)]
use std::io;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use super::{ConfigStore, ProductSettings};

// Bounded read, quarantine and failure classification are shared with the explorer, terminal
// and workspace-count loaders; see `flamewm_api::safe_config`.
#[cfg(test)]
pub(crate) use flamewm_api::safe_config::{MAX_CONFIG_BYTES as MAX_SETTINGS_BYTES, corrupt_name};
pub(crate) use flamewm_api::safe_config::{
    ReadFailure, decode_read, failure_reason, quarantine, read_bounded,
};

/// Result of a startup load: usable settings plus at most one user-visible notice.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadOutcome {
    pub(crate) settings: ProductSettings,
    pub(crate) notice: Option<String>,
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

/// Emit exactly one error-line notice and return it for callers/tests that need the text.
fn report(notice: &str) -> String {
    eprintln!("{notice}");
    notice.to_owned()
}

/// Load settings from `path`, degrading to defaults on every failure.
///
/// A missing file is a normal fresh install and is silent. An unreadable, oversized,
/// non-UTF-8 or unparsable
/// file is quarantined to `settings.corrupt-<epoch>` and reported once; defaults are used instead.
/// This function never returns an `Err`, so a bad file or a read-only home cannot abort startup.
pub(crate) fn load_settings(path: &Path, epoch_secs: u64) -> LoadOutcome {
    let defaults = ProductSettings::default();
    match decode_read(read_bounded(path)) {
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
    fn decode_read_rejects_oversized_input() {
        let huge = vec![b'#'; MAX_SETTINGS_BYTES as usize + 1];
        assert_eq!(decode_read(Ok(huge)), Err(ReadFailure::TooLarge));
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
        assert_eq!(failure_reason(ReadFailure::TooLarge), "too large");
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
