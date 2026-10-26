//! Remote locations (`sftp://`, `ssh://`, `fish://`) shown in explorer tabs.
//!
//! A remote tab keeps its canonical URL as the tab path string. Every local filesystem call must be
//! guarded with [`is_remote`] first; a remote path handed to `std::fs` would act on a relative local path.
//! All I/O goes through `flamewm-remote-fs`, which owns ssh, caching and cancellation.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use std::fs;
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError};

use flamewm_remote_fs::{
    Cancel as RemoteCancel, Entry as RemoteEntry, Error, Kind, RemoteFs, TransferEvent, Url,
};

use crate::jobs::CancellationToken;
use crate::{BatchReport, Entry, destination_for};

/// Names tried before a copy or new folder gives up on finding a free name.
const MAX_NAMES: usize = 100;

/// The blocking remote calls the file worker makes. `RemoteFs` implements it through its replies; tests use a fake.
pub trait RemoteOps {
    fn stat(&self, url: &Url) -> Result<RemoteEntry, Error>;
    fn mkdir(&self, url: &Url) -> Result<(), Error>;
    fn rename(&self, from: &Url, to: &Url) -> Result<(), Error>;
    fn remove(&self, url: &Url, recursive: bool) -> Result<(), Error>;
    /// Starts a download; the receiver ends with `TransferEvent::Done` or `Failed`.
    fn download(&self, url: &Url, local: &Path, cancel: &RemoteCancel) -> Receiver<TransferEvent>;
    fn upload(&self, local: &Path, url: &Url, cancel: &RemoteCancel) -> Receiver<TransferEvent>;
}

impl RemoteOps for RemoteFs {
    fn stat(&self, url: &Url) -> Result<RemoteEntry, Error> {
        RemoteFs::stat(self, url).wait()
    }
    fn mkdir(&self, url: &Url) -> Result<(), Error> {
        RemoteFs::mkdir(self, url).wait()
    }
    fn rename(&self, from: &Url, to: &Url) -> Result<(), Error> {
        RemoteFs::rename(self, from, to).wait()
    }
    fn remove(&self, url: &Url, recursive: bool) -> Result<(), Error> {
        RemoteFs::remove(self, url, recursive).wait()
    }
    fn download(&self, url: &Url, local: &Path, cancel: &RemoteCancel) -> Receiver<TransferEvent> {
        RemoteFs::download(self, url, local, cancel)
    }
    fn upload(&self, local: &Path, url: &Url, cancel: &RemoteCancel) -> Receiver<TransferEvent> {
        RemoteFs::upload(self, local, url, cancel)
    }
}

