//! Pure view logic for the explorer: view modes, column sorting, type-ahead, status text.
//! No UI or filesystem writes here; callers pass in already-listed entries.
use std::path::Path;

use crate::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode { Details, Icons, Compact }

impl ViewMode {
    /// Ctrl+1 / Ctrl+2 / Ctrl+3 mapping.
    pub fn from_shortcut(digit: u8) -> Option<Self> {
        match digit {
            1 => Some(Self::Details),
            2 => Some(Self::Icons),
            3 => Some(Self::Compact),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column { Name, Size, Modified, Type }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnSort { pub column: Column, pub descending: bool }

impl Default for ColumnSort {
    fn default() -> Self { Self { column: Column::Name, descending: false } }
}

impl ColumnSort {
    /// Clicking the active column flips direction; clicking another column sorts it ascending.
    pub fn click(self, column: Column) -> Self {
        if self.column == column {
            Self { column, descending: !self.descending }
        } else {
            Self { column, descending: false }
        }
    }
}

/// Human type label used by the Type column and the Properties dialog.
pub fn type_label(entry: &Entry) -> String {
    if entry.symlink { return "Link".into(); }
    if entry.directory { return "Folder".into(); }
    match entry.path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{} file", ext.to_ascii_lowercase()),
        None => "File".into(),
    }
}

/// Folders always group first; the chosen column orders the rest; name breaks ties.
pub fn sort_entries(entries: &mut [Entry], sort: ColumnSort) {
    entries.sort_by(|a, b| {
        b.directory.cmp(&a.directory).then_with(|| {
            let order = match sort.column {
                Column::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Column::Size => a.size.cmp(&b.size),
                Column::Modified => a.modified.cmp(&b.modified),
                Column::Type => type_label(a).cmp(&type_label(b)),
            };
            if sort.descending { order.reverse() } else { order }
                .then_with(|| a.name.cmp(&b.name))
        })
    });
}

/// Index of the first name, starting at `from` and wrapping, whose lowercase form starts with `prefix`.
pub fn typeahead_match(names: &[&str], prefix: &str, from: usize) -> Option<usize> {
    let prefix = prefix.to_lowercase();
    let len = names.len();
    if prefix.is_empty() || len == 0 { return None; }
    (0..len)
        .map(|step| (from + step) % len)
        .find(|&i| names[i].to_lowercase().starts_with(&prefix))
}

/// Byte count as a short label with binary units (B, KiB, MiB, ...).
pub fn human_size(bytes: u64) -> String {
    let mut n = bytes as f64;
    let mut unit = "B";
    for candidate in ["KiB", "MiB", "GiB", "TiB"] {
        if n < 1024.0 { break; }
        n /= 1024.0;
        unit = candidate;
    }
    if unit == "B" { format!("{bytes} B") } else { format!("{n:.1} {unit}") }
}

/// Status bar text: selection count and total size, plus free space when the filesystem reports it.
pub fn status_text(selected: usize, selected_bytes: u64, total: usize, free_bytes: Option<u64>) -> String {
    let mut text = format!("{selected} of {total} selected");
    if selected > 0 {
        text.push_str(&format!(" · {}", human_size(selected_bytes)));
    }
    if let Some(free) = free_bytes {
        text.push_str(&format!(" · {} free", human_size(free)));
    }
    text
}

/// Free bytes available to unprivileged users on the filesystem holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, directory: bool, size: u64, secs: u64) -> Entry {
        Entry {
            path: PathBuf::from(name), name: name.into(), directory, symlink: false,
            size, modified: Some(UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    #[test]
    fn shortcut_maps_digits_to_modes() {
        assert_eq!(ViewMode::from_shortcut(1), Some(ViewMode::Details));
        assert_eq!(ViewMode::from_shortcut(2), Some(ViewMode::Icons));
        assert_eq!(ViewMode::from_shortcut(3), Some(ViewMode::Compact));
        assert_eq!(ViewMode::from_shortcut(4), None);
    }

    #[test]
    fn clicking_column_toggles_direction_then_resets_on_new_column() {
        let s = ColumnSort::default().click(Column::Name);
        assert_eq!(s, ColumnSort { column: Column::Name, descending: true });
        assert_eq!(s.click(Column::Size), ColumnSort { column: Column::Size, descending: false });
    }

    #[test]
    fn sort_keeps_folders_first_and_orders_by_column() {
        let mut v = vec![
            entry("b.txt", false, 30, 1),
            entry("zdir", true, 0, 5),
            entry("a.txt", false, 10, 9),
        ];
        sort_entries(&mut v, ColumnSort { column: Column::Size, descending: true });
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "b.txt", "a.txt"]);

        sort_entries(&mut v, ColumnSort { column: Column::Modified, descending: false });
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "b.txt", "a.txt"]);
    }

    #[test]
    fn type_label_uses_extension_and_kind() {
        assert_eq!(type_label(&entry("x", true, 0, 0)), "Folder");
        assert_eq!(type_label(&entry("Photo.JPG", false, 0, 0)), "jpg file");
        assert_eq!(type_label(&entry("README", false, 0, 0)), "File");
    }

    #[test]
    fn typeahead_is_case_insensitive_and_wraps() {
        let names = ["Apple", "banana", "Blueberry", "cherry"];
        assert_eq!(typeahead_match(&names, "b", 0), Some(1));
        assert_eq!(typeahead_match(&names, "b", 2), Some(2));
        assert_eq!(typeahead_match(&names, "ch", 3), Some(3));
        assert_eq!(typeahead_match(&names, "a", 1), Some(0));
        assert_eq!(typeahead_match(&names, "z", 0), None);
        assert_eq!(typeahead_match(&names, "", 0), None);
    }

    #[test]
    fn status_text_reports_selection_and_free_space() {
        assert_eq!(status_text(0, 0, 12, None), "0 of 12 selected");
        assert_eq!(status_text(2, 2048, 12, Some(3 * 1024 * 1024)),
            "2 of 12 selected · 2.0 KiB · 3.0 MiB free");
    }

    #[test]
    fn human_size_uses_binary_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
    }
}
