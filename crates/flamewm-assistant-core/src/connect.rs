//! Connect helpers: one-line error messages, the minimal `.env` change that turns
//! the Hermes API server on, a check for an already-running gateway, and an
//! installer runner that streams its output line by line.
//!
//! Nothing here runs unless the caller has the user's confirmation.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{AssistantError, CancelToken};

/// A short, user-facing line for `error`, naming `url` where it helps. Never contains the API key.
pub fn explain(error: &AssistantError, url: &str) -> String {
    match error {
        AssistantError::Http { status: 401, .. } | AssistantError::Http { status: 403, .. } => {
            "Hermes rejected the API key. Check API_SERVER_KEY in ~/.hermes/.env.".into()
        }
        AssistantError::Http { status: 429, .. } => {
            "Hermes is busy with other runs. Wait a moment, then retry.".into()
        }
        AssistantError::Unreachable(_) => format!("Hermes is not reachable at {url}. Start it, then retry."),
        AssistantError::Timeout => "Hermes did not answer in time. Retry, or stop the reply.".into(),
        AssistantError::RunFailed(message) if message.contains("not logged in") || message.contains("not logged into") => {
            "Hermes is not logged in: run `hermes auth` (or `hermes model`), then Retry.".into()
        }
        AssistantError::RunFailed(message) => format!("Hermes could not finish the reply: {}", first_line(message)),
        AssistantError::Protocol(_) => "Hermes sent a reply this app could not read. Retry.".into(),
        other => first_line(&other.to_string()),
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

/// Result of `enable_api_server`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enabled {
    /// True when the file was changed. False when the API server was already set up.
    pub changed: bool,
    /// Copy of the previous `.env`, when one existed and was changed.
    pub backup: Option<PathBuf>,
    /// True when a new `API_SERVER_KEY` was generated.
    pub key_created: bool,
}

/// Turns the API server on in `env_path` with the minimum change: `API_SERVER_ENABLED=true`,
/// and `API_SERVER_KEY` only when it is missing. Every other line is kept. The previous
/// file is copied to `<name>.flamewm-backup-<unix-seconds>` first. The file is written
/// with mode 0600.
pub fn enable_api_server(env_path: &Path) -> Result<Enabled, AssistantError> {
    let existing = match fs::read_to_string(env_path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let lines: Vec<&str> = existing.as_deref().unwrap_or("").lines().collect();
    let enabled_ok = lines.iter().any(|l| assignment(l) == Some(("API_SERVER_ENABLED", "true")));
    let has_key = lines.iter().any(|l| matches!(assignment(l), Some(("API_SERVER_KEY", v)) if !v.is_empty()));
    if enabled_ok && has_key {
        return Ok(Enabled { changed: false, backup: None, key_created: false });
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut wrote_enabled = false;
    for line in &lines {
        match assignment(line) {
            Some(("API_SERVER_ENABLED", _)) => {
                out.push("API_SERVER_ENABLED=true".into());
                wrote_enabled = true;
            }
            _ => out.push((*line).to_owned()),
        }
    }
    if !wrote_enabled {
        out.push("API_SERVER_ENABLED=true".into());
    }
    let key_created = !has_key;
    if key_created {
        out.retain(|l| !matches!(assignment(l), Some(("API_SERVER_KEY", _))));
        out.push(format!("API_SERVER_KEY={}", random_key()?));
    }
    let mut text = out.join("\n");
    text.push('\n');

    let backup = match &existing {
        Some(old) => {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let name = format!(
                "{}.flamewm-backup-{stamp}",
                env_path.file_name().and_then(|n| n.to_str()).unwrap_or(".env")
            );
            let path = env_path.with_file_name(name);
            write_private(&path, old)?;
            Some(path)
        }
        None => None,
    };
    let tmp = env_path.with_extension("flamewm-tmp");
    write_private(&tmp, &text)?;
    fs::rename(&tmp, env_path)?;
    Ok(Enabled { changed: true, backup, key_created })
}

/// `KEY=value` with optional `export ` and quotes stripped. Comments and blanks give `None`.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim().trim_matches(|c| c == '"' || c == '\'')))
}

fn write_private(path: &Path, text: &str) -> Result<(), AssistantError> {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = fs::remove_file(path);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// 32 hex characters from `/dev/urandom`.
fn random_key() -> Result<String, AssistantError> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// True when `<hermes_home>/gateway.pid` names a live process. Linux only.
pub fn gateway_running(hermes_home: &Path) -> bool {
    let Ok(text) = fs::read_to_string(hermes_home.join("gateway.pid")) else {
        return false;
    };
    let Some(pid) = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("pid").and_then(serde_json::Value::as_u64))
    else {
        return false;
    };
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Runs `sh -c <command>` with no terminal. Each stdout or stderr line goes to `on_line`
/// and is appended to `log`. Returns the exit code (`None` when killed by a signal).
/// Stops early, killing the child, when `cancel` is set.
pub fn run_streaming(
    command: &str,
    log: &Path,
    cancel: &CancelToken,
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<i32>, AssistantError> {
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AssistantError::Io(format!("cannot run the installer: {e}")))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, tx.clone()));
    }
    drop(tx);
    let mut log_file = OpenOptions::new().create(true).append(true).open(log)?;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(line) => {
                let _ = writeln!(log_file, "{line}");
                on_line(&line);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().ok();
    if cancel.is_cancelled() {
        return Err(AssistantError::Cancelled);
    }
    Ok(status.and_then(|s| s.code()))
}

fn spawn_reader(pipe: impl Read + Send + 'static, tx: std::sync::mpsc::Sender<String>) -> std::thread::JoinHandle<()> {
    // Byte-wise so one non-UTF-8 line cannot stop the reader and block the child on a full pipe.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n' | b'\r')) {
                        buf.pop();
                    }
                    if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_dir;

    #[test]
    fn explains_common_failures_in_one_line() {
        let url = "http://127.0.0.1:8642";
        let cases = [
            (AssistantError::Http { status: 401, message: "bad".into() }, "rejected the API key"),
            (AssistantError::Unreachable("refused".into()), "not reachable at http://127.0.0.1:8642"),
            (AssistantError::Timeout, "did not answer in time"),
            (AssistantError::RunFailed("not logged in\nmore".into()), "not logged in"),
            (AssistantError::Protocol("x".into()), "could not read"),
            (AssistantError::RunFailed("Hermes is not logged into Nous Portal.".into()), "hermes auth"),
        ];
        for (error, want) in cases {
            let line = explain(&error, url);
            assert!(line.contains(want), "{line}");
            assert!(!line.contains('\n'));
        }
    }

    #[test]
    fn enable_creates_a_new_env_with_key() {
        let dir = test_dir("connect-enable-new");
        let env = dir.join(".env");
        let out = enable_api_server(&env).unwrap();
        assert_eq!(out, Enabled { changed: true, backup: None, key_created: true });
        let text = fs::read_to_string(&env).unwrap();
        assert!(text.contains("API_SERVER_ENABLED=true\n"));
        let key = text.lines().find_map(|l| l.strip_prefix("API_SERVER_KEY=")).unwrap();
        assert_eq!(key.len(), 32);
        assert!(!dir.join("tmp").exists());
    }

    #[test]
    fn enable_keeps_other_lines_and_backs_up() {
        let dir = test_dir("connect-enable-keep");
        let env = dir.join(".env");
        fs::write(&env, "# keep\nOPENROUTER_API_KEY=abc\nAPI_SERVER_ENABLED=false\n").unwrap();
        let out = enable_api_server(&env).unwrap();
        assert!(out.changed && out.key_created);
        let backup = out.backup.unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), "# keep\nOPENROUTER_API_KEY=abc\nAPI_SERVER_ENABLED=false\n");
        let text = fs::read_to_string(&env).unwrap();
        assert!(text.starts_with("# keep\nOPENROUTER_API_KEY=abc\nAPI_SERVER_ENABLED=true\nAPI_SERVER_KEY="));
        assert_eq!(text.matches("API_SERVER_ENABLED").count(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&env).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn enable_is_a_noop_when_already_configured() {
        let dir = test_dir("connect-enable-noop");
        let env = dir.join(".env");
        fs::write(&env, "API_SERVER_ENABLED=true\nAPI_SERVER_KEY=0123456789abcdef\n").unwrap();
        let out = enable_api_server(&env).unwrap();
        assert_eq!(out, Enabled { changed: false, backup: None, key_created: false });
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn enable_keeps_existing_key() {
        let dir = test_dir("connect-enable-key");
        let env = dir.join(".env");
        fs::write(&env, "API_SERVER_KEY=existing-key-1234\n").unwrap();
        let out = enable_api_server(&env).unwrap();
        assert!(out.changed && !out.key_created);
        let text = fs::read_to_string(&env).unwrap();
        assert!(text.contains("API_SERVER_KEY=existing-key-1234"));
        assert!(text.contains("API_SERVER_ENABLED=true"));
    }

    #[test]
    fn gateway_running_reads_pid_file() {
        let dir = test_dir("connect-gateway-pid");
        assert!(!gateway_running(&dir));
        fs::write(dir.join("gateway.pid"), format!(r#"{{"pid":{}}}"#, std::process::id())).unwrap();
        assert!(gateway_running(&dir));
        fs::write(dir.join("gateway.pid"), r#"{"pid":999999999}"#).unwrap();
        assert!(!gateway_running(&dir));
    }

    #[test]
    fn installer_output_streams_and_is_logged() {
        let dir = test_dir("connect-installer");
        let log = dir.join("install.log");
        let mut lines = Vec::new();
        let code = run_streaming("echo one; echo two >&2; exit 3", &log, &CancelToken::new(), &mut |l| {
            lines.push(l.to_owned())
        })
        .unwrap();
        assert_eq!(code, Some(3));
        lines.sort();
        assert_eq!(lines, vec!["one".to_owned(), "two".to_owned()]);
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains("one") && logged.contains("two"));
    }

    #[test]
    fn installer_cancel_kills_child() {
        let dir = test_dir("connect-installer-cancel");
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = run_streaming("sleep 30", &dir.join("l.log"), &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(err, AssistantError::Cancelled);
    }
}
