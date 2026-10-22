//! Persistence under `$XDG_CONFIG_HOME/flamewm/terminal` (fallback `~/.config`): `terminal.toml` for settings and
//! `themes/*.toml` for user themes. The TOML subset here covers `[section]` headers, `key = "string" | int | bool`
//! and `#` comments, which is all these files use.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::themes::{Theme, builtin_themes, format_hex, parse_hex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Int(i64),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub section: String,
    pub key: String,
    pub value: Value,
}

pub fn parse_toml(text: &str) -> Result<Vec<Entry>, String> {
    let mut section = String::new();
    let mut entries = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            section = name.trim().to_owned();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| format!("line {}: expected key = value", number + 1))?;
        entries.push(Entry { section: section.clone(), key: key.trim().to_owned(), value: parse_value(value.trim()).map_err(|error| format!("line {}: {error}", number + 1))? });
    }
    Ok(entries)
}

fn parse_value(raw: &str) -> Result<Value, String> {
    if let Some(body) = raw.strip_prefix('"') {
        let mut text = String::new();
        let mut chars = body.chars();
        loop {
            match chars.next() {
                None => return Err("unterminated string".into()),
                Some('"') => break,
                Some('\\') => match chars.next() {
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some(other) => text.push(other),
                    None => return Err("unterminated escape".into()),
                },
                Some(other) => text.push(other),
            }
        }
        return Ok(Value::Text(text));
    }
    let bare = raw.split('#').next().unwrap_or("").trim();
    match bare {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        _ => bare.parse::<i64>().map(Value::Int).map_err(|_| format!("unsupported value `{bare}`")),
    }
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

pub fn write_toml(entries: &[Entry]) -> String {
    let mut out = String::new();
    let mut section = String::new();
    for entry in entries {
        if entry.section != section {
            if !section.is_empty() || !out.is_empty() {
                out.push('\n');
            }
            if !entry.section.is_empty() {
                out.push_str(&format!("[{}]\n", entry.section));
            }
            section = entry.section.clone();
        }
        let value = match &entry.value {
            Value::Text(text) => quote(text),
            Value::Int(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
        };
        out.push_str(&format!("{} = {}\n", entry.key, value));
    }
    out
}

/// `terminal.toml`: which theme is active and how the terminal behaves. Opacity follows FlameWM's `surfaceOpacity`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalConfig {
    /// `FOLLOW_FLAMEWM` or a theme name.
    pub theme: String,
    pub font_size: u16,
    pub scrollback: usize,
    /// `block`, `underline` or `bar`.
    pub cursor_style: String,
    /// Monospace font family name from `fc-list :spacing=mono`.
    pub font_family: String,
    /// Background opacity in percent (100 = opaque), in steps of 5.
    pub background_opacity: u8,
}

pub const DEFAULT_FONT_FAMILY: &str = "monospace";
pub const DEFAULT_BACKGROUND_OPACITY: u8 = 80;

pub const FOLLOW_FLAMEWM: &str = "follow-flamewm";

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            theme: FOLLOW_FLAMEWM.into(),
            font_size: 13,
            scrollback: 10_000,
            cursor_style: "block".into(),
            font_family: DEFAULT_FONT_FAMILY.into(),
            background_opacity: DEFAULT_BACKGROUND_OPACITY,
        }
    }
}

impl TerminalConfig {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut config = Self::default();
        for entry in parse_toml(text)? {
            match (entry.key.as_str(), entry.value) {
                ("theme", Value::Text(name)) => config.theme = name,
                ("font_size", Value::Int(size)) => config.font_size = size.clamp(6, 40) as u16,
                ("scrollback", Value::Int(lines)) => config.scrollback = lines.clamp(0, 1_000_000) as usize,
                ("cursor_style", Value::Text(style)) if matches!(style.as_str(), "block" | "underline" | "bar") => config.cursor_style = style,
                ("font_family", Value::Text(family)) if !family.trim().is_empty() => config.font_family = family,
                ("background_opacity", Value::Int(percent)) => config.background_opacity = (percent.clamp(5, 100) / 5 * 5) as u8,
                _ => {}
            }
        }
        Ok(config)
    }

    pub fn to_toml(&self) -> String {
        write_toml(&[
            Entry { section: String::new(), key: "theme".into(), value: Value::Text(self.theme.clone()) },
            Entry { section: String::new(), key: "font_size".into(), value: Value::Int(i64::from(self.font_size)) },
            Entry { section: String::new(), key: "scrollback".into(), value: Value::Int(self.scrollback as i64) },
            Entry { section: String::new(), key: "cursor_style".into(), value: Value::Text(self.cursor_style.clone()) },
            Entry { section: String::new(), key: "font_family".into(), value: Value::Text(self.font_family.clone()) },
            Entry { section: String::new(), key: "background_opacity".into(), value: Value::Int(i64::from(self.background_opacity)) },
        ])
    }
}

