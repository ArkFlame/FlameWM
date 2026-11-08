//! `flamewm-assistant --voice-setup` must run the background setup, not open the assistant window.
//! The data directory holds a space, so setup refuses it before any network or process work.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn voice_setup_flag_runs_background_setup_and_records_status() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("voice setup arg");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).expect("base dir");

    let output = Command::new(env!("CARGO_BIN_EXE_flamewm-assistant"))
        .arg("--voice-setup")
        .env("XDG_DATA_HOME", &base)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("run flamewm-assistant");

    let status =
        fs::read_to_string(base.join("flamewm/assistant/voice/status")).unwrap_or_default();
    assert!(
        !output.status.success(),
        "a refused setup must exit non-zero"
    );
    assert!(
        status.starts_with("failed (attempt 1 of 3): configuration error:")
            && status.contains("must not contain spaces"),
        "status was {status:?}; stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&base);
}
