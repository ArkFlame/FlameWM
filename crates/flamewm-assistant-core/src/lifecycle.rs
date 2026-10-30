//! Hermes lifecycle: detect the install, start the API gateway, and track the
//! state machine. Installation is only described here, never executed.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::hermes::{discover, HermesEnv, ServerHealth};
use crate::{AssistantError, CancelToken};

pub const INSTALL_SCRIPT_URL: &str = "https://hermes-agent.nousresearch.com/install.sh";
const PROBE_LIMIT: Duration = Duration::from_secs(5);

/// Filesystem locations. Build with `Paths::for_user` or explicitly in tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `~/.hermes`, where `.env` lives.
    pub hermes_home: PathBuf,
    /// Tried in order. A bare name (`hermes`) is resolved through PATH by the OS.
    pub binary_candidates: Vec<PathBuf>,
    /// `$XDG_STATE_HOME/flamewm/assistant`. Chat history and the gateway log live here.
    pub state_dir: PathBuf,
}

impl Paths {
    /// Standard locations for the current user. `None` when no home directory is known.
    pub fn for_user() -> Option<Self> {
        let home = std::env::home_dir()?;
        let state_base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"));
        Some(Self {
            hermes_home: home.join(".hermes"),
            binary_candidates: vec![home.join(".local/bin/hermes"), PathBuf::from("hermes")],
            state_dir: state_base.join("flamewm").join("assistant"),
        })
    }

    pub fn log_path(&self) -> PathBuf {
        self.state_dir.join("hermes.log")
    }
}

/// An installed Hermes binary and its reported version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub binary: PathBuf,
    pub version: String,
}

/// Runs `<candidate> --version` for each candidate and returns the first that succeeds.
/// A candidate containing a path separator must exist as a file.
pub fn detect(candidates: &[PathBuf]) -> Option<Detected> {
    candidates.iter().find_map(|candidate| {
        if candidate.components().count() > 1 && !candidate.is_file() {
            return None;
        }
        let output = run_capture(candidate, &["--version"], PROBE_LIMIT)?;
        Some(Detected { binary: candidate.clone(), version: parse_version(&output) })
    })
}

/// Extracts `0.22.5` from `Hermes Agent v0.22.5 (2026.9.24) · stable`. Falls back to the first line.
pub fn parse_version(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    first
        .split_whitespace()
        .find(|t| t.len() > 1 && t.starts_with('v') && t[1..].starts_with(|c: char| c.is_ascii_digit()))
        .map_or_else(|| first.to_owned(), |t| t[1..].to_owned())
}