/// Resolves the configuration directory: `$XDG_CONFIG_HOME/flamewm/terminal`, else `~/.config/flamewm/terminal`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("flamewm").join("terminal")
}

/// Filesystem-safe file stem for a theme name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_end_matches('-').to_owned();
    if trimmed.is_empty() { "theme".into() } else { trimmed }
}

/// Serialises a user theme. Every colour is written as `#rrggbb` under `[colors]`.
pub fn theme_to_toml(theme: &Theme) -> String {
    let mut entries = vec![Entry { section: String::new(), key: "name".into(), value: Value::Text(theme.name.clone()) }];
    for (name, value) in theme.palette.slots() {
        entries.push(Entry { section: "colors".into(), key: name.into(), value: Value::Text(format_hex(value)) });
    }
    write_toml(&entries)
}

pub fn theme_from_toml(text: &str) -> Result<Theme, String> {
    let entries = parse_toml(text)?;
    let name = entries
        .iter()
        .find_map(|entry| match (entry.section.as_str(), entry.key.as_str(), &entry.value) {
            ("", "name", Value::Text(name)) if !name.trim().is_empty() => Some(name.trim().to_owned()),
            _ => None,
        })
        .ok_or("theme file has no name")?;
    let mut palette = builtin_themes()[0].palette.clone();
    for entry in entries.iter().filter(|entry| entry.section == "colors") {
        let Value::Text(hex) = &entry.value else { return Err(format!("colour `{}` must be a hex string", entry.key)) };
        let value = parse_hex(hex).ok_or_else(|| format!("colour `{}` is not #rrggbb", entry.key))?;
        if !palette.set_slot(&entry.key, value) {
            return Err(format!("unknown colour `{}`", entry.key));
        }
    }
    Ok(Theme { name, palette, builtin: false })
}

/// Read/write access to user themes in `dir`. Built-in themes are never written.
#[derive(Clone, Debug)]
pub struct ThemeStore {
    dir: PathBuf,
}

