//! Theme search roots, inheritance chains and configured theme names.

use super::*;

/// Value of `key` inside `[section]` of an INI-style text (KDE `kdeglobals`,
/// GTK `settings.ini`). Empty values count as absent.
#[must_use]
pub fn parse_ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            name.trim().clone_into(&mut current);
            continue;
        }
        if current != section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let value = v.trim();
            if k.trim() == key && !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// Configured icon theme under an XDG config home: KDE `kdeglobals`
/// `[Icons] Theme=` first, then GTK 3 `settings.ini` `gtk-icon-theme-name=`.
#[must_use]
pub fn configured_icon_theme(config_home: &Path) -> Option<String> {
    fs::read_to_string(config_home.join("kdeglobals"))
        .ok()
        .and_then(|text| parse_ini_value(&text, "Icons", "Theme"))
        .or_else(|| {
            fs::read_to_string(config_home.join("gtk-3.0/settings.ini"))
                .ok()
                .and_then(|text| parse_ini_value(&text, "Settings", "gtk-icon-theme-name"))
        })
}

/// Cycle-safe recursive `Inherits` expansion. `hicolor` is always the final
/// fallback. `loader` maps a theme name to its parsed index (or `None`).
pub fn inheritance_chain(
    start: &str,
    loader: &dyn Fn(&str) -> Option<IconThemeIndex>,
) -> Vec<String> {
    let mut chain = Vec::new();
    let mut visiting: Vec<String> = Vec::new();
    expand(start, loader, &mut chain, &mut visiting);
    if !chain.iter().any(|name| name == "hicolor") {
        chain.push("hicolor".to_owned());
    }
    chain
}

pub(super) fn expand(
    name: &str,
    loader: &dyn Fn(&str) -> Option<IconThemeIndex>,
    chain: &mut Vec<String>,
    visiting: &mut Vec<String>,
) {
    if name.is_empty()
        || chain.iter().any(|item| item == name)
        || visiting.iter().any(|item| item == name)
    {
        return;
    }
    visiting.push(name.to_owned());
    chain.push(name.to_owned());
    if let Some(index) = loader(name) {
        for parent in index.inherits {
            expand(&parent, loader, chain, visiting);
        }
    }
    visiting.pop();
}

/// Drop empty paths and later duplicates, keeping first-seen order.
pub(crate) fn unique_paths(values: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for value in values {
        if !value.as_os_str().is_empty() && !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

/// Theme directory under one icon root. Exact name first, then a
/// case-insensitive match so `Breeze` finds an installed `breeze`.
pub(super) fn theme_dir(root: &Path, theme: &str) -> Option<PathBuf> {
    let exact = root.join(theme);
    if exact.is_dir() {
        return Some(exact);
    }
    fs::read_dir(root).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name();
        (name.to_string_lossy().eq_ignore_ascii_case(theme) && entry.path().is_dir())
            .then(|| entry.path())
    })
}

/// Search roots: `$HOME/.icons`, `$XDG_DATA_HOME/icons`, each
/// `$XDG_DATA_DIRS/icons`. Returned roots may not exist; callers skip missing.
#[must_use]
pub fn search_roots(data_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
    {
        roots.push(home.join(".icons"));
    }
    for data_dir in data_dirs {
        roots.push(data_dir.join("icons"));
    }
    // Roots are always non-empty (`join` appends a component), so the
    // empty-path filter in `unique_paths` never drops anything here.
    unique_paths(roots)
}
