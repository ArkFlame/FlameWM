//! Remote locations (`sftp://`, `ssh://`, `fish://`) shown in explorer tabs.
//!
//! A remote tab keeps its canonical URL as the tab path string. Every local filesystem call must be
//! guarded with [`is_remote`] first; a remote path handed to `std::fs` would act on a relative local path.
//! All I/O goes through `flamewm-remote-fs`, which owns ssh, caching and cancellation.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use flamewm_remote_fs::{Entry as RemoteEntry, Error, Kind, Url};

use crate::Entry;

/// Recent remote locations kept in `explorer.toml`.
pub const RECENT_LIMIT: usize = 8;

/// True for schemes this explorer lists through `flamewm-remote-fs`.
pub fn is_remote_scheme(scheme: &str) -> bool {
    matches!(scheme.to_ascii_lowercase().as_str(), "sftp" | "ssh" | "fish")
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

/// Explorer entry for a remote child of `dir`. Symlinks are listed as files: `ls -l` does not say whether
/// the target is a directory, so the explorer does not follow them.
pub fn entry_of(dir: &Url, item: &RemoteEntry) -> Entry {
    Entry {
        path: to_path(&dir.join(&item.name)),
        name: item.name.clone(),
        directory: matches!(item.kind, Kind::Dir),
        symlink: matches!(item.kind, Kind::Symlink),
        size: item.size,
        modified: (item.mtime > 0).then(|| UNIX_EPOCH + Duration::from_secs(item.mtime as u64)),
    }
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
        Error::AuthFailed { host, password_possible } => {
            let hint = if *password_possible { " The server wants a password, which is not supported." } else { "" };
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
    let Some(start) = text.find("://").map(|i| i + 3) else { return text };
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
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("flamewm-explorer")
        .join("remote")
}

/// Per-URL cache file. Every open downloads again, so the cache never serves stale content.
pub fn cache_path(root: &Path, url: &Url, name: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    url.to_string().hash(&mut hasher);
    root.join(format!("{:016x}", hasher.finish())).join(safe_file_name(name))
}

/// Keeps a remote file name usable as one local path component.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c == '/' || c.is_control() { '_' } else { c }).collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." { "download".to_owned() } else { cleaned }
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
        assert_eq!(home.to_string(), "sftp://ubuntu@play.arkflame.com:22/home/ubuntu");
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
        assert_eq!(entry.modified, Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)));
        let unknown = RemoteEntry { mtime: 0, kind: Kind::Dir, ..file.clone() };
        let folder = entry_of(&dir, &unknown);
        assert!(folder.directory && folder.modified.is_none());
        assert!(entry_of(&dir, &RemoteEntry { kind: Kind::Symlink, ..file }).symlink);
    }

    #[test]
    fn host_key_and_auth_errors_never_suggest_trusting_silently() {
        let unknown = describe(&Error::HostKeyUnknown { host: "play".into(), port: 22 });
        assert!(unknown.contains("Not trusted automatically"));
        assert!(describe(&Error::HostKeyChanged { host: "play".into(), port: 22 }).contains("refused"));
        let auth = describe(&Error::AuthFailed { host: "play".into(), password_possible: true });
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
        assert_eq!(recent.iter().filter(|r| r.as_str() == "sftp://u@h5:22/").count(), 1);
    }
}
