//! Explorer preferences kept in `explorer.toml` under the FlameWM config directory.
use std::{fs, io, path::{Path, PathBuf}};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    pub show_hidden: bool,
}

/// `FLAMEWM_CONFIG_DIR`, else `$XDG_CONFIG_HOME/flamewm`, else `~/.config/flamewm`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FLAMEWM_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("flamewm")
}

/// Reads `key = value` lines; unknown keys and malformed lines keep the defaults.
pub fn parse(text: &str) -> Prefs {
    let mut prefs = Prefs::default();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "show_hidden" {
                prefs.show_hidden = value.trim() == "true";
            }
        }
    }
    prefs
}

pub fn render(prefs: &Prefs) -> String {
    format!("show_hidden = {}\n", prefs.show_hidden)
}

pub fn load(dir: &Path) -> Prefs {
    fs::read_to_string(dir.join("explorer.toml")).map(|text| parse(&text)).unwrap_or_default()
}

/// Writes through a temporary file so a crash never leaves a half-written preference file.
pub fn save(dir: &Path, prefs: &Prefs) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let temp = dir.join("explorer.toml.tmp");
    fs::write(&temp, render(prefs))?;
    fs::rename(temp, dir.join("explorer.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips_through_temp_dir() {
        let dir = std::env::temp_dir().join(format!("flamewm-explorer-prefs-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        assert_eq!(load(&dir), Prefs::default());
        save(&dir, &Prefs { show_hidden: true }).unwrap();
        assert_eq!(load(&dir), Prefs { show_hidden: true });
        save(&dir, &Prefs { show_hidden: false }).unwrap();
        assert!(!load(&dir).show_hidden);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_or_unknown_lines_keep_defaults() {
        assert_eq!(parse("show_hidden = yes\nother = true\ngarbage"), Prefs::default());
        assert!(parse("show_hidden=true").show_hidden);
    }
}