/// Creates `New Folder`, or `New Folder N` for the first free N, inside `parent`. Returns the created name.
pub fn new_folder(fs: &impl RemoteOps, parent: &Url) -> Result<String, Error> {
    for n in 1..=MAX_NAMES {
        let name = if n == 1 {
            "New Folder".to_owned()
        } else {
            format!("New Folder {n}")
        };
        match fs.mkdir(&parent.join(&name)) {
            Ok(()) => return Ok(name),
            Err(Error::AlreadyExists(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(Error::AlreadyExists(format!(
        "no free folder name under {}",
        parent.file_name()
    )))
}

/// Recursive delete of each URL. Per-item failures are recorded and the batch continues.
pub fn delete_items(fs: &impl RemoteOps, urls: &[Url]) -> BatchReport {
    let mut report = BatchReport::default();
    for url in urls {
        let path = to_path(url);
        let outcome = fs
            .remove(url, true)
            .map_err(|error| io::Error::other(describe(&error)));
        report.record(&path, outcome);
    }
    report
}

/// Progress for a copy batch: the planned byte total once, then absolute bytes done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyEvent {
    Total(u64),
    Progress(u64),
}

/// Renames within one remote folder or moves within one host. Never overwrites (the server refuses).
pub fn rename_item(fs: &impl RemoteOps, from: &Url, to: &Url) -> Result<(), Error> {
    fs.rename(from, to)
}

/// Copies files in either direction between this machine and a remote folder. Folders, and copies between
/// two remote folders, fail per item. Cancelling stops before the next item and removes partial output.
pub fn copy_items(
    fs: &impl RemoteOps,
    sources: &[PathBuf],
    directory: &Path,
    cancel: &CancellationToken,
    events: &mut impl FnMut(CopyEvent),
) -> BatchReport {
    let mut report = BatchReport::default();
    events(CopyEvent::Total(
        sources.iter().map(|source| size_of(fs, source)).sum(),
    ));
    let mut base = 0_u64;
    for source in sources {
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        match copy_one(fs, source, directory, cancel, base, &mut |bytes| {
            events(CopyEvent::Progress(bytes))
        }) {
            Ok(bytes) => {
                base += bytes;
                report.record(source, Ok(()));
            }
            Err(CopyError::Remote(Error::Cancelled)) => {
                report.cancelled = true;
                break;
            }
            Err(error) => report.record(source, Err(io::Error::other(error.to_string()))),
        }
    }
    report
}

fn size_of(fs: &impl RemoteOps, source: &Path) -> u64 {
    match url_of(source) {
        Some(url) => fs.stat(&url).map_or(0, |entry| entry.size),
        None => fs::metadata(source).map_or(0, |meta| if meta.is_file() { meta.len() } else { 0 }),
    }
}

/// Why one copy item failed: a refusal this module chose, or an error from the remote host.
#[derive(Debug)]
pub enum CopyError {
    Refused(&'static str),
    Remote(Error),
}

impl From<Error> for CopyError {
    fn from(error: Error) -> Self {
        CopyError::Remote(error)
    }
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::Refused(reason) => f.write_str(reason),
            CopyError::Remote(error) => f.write_str(&describe(error)),
        }
    }
}

fn copy_one(
    fs: &impl RemoteOps,
    source: &Path,
    directory: &Path,
    cancel: &CancellationToken,
    base: u64,
    progress: &mut impl FnMut(u64),
) -> Result<u64, CopyError> {
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(CopyError::Refused("source has no name"))?;
    let cancel_remote = RemoteCancel::new();
    match (url_of(source), url_of(directory)) {
        (None, Some(folder)) => {
            if !fs::metadata(source).map_or(false, |meta| meta.is_file()) {
                return Err(CopyError::Refused(
                    "folders cannot be copied to remote folders yet",
                ));
            }
            let target = free_remote_name(fs, &folder, name)?;
            Ok(drive(
                fs.upload(source, &target, &cancel_remote),
                cancel,
                &cancel_remote,
                base,
                progress,
            )?)
        }
        (Some(from), None) => {
            if fs.stat(&from)?.kind == Kind::Dir {
                return Err(CopyError::Refused(
                    "folders cannot be copied from remote folders yet",
                ));
            }
            let target = destination_for(&to_path(&from), directory);
            Ok(drive(
                fs.download(&from, &target, &cancel_remote),
                cancel,
                &cancel_remote,
                base,
                progress,
            )?)
        }
        (Some(_), Some(_)) => Err(CopyError::Refused(
            "copying between two remote folders is not supported yet",
        )),
        (None, None) => Err(CopyError::Refused("neither side is a remote folder")),
    }
}

fn free_remote_name(fs: &impl RemoteOps, folder: &Url, name: &str) -> Result<Url, Error> {
    for n in 1..=MAX_NAMES {
        let candidate = if n == 1 {
            name.to_owned()
        } else {
            format!("{name} ({n})")
        };
        let url = folder.join(&candidate);
        match fs.stat(&url) {
            Ok(_) => continue,
            Err(Error::NotFound(_)) => return Ok(url),
            Err(error) => return Err(error),
        }
    }
    Err(Error::AlreadyExists(format!("no free name for {name}")))
}

/// Waits for one transfer, forwarding progress and turning a local cancel into a remote cancel.
fn drive(
    events: Receiver<TransferEvent>,
    cancel: &CancellationToken,
    remote_cancel: &RemoteCancel,
    base: u64,
    progress: &mut impl FnMut(u64),
) -> Result<u64, Error> {
    loop {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(TransferEvent::Progress { done, .. }) => progress(base + done),
            Ok(TransferEvent::Done { bytes }) => {
                progress(base + bytes);
                return Ok(bytes);
            }
            Ok(TransferEvent::Failed(error)) => return Err(error),
            Err(RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    remote_cancel.cancel();
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::Io("transfer ended unexpectedly".into()));
            }
        }
    }
}

