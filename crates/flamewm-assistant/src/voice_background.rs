//! Background voice setup, started by Settings when speech input or output is turned on:
//! `flamewm-assistant --voice-setup`. It runs the same one-click setup as the assistant window, with
//! retries. Progress and the final state go to `voice/status` beside the voice files, one line each.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use flamewm_assistant_core::AssistantError;

use crate::voice_setup::{SETUP_BUSY, STATUS_FILE, default_voice_dir, ready_model, setup_default};

/// Command-line flag that starts the background setup. Settings passes the same literal.
pub const ARG: &str = "--voice-setup";
const ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(60);

/// Starts `flamewm-assistant --voice-setup` from this binary and does not wait for it. The child
/// exits when the setup ends, so no thread stays behind.
pub fn spawn_detached() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot find flamewm-assistant: {error}"))?;
    let mut child = std::process::Command::new(exe)
        .arg(ARG)
        .spawn()
        .map_err(|error| format!("cannot start the voice setup: {error}"))?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

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
                let next = if retry {
                    format!(", retrying in {}s", retry_delay.as_secs())
                } else {
                    String::new()
                };
                write_status(
                    dir,
                    &format!("failed (attempt {number} of {ATTEMPTS}): {last}{next}"),
                );
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
    let _ = fs::write(dir.join(STATUS_FILE), format!("{line}\n"));
}

/// The last status line the background setup wrote, trimmed. Empty when none exists.
#[must_use]
pub fn read_status(dir: &Path) -> String {
    fs::read_to_string(dir.join(STATUS_FILE))
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}

/// True when speech is on and the voice is not ready, so the assistant should start the background
/// setup. A busy lock or a finished setup makes that process exit at once, so a repeat is harmless.
#[must_use]
pub fn startup_setup_needed(speech_on: bool, dir: &Path) -> bool {
    speech_on && ready_model(dir).is_none()
}

/// Waits for a setup that another process runs. Reports each new `NN% label` status line, returns
/// the model once ready, and fails on the final failed attempt or after `timeout`. `sleep` is
/// injected so tests run without waiting.
pub fn await_ready(
    dir: &Path,
    poll: Duration,
    timeout: Duration,
    sleep: &mut dyn FnMut(Duration),
    progress: &mut dyn FnMut(u8, &str),
) -> Result<PathBuf, AssistantError> {
    let mut waited = Duration::ZERO;
    let mut last = String::new();
    loop {
        if let Some(model) = ready_model(dir) {
            progress(100, "Voice ready");
            return Ok(model);
        }
        let status = read_status(dir);
        if status != last {
            if let Some((percent, label)) = status.split_once("% ") {
                if let Ok(percent) = percent.parse::<u8>() {
                    progress(percent, label);
                }
            }
            last = status.clone();
        }
        if status.starts_with("failed") && !status.contains("retrying") {
            return Err(AssistantError::Io(status));
        }
        if waited >= timeout {
            return Err(AssistantError::Io(
                "voice setup did not finish in time; it may still run in the background".into(),
            ));
        }
        sleep(poll);
        waited += poll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flamewm-voice-bg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn status(dir: &Path) -> String {
        fs::read_to_string(dir.join(STATUS_FILE)).unwrap_or_default()
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
        assert!(
            text.starts_with("failed (attempt 3 of 3): unreachable: pypi.org"),
            "{text}"
        );
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
        let result = run_with(&dir, Duration::ZERO, |_| {
            Err(AssistantError::Io(SETUP_BUSY.into()))
        });
        assert_eq!(result, Ok(()));
        assert!(status(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_needs_a_background_setup_only_when_speech_is_on_and_voice_is_not_ready() {
        let dir = scratch("startup");
        assert!(
            !startup_setup_needed(false, &dir),
            "speech off never starts setup"
        );
        assert!(
            startup_setup_needed(true, &dir),
            "speech on and not ready starts setup"
        );
        write_ready(&dir);
        assert!(
            !startup_setup_needed(true, &dir),
            "a ready voice is not set up again"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn write_ready(dir: &Path) {
        // Mirrors the layout `ready_model` checks in flamewm-assistant-core.
        fs::create_dir_all(dir.join("bin")).expect("bin");
        fs::write(dir.join("ready"), "1\n").expect("ready");
        fs::write(dir.join("bin/speak"), "").expect("speak");
        fs::write(dir.join("bin/transcribe"), "").expect("transcribe");
    }

    #[test]
    fn await_ready_reports_progress_then_returns_the_model_when_ready() {
        let dir = scratch("await-ok");
        fs::create_dir_all(&dir).expect("dir");
        let mut ticks = 0;
        let mut seen = Vec::new();
        let result = await_ready(
            &dir,
            Duration::from_secs(2),
            Duration::from_secs(60),
            &mut |_| {
                ticks += 1;
                if ticks == 1 {
                    fs::write(
                        dir.join(STATUS_FILE),
                        "40% Installing the speech runtimes\n",
                    )
                    .unwrap();
                } else {
                    write_ready(&dir);
                }
            },
            &mut |percent, label| seen.push((percent, label.to_owned())),
        );
        assert_eq!(result, Ok(dir.join("models").join("tts-phonon-local")));
        assert_eq!(seen[0], (40, "Installing the speech runtimes".to_owned()));
        assert_eq!(seen.last(), Some(&(100, "Voice ready".to_owned())));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn await_ready_keeps_waiting_through_a_retry_and_fails_on_the_final_attempt() {
        let dir = scratch("await-fail");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(
            dir.join(STATUS_FILE),
            "failed (attempt 1 of 3): offline, retrying in 60s\n",
        )
        .unwrap();
        let mut ticks = 0;
        let result = await_ready(
            &dir,
            Duration::from_secs(2),
            Duration::from_secs(600),
            &mut |_| {
                ticks += 1;
                if ticks == 2 {
                    fs::write(
                        dir.join(STATUS_FILE),
                        "failed (attempt 3 of 3): unreachable: pypi.org\n",
                    )
                    .unwrap();
                }
            },
            &mut |_, _| {},
        );
        let error = result.expect_err("final failure must surface");
        assert!(error.to_string().contains("attempt 3 of 3"), "{error}");
        assert_eq!(ticks, 2, "a retrying status keeps the waiter alive");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn await_ready_gives_up_after_its_timeout() {
        let dir = scratch("await-timeout");
        fs::create_dir_all(&dir).expect("dir");
        let mut waited = Duration::ZERO;
        let result = await_ready(
            &dir,
            Duration::from_secs(2),
            Duration::from_secs(6),
            &mut |d| waited += d,
            &mut |_, _| {},
        );
        assert!(result.is_err());
        assert_eq!(waited, Duration::from_secs(6));
        let _ = fs::remove_dir_all(&dir);
    }
}