fn run_capture(program: &Path, args: &[&str], limit: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait().ok()? {
            Some(status) => {
                if !status.success() {
                    return None;
                }
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return Some(out);
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Lifecycle state. `Starting` is the Loading phase: the gateway was spawned and `/health` is awaited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantState {
    NotInstalled,
    Stopped { version: String },
    Starting { version: String, attempts: u32 },
    Ready { version: String, url: String },
    Error { message: String, version: Option<String> },
}

/// Inputs to `transition`. Callers run `detect` and `wait_until_ready` and map the results here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifeEvent {
    /// Result of `detect`: `None` means not installed.
    Detected(Option<String>),
    /// A health check reported ok at `url`.
    Healthy { version: String, url: String },
    StartRequested,
    /// One poll of `/health` failed while starting; keep waiting.
    NotHealthyYet,
    Failed(String),
    /// A ready gateway stopped answering.
    GatewayLost,
}

/// Pure transition function. Unlisted pairs keep the current state.
pub fn transition(state: &AssistantState, event: LifeEvent) -> AssistantState {
    use AssistantState as S;
    use LifeEvent as E;
    match (state, event) {
        (_, E::Detected(None)) => S::NotInstalled,
        (_, E::Detected(Some(version))) => S::Stopped { version },
        (_, E::Healthy { version, url }) => S::Ready { version, url },
        (S::Stopped { version }, E::StartRequested) => S::Starting { version: version.clone(), attempts: 0 },
        (S::Error { version: Some(version), .. }, E::StartRequested) => {
            S::Starting { version: version.clone(), attempts: 0 }
        }
        (S::NotInstalled, E::StartRequested) => S::Error { message: "Hermes is not installed".into(), version: None },
        (S::Starting { version, attempts }, E::NotHealthyYet) => {
            S::Starting { version: version.clone(), attempts: attempts + 1 }
        }
        (S::Ready { version, .. }, E::GatewayLost) => S::Stopped { version: version.clone() },
        (other, E::Failed(message)) => S::Error { message, version: version_of(other) },
        (other, _) => other.clone(),
    }
}

fn version_of(state: &AssistantState) -> Option<String> {
    match state {
        AssistantState::NotInstalled => None,
        AssistantState::Stopped { version }
        | AssistantState::Starting { version, .. }
        | AssistantState::Ready { version, .. } => Some(version.clone()),
        AssistantState::Error { version, .. } => version.clone(),
    }
}

/// Polls `candidates` with backoff (200 ms doubling to 2 s) until one reports ok.
/// Blocks; run it on a worker thread. Errors with `Timeout` after `deadline`.
pub fn wait_until_ready(
    candidates: &[String],
    api_key: Option<&str>,
    deadline: Duration,
    cancel: &CancelToken,
) -> Result<(String, ServerHealth), AssistantError> {
    let start = Instant::now();
    let mut delay = Duration::from_millis(200);
    loop {
        if let Some(found) = discover(candidates, api_key) {
            return Ok(found);
        }
        if cancel.is_cancelled() {
            return Err(AssistantError::Cancelled);
        }
        let elapsed = start.elapsed();
        if elapsed >= deadline {
            return Err(AssistantError::Timeout);
        }
        std::thread::sleep(delay.min(deadline - elapsed));
        delay = (delay * 2).min(Duration::from_secs(2));
    }
}

/// Starts `<binary> gateway run` detached (via `setsid -f` when available), appending
/// stdout and stderr to `paths.log_path()`. Returns the log path. Refuses unless the
/// API server is enabled in `~/.hermes/.env`; never edits that file.
pub fn start_gateway(binary: &Path, paths: &Paths, env: &HermesEnv) -> Result<PathBuf, AssistantError> {
    if !env.api_server_enabled {
        return Err(AssistantError::Config(
            "Hermes API server is disabled: set API_SERVER_ENABLED=true in ~/.hermes/.env, then retry".into(),
        ));
    }
    std::fs::create_dir_all(&paths.state_dir)?;
    let log = paths.log_path();
    let args = ["gateway", "run"];

    let (out, err) = log_stdio(&log)?;
    let detached = Command::new("setsid")
        .arg("-f")
        .arg(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .status();
    if matches!(detached, Ok(status) if status.success()) {
        return Ok(log);
    }

    let (out, err) = log_stdio(&log)?;
    // The child is intentionally not waited on; it outlives this process.
    Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|e| AssistantError::Unreachable(format!("cannot start {}: {e}", binary.display())))?;
    Ok(log)
}

/// Restarts a running gateway with `hermes gateway restart --yes --detach`: `--yes` skips the
/// CLI's live-session prompt (the app already asked the user once) and `--detach` starts the
/// replacement in the background. Blocks until the command exits; health is checked separately.
pub fn restart_gateway(binary: &Path) -> Result<(), AssistantError> {
    let output = Command::new(binary)
        .args(["gateway", "restart", "--yes", "--detach"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AssistantError::Unreachable(format!("cannot run {}: {e}", binary.display())))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_owned();
    Err(AssistantError::RunFailed(format!("hermes gateway restart failed: {last}")))
}

fn log_stdio(log: &Path) -> Result<(Stdio, Stdio), AssistantError> {
    let file: File = OpenOptions::new().create(true).append(true).open(log)?;
    let clone = file.try_clone()?;
    Ok((Stdio::from(file), Stdio::from(clone)))
}

/// Install steps for the UI to show. The UI must get explicit user confirmation before running anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    pub script_url: &'static str,
    /// Exact shell line to run after confirmation.
    pub shell_command: String,
    /// Run after install to confirm the binary works.
    pub verify_argv: Vec<String>,
}

pub fn install_plan() -> InstallPlan {
    InstallPlan {
        script_url: INSTALL_SCRIPT_URL,
        shell_command: format!("curl -fsSL {INSTALL_SCRIPT_URL} | bash"),
        verify_argv: vec!["hermes".into(), "--version".into()],
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::test_support::{ok_response, serve, test_dir};

    fn fake_binary(dir: &Path, name: &str, script: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn version_parsing() {
        assert_eq!(parse_version("Hermes Agent v0.22.5 (2026.9.24) · stable\n"), "0.22.5");
        assert_eq!(parse_version("weird output"), "weird output");
    }

    #[test]
    fn transitions() {
        let stopped = AssistantState::Stopped { version: "1.0".into() };
        let starting = transition(&stopped, LifeEvent::StartRequested);
        assert_eq!(starting, AssistantState::Starting { version: "1.0".into(), attempts: 0 });
        let waiting = transition(&starting, LifeEvent::NotHealthyYet);
        assert_eq!(waiting, AssistantState::Starting { version: "1.0".into(), attempts: 1 });
        let ready = transition(&waiting, LifeEvent::Healthy { version: "1.0".into(), url: "http://x".into() });
        assert_eq!(ready, AssistantState::Ready { version: "1.0".into(), url: "http://x".into() });
        assert_eq!(transition(&ready, LifeEvent::GatewayLost), stopped);
        let failed = transition(&starting, LifeEvent::Failed("boom".into()));
        assert_eq!(failed, AssistantState::Error { message: "boom".into(), version: Some("1.0".into()) });
        assert_eq!(transition(&failed, LifeEvent::StartRequested), AssistantState::Starting { version: "1.0".into(), attempts: 0 });
        assert_eq!(
            transition(&AssistantState::NotInstalled, LifeEvent::StartRequested),
            AssistantState::Error { message: "Hermes is not installed".into(), version: None }
        );
        assert_eq!(transition(&ready, LifeEvent::Detected(None)), AssistantState::NotInstalled);
    }

    #[test]
    fn detect_with_fake_binary_and_missing_candidates() {
        let dir = test_dir("detect");
        let good = fake_binary(&dir, "hermes", "echo 'Hermes Agent v9.8.7 (test) · stable'");
        let failing = fake_binary(&dir, "broken", "exit 3");
        let missing = dir.join("absent");
        let found = detect(&[missing.clone(), failing, good.clone()]).unwrap();
        assert_eq!(found, Detected { binary: good, version: "9.8.7".into() });
        assert_eq!(detect(&[missing]), None);
    }

    #[test]
    fn start_refused_when_api_server_disabled() {
        let dir = test_dir("start-disabled");
        let paths = Paths { hermes_home: dir.clone(), binary_candidates: vec![], state_dir: dir.join("state") };
        let bin = fake_binary(&dir, "hermes", "exit 0");
        let err = start_gateway(&bin, &paths, &HermesEnv::default()).unwrap_err();
        assert!(matches!(err, AssistantError::Config(ref m) if m.contains("API_SERVER_ENABLED")));
        assert!(!paths.log_path().exists());
    }

    #[test]
    fn start_spawns_detached_and_logs() {
        let dir = test_dir("start-log");
        let paths = Paths { hermes_home: dir.clone(), binary_candidates: vec![], state_dir: dir.join("state") };
        let bin = fake_binary(&dir, "hermes", "echo \"started: $1 $2\"");
        let env = HermesEnv { api_server_enabled: true, ..HermesEnv::default() };
        let log = start_gateway(&bin, &paths, &env).unwrap();
        assert_eq!(log, paths.log_path());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if text.contains("started: gateway run") {
                break;
            }
            assert!(Instant::now() < deadline, "log never written: {text:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn wait_until_ready_sees_health_and_times_out() {
        let (url, server) = serve(vec![ok_response("", r#"{"status":"ok","platform":"hermes-agent","authenticated":false}"#)]);
        let (found, health) =
            wait_until_ready(&[url.clone()], None, Duration::from_secs(5), &CancelToken::new()).unwrap();
        assert_eq!(found, url);
        assert!(health.is_ok());
        server.join().unwrap();

        let err = wait_until_ready(
            &["http://127.0.0.1:1".to_owned()],
            None,
            Duration::from_millis(300),
            &CancelToken::new(),
        )
        .unwrap_err();
        assert_eq!(err, AssistantError::Timeout);
    }

    #[test]
    fn install_plan_is_descriptive_only() {
        let plan = install_plan();
        assert!(plan.shell_command.contains(INSTALL_SCRIPT_URL));
        assert_eq!(plan.verify_argv, vec!["hermes".to_owned(), "--version".to_owned()]);
    }

    /// Probes the real local Hermes install. Run manually: `cargo test -- --ignored real_hermes`.
    #[test]
    #[ignore = "touches the real ~/.hermes and any running Hermes server"]
    fn real_hermes_probe() {
        let Some(paths) = Paths::for_user() else { return };
        let env = crate::read_env(&paths.hermes_home);
        let detected = detect(&paths.binary_candidates);
        let urls = crate::candidate_urls(None, &env);
        eprintln!("detected={detected:?} urls={urls:?} found={:?}", discover(&urls, env.api_key.as_deref()));
    }

    #[test]
    fn restart_gateway_runs_restart_with_yes_and_detach() {
        let dir = test_dir("lifecycle-restart");
        let log = dir.join("args.txt");
        let bin = fake_binary(&dir, "hermes", &format!("echo \"$@\" > '{}'", log.display()));
        restart_gateway(&bin).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "gateway restart --yes --detach");
    }

    #[test]
    fn restart_gateway_reports_last_stderr_line() {
        let dir = test_dir("lifecycle-restart-fail");
        let bin = fake_binary(&dir, "hermes", "echo first >&2\necho 'boom: no service' >&2\nexit 3");
        let error = restart_gateway(&bin).unwrap_err().to_string();
        assert!(error.contains("boom: no service"), "{error}");
    }

}
