//! Data-only icon and cursor theme catalog. Discovery never executes or follows
//! `Inherits` beyond the cycle-safe chain helper shared with `icon_theme`.

mod archive;
pub mod cursor;
pub mod import;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::icon_theme::{IconThemeIndex, inheritance_chain, parse_ini_value, search_roots};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThemeKind {
    Icon,
    Cursor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeEntry {
    pub id: String,
    pub name: String,
    pub kind: ThemeKind,
    pub root: PathBuf,
    /// `index.theme` was readable and the kind-specific requirement holds.
    pub valid: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ThemeCatalog {
    entries: Vec<ThemeEntry>,
}

impl ThemeCatalog {
    /// Scan `~/.icons` and `<data_dir>/icons` for each entry of `data_dirs`
    /// (user data home first, then bundled prefixes). The first root wins per
    /// `(kind, id)`.
    #[must_use]
    pub fn discover(data_dirs: &[PathBuf]) -> Self {
        let mut seen: HashSet<(ThemeKind, String)> = HashSet::new();
        let mut entries = Vec::new();
        for root in search_roots(data_dirs) {
            let Ok(read) = fs::read_dir(&root) else {
                continue;
            };
            let mut dirs: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).collect();
            dirs.sort();
            for dir in dirs {
                if !dir.is_dir() {
                    continue;
                }
                let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                    continue;
                };
                if !dir.join("index.theme").is_file() {
                    continue;
                }
                let text = fs::read_to_string(dir.join("index.theme"));
                let readable = text.is_ok();
                let text = text.unwrap_or_default();
                let parsed = IconThemeIndex::parse(&id, &text);
                let name = parse_ini_value(&text, "Icon Theme", "Name").unwrap_or_else(|| id.clone());
                if !parsed.directories.is_empty() && seen.insert((ThemeKind::Icon, id.clone())) {
                    let valid = readable && parsed.directories.iter().any(|d| dir.join(d).is_dir());
                    entries.push(ThemeEntry {
                        id: id.clone(),
                        name: name.clone(),
                        kind: ThemeKind::Icon,
                        root: dir.clone(),
                        valid,
                    });
                }
                if dir.join("cursors").is_dir() && seen.insert((ThemeKind::Cursor, id.clone())) {
                    entries.push(ThemeEntry {
                        id: id.clone(),
                        name,
                        kind: ThemeKind::Cursor,
                        root: dir.clone(),
                        valid: readable,
                    });
                }
            }
        }
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[ThemeEntry] {
        &self.entries
    }

    #[must_use]
    pub fn find(&self, kind: ThemeKind, id: &str) -> Option<&ThemeEntry> {
        self.entries.iter().find(|e| e.kind == kind && e.id == id)
    }

    /// Cycle-safe icon inheritance chain for `id`, ending in `hicolor`.
    #[must_use]
    pub fn icon_chain(&self, id: &str) -> Vec<String> {
        inheritance_chain(id, &|name: &str| {
            self.find(ThemeKind::Icon, name)
                .and_then(|entry| IconThemeIndex::load(&entry.root))
        })
    }
}
