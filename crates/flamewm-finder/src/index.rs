//! Bounded filename index. A breadth-first walk of the roots is limited by entry count, depth and
//! wall-clock budget; symlinks are never followed and hidden or dependency folders are skipped. The
//! last walk is cached on disk so the next launch can search before the new walk finishes.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const MAX_ENTRIES: usize = 250_000;
pub const MAX_DEPTH: usize = 12;
pub const WALK_BUDGET: Duration = Duration::from_secs(20);
/// Number of most recently modified files kept for the empty-query view.
pub const RECENT_LIMIT: usize = 50;

const CACHE_HEADER: &str = "flamewm-finder-index 1";
const SKIPPED_DIRS: [&str; 3] = ["node_modules", "target", "__pycache__"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// Lowercase copy of `path`, used for matching.
    pub lower: String,
    /// Byte offset of the file name inside `lower`.
    pub name_at: usize,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
}

impl Entry {
    #[must_use]
    pub fn new(path: String, is_dir: bool, size: u64, modified: i64) -> Self {
        let lower = path.to_lowercase();
        let name_at = lower.rfind('/').map_or(0, |slash| slash + 1);
        Self {
            path,
            lower,
            name_at,
            is_dir,
            size,
            modified,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Index {
    pub entries: Vec<Entry>,
    /// Indices of the newest files, newest first.
    pub recent: Vec<usize>,
    /// True when the walk stopped at a limit instead of finishing.
    pub truncated: bool,
    /// Recently used document paths, newest use first, as listed by the desktop's recent files.
    pub doc_paths: Vec<String>,
    /// Indices of `doc_paths` that are in `entries`, in the same order.
    pub docs: Vec<usize>,
}

impl Index {
    #[must_use]
    pub fn new(entries: Vec<Entry>, truncated: bool) -> Self {
        let mut index = Self {
            entries,
            truncated,
            ..Self::default()
        };
        index.finish();
        index
    }

    /// Replaces the recently used documents and resolves them against the entries.
    pub fn set_docs(&mut self, paths: Vec<String>) {
        self.doc_paths = paths;
        self.finish();
    }

    /// Recomputes the newest-files and recent-document views after `entries` changed.
    pub fn finish(&mut self) {
        let entries = &self.entries;
        let newest = |&a: &usize, &b: &usize| {
            entries[b]
                .modified
                .cmp(&entries[a].modified)
                .then_with(|| entries[a].path.cmp(&entries[b].path))
        };
        let mut recent: Vec<usize> = (0..entries.len()).filter(|&i| !entries[i].is_dir).collect();
        if recent.len() > RECENT_LIMIT {
            recent.select_nth_unstable_by(RECENT_LIMIT - 1, newest);
            recent.truncate(RECENT_LIMIT);
        }
        recent.sort_unstable_by(newest);
        self.recent = recent;

        self.docs.clear();
        if !self.doc_paths.is_empty() {
            let position: HashMap<&str, usize> = entries
                .iter()
                .enumerate()
                .map(|(i, entry)| (entry.path.as_str(), i))
                .collect();
            self.docs = self
                .doc_paths
                .iter()
                .filter_map(|path| position.get(path.as_str()).copied())
                .take(RECENT_LIMIT)
                .collect();
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// True for names the index never stores: hidden entries and dependency folders.
#[must_use]
pub fn accept_name(name: &str) -> bool {
    !name.starts_with('.') && !SKIPPED_DIRS.contains(&name)
}

fn entry_from(text: &str, is_dir: bool, meta: &fs::Metadata) -> Entry {
    let size = if is_dir { 0 } else { meta.len() };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX));
    Entry::new(text.to_owned(), is_dir, size, modified)
}

/// The entry for one path, with the same exclusions as [`walk`]. Symlinks are never indexed.
#[must_use]
pub fn stat(path: &Path) -> Option<Entry> {
    let meta = fs::symlink_metadata(path).ok()?;
    let file_type = meta.file_type();
    if file_type.is_symlink() || !accept_name(path.file_name()?.to_str()?) {
        return None;
    }
    let text = path.to_str()?;
    if text.contains(['\t', '\n']) {
        return None;
    }
    Some(entry_from(text, file_type.is_dir(), &meta))
}

/// The index shared by the window and its watcher. The worker is the only writer; the window
/// reads it for each paint and search, so the lock is held only for that short work.
#[derive(Clone, Default)]
pub struct Shared(Arc<RwLock<Index>>);

impl Shared {
    pub fn read(&self) -> RwLockReadGuard<'_, Index> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, Index> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Walks `roots` breadth-first. Stops at `max_entries`, `max_depth` or `budget`, whichever comes first.
#[must_use]
pub fn walk(roots: &[PathBuf], max_entries: usize, max_depth: usize, budget: Duration) -> Index {
    let deadline = Instant::now() + budget;
    let mut queue: VecDeque<(PathBuf, usize)> =
        roots.iter().map(|root| (root.clone(), 0)).collect();
    let mut entries = Vec::new();
    let mut truncated = false;
    'walk: while let Some((dir, depth)) = queue.pop_front() {
        if Instant::now() >= deadline {
            truncated = true;
            break;
        }
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            if Instant::now() >= deadline || entries.len() >= max_entries {
                truncated = true;
                break 'walk;
            }
            let name = item.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !accept_name(name) {
                continue;
            }
            let Ok(file_type) = item.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = item.path();
            let Some(text) = path.to_str() else {
                continue;
            };
            // The cache is tab and newline separated, so such names cannot be stored.
            if text.contains(['\t', '\n']) {
                continue;
            }
            let is_dir = file_type.is_dir();
            entries.push(match item.metadata() {
                Ok(meta) => entry_from(text, is_dir, &meta),
                Err(_) => Entry::new(text.to_owned(), is_dir, 0, 0),
            });
            if entries.len() >= max_entries {
                truncated = true;
                break 'walk;
            }
            if is_dir && depth + 1 < max_depth {
                queue.push_back((path, depth + 1));
            }
        }
    }
    Index::new(entries, truncated)
}

/// Groups of two or more files with the same name and size, largest first, at most `limit` groups.
/// Each group lists its entries in path order. Empty files and folders are not duplicates.
#[must_use]
pub fn duplicate_groups(index: &Index, limit: usize) -> Vec<Vec<usize>> {
    let mut by_key: std::collections::BTreeMap<(&str, u64), Vec<usize>> =
        std::collections::BTreeMap::new();
    for (position, entry) in index.entries.iter().enumerate() {
        if entry.is_dir || entry.size == 0 {
            continue;
        }
        by_key
            .entry((entry.name(), entry.size))
            .or_default()
            .push(position);
    }
    let mut groups: Vec<Vec<usize>> = by_key
        .into_values()
        .filter(|group| group.len() >= 2)
        .map(|mut group| {
            group.sort_by(|a, b| index.entries[*a].path.cmp(&index.entries[*b].path));
            group
        })
        .collect();
    groups.sort_by(|a, b| {
        index.entries[b[0]]
            .size
            .cmp(&index.entries[a[0]].size)
            .then_with(|| index.entries[a[0]].path.cmp(&index.entries[b[0]].path))
    });
    groups.truncate(limit);
    groups
}

/// Home folder, the only root of the default index.
#[must_use]
pub fn default_roots(home: Option<OsString>) -> Vec<PathBuf> {
    home.map(PathBuf::from).into_iter().collect()
}

/// `$XDG_CACHE_HOME/flamewm-finder/index.tsv`, else under `$HOME/.cache`.
#[must_use]
pub fn cache_path(cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = cache_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("flamewm-finder").join("index.tsv"))
}

/// Reads a cache written by [`save_cache`]. A missing or foreign file yields `None`.
#[must_use]
pub fn load_cache(path: &Path) -> Option<Index> {
    let file = fs::File::open(path).ok()?;
    let mut lines = io::BufReader::new(file).lines();
    if lines.next()?.ok()? != CACHE_HEADER {
        return None;
    }
    let mut entries = Vec::new();
    for line in lines {
        if entries.len() >= MAX_ENTRIES {
            return None;
        }
        let line = line.ok()?;
        let mut fields = line.splitn(4, '\t');
        let is_dir = fields.next()? == "d";
        let modified = fields.next()?.parse().ok()?;
        let size = fields.next()?.parse().ok()?;
        let path = fields.next()?.to_owned();
        entries.push(Entry::new(path, is_dir, size, modified));
    }
    Some(Index::new(entries, false))
}

/// Writes the index through a temporary file so a reader never sees a half-written cache.
pub fn save_cache(path: &Path, index: &Index) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tsv.tmp");
    {
        let mut out = io::BufWriter::new(fs::File::create(&temp)?);
        writeln!(out, "{CACHE_HEADER}")?;
        for entry in &index.entries {
            writeln!(
                out,
                "{}\t{}\t{}\t{}",
                if entry.is_dir { "d" } else { "f" },
                entry.modified,
                entry.size,
                entry.path
            )?;
        }
        out.flush()?;
    }
    fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_groups_pair_same_name_and_size_largest_first() {
        let entries = vec![
            Entry::new("/h/a/notes.txt".into(), false, 10, 0),
            Entry::new("/h/b/notes.txt".into(), false, 10, 0),
            Entry::new("/h/c/notes.txt".into(), false, 11, 0),
            Entry::new("/h/a/big.bin".into(), false, 900, 0),
            Entry::new("/h/b/big.bin".into(), false, 900, 0),
            Entry::new("/h/a/empty".into(), false, 0, 0),
            Entry::new("/h/b/empty".into(), false, 0, 0),
            Entry::new("/h/a/dir".into(), true, 5, 0),
            Entry::new("/h/b/dir".into(), true, 5, 0),
        ];
        let index = Index::new(entries, false);
        let groups = duplicate_groups(&index, 10);
        let names: Vec<Vec<&str>> = groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|i| index.entries[*i].path.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                vec!["/h/a/big.bin", "/h/b/big.bin"],
                vec!["/h/a/notes.txt", "/h/b/notes.txt"],
            ]
        );
        assert_eq!(duplicate_groups(&index, 1).len(), 1);
    }

    use super::*;

    fn entry(path: &str, is_dir: bool, modified: i64) -> Entry {
        Entry::new(path.to_owned(), is_dir, 10, modified)
    }

    #[test]
    fn entry_splits_name_on_last_separator_in_lowercase() {
        let item = entry("/home/Ann/Notes/Report.TXT", false, 0);
        assert_eq!(item.name(), "Report.TXT");
        assert_eq!(&item.lower[item.name_at..], "report.txt");
    }

    #[test]
    fn name_handles_case_mapping_that_expands_in_parent_directory() {
        let item = entry("/home/İ/file.txt", false, 0);
        assert_eq!(item.name(), "file.txt");
        assert_eq!(&item.lower[item.name_at..], "file.txt");
    }

    #[test]
    fn walk_with_zero_entry_limit_returns_no_entries() {
        let root = std::env::temp_dir();
        let index = walk(&[root], 0, 1, Duration::from_secs(5));
        assert!(index.is_empty());
    }

    #[test]
    fn recent_lists_newest_files_only() {
        let index = Index::new(
            vec![
                entry("/h/old.txt", false, 1),
                entry("/h/dir", true, 99),
                entry("/h/new.txt", false, 5),
            ],
            false,
        );
        let names: Vec<&str> = index
            .recent
            .iter()
            .map(|&i| index.entries[i].name())
            .collect();
        assert_eq!(names, ["new.txt", "old.txt"]);
    }

    #[test]
    fn walk_respects_entry_limit_and_skips_hidden_and_symlinks() {
        let root = std::env::temp_dir().join(format!("finder-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("visible/deep")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::write(root.join("visible/a.txt"), b"abc").unwrap();
        fs::write(root.join("visible/deep/b.txt"), b"abcd").unwrap();
        fs::write(root.join(".hidden/c.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("visible"), root.join("link")).unwrap();

        let full = walk(std::slice::from_ref(&root), 100, 8, Duration::from_secs(5));
        let mut names: Vec<&str> = full.entries.iter().map(Entry::name).collect();
        names.sort_unstable();
        assert_eq!(names, ["a.txt", "b.txt", "deep", "visible"]);
        assert!(!full.truncated);

        let capped = walk(std::slice::from_ref(&root), 2, 8, Duration::from_secs(5));
        assert_eq!(capped.len(), 2);
        assert!(capped.truncated);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_round_trips_and_rejects_foreign_files() {
        let dir = std::env::temp_dir().join(format!("finder-cache-{}", std::process::id()));
        let path = dir.join("index.tsv");
        let index = Index::new(
            vec![entry("/h/dir", true, 3), entry("/h/x y.txt", false, 4)],
            false,
        );
        save_cache(&path, &index).unwrap();
        let loaded = load_cache(&path).unwrap();
        assert_eq!(loaded.entries, index.entries);
        fs::write(&path, "something else\n").unwrap();
        assert!(load_cache(&path).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_rejects_more_than_max_entries() {
        let dir = std::env::temp_dir().join(format!("finder-cache-limit-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.tsv");
        let mut file = io::BufWriter::new(fs::File::create(&path).unwrap());
        writeln!(file, "{CACHE_HEADER}").unwrap();
        for _ in 0..=MAX_ENTRIES {
            writeln!(file, "f\t0\t0\t/a").unwrap();
        }
        drop(file);
        assert!(load_cache(&path).is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_path_prefers_xdg_cache_home() {
        assert_eq!(
            cache_path(Some("/c".into()), Some("/home/u".into())),
            Some(PathBuf::from("/c/flamewm-finder/index.tsv"))
        );
        assert_eq!(
            cache_path(Some(OsString::new()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.cache/flamewm-finder/index.tsv"))
        );
        assert_eq!(cache_path(None, None), None);
    }
}

/// Resident set size in KiB from `/proc/self/status`; zero where it is unavailable.
#[cfg(test)]
fn resident_kib() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))
                .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        })
        .unwrap_or(0)
}

/// Manual benchmark over a synthetic tree. Run with
/// `FLAMEWM_FINDER_BENCH_ROOT=<dir> cargo test -p flamewm-finder bench_index -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore = "benchmark: set FLAMEWM_FINDER_BENCH_ROOT"]
fn bench_index_walk_and_first_query() {
    let Some(root) = std::env::var_os("FLAMEWM_FINDER_BENCH_ROOT") else {
        return;
    };
    let roots = vec![PathBuf::from(root)];
    let before = resident_kib();
    let started = Instant::now();
    let index = walk(&roots, MAX_ENTRIES, MAX_DEPTH, WALK_BUDGET);
    let walk_ms = started.elapsed().as_millis();
    let after = resident_kib();
    let cache = std::env::temp_dir().join(format!("finder-bench-{}.tsv", std::process::id()));
    save_cache(&cache, &index).unwrap();

    // First result on a warm start: read the cache, then rank one query, as the window does.
    let started = Instant::now();
    let cached = load_cache(&cache).unwrap();
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let query = crate::search::Query::parse("report", "/home/bench");
    let started = Instant::now();
    let hits = crate::search::rank(&cached, &query, 1_700_000_000);
    let rank_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "bench entries={} truncated={} walk_ms={walk_ms} index_rss_delta_kib={} rss_kib={after} cache_load_ms={load_ms:.2} first_query_rank_ms={rank_ms:.2} matches={}",
        index.len(),
        index.truncated,
        after.saturating_sub(before),
        hits.total
    );
    let _ = fs::remove_file(cache);
}
