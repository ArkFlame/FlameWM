//! Pure view logic for the explorer: view modes, column sorting, type-ahead, status text.
//! No UI or filesystem writes here; callers pass in already-listed entries.
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

/// Folders always group first; the chosen column orders the rest; name breaks ties.
pub fn sort_entries(entries: &mut [Entry], sort: ColumnSort) {
    entries.sort_by(|a, b| column_order(a, b, sort));
}

/// True when `sort_entries` would not move any row. The UI thread uses it to skip a full re-sort
/// of a listing the worker already sorted in the same order (the common case: Name ascending).
#[must_use]
pub fn is_sorted_by(entries: &[Entry], sort: ColumnSort) -> bool {
    entries
        .windows(2)
        .all(|pair| column_order(&pair[0], &pair[1], sort) != std::cmp::Ordering::Greater)
}

pub(crate) fn column_order(a: &Entry, b: &Entry, sort: ColumnSort) -> std::cmp::Ordering {
    b.directory.cmp(&a.directory).then_with(|| {
        let order = match sort.column {
            Column::Name => a.cmp_lower_name(b),
            Column::Size => a.size.cmp(&b.size),
            Column::Modified => a.modified.cmp(&b.modified),
            // Comparing the label values instead of the formatted labels: "Type" used to format two
            // Strings per comparison (and two per adjacent pair in `is_sorted_by`).
            Column::Type => type_order(a).cmp(&type_order(b)),
        };
        if sort.descending {
            order.reverse()
        } else {
            order
        }
        .then_with(|| a.name.cmp(&b.name))
    })
}

/// Comparable form of the Type label that borrows the extension instead of formatting a String.
#[derive(PartialEq, Eq)]
enum TypeOrder<'a> {
    /// No extension: labelled "File".
    Plain,
    Folder,
    Link,
    Extension(&'a str),
}

impl TypeOrder<'_> {
    fn rank(&self) -> u8 {
        match self {
            Self::Plain => 0,
            Self::Folder => 1,
            Self::Link => 2,
            Self::Extension(_) => 3,
        }
    }
}

impl Ord for TypeOrder<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Extension(a), Self::Extension(b)) => cmp_ignore_ascii_case(a, b),
            // The rank order matches the byte order of the labels: "File" < "Folder" < "Link" < "<ext> file".
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for TypeOrder<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The value the removed Type label compared: kind for links and folders, the ASCII-lowercased
/// extension otherwise.
fn type_order(entry: &Entry) -> TypeOrder<'_> {
    if entry.symlink {
        return TypeOrder::Link;
    }
    if entry.directory {
        return TypeOrder::Folder;
    }
    match entry.path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => TypeOrder::Extension(ext),
        None => TypeOrder::Plain,
    }
}

/// Orders two strings as their `to_ascii_lowercase` forms would, without allocating either copy.
fn cmp_ignore_ascii_case(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(b.bytes().map(|byte| byte.to_ascii_lowercase()))
}

/// Same wrap-and-match over the listing itself. Typeahead on the projection path has no filter
/// active, so it reads `Entry::name` in place instead of collecting every name into a `Vec<&str>`
/// (an 800 KB allocation per typed character in a 100k-row folder).
#[must_use]
pub fn typeahead_match_entries(entries: &[Entry], prefix: &str, from: usize) -> Option<usize> {
    typeahead_match_iter(
        entries.iter().map(|entry| entry.name.as_str()),
        entries.len(),
        prefix,
        from,
    )
}

/// Wrap-and-match over any name iterator, so a filtered listing can be searched without first
/// collecting its names. `len` is how many names `names` yields (a caller with a filter walks the
/// same predicate twice instead of materialising a `Vec<&str>`); matching starts at `from` and wraps.
#[must_use]
pub fn typeahead_match_iter<'a, I>(names: I, len: usize, prefix: &str, from: usize) -> Option<usize>
where
    I: Iterator<Item = &'a str>,
{
    let prefix = prefix.to_lowercase();
    if prefix.is_empty() || len == 0 {
        return None;
    }
    let from = from.min(len);
    // A match before `from` is only used when the wrapped scan finds nothing at or after it.
    let mut wrap = None;
    for (index, name) in names.enumerate() {
        if starts_with_lowercase(name, &prefix) {
            if index >= from {
                return Some(index);
            }
            wrap.get_or_insert(index);
        }
    }
    wrap
}

