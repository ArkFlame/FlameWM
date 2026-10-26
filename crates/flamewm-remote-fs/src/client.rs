//! Asynchronous client. Requests are queued to a fixed worker pool (std threads + mpsc); every call
//! returns immediately with a receiver. Listings stream in chunks so the first page can render
//! before the directory is complete.

use crate::Error;
use crate::cache::ListCache;
use crate::cancel::Cancel;
use crate::entry::{Entry, Kind, parse_ls_numeric};
use crate::pool::Pool;
use crate::shell;
use crate::transport::{
    ConnKey, POLL, ReadEvent, SshConnector, Transport, classify, ensure_private_dir,
};
use crate::url::Url;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FIRST_PAGE: usize = 64;
const PAGE: usize = 512;
const COPY_BLOCK: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    /// ssh binary. Defaults to `ssh` or `$FLAMEWM_RFS_SSH` (tests point this at a shim).
    pub ssh: PathBuf,
    pub keyscan: PathBuf,
    pub keygen: PathBuf,
    pub control_dir: PathBuf,
    pub workers: usize,
    pub cache_ttl: Duration,
    pub connect_timeout_secs: u32,
    /// Abort a remote operation that produces no output/progress for this long.
    pub idle_timeout: Duration,
    /// Re-check a master's liveness after this interval.
    pub verify_after: Duration,
    pub pool_idle: Duration,
}

impl Default for Config {
    fn default() -> Self {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
            || {
                std::env::temp_dir().join(format!(
                    "flamewm-rfs-{}",
                    std::env::var("USER").unwrap_or_default()
                ))
            },
            PathBuf::from,
        );
        Self {
            ssh: std::env::var_os("FLAMEWM_RFS_SSH").map_or_else(|| "ssh".into(), PathBuf::from),
            keyscan: "ssh-keyscan".into(),
            keygen: "ssh-keygen".into(),
            control_dir: runtime.join("flamewm-rfs"),
            workers: 4,
            cache_ttl: Duration::from_secs(2),
            connect_timeout_secs: 10,
            idle_timeout: Duration::from_secs(60),
            verify_after: Duration::from_secs(30),
            pool_idle: Duration::from_secs(300),
        }
    }
}

#[derive(Debug)]
pub enum ListEvent {
    Chunk(Vec<Entry>),
    Done { total: usize },
    Failed(Error),
}

#[derive(Debug)]
pub enum TransferEvent {
    Progress { done: u64, total: Option<u64> },
    Done { bytes: u64 },
    Failed(Error),
}

/// Host key offered by the server, to be shown to the user before trusting it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint_sha256: String,
    /// Exact `known_hosts` line to append on acceptance.
    pub known_hosts_line: String,
}

/// One-shot result of a request.
pub struct Reply<T>(Receiver<Result<T, Error>>);

impl<T> Reply<T> {
    pub fn wait(self) -> Result<T, Error> {
        self.0
            .recv()
            .unwrap_or(Err(Error::Io("worker stopped".into())))
    }

    #[must_use]
    pub fn try_take(&self) -> Option<Result<T, Error>> {
        self.0.try_recv().ok()
    }
}

pub(crate) struct Shared {
    transport: Transport,
    pool: Pool<SshConnector>,
    cache: Mutex<ListCache>,
    keyscan: PathBuf,
    keygen: PathBuf,
    idle: Duration,
}

type Job = Box<dyn FnOnce(&Shared) + Send>;

/// Cheap handle to the shared worker pool; clones may move to any thread.
#[derive(Clone)]
pub struct RemoteFs {
    tx: Sender<Job>,
}

