//! Safe persistence for the settings file.
//!
//! File IO only: the key tables and validation rules stay with `ProductSettings`. Every write goes
//! through the existing atomic writer (unique sibling temp, fsync, rename, fsync parent), so a
//! crash or a competing writer can never leave a partial settings file. A failed write never
//! propagates: the in-memory settings remain authoritative and the user gets one error-line notice.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{ConfigStore, ProductSettings};

/// Outcome of a persistence attempt. `Degraded` carries the single notice already emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistOutcome {
    Durable,
    Degraded(String),
}

/// Pure check that `path` can be handed to the atomic writer.
pub(crate) fn is_writable_target(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.file_name().is_some()
}

/// Pure sibling temp name used for a write attempt, mirroring the atomic writer's scheme.
pub(crate) fn temp_name(path: &Path, pid: u32, sequence: u64) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut name = file_name.to_os_string();
    name.push(format!(".{pid}.{sequence}.tmp"));
    Some(parent.join(name))
}

/// True when `name` is a temp file left by a writer with this pid. Pure: no filesystem access.
pub(crate) fn is_own_temp(name: &std::ffi::OsStr, pid: u32) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let suffix = format!(".{pid}.");
    name.rsplit_once(".tmp")
        .is_some_and(|(stem, _)| stem.contains(&suffix))
}

/// Build the single-line notice for a failed write. Pure: no printing, no IO.
pub(crate) fn degraded_notice(path: &Path, reason: &str) -> String {
    format!(
        "flamewm: could not persist settings {} ({}); keeping in-memory settings",
        path.display(),
        reason
    )
}

/// Emit exactly one error-line notice and return it for callers/tests that need the text.
fn report(notice: String) -> PersistOutcome {
    eprintln!("{notice}");
    PersistOutcome::Degraded(notice)
}

/// Persist `snapshot` atomically, degrading to in-memory settings on any failure.
///
/// Validation, an empty path and an unwritable config directory (including a read-only home) all
/// return [`PersistOutcome::Degraded`] after one notice; nothing is returned as an error that could
/// abort a settings transaction or shutdown. Concurrent writers are serialized by the atomic
/// rename: the last completed write wins and readers only ever see a whole file.
pub(crate) fn persist_settings(path: &Path, snapshot: &ProductSettings) -> PersistOutcome {
    if let Err(error) = snapshot.validate() {
        return report(degraded_notice(path, &error.message));
    }
    if !is_writable_target(path) {
        return report(degraded_notice(path, "no file path"));
    }
    match crate::storage::write_atomic_text(path, &ConfigStore::serialize(snapshot)) {
        Ok(()) => PersistOutcome::Durable,
        Err(error) => report(degraded_notice(path, &error.to_string())),
    }
}

/// Best-effort removal of temp files this process left behind after an interrupted write.
///
/// Called only at startup, before the settings writer thread starts, so it cannot race our own
/// in-flight temp. Other processes keep their own pid and are never touched.
pub(crate) fn cleanup_own_temps(path: &Path, pid: u32) {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if is_own_temp(&entry.file_name(), pid) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_target_requires_non_empty_path_with_file_name() {
        assert!(!is_writable_target(Path::new("")));
        assert!(!is_writable_target(Path::new("/")));
        assert!(is_writable_target(Path::new("settings")));
        assert!(is_writable_target(Path::new("/tmp/flamewm/settings")));
    }

    #[test]
    fn temp_name_is_a_sibling_with_pid_and_sequence() {
        let path = Path::new("/home/user/.config/flamewm/settings");
        assert_eq!(
            temp_name(path, 4242, 7),
            Some(PathBuf::from(
                "/home/user/.config/flamewm/settings.4242.7.tmp"
            ))
        );
    }

    #[test]
    fn temp_name_rejects_path_without_file_name() {
        assert_eq!(temp_name(Path::new(""), 1, 0), None);
        assert_eq!(temp_name(Path::new("/"), 1, 0), None);
    }

    #[test]
    fn own_temp_matches_only_this_pid() {
        assert!(is_own_temp(
            std::ffi::OsStr::new("settings.4242.7.tmp"),
            4242
        ));
        assert!(!is_own_temp(
            std::ffi::OsStr::new("settings.4243.7.tmp"),
            4242
        ));
        assert!(!is_own_temp(
            std::ffi::OsStr::new("settings.corrupt-1"),
            4242
        ));
    }

    #[test]
    fn degraded_notice_is_one_line_and_keeps_memory_authoritative() {
        let notice = degraded_notice(Path::new("settings"), "permission denied");
        assert!(!notice.contains('\n'));
        assert!(notice.starts_with("flamewm: could not persist settings "));
        assert!(notice.contains("permission denied"));
        assert!(notice.contains("keeping in-memory settings"));
    }

    #[test]
    fn persist_outcome_distinguishes_durable_from_degraded() {
        let degraded = PersistOutcome::Degraded("notice".to_owned());
        assert_ne!(PersistOutcome::Durable, degraded);
        assert_eq!(degraded, PersistOutcome::Degraded("notice".to_owned()));
    }
}
