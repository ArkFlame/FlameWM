//! Fault-injection coverage for the settings load quarantine path.
//!
//! Each case uses a private scratch directory under the system temp dir, so the real settings
//! file is never touched. All four tests drive the real `load_settings` and `persist_settings`
//! entry points with their actual signatures.

use std::fs;
use std::path::PathBuf;

use super::load::corrupt_name;
use super::{load_settings, persist_settings, LoadOutcome, PersistOutcome, ProductSettings};

/// Fixed epoch keeps the quarantine name deterministic without reading the clock.
const EPOCH: u64 = 1_764_200_000;

/// Scratch directory under the system temp dir, removed even when a test panics.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "flamewm-load-tests-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn entries(&self) -> Vec<PathBuf> {
        fs::read_dir(&self.0)
            .expect("scratch dir still exists")
            .map(|entry| entry.expect("dir entry").path())
            .collect()
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Case 1: a file the line-based snapshot parser cannot read is moved aside, not destroyed.
#[test]
fn corrupt_json_is_quarantined_once_and_defaults_are_used() {
    let scratch = ScratchDir::new("corrupt-json");
    let path = scratch.file("settings.conf");
    let bad = b"{\n  \"revision\": 9,\n  \"accent\": broken\n}\n";
    fs::write(&path, bad).expect("corrupt fixture");

    let outcome: LoadOutcome = load_settings(&path, EPOCH);

    assert_eq!(outcome.settings, ProductSettings::default());
    let notice = outcome.notice.expect("one notice for a corrupt file");
    assert_eq!(notice.matches("flamewm:").count(), 1, "{notice}");
    assert!(!notice.contains('\n'), "one error line: {notice}");
    assert!(notice.contains("unparsable"), "{notice}");
    assert!(notice.contains("using defaults"), "{notice}");
    assert!(!path.exists(), "corrupt file was moved aside");

    let backup = corrupt_name(&path, EPOCH, 0).expect("quarantine name");
    let backup_text = backup.display().to_string();
    assert_eq!(backup.parent(), path.parent());
    assert!(notice.contains(&backup_text), "{notice}");
    assert_eq!(fs::read(&backup).expect("quarantined bytes"), bad.to_vec());
    assert_eq!(scratch.entries(), vec![backup], "exactly one quarantine file");

    // The original path is gone, so a repeat load is silent: one notice in total.
    let repeat = load_settings(&path, EPOCH);
    assert_eq!(repeat.settings, ProductSettings::default());
    assert!(repeat.notice.is_none(), "no second notice: {:?}", repeat.notice);
}

/// Case 2: absence is a normal fresh install, not a fault.
#[test]
fn missing_file_loads_defaults_without_quarantine() {
    let scratch = ScratchDir::new("missing");
    let path = scratch.file("settings.conf");

    let outcome = load_settings(&path, EPOCH);

    assert_eq!(outcome.settings, ProductSettings::default());
    assert!(outcome.notice.is_none(), "{:?}", outcome.notice);
    assert!(!path.exists());
    assert!(scratch.entries().is_empty(), "nothing quarantined");
}

/// Case 3: keys added after a file was written are filled from the current defaults.
#[test]
fn old_file_without_new_keys_fills_defaults() {
    let scratch = ScratchDir::new("old-keys");
    let path = scratch.file("settings.conf");
    fs::write(&path, "revision=4\naccent=#112233\n").expect("legacy fixture");

    let outcome = load_settings(&path, EPOCH);

    assert!(outcome.notice.is_none(), "{:?}", outcome.notice);
    let settings = outcome.settings;
    let defaults = ProductSettings::default();
    assert_eq!(settings.revision, 4);
    assert_eq!(settings.accent, "#112233");
    assert_eq!(settings.font_family, defaults.font_family);
    assert_eq!(settings.taskbar_height, defaults.taskbar_height);
    assert_eq!(settings.accent_opacity, defaults.accent_opacity);
    assert_eq!(settings.animation_speed, defaults.animation_speed);
    assert!(path.exists(), "a valid legacy file is not quarantined");
    assert_eq!(scratch.entries(), vec![path], "no quarantine file");
}

/// Case 4: a real atomic write reloads to the same typed snapshot.
#[test]
fn persisted_settings_round_trip_through_load() {
    let scratch = ScratchDir::new("round-trip");
    let path = scratch.file("settings.conf");
    let mut snapshot = ProductSettings {
        revision: 9,
        accent: "#112233".to_owned(),
        accent_custom: true,
        taskbar_height: 56,
        ..ProductSettings::default()
    };
    snapshot
        .unknown_keys
        .insert("extension.key".to_owned(), "value".to_owned());

    assert_eq!(persist_settings(&path, &snapshot), PersistOutcome::Durable);

    let outcome = load_settings(&path, EPOCH);
    assert!(outcome.notice.is_none(), "{:?}", outcome.notice);
    assert_eq!(outcome.settings, snapshot);
}
