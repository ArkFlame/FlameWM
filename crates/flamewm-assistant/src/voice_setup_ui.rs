//! Worker-thread glue for the one-click voice setup and the notice text it shows.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use flamewm_assistant_core::AssistantError;

use crate::voice_background::await_ready;
use crate::voice_setup::{SETUP_BUSY, default_voice_dir, setup_default};

/// How often a busy window re-reads the background status, and how long it waits in total.
const POLL: Duration = Duration::from_secs(2);
const WAIT: Duration = Duration::from_secs(90 * 60);

/// Runs the setup on a worker thread. `on_progress` and `on_done` run on that thread. When a
/// background setup already holds the lock, the worker follows its status until the voice is ready,
/// so the mic works as soon as the background finishes.
pub fn spawn(
    mut on_progress: impl FnMut(u8, String) + Send + 'static,
    on_done: impl FnOnce(Result<PathBuf, AssistantError>) + Send + 'static,
) {
    thread::spawn(move || {
        let mut progress = |percent: u8, label: &str| on_progress(percent, label.to_owned());
        let result = match setup_default(&mut progress) {
            Err(AssistantError::Io(message)) if message == SETUP_BUSY => {
                match default_voice_dir() {
                    Some(dir) => await_ready(
                        &dir,
                        POLL,
                        WAIT,
                        &mut |delay| thread::sleep(delay),
                        &mut progress,
                    ),
                    None => Err(AssistantError::Config(
                        "no data directory is available".into(),
                    )),
                }
            }
            other => other,
        };
        on_done(result);
    });
}

/// Notice text for one setup step, such as `Setting up voice 40%... Installing the speech runtimes`.
pub fn progress_notice(percent: u8, label: &str) -> String {
    format!("Setting up voice {percent}%... {label}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_notice_shows_percent_and_step() {
        assert_eq!(
            super::progress_notice(40, "Installing the speech runtimes"),
            "Setting up voice 40%... Installing the speech runtimes"
        );
    }
}
