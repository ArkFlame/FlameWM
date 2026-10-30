//! Connect flow and turn feedback for the assistant window: the start, enable and
//! install jobs, the working clock, interrupted replies and one-line error text.
//! Kept apart from `lib.rs` so the window code only has hooks into it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_assistant_core::{
    candidate_urls, discover, enable_api_server, explain, gateway_running, install_plan, read_env,
    restart_gateway, run_streaming, start_gateway, wait_until_ready, AssistantError, CancelToken, Detected, MsgState,
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
///
/// `allow_restart` is set only after the user confirmed enabling the API server, which covers a
/// running gateway's restart in the same confirm. Otherwise a running gateway asks for its own restart.
pub(super) fn start_job(paths: &Paths, binary: &Path, url: &str, allow_restart: bool) -> Result<StartJob, String> {
    let mut env = read_env(&paths.hermes_home);
    if let Some((found, health)) = discover(&candidate_urls(Some(url), &env), env.api_key.as_deref()) {
        return Ok(StartJob::Ready(found, health.model_only()));
    }
    if !env.api_server_enabled || env.api_key.is_none() {
        enable_api_server(&paths.hermes_home.join(".env")).map_err(|error| explain(&error, url))?;
        env = read_env(&paths.hermes_home);
    }
    if gateway_running(&paths.hermes_home) {
        if !allow_restart {
            // A second gateway would conflict with the running one: the user confirms a restart first.
            return Ok(StartJob::RestartNeeded);
        }
        restart_gateway(binary).map_err(|error| explain(&error, url))?;
        let candidates = candidate_urls(Some(url), &env);
        return wait_until_ready(&candidates, env.api_key.as_deref(), CONNECT_DEADLINE, &CancelToken::new())
            .map(|(found, health)| StartJob::Ready(found, health.model_only()))
            .map_err(|error| explain(&error, url));
    }
    let candidates = candidate_urls(Some(url), &env);
    start_gateway(binary, paths, &env).map_err(|error| explain(&error, url))?;
    wait_until_ready(&candidates, env.api_key.as_deref(), CONNECT_DEADLINE, &CancelToken::new())
        .map(|(found, health)| StartJob::Ready(found, health.model_only()))
        .map_err(|error| explain(&error, url))
}

/// Restarts the running gateway after the user confirmed it, then waits until it reports healthy.
pub(super) fn restart_job(paths: &Paths, binary: &Path, url: &str) -> Result<(String, bool), String> {
    let env = read_env(&paths.hermes_home);
    let candidates = candidate_urls(Some(url), &env);
    restart_gateway(binary).map_err(|error| explain(&error, url))?;
    wait_until_ready(&candidates, env.api_key.as_deref(), CONNECT_DEADLINE, &CancelToken::new())
        .map(|(found, health)| (found, health.model_only()))
        .map_err(|error| explain(&error, url))
}

/// True when Hermes is installed but its API server is off or has no key: Connect asks once to
/// enable it before any `.env` change. Never true for a missing install.
pub(super) fn api_server_needs_enable(paths: &Paths) -> bool {
    let env = read_env(&paths.hermes_home);
    !(env.api_server_enabled && env.api_key.is_some())
}

/// Confirm text shown before the API server is enabled.
pub(super) const ENABLE_CONFIRM: &str =
    "Enable the Hermes API server? Flamy talks to Hermes through it. This edits ~/.hermes/.env (a backup is kept) and restarts a running gateway.";

/// Shown on the connect screen when a Hermes gateway is already running: a second gateway would
/// conflict, so the app asks once before running `hermes gateway restart`.
pub(super) const RESTART_NEEDED: &str = "Hermes needs a quick restart to let Flamy talk to it. Restart now?";

/// Result of the start job: a healthy URL, or a running gateway that needs a confirmed restart.
pub(super) enum StartJob {
    /// A healthy URL, and whether it is the model-only proxy.
    Ready(String, bool),
    RestartNeeded,
}

const LINE_CHARS: usize = 300;

/// Makes one installer output line safe for a single-line label: drops ANSI escapes and the
/// replacement character, keeps the last segment of a carriage-return progress line, turns tabs
/// into spaces, removes other control characters and collapses spaces.
pub(super) fn sanitize_line(raw: &str) -> String {
    let plain = strip_ansi(raw);
    let last = plain.split('\r').rev().find(|part| !part.trim().is_empty()).unwrap_or("");
    let clean: String = last
        .chars()
        .filter(|c| *c != '\u{FFFD}')
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(LINE_CHARS).collect()
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            // CSI: parameter bytes, then one final byte in 0x40..=0x7e.
            Some('[') => {
                chars.next();
                for d in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&d) {
                        break;
                    }
                }
            }
            // OSC: ends with BEL or ESC \.
            Some(']') => {
                chars.next();
                while let Some(d) = chars.next() {
                    if d == '\u{7}' {
                        break;
                    }
                    if d == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            // Any other two-byte escape.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Runs the user-confirmed installer. Each output line goes to the log; the window gets the
/// newest sanitized line at most about five times a second, and the final one before it returns.
pub(super) fn install_job(paths: &Paths, bus: &Bus) -> Result<Detected, String> {
    let plan = install_plan();
    let latest = Arc::new(Mutex::new(None::<String>));
    let done = Arc::new(AtomicBool::new(false));
    let flusher = {
        let (latest, done, bus) = (Arc::clone(&latest), Arc::clone(&done), bus.clone());
        thread::spawn(move || loop {
            let finished = done.load(Ordering::SeqCst);
            if let Some(line) = latest.lock().ok().and_then(|mut slot| slot.take()) {
                bus.send(Msg::Progress(line));
            }
            if finished {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        })
    };
    let code = run_streaming(&plan.shell_command, &paths.log_path(), &CancelToken::new(), &mut |line| {
        let clean = sanitize_line(line);
        if let (false, Ok(mut slot)) = (clean.is_empty(), latest.lock()) {
            *slot = Some(clean);
        }
    });
    done.store(true, Ordering::SeqCst);
    let _ = flusher.join();
    let code = code.map_err(|error| error.to_string())?;
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

    #[test]
    fn sanitize_strips_ansi_sequences() {
        assert_eq!(sanitize_line("\u{1b}[1;32mSuccess\u{1b}[0m done"), "Success done");
        assert_eq!(sanitize_line("\u{1b}]0;title\u{7}Ready"), "Ready");
        assert_eq!(sanitize_line("\u{1b}]8;;http://x\u{1b}\\link\u{1b}c"), "link");
    }

    #[test]
    fn sanitize_keeps_last_carriage_return_segment() {
        assert_eq!(sanitize_line("10%\r20%\r100% done\r"), "100% done");
        assert_eq!(sanitize_line("fetch 5/9\r   \r"), "fetch 5/9");
    }

    #[test]
    fn sanitize_truncates_long_lines_and_drops_controls() {
        let long = sanitize_line(&"a".repeat(500));
        assert_eq!(long.chars().count(), LINE_CHARS);
        assert_eq!(sanitize_line("x\u{0}y\tz\u{7f}"), "xy z");
        assert_eq!(sanitize_line("   \t  "), "");
    }

    #[test]
    fn sanitize_handles_non_utf8_bytes() {
        let raw = String::from_utf8_lossy(&[b'o', b'k', 0xff, b'!']).into_owned();
        assert_eq!(sanitize_line(&raw), "ok!");
    }
}