impl RemoteFs {
    #[must_use]
    pub fn new(config: Config) -> Self {
        let transport = Transport {
            ssh: config.ssh.clone(),
            control_dir: config.control_dir.clone(),
            connect_timeout_secs: config.connect_timeout_secs,
        };
        let connector = SshConnector {
            transport: transport.clone(),
            idle: config.idle_timeout,
        };
        let shared = Arc::new(Shared {
            transport,
            pool: Pool::new(connector, config.verify_after, config.pool_idle),
            cache: Mutex::new(ListCache::new(config.cache_ttl)),
            keyscan: config.keyscan,
            keygen: config.keygen,
            idle: config.idle_timeout,
        });
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..config.workers.max(1) {
            let rx = Arc::clone(&rx);
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name(format!("flamewm-rfs-{i}"))
                .spawn(move || {
                    loop {
                        // Lock only while receiving; never while running the job.
                        let job = {
                            let Ok(guard) = rx.lock() else { break };
                            guard.recv()
                        };
                        match job {
                            Ok(job) => job(&shared),
                            Err(_) => break,
                        }
                    }
                })
                .expect("spawn flamewm-rfs worker");
        }
        Self { tx }
    }

    fn submit<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Shared) -> Result<T, Error> + Send + 'static,
    ) -> Reply<T> {
        let (tx, rx) = channel();
        let _ = self.tx.send(Box::new(move |sh: &Shared| {
            let _ = tx.send(f(sh));
        }));
        Reply(rx)
    }

    /// Stream a directory listing in chunks. Cancel with `cancel`; cached listings return at once.
    #[must_use]
    pub fn list_dir(&self, url: &Url, cancel: &Cancel) -> Receiver<ListEvent> {
        let (tx, rx) = channel();
        let (url, cancel) = (url.clone(), cancel.clone());
        let _ = self.tx.send(Box::new(move |sh: &Shared| {
            list_job(sh, &url, &cancel, &tx);
        }));
        rx
    }

    #[must_use]
    pub fn stat(&self, url: &Url) -> Reply<Entry> {
        let url = url.clone();
        self.submit(move |sh| stat_entry(sh, &url))
    }

    /// Resolve the remote home directory to an absolute URL.
    #[must_use]
    pub fn home(&self, url: &Url) -> Reply<Url> {
        let url = url.clone();
        self.submit(move |sh| {
            let key = ensure(sh, &url)?;
            let out = sh.transport.run_checked(&key, shell::HOME, "~", sh.idle)?;
            let home = String::from_utf8_lossy(&out).trim().to_string();
            if !home.starts_with('/') {
                return Err(Error::Remote {
                    status: 0,
                    message: "unexpected HOME value".into(),
                });
            }
            let mut resolved = url.clone();
            resolved.path = home;
            Ok(resolved)
        })
    }

    #[must_use]
    pub fn mkdir(&self, url: &Url) -> Reply<()> {
        let url = url.clone();
        self.submit(move |sh| {
            check_mutable(&url)?;
            mutate(
                sh,
                &url,
                &shell::mkdir(&url.remote_path()),
                &url.remote_path(),
            )
        })
    }

    #[must_use]
    pub fn remove(&self, url: &Url, recursive: bool) -> Reply<()> {
        let url = url.clone();
        self.submit(move |sh| {
            check_mutable(&url)?;
            mutate(
                sh,
                &url,
                &shell::remove(&url.remote_path(), recursive),
                &url.remote_path(),
            )
        })
    }

    #[must_use]
    pub fn rename(&self, from: &Url, to: &Url) -> Reply<()> {
        let (from, to) = (from.clone(), to.clone());
        self.submit(move |sh| {
            check_mutable(&from)?;
            check_mutable(&to)?;
            if from.conn_key() != to.conn_key() || from.protocol != to.protocol {
                return Err(Error::UnsupportedProtocol("cross-host rename"));
            }
            mutate(
                sh,
                &from,
                &shell::rename(&from.remote_path(), &to.remote_path()),
                &from.remote_path(),
            )?;
            invalidate_both(sh, &to);
            Ok(())
        })
    }

    /// Copy a remote file to `local` via a `.flamewm-part` temp file, renamed on success.
    #[must_use]
    pub fn download(&self, url: &Url, local: &Path, cancel: &Cancel) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        let (url, local, cancel) = (url.clone(), local.to_path_buf(), cancel.clone());
        let _ = self.tx.send(Box::new(move |sh: &Shared| {
            let event = match download_job(sh, &url, &local, &cancel, &tx) {
                Ok(bytes) => TransferEvent::Done { bytes },
                Err(e) => TransferEvent::Failed(e),
            };
            let _ = tx.send(event);
        }));
        rx
    }

    /// Copy `local` to the remote path via a temp file and atomic rename. The remote temp file is
    /// removed on failure or cancellation.
    #[must_use]
    pub fn upload(&self, local: &Path, url: &Url, cancel: &Cancel) -> Receiver<TransferEvent> {
        let (tx, rx) = channel();
        let (local, url, cancel) = (local.to_path_buf(), url.clone(), cancel.clone());
        let _ = self.tx.send(Box::new(move |sh: &Shared| {
            let event = match upload_job(sh, &local, &url, &cancel, &tx) {
                Ok(bytes) => TransferEvent::Done { bytes },
                Err(e) => TransferEvent::Failed(e),
            };
            invalidate_both(sh, &url);
            let _ = tx.send(event);
        }));
        rx
    }

    /// Fetch the host's key for a first-time trust decision. Does not modify `known_hosts`.
    #[must_use]
    pub fn probe_host_key(&self, url: &Url) -> Reply<HostKeyPrompt> {
        let url = url.clone();
        self.submit(move |sh| probe_host_key(sh, &url))
    }

    /// Append a user-confirmed key to ~/.`ssh/known_hosts`. Refuses to overwrite a different key.
    #[must_use]
    pub fn trust_host_key(&self, prompt: &HostKeyPrompt) -> Reply<()> {
        let prompt = prompt.clone();
        self.submit(move |_| trust_host_key(&prompt))
    }

    pub fn invalidate(&self, url: &Url) {
        let url = url.clone();
        let _ = self
            .tx
            .send(Box::new(move |sh: &Shared| invalidate_both(sh, &url)));
    }
}

