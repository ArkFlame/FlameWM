//! Worker-thread glue for the one-click voice setup and the notice text it shows.

use std::path::PathBuf;
use std::thread;

use flamewm_assistant_core::{setup_default, AssistantError};

/// Runs the setup on a worker thread. `on_progress` and `on_done` run on that thread.
pub fn spawn(
    mut on_progress: impl FnMut(u8, String) + Send + 'static,
    on_done: impl FnOnce(Result<PathBuf, AssistantError>) + Send + 'static,
) {
    thread::spawn(move || {
        let result = setup_default(&mut |percent, label| on_progress(percent, label.to_owned()));
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
        assert_eq!(super::progress_notice(40, "Installing the speech runtimes"), "Setting up voice 40%... Installing the speech runtimes");
    }
}
