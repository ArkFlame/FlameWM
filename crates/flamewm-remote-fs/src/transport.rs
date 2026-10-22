//! Transport over the system OpenSSH client.
//!
//! Every connection shares one ControlMaster socket per (user, host, port) via `ControlPath=<dir>/%C`.
//! The first command opens the master; later commands multiplex over it, so each remote command
//! costs one round trip. `ServerAliveInterval` detects dead links; `ControlPersist` keeps the master.
//! Host keys are verified by ssh itself against ~/.ssh/known_hosts (`StrictHostKeyChecking=yes`).
//! `BatchMode=yes` forbids interactive prompts: auth is key or agent only.

use crate::cancel::Cancel;
use crate::pool::Connector;
use crate::Error;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConnKey {
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug)]
pub struct Transport {
    pub ssh: PathBuf,
    pub control_dir: PathBuf,
    pub connect_timeout_secs: u32,
}

/// Poll slice used by all read loops, so cancellation and idle timeouts are checked ~40 times/s.
pub const POLL: Duration = Duration::from_millis(25);

pub enum ReadEvent {
    Data(Vec<u8>),
    Idle,
    Eof,
}

/// A running child with stdout/stderr pumped by reader threads (unbounded channels, so the child
/// never blocks on a full pipe while the caller is busy).
pub struct Proc {
    child: Child,
    stdout: Receiver<Vec<u8>>,
    stderr: Receiver<Vec<u8>>,
    stdin: Option<ChildStdin>,
}

fn pump(mut r: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

impl Proc {
    fn spawn(mut cmd: Command, with_stdin: bool) -> Result<Self, Error> {
        let program = cmd.get_program().to_string_lossy().into_owned();
        cmd.stdin(if with_stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| Error::Io(format!("cannot run {program}: {e}")))?;
        let stdout = pump(child.stdout.take().ok_or_else(|| Error::Io("no stdout pipe".into()))?);
        let stderr = pump(child.stderr.take().ok_or_else(|| Error::Io("no stderr pipe".into()))?);
        let stdin = child.stdin.take();
        Ok(Self { child, stdout, stderr, stdin })
    }

    pub fn read(&self, timeout: Duration) -> ReadEvent {
        match self.stdout.recv_timeout(timeout) {
            Ok(v) => ReadEvent::Data(v),
            Err(RecvTimeoutError::Timeout) => ReadEvent::Idle,
            Err(RecvTimeoutError::Disconnected) => ReadEvent::Eof,
        }
    }

    pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        match self.stdin.as_mut() {
            Some(s) => s.write_all(data),
            None => Err(io::Error::new(io::ErrorKind::BrokenPipe, "stdin closed")),
        }
    }

    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Wait for exit and collect stderr. Call after stdout reached EOF or after `kill`.
    pub fn finish(mut self) -> (i32, String) {
        self.close_stdin();
        let status = self.child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        let mut err = Vec::new();
        // Drain whatever stderr arrives; stop once the pipe is quiet (a ControlPersist master may
        // keep an inherited fd open briefly, so do not wait for full disconnect).
        while let Ok(chunk) = self.stderr.recv_timeout(Duration::from_millis(50)) {
            err.extend_from_slice(&chunk);
        }
        (status, String::from_utf8_lossy(&err).into_owned())
    }
}

/// Result of a non-streaming remote command.
pub struct Collected {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Transport {
    /// ssh invocation for `key`. `mux_op` is `-O check|exit`; `remote` is the command to run.
    #[must_use]
    pub fn ssh_command(&self, key: &ConnKey, mux_op: Option<&str>, remote: Option<&str>) -> Command {
        let mut cmd = Command::new(&self.ssh);
        let control_path = format!("ControlPath={}/%C", self.control_dir.display());
        let connect_timeout = format!("ConnectTimeout={}", self.connect_timeout_secs);
        for opt in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
            "ControlMaster=auto",
            "ControlPersist=600",
            connect_timeout.as_str(),
            control_path.as_str(),
        ] {
            cmd.arg("-o").arg(opt);
        }
        if let Some(op) = mux_op {
            cmd.arg("-O").arg(op);
        }
        cmd.arg("-p").arg(key.port.to_string());
        if let Some(user) = &key.user {
            cmd.arg("-l").arg(user);
        }
        cmd.arg(&key.host);
        if let Some(r) = remote {
            cmd.arg(r);
        }
        cmd
    }

    /// Start a streaming remote command (stdin piped when `with_stdin`).
    pub fn spawn(&self, key: &ConnKey, remote: &str, with_stdin: bool) -> Result<Proc, Error> {
        Proc::spawn(self.ssh_command(key, None, Some(remote)), with_stdin)
    }

    /// Run a remote command to completion, enforcing an idle timeout and honouring cancellation.
    pub fn run(&self, key: &ConnKey, remote: &str, idle: Duration, cancel: &Cancel) -> Result<Collected, Error> {
        let mut p = self.spawn(key, remote, false)?;
        let mut stdout = Vec::new();
        let mut last = Instant::now();
        loop {
            if cancel.is_cancelled() {
                p.kill();
                return Err(Error::Cancelled);
            }
            match p.read(POLL) {
                ReadEvent::Data(d) => {
                    stdout.extend_from_slice(&d);
                    last = Instant::now();
                }
                ReadEvent::Idle => {
                    if last.elapsed() > idle {
                        p.kill();
                        return Err(Error::Timeout(format!("{}", key.host)));
                    }
                }
                ReadEvent::Eof => break,
            }
        }
        let (status, stderr) = p.finish();
        Ok(Collected { status, stdout, stderr })
    }