fn ensure(sh: &Shared, url: &Url) -> Result<ConnKey, Error> {
    if !url.protocol.is_ssh() {
        return Err(Error::UnsupportedProtocol(url.protocol.scheme()));
    }
    let key = url.conn_key();
    sh.pool.ensure(&key)?;
    Ok(key)
}

/// Drop the pool entry on transport-level failure so the next request re-handshakes.
fn on_err<T>(sh: &Shared, key: &ConnKey, r: Result<T, Error>) -> Result<T, Error> {
    if let Err(Error::ConnectionLost(_) | Error::Timeout(_)) = &r {
        sh.pool.drop_key(key);
    }
    r
}

fn invalidate_both(sh: &Shared, url: &Url) {
    if let Ok(mut c) = sh.cache.lock() {
        c.invalidate(&url.to_string());
        if let Some(p) = url.parent() {
            c.invalidate(&p.to_string());
        }
    }
}

fn check_mutable(url: &Url) -> Result<(), Error> {
    let p = url.remote_path();
    let trimmed = p.trim_end_matches('/');
    if matches!(trimmed, "" | "~" | "." | "..")
        || trimmed.ends_with("/.")
        || trimmed.ends_with("/..")
    {
        return Err(Error::InvalidUrl(
            "refusing to modify root, home or a relative special path".into(),
        ));
    }
    Ok(())
}

fn mutate(sh: &Shared, url: &Url, cmd: &str, path: &str) -> Result<(), Error> {
    let key = ensure(sh, url)?;
    let r = sh
        .transport
        .run_checked(&key, cmd, path, sh.idle)
        .map(|_| ());
    let r = on_err(sh, &key, r);
    if r.is_ok() {
        invalidate_both(sh, url);
    }
    r
}

fn stat_entry(sh: &Shared, url: &Url) -> Result<Entry, Error> {
    let key = ensure(sh, url)?;
    let path = url.remote_path();
    let out = on_err(
        sh,
        &key,
        sh.transport
            .run_checked(&key, &shell::stat(&path), &path, sh.idle),
    )?;
    let text = String::from_utf8_lossy(&out);
    let line = text.lines().next().unwrap_or("");
    let mut e = parse_ls_numeric(line).ok_or_else(|| Error::Remote {
        status: 0,
        message: "unparseable stat".into(),
    })?;
    e.name = url.file_name().to_string();
    Ok(e)
}

