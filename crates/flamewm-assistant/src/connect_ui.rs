//! Connect flow and turn feedback for the assistant window: the start, enable and
//! install jobs, the working clock, interrupted replies and one-line error text.
//! Kept apart from `lib.rs` so the window code only has hooks into it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use flamewm_assistant_core::{
    candidate_urls, discover, enable_api_server, explain, gateway_running, install_plan, read_env,
    run_streaming, start_gateway, wait_until_ready, AssistantError, CancelToken, Detected, MsgState,
    Paths, Role, TurnEvent,
};

use super::{Assistant, Bus, Msg, CONNECT_DEADLINE};

/// Turns a streamed event into window state. Deltas and session ids stay in `lib.rs`.
pub(super) fn on_turn_activity(app: &mut Assistant, event: TurnEvent) {
    if let TurnEvent::Activity(text) = event {
        app.notice = text;
    }
}

/// Clock tick while a turn runs: shows elapsed time and how to stop. Ignored when idle.
pub(super) fn working_tick(app: &mut Assistant, elapsed_secs: u64) {
    if app.working {
        app.notice = format!("Working... {} (Esc to stop)", elapsed_text(elapsed_secs));
    }
}

fn elapsed_text(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Sends a `Msg::Tick` once a second until dropped. Dropping it ends the clock.
pub(super) struct Ticker(Arc<AtomicBool>);

impl Ticker {
    pub(super) fn start(bus: Bus) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        thread::spawn(move || {
            let began = Instant::now();
            loop {
                thread::sleep(Duration::from_secs(1));
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                bus.send(Msg::Tick(began.elapsed().as_secs()));
            }
        });
        Self(stop)
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A reply that stopped before it finished: keep what arrived, mark it, and say why.
pub(super) fn interrupted(app: &mut Assistant, partial: &str, message: &str) {
    if !partial.is_empty() {
        app.save(Role::Assistant, partial, None, MsgState::Partial);
    }
    app.save(Role::SystemNote, message, None, MsgState::Error);
    app.notice = format!("{message} Press Enter to retry.");
    app.retry_input();
}

/// A failed turn: one line in the chat and in the notice, and the prompt restored for Retry.
pub(super) fn failed(app: &mut Assistant, error: &AssistantError) {
    let line = explain(error, &app.base_url());
    app.save(Role::SystemNote, &line, None, MsgState::Error);
    app.notice = format!("{line} Press Enter to retry.");
    app.retry_input();
}

/// Starts the gateway unless it already answers. Enables the API server in `~/.hermes/.env`
/// first when it is off (a minimal change with a backup), then waits until it reports healthy.
pub(super) fn start_job(paths: &Paths, binary: &Path, url: &str) -> Result<String, String> {
    let mut env = read_env(&paths.hermes_home);
    if let Some((found, _)) = discover(&candidate_urls(Some(url), &env), env.api_key.as_deref()) {
        return Ok(found);
    }
    if !env.api_server_enabled || env.api_key.is_none() {
        enable_api_server(&paths.hermes_home.join(".env")).map_err(|error| explain(&error, url))?;
        env = read_env(&paths.hermes_home);
    }
    if gateway_running(&paths.hermes_home) {
        return Err("Hermes is already running but its API server is not answering. Run `hermes gateway restart`, then Retry.".into());
    }
    let candidates = candidate_urls(Some(url), &env);
    start_gateway(binary, paths, &env).map_err(|error| explain(&error, url))?;
    wait_until_ready(&candidates, env.api_key.as_deref(), CONNECT_DEADLINE, &CancelToken::new())
        .map(|(found, _)| found)
        .map_err(|error| explain(&error, url))
}

/// Runs the user-confirmed installer. Each output line goes to the window and the log.
pub(super) fn install_job(paths: &Paths, bus: &Bus) -> Result<Detected, String> {
    let plan = install_plan();
    let progress = bus.clone();
    let code = run_streaming(&plan.shell_command, &paths.log_path(), &CancelToken::new(), &mut |line| {
        progress.send(Msg::Progress(line.to_owned()));
    })
    .map_err(|error| error.to_string())?;
    if code != Some(0) {
        return Err(format!("the installer exited with status {code:?}"));
    }
    flamewm_assistant_core::detect(&paths.binary_candidates)
        .ok_or_else(|| "Hermes is still not found after the installer ran".to_owned())
}

impl Assistant {
    /// Puts the last prompt back in the input so Enter retries it.
    pub(super) fn retry_input(&mut self) {
        if self.input.is_empty() {
            self.input = self.last_prompt.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_text_formats_seconds_and_minutes() {
        assert_eq!(elapsed_text(0), "0s");
        assert_eq!(elapsed_text(59), "59s");
        assert_eq!(elapsed_text(61), "1m 01s");
    }
}