impl ThemeStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.toml", slug(name)))
    }

    /// User themes sorted by name. Unreadable files are skipped rather than hiding every other theme.
    pub fn user_themes(&self) -> Vec<Theme> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut themes: Vec<Theme> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .filter_map(|path| fs::read_to_string(path).ok().and_then(|text| theme_from_toml(&text).ok()))
            .collect();
        themes.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        themes
    }

    /// Built-in themes first, then user themes.
    pub fn all_themes(&self) -> Vec<Theme> {
        let mut themes = builtin_themes();
        themes.extend(self.user_themes());
        themes
    }

    pub fn save(&self, theme: &Theme) -> io::Result<()> {
        if theme.builtin {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "built-in themes are read-only"));
        }
        fs::create_dir_all(&self.dir)?;
        fs::write(self.path_for(&theme.name), theme_to_toml(theme))
    }

    /// Copies any theme into a new user theme with a unique name, e.g. `Dark+ copy` or `Dark+ copy 2`.
    pub fn copy_of(&self, source: &Theme) -> io::Result<Theme> {
        let existing: Vec<String> = self.all_themes().into_iter().map(|t| t.name).collect();
        let base = format!("{} copy", source.name);
        let mut name = base.clone();
        let mut counter = 2;
        while existing.contains(&name) {
            name = format!("{base} {counter}");
            counter += 1;
        }
        let theme = Theme { name, palette: source.palette.clone(), builtin: false };
        self.save(&theme)?;
        Ok(theme)
    }

    pub fn delete(&self, name: &str) -> io::Result<()> {
        fs::remove_file(self.path_for(name))
    }

    /// Renames a user theme: writes the new file and removes the old one. Fails if the new name is taken.
    pub fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        let new = new.trim();
        if new.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "theme name is empty"));
        }
        if self.all_themes().iter().any(|t| t.name == new && t.name != old) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "theme name already exists"));
        }
        let Some(mut theme) = self.user_themes().into_iter().find(|t| t.name == old) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such user theme"));
        };
        theme.name = new.to_owned();
        self.save(&theme)?;
        if slug(old) != slug(new) {
            self.delete(old)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-terminal-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn toml_subset_round_trips_strings_ints_bools_and_sections() {
        let entries = vec![
            Entry { section: String::new(), key: "name".into(), value: Value::Text("a \"quoted\" \\ name".into()) },
            Entry { section: "s".into(), key: "n".into(), value: Value::Int(-4) },
            Entry { section: "s".into(), key: "flag".into(), value: Value::Bool(true) },
        ];
        let text = write_toml(&entries);
        assert_eq!(parse_toml(&text).unwrap(), entries);
    }

    #[test]
    fn toml_reader_reports_bad_lines() {
        assert!(parse_toml("novalue").is_err());
        assert!(parse_toml("k = \"open").is_err());
        assert!(parse_toml("# only a comment\n\n").unwrap().is_empty());
    }

    #[test]
    fn config_defaults_and_clamps() {
        let defaults = TerminalConfig::from_toml("").unwrap();
        assert_eq!(defaults, TerminalConfig::default());
        let parsed = TerminalConfig::from_toml("theme = \"Nord\"\nfont_size = 99\nscrollback = 50\ncursor_style = \"bar\"\n").unwrap();
        assert_eq!((parsed.theme.as_str(), parsed.font_size, parsed.scrollback, parsed.cursor_style.as_str()), ("Nord", 40, 50, "bar"));
        let round = TerminalConfig::from_toml(&parsed.to_toml()).unwrap();
        assert_eq!(round, parsed);
        assert_eq!(TerminalConfig::from_toml("cursor_style = \"weird\"\n").unwrap().cursor_style, "block");
    }

    #[test]
    fn user_theme_file_round_trips_every_colour() {
        let mut theme = builtin_themes()[5].clone();
        theme.name = "My Dracula".into();
        theme.builtin = false;
        let text = theme_to_toml(&theme);
        let parsed = theme_from_toml(&text).unwrap();
        assert_eq!(parsed.name, "My Dracula");
        assert_eq!(parsed.palette, theme.palette);
        assert!(!parsed.builtin);
    }

    #[test]
    fn theme_file_rejects_unknown_colour_and_bad_hex() {
        assert!(theme_from_toml("name = \"x\"\n[colors]\nnope = \"#000000\"\n").is_err());
        assert!(theme_from_toml("name = \"x\"\n[colors]\nbackground = \"#00\"\n").is_err());
        assert!(theme_from_toml("[colors]\nbackground = \"#000000\"\n").is_err());
    }

    #[test]
    fn store_copies_renames_deletes_and_refuses_builtins() {
        let dir = scratch("store");
        let store = ThemeStore::new(&dir);
        let builtins = builtin_themes();
        assert!(store.save(&builtins[0]).is_err(), "built-in themes are read-only");

        let copy = store.copy_of(&builtins[0]).unwrap();
        assert_eq!(copy.name, "Dark+ copy");
        let second = store.copy_of(&builtins[0]).unwrap();
        assert_eq!(second.name, "Dark+ copy 2");
        assert_eq!(store.user_themes().len(), 2);

        store.rename("Dark+ copy", "Ocean").unwrap();
        assert!(store.user_themes().iter().any(|t| t.name == "Ocean"));
        assert!(!store.user_themes().iter().any(|t| t.name == "Dark+ copy"));
        assert!(store.rename("Ocean", "Light+").is_err(), "cannot take a built-in name");
        assert!(store.rename("Ocean", "  ").is_err());

        store.delete("Ocean").unwrap();
        assert_eq!(store.user_themes().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(slug("Synthwave '84"), "synthwave-84");
        assert_eq!(slug("  ../etc "), "etc");
        assert_eq!(slug("???"), "theme");
    }

    #[test]
    fn config_dir_honours_xdg() {
        // Only the path shape is asserted: the environment is process-wide, so no variable is mutated here.
        assert!(config_dir().ends_with("flamewm/terminal"));
    }
}
