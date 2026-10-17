//! Data-only icon and cursor theme catalog. Discovery never executes or follows
//! `Inherits` beyond the cycle-safe chain helper shared with `icon_theme`.

mod archive;
pub mod cursor;
mod xz;
pub mod import;

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::icon_theme::{IconThemeIndex, inheritance_chain, parse_ini_value, search_roots};

const O_NOFOLLOW: i32 = if cfg!(target_arch = "aarch64") { 0o100_000 } else { 0o400_000 };
const O_NONBLOCK: i32 = 0o4_000;

/// Read a regular file without following a final symlink. The cap applies to
/// bytes actually read from the open handle, not to a prior `stat`. Reads at
/// most `limit + 1` bytes; `Ok(None)` means the file holds more than `limit`.
pub(crate) fn read_regular_capped(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let mut buf = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
    let fits = buf.len() as u64 <= limit;
    Ok(fits.then_some(buf))
}

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
                let Some(id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                    continue;
                };
                let index = dir.join("index.theme");
                if !dir.is_dir() || !index.is_file() {
                    continue;
                }
                let text = fs::read_to_string(&index);
                let readable = text.is_ok();
                let text = text.unwrap_or_default();
                let parsed = IconThemeIndex::parse(&id, &text);
                let name = parse_ini_value(&text, "Icon Theme", "Name").unwrap_or_else(|| id.clone());
                let mut push = |kind: ThemeKind, name: String, valid: bool| {
                    entries.push(ThemeEntry { id: id.clone(), name, kind, root: dir.clone(), valid });
                };
                if !parsed.directories.is_empty() && seen.insert((ThemeKind::Icon, id.clone())) {
                    let valid = readable && parsed.directories.iter().any(|d| dir.join(d).is_dir());
                    push(ThemeKind::Icon, name.clone(), valid);
                }
                if dir.join("cursors").is_dir() && seen.insert((ThemeKind::Cursor, id.clone())) {
                    push(ThemeKind::Cursor, name, readable);
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
