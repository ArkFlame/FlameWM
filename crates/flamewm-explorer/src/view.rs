//! Pure view logic for the explorer: view modes, column sorting, type-ahead, status text.
//! No UI or filesystem writes here; callers pass in already-listed entries.
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, TimeZone};

use crate::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Size,
    Modified,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnSort {
    pub column: Column,
    pub descending: bool,
}

impl Default for ColumnSort {
    fn default() -> Self {
        Self {
            column: Column::Name,
            descending: false,
        }
    }
}

impl ColumnSort {
    /// Clicking the active column flips direction; clicking another column sorts it ascending.
    #[must_use]
    pub fn click(self, column: Column) -> Self {
        if self.column == column {
            Self {
                column,
                descending: !self.descending,
            }
        } else {
            Self {
                column,
                descending: false,
            }
        }
    }
}

/// Human type label used by the Type column and the Properties dialog.
#[must_use]
pub fn type_label(entry: &Entry) -> String {
    if entry.symlink {
        return "Link".into();
    }
    if entry.directory {
        return "Folder".into();
    }
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
                Column::Name => a.lower_name.cmp(&b.lower_name),
                Column::Size => a.size.cmp(&b.size),
                Column::Modified => a.modified.cmp(&b.modified),
                Column::Type => type_label(a).cmp(&type_label(b)),
            };
            if sort.descending {
                order.reverse()
            } else {
                order
            }
            .then_with(|| a.name.cmp(&b.name))
        })
    });
}

/// Index of the first name, starting at `from` and wrapping, whose lowercase form starts with `prefix`.
#[must_use]
pub fn typeahead_match(names: &[&str], prefix: &str, from: usize) -> Option<usize> {
    let prefix = prefix.to_lowercase();
    let len = names.len();
    if prefix.is_empty() || len == 0 {
        return None;
    }
    (0..len)
        .map(|step| (from + step) % len)
        .find(|&i| names[i].to_lowercase().starts_with(&prefix))
}

/// Byte count as a short label with binary units (B, KiB, MiB, ...).
#[must_use]
pub fn human_size(bytes: u64) -> String {
    let mut n = bytes as f64;
    let mut unit = "B";
    for candidate in ["KiB", "MiB", "GiB", "TiB"] {
        if n < 1024.0 {
            break;
        }
        n /= 1024.0;
        unit = candidate;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{n:.1} {unit}")
    }
}

/// Status strip count: "12 items", plus the selection count when something is selected.
#[must_use]
pub fn items_text(total: usize, selected: usize) -> String {
    let noun = if total == 1 { "item" } else { "items" };
    if selected == 0 {
        format!("{total} {noun}")
    } else {
        format!("{total} {noun} · {selected} selected")
    }
}

/// Last statvfs sample: the path it describes, when it was taken, and its result.
type SpaceSample = (PathBuf, Instant, Option<(u64, u64)>);
/// Projections run on every wake, so statvfs is re-read at most once per TTL for the same path.
const SPACE_TTL: Duration = Duration::from_secs(2);
thread_local! {
    static SPACE: RefCell<Option<SpaceSample>> = const { RefCell::new(None) };
}

/// Free and total bytes of the filesystem holding `path`, cached for `SPACE_TTL`.
/// Free counts only space open to unprivileged users.
#[must_use]
pub fn fs_space(path: &Path) -> Option<(u64, u64)> {
    SPACE.with(|slot| cached_space(&mut slot.borrow_mut(), path, Instant::now(), statvfs_space))
}

fn cached_space(
    slot: &mut Option<SpaceSample>,
    path: &Path,
    now: Instant,
    read: impl FnOnce(&Path) -> Option<(u64, u64)>,
) -> Option<(u64, u64)> {
    if let Some((cached, taken, value)) = slot.as_ref() {
        if cached.as_path() == path && now.duration_since(*taken) < SPACE_TTL {
            return *value;
        }
    }
    let value = read(path);
    *slot = Some((path.to_path_buf(), now, value));
    value
}

fn statvfs_space(path: &Path) -> Option<(u64, u64)> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some((
        stat.f_bavail.saturating_mul(stat.f_frsize),
        stat.f_blocks.saturating_mul(stat.f_frsize),
    ))
}

#[cfg(test)]
mod space_cache_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn statvfs_runs_once_per_ttl_and_per_path() {
        let calls = Cell::new(0);
        let read = |_: &Path| {
            calls.set(calls.get() + 1);
            Some((1, 2))
        };
        let start = Instant::now();
        let mut slot = None;
        let a = Path::new("/a");
        assert_eq!(cached_space(&mut slot, a, start, read), Some((1, 2)));
        assert_eq!(
            cached_space(&mut slot, a, start + Duration::from_millis(500), read),
            Some((1, 2))
        );
        assert_eq!(calls.get(), 1);
        cached_space(&mut slot, a, start + SPACE_TTL, read);
        assert_eq!(calls.get(), 2);
        cached_space(&mut slot, Path::new("/b"), start + SPACE_TTL, read);
        assert_eq!(calls.get(), 3);
    }
}

/// Extensions shown as "Source file" with the code icon.
const SOURCE_EXTENSIONS: [&str; 30] = [
    "c", "h", "cc", "cpp", "cxx", "hpp", "rs", "go", "java", "kt", "swift", "py", "rb", "php",
    "lua", "js", "mjs", "ts", "tsx", "jsx", "sh", "bash", "zsh", "fish", "toml", "yaml", "yml",
    "json", "html", "md",
];

fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// Row icon family: folder, pdf, source or generic file. Names match `ui/explorer/icons/row-*.svg`.
///
/// Image, SVG, ICO and `.desktop` rows keep the `file` glyph and get a real thumbnail instead (see
/// `thumbs.rs`); the glyph is what the slot shows whenever that thumbnail is missing or fails, so it
/// must always name one of the compiled families.
#[must_use]
pub fn file_icon(entry: &Entry) -> &'static str {
    if entry.directory {
        return "folder";
    }
    match extension_lower(&entry.path) {
        Some(ext) if ext == "pdf" => "pdf",
        Some(ext) if SOURCE_EXTENSIONS.contains(&ext.as_str()) => "source",
        _ => "file",
    }
}

/// Kind column text, matching the icon family. A `.desktop` launcher reads as an application because
/// its row shows the application's own icon.
#[must_use]
pub fn kind_name(entry: &Entry) -> String {
    if entry.directory {
        return "Folder".into();
    }
    if entry.symlink {
        return "Link".into();
    }
    match extension_lower(&entry.path) {
        Some(ext) if ext == "pdf" => "PDF Document".into(),
        Some(ext) if ext == "desktop" => "Application".into(),
        Some(ext) if SOURCE_EXTENSIONS.contains(&ext.as_str()) => "Source file".into(),
        Some(ext) => format!("{} File", ext.to_ascii_uppercase()),
        None => "File".into(),
    }
}

/// Modified column: "Today, 10:24", "Yesterday", or an ISO date. `now` carries the local zone.
pub fn date_label<Tz: TimeZone>(modified: SystemTime, now: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Ok(since) = modified.duration_since(UNIX_EPOCH) else {
        return "—".into();
    };
    let Some(utc) = DateTime::from_timestamp(since.as_secs() as i64, since.subsec_nanos()) else {
        return "—".into();
    };
    let when = utc.with_timezone(&now.timezone());
    let day = when.date_naive();
    let today = now.date_naive();
    if day == today {
        format!("Today, {}", when.format("%H:%M"))
    } else if Some(day) == today.pred_opt() {
        "Yesterday".into()
    } else {
        day.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, directory: bool, size: u64, secs: u64) -> Entry {
        Entry::new(
            PathBuf::from(name),
            name.into(),
            directory,
            false,
            size,
            Some(UNIX_EPOCH + Duration::from_secs(secs)),
        )
    }

    #[test]
    fn clicking_column_toggles_direction_then_resets_on_new_column() {
        let s = ColumnSort::default().click(Column::Name);
        assert_eq!(
            s,
            ColumnSort {
                column: Column::Name,
                descending: true
            }
        );
        assert_eq!(
            s.click(Column::Size),
            ColumnSort {
                column: Column::Size,
                descending: false
            }
        );
    }

    #[test]
    fn sort_keeps_folders_first_and_orders_by_column() {
        let mut v = vec![
            entry("b.txt", false, 30, 1),
            entry("zdir", true, 0, 5),
            entry("a.txt", false, 10, 9),
        ];
        sort_entries(
            &mut v,
            ColumnSort {
                column: Column::Size,
                descending: true,
            },
        );
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "b.txt", "a.txt"]);

        sort_entries(
            &mut v,
            ColumnSort {
                column: Column::Modified,
                descending: false,
            },
        );
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
    fn items_text_counts_items_and_selection() {
        assert_eq!(items_text(12, 0), "12 items");
        assert_eq!(items_text(12, 1), "12 items · 1 selected");
        assert_eq!(items_text(1, 1), "1 item · 1 selected");
    }

    #[test]
    fn file_icons_and_kind_names_follow_extension() {
        assert_eq!(file_icon(&entry("Design", true, 0, 0)), "folder");
        assert_eq!(file_icon(&entry("Roadmap.PDF", false, 0, 0)), "pdf");
        assert_eq!(file_icon(&entry("README.md", false, 0, 0)), "source");
        assert_eq!(file_icon(&entry("photo.jpg", false, 0, 0)), "file");
        // Rows that get a decoded thumbnail keep the file glyph as their fallback.
        assert_eq!(file_icon(&entry("clip.svg", false, 0, 0)), "file");
        assert_eq!(file_icon(&entry("app.ico", false, 0, 0)), "file");
        assert_eq!(file_icon(&entry("app.desktop", false, 0, 0)), "file");
        assert_eq!(
            kind_name(&entry("Roadmap.pdf", false, 0, 0)),
            "PDF Document"
        );
        assert_eq!(kind_name(&entry("README.md", false, 0, 0)), "Source file");
        assert_eq!(kind_name(&entry("photo.jpg", false, 0, 0)), "JPG File");
        assert_eq!(
            kind_name(&entry("firefox.desktop", false, 0, 0)),
            "Application"
        );
        assert_eq!(kind_name(&entry("LICENSE", false, 0, 0)), "File");
    }

    #[test]
    fn modified_labels_are_relative_for_today_and_yesterday() {
        use chrono::{Duration as ChronoDuration, Utc};
        let now = Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0).unwrap();
        let at = |t: DateTime<Utc>| -> SystemTime { t.into() };
        assert_eq!(
            date_label(
                at(Utc.with_ymd_and_hms(2026, 10, 21, 10, 24, 0).unwrap()),
                now
            ),
            "Today, 10:24"
        );
        assert_eq!(
            date_label(at(now - ChronoDuration::days(1)), now),
            "Yesterday"
        );
        assert_eq!(
            date_label(at(Utc.with_ymd_and_hms(2026, 10, 1, 9, 0, 0).unwrap()), now),
            "2026-10-01"
        );
    }

    #[test]
    fn human_size_uses_binary_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
    }
}