#[cfg(test)]
mod ops_tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};
    use std::sync::mpsc::channel;

    /// In-memory stand-in for the remote host. Transfers write local files with fixed contents.
    #[derive(Default)]
    struct FakeRemote {
        existing: RefCell<HashSet<String>>,
        dirs: RefCell<HashSet<String>>,
        uploads: RefCell<HashMap<String, Vec<u8>>>,
        removed: RefCell<Vec<String>>,
    }

    impl FakeRemote {
        fn with(urls: &[Url]) -> Self {
            let fake = Self::default();
            for url in urls {
                fake.existing.borrow_mut().insert(url.to_string());
            }
            fake
        }
    }

    impl RemoteOps for FakeRemote {
        fn stat(&self, url: &Url) -> Result<RemoteEntry, Error> {
            let key = url.to_string();
            if !self.existing.borrow().contains(&key) {
                return Err(Error::NotFound(key));
            }
            let kind = if self.dirs.borrow().contains(&key) {
                Kind::Dir
            } else {
                Kind::File
            };
            Ok(RemoteEntry {
                name: url.file_name().into(),
                kind,
                size: 4,
                mtime: 0,
                mode: 0o644,
                link_target: None,
            })
        }
        fn mkdir(&self, url: &Url) -> Result<(), Error> {
            let key = url.to_string();
            if !self.existing.borrow_mut().insert(key.clone()) {
                return Err(Error::AlreadyExists(key));
            }
            self.dirs.borrow_mut().insert(key);
            Ok(())
        }
        fn rename(&self, from: &Url, to: &Url) -> Result<(), Error> {
            if !self.existing.borrow().contains(&from.to_string()) {
                return Err(Error::NotFound(from.to_string()));
            }
            if self.existing.borrow().contains(&to.to_string()) {
                return Err(Error::AlreadyExists(to.to_string()));
            }
            Ok(())
        }
        fn remove(&self, url: &Url, _recursive: bool) -> Result<(), Error> {
            self.removed.borrow_mut().push(url.to_string());
            if self.existing.borrow_mut().remove(&url.to_string()) {
                Ok(())
            } else {
                Err(Error::NotFound(url.to_string()))
            }
        }
        fn download(
            &self,
            _url: &Url,
            local: &Path,
            cancel: &RemoteCancel,
        ) -> Receiver<TransferEvent> {
            let (tx, rx) = channel();
            if cancel.is_cancelled() {
                let _ = tx.send(TransferEvent::Failed(Error::Cancelled));
            } else {
                fs::write(local, b"data").unwrap();
                let _ = tx.send(TransferEvent::Progress {
                    done: 4,
                    total: Some(4),
                });
                let _ = tx.send(TransferEvent::Done { bytes: 4 });
            }
            rx
        }
        fn upload(
            &self,
            local: &Path,
            url: &Url,
            _cancel: &RemoteCancel,
        ) -> Receiver<TransferEvent> {
            let (tx, rx) = channel();
            let bytes = fs::read(local).unwrap();
            let len = bytes.len() as u64;
            self.uploads.borrow_mut().insert(url.to_string(), bytes);
            self.existing.borrow_mut().insert(url.to_string());
            let _ = tx.send(TransferEvent::Done { bytes: len });
            rx
        }
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "flamewm-remote-ops-{tag}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_folder_skips_taken_names() {
        let parent = url("sftp://u@h/srv");
        let fake = FakeRemote::with(&[parent.join("New Folder")]);
        assert_eq!(new_folder(&fake, &parent).unwrap(), "New Folder 2");
        assert!(
            fake.dirs
                .borrow()
                .contains(&parent.join("New Folder 2").to_string())
        );
    }

    #[test]
    fn delete_reports_each_item_and_keeps_going_after_failures() {
        let fake = FakeRemote::with(&[url("sftp://u@h/srv/a")]);
        let report = delete_items(
            &fake,
            &[url("sftp://u@h/srv/missing"), url("sftp://u@h/srv/a")],
        );
        assert_eq!((report.completed, report.failed), (1, 1));
        assert_eq!(
            report.failed_sources,
            vec![to_path(&url("sftp://u@h/srv/missing"))]
        );
        assert_eq!(fake.removed.borrow().len(), 2);
    }

    #[test]
    fn download_copies_into_a_fresh_local_name_and_reports_progress() {
        let dir = scratch("download");
        let fake = FakeRemote::with(&[url("sftp://u@h/srv/file.txt")]);
        fs::write(dir.join("file.txt"), b"local old").unwrap();
        let source = to_path(&url("sftp://u@h/srv/file.txt"));
        let cancel = CancellationToken::new();
        let mut seen = Vec::new();
        let report = copy_items(&fake, &[source], &dir, &cancel, &mut |event| {
            seen.push(event)
        });
        assert_eq!(report.completed, 1, "{report:?}");
        assert_eq!(fs::read(dir.join("file.txt")).unwrap(), b"local old");
        assert_eq!(fs::read(dir.join("file.txt (2)")).unwrap(), b"data");
        assert_eq!(seen.first(), Some(&CopyEvent::Total(4)));
        assert_eq!(seen.last(), Some(&CopyEvent::Progress(4)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upload_uses_a_free_remote_name_and_refuses_folders() {
        let dir = scratch("upload");
        fs::write(dir.join("note.txt"), b"hello").unwrap();
        fs::create_dir(dir.join("folder")).unwrap();
        let fake = FakeRemote::with(&[url("sftp://u@h/srv/note.txt")]);
        let renamed = url("sftp://u@h/srv").join("note.txt (2)").to_string();
        let destination = to_path(&url("sftp://u@h/srv"));
        let sources = vec![dir.join("note.txt"), dir.join("folder")];
        let report = copy_items(
            &fake,
            &sources,
            &destination,
            &CancellationToken::new(),
            &mut |_| {},
        );
        assert_eq!((report.completed, report.failed), (1, 1), "{report:?}");
        assert_eq!(
            fake.uploads.borrow().get(&renamed).map(Vec::as_slice),
            Some(&b"hello"[..])
        );
        assert!(
            report
                .first_error
                .unwrap()
                .contains("folders cannot be copied")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_batch_starts_no_further_items() {
        let dir = scratch("cancel");
        fs::write(dir.join("a"), b"a").unwrap();
        fs::write(dir.join("b"), b"b").unwrap();
        let fake = FakeRemote::default();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let report = copy_items(
            &fake,
            &[dir.join("a"), dir.join("b")],
            &to_path(&url("sftp://u@h/srv")),
            &cancel,
            &mut |_| {},
        );
        assert!(report.cancelled);
        assert_eq!((report.completed, report.failed), (0, 0));
        assert!(fake.uploads.borrow().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_between_two_remote_folders_is_refused_per_item() {
        let fake = FakeRemote::with(&[url("sftp://u@h/srv/x")]);
        let report = copy_items(
            &fake,
            &[to_path(&url("sftp://u@h/srv/x"))],
            &to_path(&url("sftp://u@h/other")),
            &CancellationToken::new(),
            &mut |_| {},
        );
        assert_eq!(report.failed, 1);
        assert!(
            report
                .first_error
                .unwrap()
                .contains("between two remote folders")
        );
    }
}

/// Recent remote locations kept in `explorer.toml`.
pub const RECENT_LIMIT: usize = 8;

/// True for schemes this explorer lists through `flamewm-remote-fs`.
pub fn is_remote_scheme(scheme: &str) -> bool {
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "sftp" | "ssh" | "fish"
    )
}

/// The URL behind a remote tab or entry path; `None` for local paths.
pub fn url_of(path: &Path) -> Option<Url> {
    let text = path.to_str()?;
    let (scheme, _) = text.split_once("://")?;
    if !is_remote_scheme(scheme) {
        return None;
    }
    Url::parse(text).ok()
}

pub fn is_remote(path: &Path) -> bool {
    url_of(path).is_some()
}

/// Tab/entry path form of a URL. Never pass the result to local filesystem calls.
pub fn to_path(url: &Url) -> PathBuf {
    PathBuf::from(url.to_string())
}

/// Explorer entry for a remote child of `dir`. Symlinks to directories arrive as `Kind::Dir` (resolved by
/// `flamewm-remote-fs`); other symlinks stay symlinks.
pub fn entry_of(dir: &Url, item: &RemoteEntry) -> Entry {
    Entry::new(
        to_path(&dir.join(&item.name)),
        item.name.clone(),
        matches!(item.kind, Kind::Dir),
        matches!(item.kind, Kind::Symlink),
        item.size,
        (item.mtime > 0).then(|| UNIX_EPOCH + Duration::from_secs(item.mtime as u64)),
    )
}

/// Status-strip wording for a typed remote error. Never includes secrets; never trusts a host key.
pub fn describe(error: &Error) -> String {
    match error {
        Error::HostKeyUnknown { host, port } => format!(
            "Host key for {host}:{port} is not in known_hosts. Connect once with ssh to verify it. Not trusted automatically."
        ),
        Error::HostKeyChanged { host, port } => {
            format!("Host key for {host}:{port} changed. Connection refused.")
        }
        Error::AuthFailed {
            host,
            password_possible,
        } => {
            let hint = if *password_possible {
                " The server wants a password, which is not supported."
            } else {
                ""
            };
            format!("Authentication failed for {host}. Add a key or load an ssh agent.{hint}")
        }
        Error::Timeout(_) => "Remote host timed out. Try again.".to_owned(),
        Error::ConnectionLost(message) => format!("Connection lost: {message}"),
        other => other.to_string(),
    }
}

/// Human-facing URL text: the canonical form without a default port (`sftp://u@h/x`, not `:22`).
/// Canonical text stays unchanged for recents and cache keys.
pub fn display_text(url: &Url) -> String {
    // sftp, ssh and fish all default to port 22 (`Url::parse` fills it in).
    const DEFAULT_PORT: &str = ":22";
    let text = url.to_string();
    let Some(start) = text.find("://").map(|i| i + 3) else {
        return text;
    };
    let end = text[start..].find('/').map_or(text.len(), |i| start + i);
    match text[start..end].strip_suffix(DEFAULT_PORT) {
        Some(host) => format!("{}{}{}", &text[..start], host, &text[end..]),
        None => text,
    }
}

/// Local cache for files opened from remote locations.
pub fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        })
        .join("flamewm-explorer")
        .join("remote")
}

/// Per-URL cache file. Every open downloads again, so the cache never serves stale content.
pub fn cache_path(root: &Path, url: &Url, name: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    url.to_string().hash(&mut hasher);
    root.join(format!("{:016x}", hasher.finish()))
        .join(safe_file_name(name))
}

/// Keeps a remote file name usable as one local path component.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "download".to_owned()
    } else {
        cleaned
    }
}

