//! Background voice setup, started by Settings when speech input or output is turned on:
//! `flamewm-assistant --voice-setup`. It runs the same one-click setup as the assistant window, with
//! retries. Progress and the final state go to `voice/status` beside the voice files, one line each.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use flamewm_assistant_core::{default_voice_dir, setup_default, AssistantError, SETUP_BUSY};

const ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(60);
const STATUS: &str = "status";

/// Runs the setup with the real runners. Returns at once when voice is already set up.
pub fn run() -> Result<(), String> {
    let dir = default_voice_dir().ok_or("no data directory is available for voice files")?;
    run_with(&dir, RETRY_DELAY, setup_default)
}

/// Retries `attempt` up to [`ATTEMPTS`] times and records each state in `dir/status`. A setup already
/// running in another process is left alone, since that process records its own state.
pub fn run_with(
    dir: &Path,
    retry_delay: Duration,
    mut attempt: impl FnMut(&mut dyn FnMut(u8, &str)) -> Result<PathBuf, AssistantError>,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let mut last = String::new();
    for number in 1..=ATTEMPTS {
        let mut last_percent = None;
        let mut progress = |percent: u8, label: &str| {
            if last_percent != Some(percent) {
                last_percent = Some(percent);
                write_status(dir, &format!("{percent}% {label}"));
            }
        };
        match attempt(&mut progress) {
            Ok(_) => {
                write_status(dir, "ready");
                return Ok(());
            }
            Err(AssistantError::Io(message)) if message == SETUP_BUSY => return Ok(()),
            Err(error) => {
                last = error.to_string();
                let retry = number < ATTEMPTS && !matches!(error, AssistantError::Config(_));
                let next = if retry { format!(", retrying in {}s", retry_delay.as_secs()) } else { String::new() };
                write_status(dir, &format!("failed (attempt {number} of {ATTEMPTS}): {last}{next}"));
                if !retry {
                    break;
                }
                thread::sleep(retry_delay);
            }
        }
    }
    Err(last)
}

fn write_status(dir: &Path, line: &str) {
    // Status is informative only; a failed write must not change the setup outcome.
    let _ = fs::write(dir.join(STATUS), format!("{line}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-voice-bg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn status(dir: &Path) -> String {
        fs::read_to_string(dir.join(STATUS)).unwrap_or_default()
    }

    #[test]
    fn success_records_ready_after_progress() {
        let dir = scratch("ok");
        let result = run_with(&dir, Duration::ZERO, |progress| {
            progress(40, "Installing the speech runtimes");
            Ok(dir.join("models/tts-phonon-local"))
        });
        assert_eq!(result, Ok(()));
        assert_eq!(status(&dir), "ready\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn transient_failure_is_retried_and_then_succeeds() {
        let dir = scratch("retry");
        let calls = Cell::new(0);
        let result = run_with(&dir, Duration::ZERO, |_| {
            calls.set(calls.get() + 1);
            if calls.get() < 2 {
                Err(AssistantError::Unreachable("huggingface.co".into()))
            } else {
                Ok(dir.join("models/tts-phonon-local"))
            }
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls.get(), 2);
        assert_eq!(status(&dir), "ready\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn persistent_failure_gives_up_with_a_legible_status() {
        let dir = scratch("fail");
        let calls = Cell::new(0);
        let result = run_with(&dir, Duration::ZERO, |_| {
            calls.set(calls.get() + 1);
            Err(AssistantError::Unreachable("pypi.org".into()))
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), ATTEMPTS);
        let text = status(&dir);
        assert!(text.starts_with("failed (attempt 3 of 3): unreachable: pypi.org"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn configuration_errors_are_not_retried() {
        let dir = scratch("config");
        let calls = Cell::new(0);
        let result = run_with(&dir, Duration::ZERO, |_| {
            calls.set(calls.get() + 1);
            Err(AssistantError::Config("voice path has spaces".into()))
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn busy_setup_in_another_process_is_left_to_it() {
        let dir = scratch("busy");
        let result = run_with(&dir, Duration::ZERO, |_| Err(AssistantError::Io(SETUP_BUSY.into())));
        assert_eq!(result, Ok(()));
        assert!(status(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
