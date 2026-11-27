//! Fault-injection coverage for the settings load quarantine path.
//!
//! Each case uses a private scratch directory under the system temp dir, so the real settings
//! file is never touched. Every test drives the real `load_settings` and `persist_settings`
//! entry points with their actual signatures.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::load::{MAX_SETTINGS_BYTES, corrupt_name};
use super::{LoadOutcome, PersistOutcome, ProductSettings, load_settings, persist_settings};

/// Fixed epoch keeps the quarantine name deterministic without reading the clock.
const EPOCH: u64 = 1_764_200_000;

/// Scratch directory under the system temp dir, removed even when a test panics.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("flamewm-load-tests-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn set_mode(&self, mode: u32) {
        fs::set_permissions(&self.0, fs::Permissions::from_mode(mode)).expect("scratch mode");
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
    assert_eq!(
        scratch.entries(),
        vec![backup],
        "exactly one quarantine file"
    );

    // The original path is gone, so a repeat load is silent: one notice in total.
    let repeat = load_settings(&path, EPOCH);
    assert_eq!(repeat.settings, ProductSettings::default());
    assert!(
        repeat.notice.is_none(),
        "no second notice: {:?}",
        repeat.notice
    );
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

/// Case 5: a file cut off mid-line cannot parse and is quarantined, not partially applied.
#[test]
fn truncated_file_is_quarantined_and_defaults_are_used() {
    let scratch = ScratchDir::new("truncated");
    let path = scratch.file("settings.conf");
    let truncated = b"revision=9\naccent=#112233\nwindowOpenAnim";
    fs::write(&path, truncated).expect("truncated fixture");

    let outcome = load_settings(&path, EPOCH);

    assert_eq!(outcome.settings, ProductSettings::default());
    let notice = outcome.notice.expect("one notice");
    assert_eq!(notice.matches("flamewm:").count(), 1, "{notice}");
    assert!(!notice.contains('\n'), "{notice}");
    assert!(notice.contains("unparsable"), "{notice}");
    assert!(notice.contains("using defaults"), "{notice}");
    let backup = corrupt_name(&path, EPOCH, 0).expect("quarantine name");
    assert!(!path.exists());
    assert_eq!(
        fs::read(&backup).expect("quarantined bytes"),
        truncated.to_vec()
    );
    assert_eq!(scratch.entries(), vec![backup]);
}

/// Case 6: binary junk is rejected at decode time; no partial settings leak through.
#[test]
fn binary_junk_is_quarantined_and_defaults_are_used() {
    let scratch = ScratchDir::new("binary-junk");
    let path = scratch.file("settings.conf");
    let junk = [0xff, 0xfe, 0x00, b'r', b'e', b'v', 0xff];
    fs::write(&path, junk).expect("binary fixture");

    let outcome = load_settings(&path, EPOCH);

    assert_eq!(outcome.settings, ProductSettings::default());
    let notice = outcome.notice.expect("one notice");
    assert_eq!(notice.matches("flamewm:").count(), 1, "{notice}");
    assert!(notice.contains("not UTF-8"), "{notice}");
    assert!(notice.contains("using defaults"), "{notice}");
    let backup = corrupt_name(&path, EPOCH, 0).expect("quarantine name");
    assert_eq!(fs::read(&backup).expect("quarantined bytes"), junk.to_vec());
    assert!(!path.exists());
}

/// Case 7: an oversized file is rejected before it is parsed or read fully into memory.
#[test]
fn huge_file_is_quarantined_and_defaults_are_used() {
    let scratch = ScratchDir::new("huge");
    let path = scratch.file("settings.conf");
    let huge = vec![b'#'; MAX_SETTINGS_BYTES as usize + 1];
    fs::write(&path, &huge).expect("huge fixture");

    let outcome = load_settings(&path, EPOCH);

    assert_eq!(outcome.settings, ProductSettings::default());
    let notice = outcome.notice.expect("one notice");
    assert_eq!(notice.matches("flamewm:").count(), 1, "{notice}");
    assert!(notice.contains("too large"), "{notice}");
    assert!(notice.contains("using defaults"), "{notice}");
    let backup = corrupt_name(&path, EPOCH, 0).expect("quarantine name");
    assert_eq!(fs::read(&backup).expect("quarantined bytes"), huge);
    assert!(!path.exists());
}

/// Case 8: an unreadable directory cannot be traversed; load still degrades to one notice.
/// A privileged (root) runner can still read it, so the reason is conditional but the defaults
/// plus exactly-one-notice contract holds in both cases.
#[test]
fn unreadable_dir_degrades_to_defaults_with_one_notice() {
    let scratch = ScratchDir::new("unreadable-dir");
    let path = scratch.file("settings.conf");
    fs::write(&path, b"revision=9\naccent=broken\n").expect("corrupt fixture");
    scratch.set_mode(0o000);

    let outcome = load_settings(&path, EPOCH);

    scratch.set_mode(0o755);
    assert_eq!(outcome.settings, ProductSettings::default());
    let notice = outcome.notice.expect("one notice");
    assert_eq!(notice.matches("flamewm:").count(), 1, "{notice}");
    assert!(!notice.contains('\n'), "{notice}");
    assert!(notice.contains("using defaults"), "{notice}");
    if notice.contains("unreadable") {
        assert!(notice.contains("preservation failed"), "{notice}");
    } else {
        assert!(notice.contains("unparsable"), "{notice}");
    }
}

/// Case 9: a read-only home cannot quarantine a corrupt file; one notice, defaults preserved.
#[test]
fn read_only_home_degrades_to_defaults_with_one_notice() {
    let scratch = ScratchDir::new("read-only-home");
    let path = scratch.file("settings.conf");
    let bad = b"revision=9\naccent=broken\n";
    fs::write(&path, bad).expect("corrupt fixture");
    scratch.set_mode(0o555);

    let outcome = load_settings(&path, EPOCH);

    scratch.set_mode(0o755);
    assert_eq!(outcome.settings, ProductSettings::default());
    let notice = outcome.notice.expect("one notice");
    assert_eq!(notice.matches("flamewm:").count(), 1, "{notice}");
    assert!(notice.contains("unparsable"), "{notice}");
    assert!(notice.contains("using defaults"), "{notice}");
    if notice.contains("preservation failed") {
        assert!(
            path.exists(),
            "unsuccessful quarantine leaves original in place"
        );
    } else {
        let backup = corrupt_name(&path, EPOCH, 0).expect("quarantine name");
        assert_eq!(fs::read(&backup).expect("quarantined bytes"), bad.to_vec());
    }
}

/// Case 10: a read-only home still loads a valid file silently; no false quarantine.
#[test]
fn read_only_home_loads_valid_file_without_notice() {
    let scratch = ScratchDir::new("read-only-valid");
    let path = scratch.file("settings.conf");
    let valid = b"revision=7\naccent=#112233\n";
    fs::write(&path, valid).expect("valid fixture");
    scratch.set_mode(0o555);

    let outcome = load_settings(&path, EPOCH);

    scratch.set_mode(0o755);
    assert!(outcome.notice.is_none(), "{:?}", outcome.notice);
    assert_eq!(outcome.settings.accent, "#112233");
    assert_eq!(fs::read(&path).expect("original kept"), valid.to_vec());
    assert_eq!(scratch.entries(), vec![path], "no quarantine file");
}