/// Moves `url` to the front of the recent list, dropping duplicates and anything beyond the limit.
pub fn remember(recent: &mut Vec<String>, url: &Url) {
    let text = url.to_string();
    recent.retain(|r| *r != text);
    recent.insert(0, text);
    recent.truncate(RECENT_LIMIT);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn remote_paths_round_trip_and_local_paths_are_not_remote() {
        let home = url("sftp://ubuntu@play.arkflame.com/home/ubuntu");
        let path = to_path(&home);
        assert!(is_remote(&path));
        // Url::parse fills the default port, so compare canonical text.
        assert_eq!(url_of(&path).map(|u| u.to_string()), Some(home.to_string()));
        assert_eq!(
            home.to_string(),
            "sftp://ubuntu@play.arkflame.com:22/home/ubuntu"
        );
        assert!(!is_remote(Path::new("/home/ubuntu")));
        assert!(!is_remote(Path::new("smb://server/share")));
        assert!(!is_remote(Path::new("relative/sftp")));
    }

    #[test]
    fn fish_and_ssh_schemes_are_remote_but_ftp_is_not() {
        assert!(is_remote(&to_path(&url("fish://host/~/x"))));
        assert!(is_remote(&to_path(&url("ssh://host/srv"))));
        assert!(!is_remote_scheme("ftp"));
    }

    #[test]
    fn entries_map_kind_size_and_mtime() {
        let dir = url("sftp://ubuntu@host/srv");
        let file = RemoteEntry {
            name: "a b.txt".into(),
            kind: Kind::File,
            size: 12,
            mtime: 1_700_000_000,
            mode: 0o644,
            link_target: None,
        };
        let entry = entry_of(&dir, &file);
        assert_eq!(entry.path, to_path(&dir.join("a b.txt")));
        assert!(!entry.directory && !entry.symlink);
        assert_eq!(entry.size, 12);
        assert_eq!(
            entry.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        let unknown = RemoteEntry {
            mtime: 0,
            kind: Kind::Dir,
            ..file.clone()
        };
        let folder = entry_of(&dir, &unknown);
        assert!(folder.directory && folder.modified.is_none());
        assert!(
            entry_of(
                &dir,
                &RemoteEntry {
                    kind: Kind::Symlink,
                    ..file
                }
            )
            .symlink
        );
    }

    #[test]
    fn host_key_and_auth_errors_never_suggest_trusting_silently() {
        let unknown = describe(&Error::HostKeyUnknown {
            host: "play".into(),
            port: 22,
        });
        assert!(unknown.contains("Not trusted automatically"));
        assert!(
            describe(&Error::HostKeyChanged {
                host: "play".into(),
                port: 22
            })
            .contains("refused")
        );
        let auth = describe(&Error::AuthFailed {
            host: "play".into(),
            password_possible: true,
        });
        assert!(auth.contains("password"));
        assert!(describe(&Error::ConnectionLost("reset".into())).starts_with("Connection lost"));
    }

    #[test]
    fn cache_names_stay_inside_their_directory() {
        let root = Path::new("/cache/remote");
        let target = cache_path(root, &url("sftp://u@h/a"), "../../etc");
        assert_eq!(target.parent().unwrap().parent(), Some(root));
        assert_eq!(safe_file_name("x/y\nz"), "x_y_z");
        assert_eq!(safe_file_name(".."), "download");
    }

    #[test]
    fn recent_list_moves_to_front_and_is_capped() {
        let mut recent = Vec::new();
        for i in 0..(RECENT_LIMIT + 3) {
            remember(&mut recent, &url(&format!("sftp://u@h{i}/")));
        }
        assert_eq!(recent.len(), RECENT_LIMIT);
        assert_eq!(recent[0], "sftp://u@h10:22/");
        remember(&mut recent, &url("sftp://u@h5/"));
        assert_eq!(recent[0], "sftp://u@h5:22/");
        assert_eq!(
            recent
                .iter()
                .filter(|r| r.as_str() == "sftp://u@h5:22/")
                .count(),
            1
        );
    }
}