    /// Like `run`, but maps non-zero status to a typed error.
    pub fn run_checked(&self, key: &ConnKey, remote: &str, path: &str, idle: Duration) -> Result<Vec<u8>, Error> {
        let c = self.run(key, remote, idle, &Cancel::default())?;
        if c.status == 0 {
            Ok(c.stdout)
        } else {
            Err(classify(c.status, &c.stderr, key, path))
        }
    }

    pub fn control_dir_ensure(&self) -> io::Result<()> {
        ensure_private_dir(&self.control_dir)
    }
}

pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// Map an ssh/remote failure to a typed error. ssh uses status 255 for its own failures; any other
/// status came from the remote command. Secrets are never present in ssh's diagnostics.
#[must_use]
pub fn classify(status: i32, stderr: &str, key: &ConnKey, path: &str) -> Error {
    let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    if status == 255 {
        if stderr.contains("Host key verification failed") {
            return Error::HostKeyUnknown { host: key.host.clone(), port: key.port };
        }
        if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
            || (stderr.contains("Host key for") && stderr.contains("has changed"))
        {
            return Error::HostKeyChanged { host: key.host.clone(), port: key.port };
        }
        if stderr.contains("Permission denied") {
            let password_possible = stderr.contains("password") || stderr.contains("keyboard-interactive");
            return Error::AuthFailed { host: key.host.clone(), password_possible };
        }
        if stderr.contains("timed out") {
            return Error::Timeout(key.host.clone());
        }
        return Error::ConnectionLost(first);
    }
    if stderr.contains("No such file or directory") {
        return Error::NotFound(path.to_string());
    }
    if stderr.contains("Permission denied") {
        return Error::PermissionDenied(path.to_string());
    }
    if stderr.contains("Is a directory") {
        return Error::IsDirectory(path.to_string());
    }
    if status == 17 || stderr.contains("File exists") {
        return Error::AlreadyExists(path.to_string());
    }
    Error::Remote { status, message: first }
}

/// Connection establishment and liveness for the pool, implemented with ssh's mux commands.
#[derive(Clone, Debug)]
pub struct SshConnector {
    pub transport: Transport,
    pub idle: Duration,
}

impl Connector for SshConnector {
    fn establish(&self, key: &ConnKey) -> Result<(), Error> {
        self.transport.control_dir_ensure()?;
        // `true` opens the ControlMaster; ControlPersist keeps it after this command exits.
        let c = self.transport.run(key, "true", self.idle, &Cancel::default())?;
        if c.status == 0 { Ok(()) } else { Err(classify(c.status, &c.stderr, key, "")) }
    }

    fn is_alive(&self, key: &ConnKey) -> bool {
        let mut cmd = self.transport.ssh_command(key, Some("check"), None);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        cmd.status().map(|s| s.success()).unwrap_or(false)
    }

    fn close(&self, key: &ConnKey) {
        let mut cmd = self.transport.ssh_command(key, Some("exit"), None);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let _ = cmd.status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ConnKey {
        ConnKey { user: Some("ubuntu".into()), host: "h.example".into(), port: 22 }
    }

    #[test]
    fn classifies_ssh_failures() {
        assert!(matches!(classify(255, "Host key verification failed.", &key(), ""), Error::HostKeyUnknown { .. }));
        assert!(matches!(
            classify(255, "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@", &key(), ""),
            Error::HostKeyChanged { .. }
        ));
        assert!(matches!(
            classify(255, "ubuntu@h: Permission denied (publickey).", &key(), ""),
            Error::AuthFailed { password_possible: false, .. }
        ));
        assert!(matches!(
            classify(255, "Permission denied (publickey,password).", &key(), ""),
            Error::AuthFailed { password_possible: true, .. }
        ));
        assert!(matches!(classify(255, "ssh: connect to host h port 22: Connection timed out", &key(), ""), Error::Timeout(_)));
        assert!(matches!(classify(255, "ssh: Could not resolve hostname", &key(), ""), Error::ConnectionLost(_)));
    }

    #[test]
    fn classifies_remote_failures() {
        assert!(matches!(classify(2, "ls: cannot access 'x': No such file or directory", &key(), "/x"), Error::NotFound(p) if p == "/x"));
        assert!(matches!(classify(1, "cat: a: Permission denied", &key(), "/a"), Error::PermissionDenied(_)));
        assert!(matches!(classify(17, "", &key(), "/a"), Error::AlreadyExists(_)));
        assert!(matches!(classify(1, "cat: d: Is a directory", &key(), "/d"), Error::IsDirectory(_)));
        assert!(matches!(classify(5, "boom", &key(), "/a"), Error::Remote { status: 5, .. }));
    }

    #[test]
    fn ssh_args_pin_security_options_and_no_prompts() {
        let t = Transport { ssh: "ssh".into(), control_dir: "/run/u/flamewm-rfs".into(), connect_timeout_secs: 10 };
        let cmd = t.ssh_command(&key(), None, Some("true"));
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"StrictHostKeyChecking=yes".to_string()));
        assert!(args.contains(&"ControlPath=/run/u/flamewm-rfs/%C".to_string()));
        assert!(!args.iter().any(|a| a.contains("StrictHostKeyChecking=no")));
        let host_pos = args.iter().position(|a| a == "h.example").unwrap();
        assert_eq!(args[host_pos + 1], "true");
    }
}
