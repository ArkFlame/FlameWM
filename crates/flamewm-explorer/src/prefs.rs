//! Explorer preferences kept in `explorer.toml` under the FlameWM config directory.
use std::{fs, io, path::{Path, PathBuf}};

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
        Self { show_hidden: false, show_type: false, col_modified: 120, col_size: 100, col_kind: 130 }
    }
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

pub fn render(prefs: &Prefs) -> String {
    format!(
        "show_hidden = {}\nshow_type = {}\ncol_modified = {}\ncol_size = {}\ncol_kind = {}\n",
        prefs.show_hidden, prefs.show_type, prefs.col_modified, prefs.col_size, prefs.col_kind
    )
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
        let custom = Prefs { show_hidden: true, show_type: true, col_modified: 150, col_size: 90, col_kind: 111 };
        save(&dir, &custom).unwrap();
        assert_eq!(load(&dir), custom);
        save(&dir, &Prefs { show_hidden: false, ..Prefs::default() }).unwrap();
        assert!(!load(&dir).show_hidden);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn malformed_or_unknown_lines_keep_defaults() {
        assert_eq!(parse("show_hidden = yes\nother = true\ngarbage"), Prefs::default());
        assert!(parse("show_hidden=true").show_hidden);
        // A non-numeric width keeps the default width; the Type column is off unless asked for.
        let parsed = parse("col_size = wide\nshow_type = true");
        assert_eq!(parsed.col_size, Prefs::default().col_size);
        assert!(parsed.show_type);
    }
}