/// `name.to_lowercase().starts_with(prefix)` without allocating a lowercase copy of every name.
/// `prefix` must already be lowercase. Typeahead runs this over the whole listing on each keystroke.
fn starts_with_lowercase(name: &str, prefix: &str) -> bool {
    let mut lowered = name.chars().flat_map(char::to_lowercase);
    prefix
        .chars()
        .all(|expected| lowered.next() == Some(expected))
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

/// Free and total bytes of the filesystem holding `path`. A worker-only blocking `statvfs`:
/// `statvfs` can stall for seconds on a stale network mount, so the UI thread never calls it (it
/// uses `SpaceCache` instead).
#[must_use]
pub fn fs_space(path: &Path) -> Option<(u64, u64)> {
    statvfs_space(path)
}

/// A sample is reused for this long before a worker refreshes it.
pub const SPACE_TTL: Duration = Duration::from_secs(2);

/// UI-side free-space cache. It never reads the filesystem: `get` returns the last worker sample
/// and `request` names the path a worker should sample next, so a projection on a slow mount cannot
/// block. Free counts only space open to unprivileged users.
#[derive(Default)]
pub struct SpaceCache {
    /// Last sample: the path it describes, when it was taken, and its result.
    #[allow(clippy::type_complexity)]
    sample: Option<(PathBuf, Instant, Option<(u64, u64)>)>,
    /// The path a worker is sampling, with the instant of the request: a lost result self-heals
    /// after `SPACE_TTL` instead of blocking refreshes forever.
    pending: Option<(PathBuf, Instant)>,
}

impl SpaceCache {
    /// Free/total bytes for `path` from the last sample, or `None` when the sample is for another
    /// path (or failed). A sample older than `SPACE_TTL` is still returned so the bar never blanks
    /// while its refresh is in flight.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<(u64, u64)> {
        self.sample
            .as_ref()
            .filter(|(cached, _, _)| cached.as_path() == path)
            .and_then(|(_, _, value)| *value)
    }

    /// The path to sample on a worker when the sample is missing, stale or for another path and no
    /// fresh request is already in flight. Marks the request.
    pub fn request(&mut self, path: &Path, now: Instant) -> Option<PathBuf> {
        let fresh = self.sample.as_ref().is_some_and(|(cached, taken, _)| {
            cached.as_path() == path && now.saturating_duration_since(*taken) < SPACE_TTL
        });
        if fresh {
            return None;
        }
        if let Some((pending, asked)) = self.pending.as_ref()
            && pending.as_path() == path
            && now.saturating_duration_since(*asked) < SPACE_TTL
        {
            return None;
        }
        self.pending = Some((path.to_path_buf(), now));
        Some(path.to_path_buf())
    }

    /// Stores a worker's sample and clears the matching in-flight marker.
    pub fn store(&mut self, path: PathBuf, value: Option<(u64, u64)>, now: Instant) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(asked, _)| asked == &path)
        {
            self.pending = None;
        }
        self.sample = Some((path, now, value));
    }

    /// Drops a failed request's in-flight marker so the next projection retries immediately.
    pub fn release(&mut self, path: &Path) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(asked, _)| asked.as_path() == path)
        {
            self.pending = None;
        }
    }
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

    #[test]
    fn cache_serves_one_sample_per_path_and_requests_a_refresh_after_the_ttl() {
        let start = Instant::now();
        let a = Path::new("/a");
        let mut cache = SpaceCache::default();
        // Empty cache: nothing to show, and one request for the visible path.
        assert_eq!(cache.get(a), None);
        assert_eq!(cache.request(a, start), Some(a.to_path_buf()));
        // A request is already in flight, so a second frame asks nothing.
        assert_eq!(cache.request(a, start + Duration::from_millis(500)), None);
        // The worker result fills the cache and clears the in-flight marker.
        cache.store(
            a.to_path_buf(),
            Some((1, 2)),
            start + Duration::from_millis(600),
        );
        assert_eq!(cache.get(a), Some((1, 2)));
        assert_eq!(cache.get(Path::new("/b")), None);
        // A fresh sample needs no request until the TTL expires after it was taken.
        assert_eq!(cache.request(a, start + Duration::from_millis(700)), None);
        assert_eq!(
            cache.request(a, start + Duration::from_millis(600) + SPACE_TTL),
            Some(a.to_path_buf())
        );
    }

    #[test]
    fn a_lost_request_self_heals_after_the_ttl() {
        let start = Instant::now();
        let a = Path::new("/a");
        let mut cache = SpaceCache::default();
        assert_eq!(cache.request(a, start), Some(a.to_path_buf()));
        assert_eq!(cache.request(a, start + SPACE_TTL), Some(a.to_path_buf()));
        cache.release(a);
        assert_eq!(cache.request(a, start + SPACE_TTL), Some(a.to_path_buf()));
    }

    #[test]
    fn a_stale_value_stays_visible_while_its_refresh_is_requested() {
        let start = Instant::now();
        let a = Path::new("/a");
        let mut cache = SpaceCache::default();
        cache.store(a.to_path_buf(), Some((3, 4)), start);
        assert_eq!(cache.get(a), Some((3, 4)));
        assert_eq!(cache.request(a, start + SPACE_TTL), Some(a.to_path_buf()));
        assert_eq!(cache.get(a), Some((3, 4)));
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
    fn type_column_orders_by_kind_then_extension() {
        let mut entries = vec![
            entry("Photo.JPG", false, 0, 0),
            entry("README", false, 0, 0),
            entry("zdir", true, 0, 0),
            entry("a.txt", false, 0, 0),
        ];
        sort_entries(
            &mut entries,
            ColumnSort {
                column: Column::Type,
                descending: false,
            },
        );
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        // Folders first, then "File" (no extension), then the lowercased extension labels in order.
        assert_eq!(names, ["zdir", "README", "Photo.JPG", "a.txt"]);
    }

    #[test]
    fn typeahead_is_case_insensitive_and_wraps() {
        let names = ["Apple", "banana", "Blueberry", "cherry"];
        let matched = |prefix: &str, from: usize| {
            typeahead_match_iter(names.iter().copied(), names.len(), prefix, from)
        };
        assert_eq!(matched("b", 0), Some(1));
        assert_eq!(matched("b", 2), Some(2));
        assert_eq!(matched("ch", 3), Some(3));
        assert_eq!(matched("a", 1), Some(0));
        assert_eq!(matched("z", 0), None);
        assert_eq!(matched("", 0), None);
    }

    #[test]
    fn starts_with_lowercase_matches_allocating_form() {
        for (name, prefix) in [
            ("Apple", "ap"),
            ("Apple", "apple!"),
            ("ÉCOLE", "éc"),
            ("straße", "straß"),
            ("x", ""),
            ("", "a"),
        ] {
            assert_eq!(
                starts_with_lowercase(name, prefix),
                name.to_lowercase().starts_with(prefix),
                "{name:?} vs {prefix:?}"
            );
        }
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
}
