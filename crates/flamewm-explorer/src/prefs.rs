//! Explorer preferences kept in `explorer.toml` under the `FlameWM` config directory.
#[cfg(test)]
use std::fs;
use std::{
    io,
    path::{Path, PathBuf},
};

use flamewm_api::safe_config::{atomic_write_text, load_or_default, quarantine_before_save};

/// Column widths in pixels as persisted; the Name column takes the remaining width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub show_hidden: bool,
    pub show_type: bool,
    pub col_modified: u32,
    pub col_size: u32,
    pub col_kind: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            show_hidden: false,
            show_type: false,
            col_modified: 120,
            col_size: 100,
            col_kind: 130,
        }
    }
}

/// `FLAMEWM_CONFIG_DIR`, else `$XDG_CONFIG_HOME/flamewm`, else `~/.config/flamewm`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FLAMEWM_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(
            || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
            PathBuf::from,
        );
    base.join("flamewm")
}

/// Reads `key = value` lines; unknown keys and malformed lines keep the defaults.
#[must_use]
pub fn parse(text: &str) -> Prefs {
    let mut prefs = Prefs::default();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            match key.trim() {
                "show_hidden" => prefs.show_hidden = value == "true",
                "show_type" => prefs.show_type = value == "true",
                "col_modified" => prefs.col_modified = value.parse().unwrap_or(prefs.col_modified),
                "col_size" => prefs.col_size = value.parse().unwrap_or(prefs.col_size),
                "col_kind" => prefs.col_kind = value.parse().unwrap_or(prefs.col_kind),
                _ => {}
            }
        }
    }
    prefs
}

#[must_use]
pub fn render(prefs: &Prefs) -> String {
    format!(
        "show_hidden = {}\nshow_type = {}\ncol_modified = {}\ncol_size = {}\ncol_kind = {}\n",
        prefs.show_hidden, prefs.show_type, prefs.col_modified, prefs.col_size, prefs.col_kind
    )
}

/// Recent remote locations (`remote_recent = sftp://...` lines). URLs never carry passwords; the parser rejects them.
pub fn recent_remotes(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            line.split_once('=')
                .filter(|(key, _)| key.trim() == "remote_recent")
                .map(|(_, value)| value.trim())
        })
        .filter(|value| {
            flamewm_remote_fs::Url::parse(value).is_ok_and(|url| url.to_string() == *value)
        })
        .take(crate::remote::RECENT_LIMIT)
        .map(str::to_owned)
        .collect()
}

#[must_use]
pub fn render_with_recent(prefs: &Prefs, recent: &[String]) -> String {
    let mut text = render(prefs);
    for url in recent {
        text.push_str(&format!("remote_recent = {url}\n"));
    }
    text
}

/// Blast radius named in the notice when `explorer.toml` is quarantined.
const BLAST_RADIUS: &str =
    "explorer view preferences and recent remote locations reset to defaults";

fn prefs_path(dir: &Path) -> PathBuf {
    dir.join("explorer.toml")
}

#[must_use]
pub fn load_recent(dir: &Path) -> Vec<String> {
    load_or_default(
        &prefs_path(dir),
        |text| Ok::<_, std::convert::Infallible>(recent_remotes(text)),
        BLAST_RADIUS,
    )
}

/// Replaces the recent remote list and keeps the stored view preferences.
pub fn save_recent(dir: &Path, recent: &[String]) -> io::Result<()> {
    quarantine_before_save(&prefs_path(dir), |_| true)?;
    let prefs = load(dir);
    write_atomic(dir, &render_with_recent(&prefs, recent))
}

#[must_use]
pub fn load(dir: &Path) -> Prefs {
    load_or_default(
        &prefs_path(dir),
        |text| Ok::<_, std::convert::Infallible>(parse(text)),
        BLAST_RADIUS,
    )
}