fn list_job(sh: &Shared, url: &Url, cancel: &Cancel, tx: &Sender<ListEvent>) {
    let cache_key = url.to_string();
    let hit = sh.cache.lock().ok().and_then(|mut c| c.get(&cache_key));
    if let Some(all) = hit {
        for chunk in all.chunks(PAGE) {
            let _ = tx.send(ListEvent::Chunk(chunk.to_vec()));
        }
        let _ = tx.send(ListEvent::Done { total: all.len() });
        return;
    }
    match list_stream(sh, url, cancel, tx) {
        Ok(all) => {
            let total = all.len();
            if let Ok(mut c) = sh.cache.lock() {
                c.put(cache_key, all);
            }
            let _ = tx.send(ListEvent::Done { total });
        }
        Err(e) => {
            let _ = tx.send(ListEvent::Failed(e));
        }
    }
}

fn list_stream(
    sh: &Shared,
    url: &Url,
    cancel: &Cancel,
    tx: &Sender<ListEvent>,
) -> Result<Vec<Entry>, Error> {
    let key = ensure(sh, url)?;
    let path = url.remote_path();
    let mut proc = sh
        .transport
        .spawn(&key, &shell::list_with_links(&path), false)?;
    let mut all = Vec::new();
    let mut page = Vec::new();
    let mut carry: Vec<u8> = Vec::new();
    let mut limit = FIRST_PAGE;
    let mut last = Instant::now();
    // Symlink names that point at directories; known once the prefix before the ls rows has arrived.
    let mut links: Option<HashSet<String>> = None;
    loop {
        if cancel.is_cancelled() {
            proc.kill();
            return Err(Error::Cancelled);
        }
        match proc.read(POLL) {
            ReadEvent::Data(d) => {
                last = Instant::now();
                carry.extend_from_slice(&d);
                if links.is_none() {
                    links = split_link_prefix(&mut carry);
                }
                let Some(known) = links.as_ref() else {
                    continue;
                };
                while let Some(pos) = carry.iter().position(|&b| b == b'\n') {
                    let line = String::from_utf8_lossy(&carry[..pos]).into_owned();
                    carry.drain(..=pos);
                    if let Some(e) = parse_listing_line(&line) {
                        page.push(mark_link_dir(e, known));
                    }
                    // Check per line: one read may carry many rows, and the first page must stay small.
                    if page.len() >= limit {
                        all.extend_from_slice(&page);
                        let _ = tx.send(ListEvent::Chunk(std::mem::take(&mut page)));
                        limit = PAGE;
                    }
                }
            }
            ReadEvent::Idle => {
                if last.elapsed() > sh.idle {
                    proc.kill();
                    return Err(Error::Timeout(url.host.clone()));
                }
            }
            ReadEvent::Eof => break,
        }
    }
    // No prefix marker means the remote shell skipped the link probe; list the rows without link resolution.
    let known = links.unwrap_or_default();
    for line in carry.split(|&b| b == b'\n') {
        if let Some(e) = parse_listing_line(&String::from_utf8_lossy(line)) {
            page.push(mark_link_dir(e, &known));
        }
    }
    let (status, stderr) = proc.finish();
    if status != 0 {
        let err = classify(status, &stderr, &key, &path);
        if matches!(err, Error::ConnectionLost(_) | Error::Timeout(_)) {
            sh.pool.drop_key(&key);
        }
        return Err(err);
    }
    all.extend_from_slice(&page);
    if !page.is_empty() {
        let _ = tx.send(ListEvent::Chunk(page));
    }
    Ok(all)
}

fn parse_listing_line(line: &str) -> Option<Entry> {
    if line.starts_with("total ") {
        return None;
    }
    parse_ls_numeric(line)
}