/// Writes atomically through the shared facade so a crash never leaves a half-written preference
/// file. A bad existing file is quarantined first and its bytes are never overwritten.
pub fn save(dir: &Path, prefs: &Prefs) -> io::Result<()> {
    quarantine_before_save(&prefs_path(dir), |_| true)?;
    let recent = load_recent(dir);
    write_atomic(dir, &render_with_recent(prefs, &recent))
}

fn write_atomic(dir: &Path, text: &str) -> io::Result<()> {
    atomic_write_text(&prefs_path(dir), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips_through_temp_dir() {
        let dir = std::env::temp_dir().join(format!(
            "flamewm-explorer-prefs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(load(&dir), Prefs::default());
        let custom = Prefs {
            show_hidden: true,
            show_type: true,
            col_modified: 150,
            col_size: 90,
            col_kind: 111,
        };
        save(&dir, &custom).unwrap();
        assert_eq!(load(&dir), custom);
        save(
            &dir,
            &Prefs {
                show_hidden: false,
                ..Prefs::default()
            },
        )
        .unwrap();
        assert!(!load(&dir).show_hidden);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recent_remotes_survive_preference_saves_and_reject_secrets() {
        let dir =
            std::env::temp_dir().join(format!("flamewm-explorer-recent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        save_recent(&dir, &["sftp://ubuntu@play.arkflame.com:22/".into()]).unwrap();
        save(
            &dir,
            &Prefs {
                show_hidden: true,
                ..Prefs::default()
            },
        )
        .unwrap();
        assert_eq!(
            load_recent(&dir),
            vec!["sftp://ubuntu@play.arkflame.com:22/".to_owned()]
        );
        assert!(load(&dir).show_hidden);
        let text = "remote_recent = sftp://u:secret@h:22/\nremote_recent = fish://h:22/~/x\nremote_recent = ftp://h/\nremote_recent = sftp://h/x\n";
        assert_eq!(recent_remotes(text), vec!["fish://h:22/~/x".to_owned()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_or_unknown_lines_keep_defaults() {
        assert_eq!(
            parse("show_hidden = yes\nother = true\ngarbage"),
            Prefs::default()
        );
        assert!(parse("show_hidden=true").show_hidden);
        // A non-numeric width keeps the default width; the Type column is off unless asked for.
        let parsed = parse("col_size = wide\nshow_type = true");
        assert_eq!(parsed.col_size, Prefs::default().col_size);
        assert!(parsed.show_type);
    }

    #[test]
    fn save_uses_the_shared_atomic_writer_with_owner_only_mode_and_no_stale_temp() {
        let dir = std::env::temp_dir().join(format!(
            "flamewm-explorer-atomic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        save(&dir, &Prefs::default()).expect("save through shared atomic writer");
        let path = prefs_path(&dir);
        assert_eq!(
            fs::read_to_string(&path).expect("read prefs"),
            render(&Prefs::default())
        );
        assert!(
            !dir.join("explorer.toml.tmp").exists(),
            "fixed-name temp must not survive a completed save"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "prefs file must be owner-only");
        }
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}

#[cfg(test)]
mod corrupt_file_tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!(
            "flamewm-explorer-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn quarantined(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("explorer.corrupt-"))
            })
            .collect()
    }

    #[test]
    fn corrupt_prefs_load_defaults_are_quarantined_and_survive_save() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).expect("temp dir");
        let bad: &[u8] = &[0xff, 0xfe, 0x00, b'x'];
        fs::write(dir.join("explorer.toml"), bad).expect("write bad prefs");

        assert_eq!(load(&dir), Prefs::default());
        let copies = quarantined(&dir);
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0]).expect("read copy"), bad);

        let custom = Prefs {
            show_hidden: true,
            ..Prefs::default()
        };
        save(&dir, &custom).expect("save after quarantine");
        assert_eq!(load(&dir), custom);
        assert_eq!(fs::read(&copies[0]).expect("copy intact"), bad);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_prefs_are_silent_defaults_and_create_no_quarantine() {
        let dir = temp_dir("missing");
        assert_eq!(load(&dir), Prefs::default());
        assert!(!dir.exists());
    }
}