/// Splits the link prefix off `carry` (names up to [`shell::LINK_MARK`] followed by a newline).
/// Returns `None` while the marker has not arrived; `carry` is untouched then.
fn split_link_prefix(carry: &mut Vec<u8>) -> Option<HashSet<String>> {
    let mark = [shell::LINK_MARK.as_bytes()[0], b'\n'];
    let pos = carry.windows(2).position(|w| w == mark)?;
    let names = carry[..pos]
        .split(|&b| b == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    carry.drain(..pos + 2);
    Some(names)
}

/// A symlink whose target is a directory lists as a directory, so the explorer opens it.
fn mark_link_dir(mut entry: Entry, dir_links: &HashSet<String>) -> Entry {
    if entry.kind == Kind::Symlink && dir_links.contains(&entry.name) {
        entry.kind = Kind::Dir;
    }
    entry
}

fn download_job(
    sh: &Shared,
    url: &Url,
    local: &Path,
    cancel: &Cancel,
    tx: &Sender<TransferEvent>,
) -> Result<u64, Error> {
    let key = ensure(sh, url)?;
    let path = url.remote_path();
    let total = stat_entry(sh, url).ok().map(|e| e.size);
    let part = part_name(local);
    let mut file = File::create(&part)?;
    let mut proc = sh.transport.spawn(&key, &shell::cat(&path), false)?;
    let mut done = 0u64;
    let mut last = Instant::now();
    let result = (|| -> Result<(), Error> {
        loop {
            if cancel.is_cancelled() {
                proc.kill();
                return Err(Error::Cancelled);
            }
            match proc.read(POLL) {
                ReadEvent::Data(d) => {
                    last = Instant::now();
                    file.write_all(&d)?;
                    done += d.len() as u64;
                    let _ = tx.send(TransferEvent::Progress { done, total });
                }
                ReadEvent::Idle => {
                    if last.elapsed() > sh.idle {
                        proc.kill();
                        return Err(Error::Timeout(url.host.clone()));
                    }
                }
                ReadEvent::Eof => break,
            }
        }
        let (status, stderr) = proc.finish();
        if status != 0 {
            return Err(classify(status, &stderr, &key, &path));
        }
        file.flush()?;
        Ok(())
    })();
    drop(file);
    match result {
        Ok(()) => {
            std::fs::rename(&part, local)?;
            Ok(done)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn part_name(local: &Path) -> PathBuf {
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    local.with_file_name(format!("{name}.flamewm-part"))
}

fn upload_job(
    sh: &Shared,
    local: &Path,
    url: &Url,
    cancel: &Cancel,
    tx: &Sender<TransferEvent>,
) -> Result<u64, Error> {
    let key = ensure(sh, url)?;
    let dst = url.remote_path();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = format!("{dst}.flamewm-part-{}-{nanos:x}", std::process::id());
    let total = std::fs::metadata(local)?.len();
    let mut src = File::open(local)?;
    let mut proc = sh
        .transport
        .spawn(&key, &shell::write_atomic(&tmp, &dst), true)?;
    let mut buf = vec![0u8; COPY_BLOCK];
    let mut done = 0u64;
    let mut write_failed = false;
    let result = (|| -> Result<(), Error> {
        loop {
            if cancel.is_cancelled() {
                proc.kill();
                return Err(Error::Cancelled);
            }
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if proc.write_all(&buf[..n]).is_err() {
                // Remote side closed early; its exit status below carries the real reason.
                write_failed = true;
                break;
            }
            done += n as u64;
            let _ = tx.send(TransferEvent::Progress {
                done,
                total: Some(total),
            });
        }
        Ok(())
    })();
    proc.close_stdin();
    let (status, stderr) = proc.finish();
    if result.is_ok() && status == 0 && !write_failed {
        return Ok(done);
    }
    // Best-effort cleanup of the remote temp file; the original destination is untouched.
    let _ = sh.transport.run(
        &key,
        &shell::remove(&tmp, false),
        sh.idle,
        &Cancel::default(),
    );
    result?;
    Err(classify(status, &stderr, &key, &dst))
}

fn probe_host_key(sh: &Shared, url: &Url) -> Result<HostKeyPrompt, Error> {
    let host = url.host.clone();
    let port = url.port_or_default();
    let out = std::process::Command::new(&sh.keyscan)
        .args(["-T", "10", "-p", &port.to_string(), &host])
        .output()
        .map_err(|e| Error::Io(format!("cannot run ssh-keyscan: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let (key_type, line) =
        pick_host_key(&text).ok_or_else(|| Error::ConnectionLost("no host key offered".into()))?;
    let fp = fingerprint(&sh.keygen, &line)?;
    Ok(HostKeyPrompt {
        host,
        port,
        key_type,
        fingerprint_sha256: fp,
        known_hosts_line: line,
    })
}

/// Prefer ed25519, then ecdsa, then rsa, from `ssh-keyscan` output.
#[must_use]
pub fn pick_host_key(scan_output: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = scan_output
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    for want in ["ssh-ed25519", "ecdsa-sha2-nistp256", "ssh-rsa"] {
        if let Some(l) = lines
            .iter()
            .find(|l| l.split_whitespace().nth(1) == Some(want))
        {
            return Some((want.to_string(), (*l).to_string()));
        }
    }
    None
}

fn fingerprint(keygen: &Path, line: &str) -> Result<String, Error> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(keygen)
        .args(["-lf", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Io(format!("cannot run ssh-keygen: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    let out = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .find(|t| t.starts_with("SHA256:"))
        .map(str::to_string)
        .ok_or_else(|| Error::ConnectionLost("could not compute host key fingerprint".into()))
}

fn known_hosts_path() -> Result<PathBuf, Error> {
    let home = std::env::var_os("HOME").ok_or_else(|| Error::Io("HOME is not set".into()))?;
    Ok(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

fn trust_host_key(prompt: &HostKeyPrompt) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = known_hosts_path()?;
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing
        .lines()
        .any(|l| l.trim() == prompt.known_hosts_line.trim())
    {
        return Ok(());
    }
    if existing
        .lines()
        .any(|l| l.split_whitespace().next() == prompt.known_hosts_line.split_whitespace().next())
    {
        // Same host name with a different key: never replace automatically.
        return Err(Error::HostKeyChanged {
            host: prompt.host.clone(),
            port: prompt.port,
        });
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)?;
    writeln!(f, "{}", prompt.known_hosts_line.trim_end())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_prefix_splits_names_before_the_marker_and_keeps_rows() {
        let mut carry = b"dirlink\0other link\0\x1e\n-rw-r--r-- 1 0 0 3 1700000000 file\n".to_vec();
        let names = split_link_prefix(&mut carry).expect("marker present");
        assert_eq!(
            names,
            HashSet::from(["dirlink".to_string(), "other link".to_string()])
        );
        assert_eq!(carry, b"-rw-r--r-- 1 0 0 3 1700000000 file\n");
    }

    #[test]
    fn link_prefix_waits_for_the_marker() {
        let mut partial = b"dirlink\0\x1e".to_vec();
        assert!(split_link_prefix(&mut partial).is_none());
        assert_eq!(partial, b"dirlink\0\x1e");
        let mut empty_prefix = b"\x1e\nrow".to_vec();
        assert_eq!(split_link_prefix(&mut empty_prefix), Some(HashSet::new()));
    }

    #[test]
    fn only_symlinks_named_in_the_prefix_become_directories() {
        let names = HashSet::from(["sub".to_string()]);
        let link = Entry {
            name: "sub".into(),
            kind: Kind::Symlink,
            size: 4,
            mtime: 0,
            mode: 0o777,
            link_target: Some("real".into()),
        };
        assert_eq!(mark_link_dir(link.clone(), &names).kind, Kind::Dir);
        let other = Entry {
            name: "other".into(),
            ..link.clone()
        };
        assert_eq!(mark_link_dir(other, &names).kind, Kind::Symlink);
        let file = Entry {
            name: "sub".into(),
            kind: Kind::File,
            ..link
        };
        assert_eq!(mark_link_dir(file, &names).kind, Kind::File);
    }
}
