//! Icon lookup index: candidate probing and build counters.

use super::*;

pub(super) fn directory_distance(
    index: &IconLookupIndex,
    file: IndexedIconFile,
    physical: u32,
) -> u64 {
    let meta = index.files().meta(file);
    let dirs = index
        .ordered_themes
        .get(meta.theme_rank as usize)
        .and_then(|theme| index.indexes().get(theme)?.as_ref());
    dirs.and_then(|d| d.dirs.get(d.directories.get(meta.directory_rank as usize)?))
        .map_or(u64::MAX / 2, |e| e.distance(physical))
}

/// Directory scan result: base icon name -> files found by reading each
/// declared directory once, plus pixmaps (sibling `pixmaps` and
/// `/usr/share/pixmaps`).
#[derive(Debug, Default)]
pub(super) struct IconScan {
    /// Parsed `index.theme` per theme (`None` = no index on disk).
    pub(super) indexes: HashMap<String, Option<IconThemeIndex>>,
    pub(super) files: IconFileIndex,
    pub(super) pixmaps: HashMap<String, Vec<PathBuf>>,
}

#[derive(Debug)]
pub struct IconLookupIndex {
    /// Theme generation this snapshot was built for.
    pub generation: u64,
    /// Deduped search roots (`$HOME/.icons`, `$XDG_DATA_HOME/icons`, ...).
    pub roots: Vec<PathBuf>,
    /// Ordered theme chain (seeds + inheritance, Breeze before hicolor).
    pub ordered_themes: Vec<String>,
    /// Per-theme roots that actually carry the theme directory.
    pub theme_roots: HashMap<String, Vec<PathBuf>>,
    /// Parsed `index.theme` per theme, the real name -> existing-files index
    /// and pixmaps, all produced on the first lookup so processes that never
    /// resolve a themed icon keep no copy.
    pub(super) scan: std::sync::OnceLock<IconScan>,
    /// Top-level roots `mtime` snapshot + last check (5s cooldown owner).
    pub watch_mtime: Option<std::time::SystemTime>,
    pub last_mtime_check: std::sync::Mutex<Option<std::time::Instant>>,
    /// Build inputs needed for atomic rebuild on invalidation.
    pub theme_names: Vec<String>,
    pub data_dirs: Vec<PathBuf>,
    /// Shared directory reads for the lazy scan; `None` reads the disk directly.
    pub(super) listings: Option<Arc<DirListings>>,
    pub(super) memo: std::sync::Mutex<NameMemo>,
    /// Cap for the name memo (bounded legacy fallback below counts too).
    pub memo_cap: usize,
    pub(super) hits: std::sync::atomic::AtomicU64,
    pub(super) misses: std::sync::atomic::AtomicU64,
}

#[cfg(test)]
thread_local! {
    /// Test seam: `index.theme` reads performed by the current thread.
    pub(super) static INDEX_THEME_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Live count of [`IconLookupIndex::build`] calls (one-time index proof).
pub static INDEX_BUILD_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl IconLookupIndex {
    /// Candidate file extensions in probe order (PPM legacy last).
    pub const EXTENSIONS: &'static [&'static str] = &["png", "svg", "xpm", "ppm"];

    #[must_use]
    pub fn new(
        roots: Vec<PathBuf>,
        ordered_themes: Vec<String>,
        indexes: HashMap<String, Option<IconThemeIndex>>,
        theme_roots: HashMap<String, Vec<PathBuf>>,
        generation: u64,
    ) -> Self {
        Self::new_full(
            roots,
            ordered_themes,
            indexes,
            theme_roots,
            IconFileIndex::default(),
            HashMap::new(),
            Vec::new(),
            Vec::new(),
            generation,
        )
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_full(
        roots: Vec<PathBuf>,
        ordered_themes: Vec<String>,
        indexes: HashMap<String, Option<IconThemeIndex>>,
        theme_roots: HashMap<String, Vec<PathBuf>>,
        files: IconFileIndex,
        pixmaps: HashMap<String, Vec<PathBuf>>,
        theme_names: Vec<String>,
        data_dirs: Vec<PathBuf>,
        generation: u64,
    ) -> Self {
        Self {
            generation,
            roots,
            ordered_themes,
            theme_roots,
            scan: std::sync::OnceLock::from(IconScan {
                indexes,
                files,
                pixmaps,
            }),
            watch_mtime: None,
            last_mtime_check: std::sync::Mutex::new(None),
            theme_names,
            data_dirs,
            listings: None,
            memo: std::sync::Mutex::new(NameMemo::default()),
            memo_cap: 2048,
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Builds a snapshot from live theme state (does FS I/O once).
    #[must_use]
    pub fn build(theme_names: &[String], data_dirs: &[PathBuf], generation: u64) -> Self {
        Self::build_with(theme_names, data_dirs, generation, None)
    }

    /// Like [`IconLookupIndex::build`], but directory reads go through `listings`, so
    /// several one-shot builds in one scan (e.g. one per installed icon theme) read each
    /// directory once. Use only for indexes that are not rebuilt by `needs_rebuild`.
    #[must_use]
    pub fn build_shared(
        theme_names: &[String],
        data_dirs: &[PathBuf],
        generation: u64,
        listings: &Arc<DirListings>,
    ) -> Self {
        Self::build_with(
            theme_names,
            data_dirs,
            generation,
            Some(Arc::clone(listings)),
        )
    }

    pub(super) fn build_with(
        theme_names: &[String],
        data_dirs: &[PathBuf],
        generation: u64,
        listings: Option<Arc<DirListings>>,
    ) -> Self {
        INDEX_BUILD_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let roots = search_roots(data_dirs);
        let mut seeds: Vec<String> = theme_names
            .iter()
            .filter(|t| !t.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        if seeds.is_empty() {
            seeds.push("hicolor".to_owned());
        }
        // Parse needed indexes on demand through the inheritance walk.
        let parsed: std::cell::RefCell<HashMap<String, Option<IconThemeIndex>>> =
            std::cell::RefCell::new(HashMap::new());
        let load = |theme: &str| -> Option<IconThemeIndex> {
            if let Some(hit) = parsed.borrow().get(theme) {
                return hit.clone();
            }
            let found = read_theme_inherits(&roots, theme);
            parsed.borrow_mut().insert(theme.to_owned(), found.clone());
            found
        };
        let mut ordered: Vec<String> = Vec::new();
        for seed in &seeds {
            for name in inheritance_chain(seed, &load) {
                // Ensure parsed entry exists for every chained theme.
                let _ = load(&name);
                if !ordered.contains(&name) {
                    ordered.push(name);
                }
            }
        }
        // Chain order is selected theme -> its Inherits -> Breeze -> hicolor.
        // A selected theme that is not installed keeps its slot but has no
        // files, so lookups fall through to Breeze instead of blank icons.
        ordered.retain(|name| name != "hicolor");
        if !ordered.contains(&"Breeze".to_owned()) {
            ordered.push("Breeze".to_owned());
        }
        ordered.push("hicolor".to_owned());
        for theme in &ordered {
            let _ = load(theme);
        }
        let mut theme_roots: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for theme in &ordered {
            let mut truth = Vec::new();
            for root in &roots {
                if theme_dir(root, theme).is_some() && !truth.contains(root) {
                    truth.push(root.clone());
                }
            }
            theme_roots.insert(theme.clone(), truth);
        }
        let mut index = Self::new_full(
            roots,
            ordered,
            HashMap::new(),
            theme_roots,
            IconFileIndex::default(),
            HashMap::new(),
            theme_names.to_vec(),
            data_dirs.to_vec(),
            generation,
        );
        index.scan = std::sync::OnceLock::new();
        index.watch_mtime =
            current_watch_mtime(&index.roots, &index.data_dirs, listings.as_deref());
        index.listings = listings;
        index
    }

    pub(super) fn scan(&self) -> IconScan {
        // Real name -> existing-files index: read each declared dir once,
        // record actual supported files only.
        let indexes: HashMap<String, Option<IconThemeIndex>> = self
            .ordered_themes
            .iter()
            .map(|theme| (theme.clone(), read_theme_index(&self.roots, theme)))
            .collect();
        let mut files = IconFileBuilder::default();
        for (theme_rank, theme) in self.ordered_themes.iter().enumerate() {
            let Some(Some(index)) = indexes.get(theme) else {
                continue;
            };
            let Some(roots_for_theme) = self.theme_roots.get(theme) else {
                continue;
            };
            for (directory_rank, dir) in index.directories.iter().enumerate() {
                if !index.dirs.contains_key(dir) {
                    continue;
                }
                for root in roots_for_theme {
                    let Some(theme_path) = theme_dir(root, theme) else {
                        continue;
                    };
                    let dir_path = theme_path.join(dir);
                    for item in dir_items(&dir_path, self.listings.as_deref()).iter() {
                        files.push(
                            theme_rank,
                            directory_rank,
                            Path::new(dir),
                            &dir_path,
                            &item.name,
                        );
                    }
                }
            }
        }
        // Store per-file distance lazily at lookup (size-dependent).
        for (theme_rank, theme) in self.ordered_themes.iter().enumerate() {
            if indexes.get(theme).is_some_and(std::option::Option::is_some) {
                continue;
            }
            let Some(roots_for_theme) = self.theme_roots.get(theme) else {
                continue;
            };
            for root in roots_for_theme {
                let base = root.join(theme);
                walk_theme_dir(&base, &mut files, theme_rank, usize::MAX / 4);
            }
        }
        let mut pixmaps: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let pixmap_dirs = unique_paths(
            self.roots
                .iter()
                .filter_map(|root| root.parent())
                .map(|parent| parent.join("pixmaps"))
                .chain(
                    self.data_dirs
                        .iter()
                        .map(|data_dir| data_dir.join("pixmaps")),
                )
                .chain(std::iter::once(PathBuf::from("/usr/share/pixmaps")))
                .collect(),
        );
        for dir in &pixmap_dirs {
            let Ok(read) = fs::read_dir(dir) else {
                continue;
            };
            for item in read.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                let Some((base, ext)) = split_icon_filename(&name) else {
                    continue;
                };
                if !Self::EXTENSIONS.contains(&ext.as_str()) {
                    continue;
                }
                pixmaps.entry(base).or_default().push(item.path());
            }
        }
        IconScan {
            indexes,
            files: files.finish(),
            pixmaps,
        }
    }

    pub(super) fn indexes(&self) -> &HashMap<String, Option<IconThemeIndex>> {
        &self.scan.get_or_init(|| self.scan()).indexes
    }

    pub(super) fn files(&self) -> &IconFileIndex {
        &self.scan.get_or_init(|| self.scan()).files
    }

    pub(super) fn pixmaps(&self) -> &HashMap<String, Vec<PathBuf>> {
        &self.scan.get_or_init(|| self.scan()).pixmaps
    }

    /// Test/measurement seam: component counts of the built scan.
    #[cfg(test)]
    pub(super) fn scan_footprint(&self) -> String {
        let scan = self.scan.get_or_init(|| self.scan());
        let files = &scan.files;
        let dir_bytes: usize = files.dirs.iter().map(|d| d.as_os_str().len()).sum();
        let dir_entries: usize = scan.indexes.values().flatten().map(|t| t.dirs.len()).sum();
        let section_bytes: usize = scan
            .indexes
            .values()
            .flatten()
            .map(|t| t.directories.iter().map(String::len).sum::<usize>())
            .sum();
        let pixmap_paths: usize = scan.pixmaps.values().map(Vec::len).sum();
        format!(
            "entries={} name_bytes_cap={} names={} dirs={} dir_path_bytes={} exts={} \
             index_themes={} index_dir_map_entries={} index_section_bytes={} \
             pixmap_keys={} pixmap_paths={}",
            files.entries.len(),
            files.name_bytes.capacity(),
            files.names.len(),
            files.dirs.len(),
            dir_bytes,
            files.exts.len(),
            scan.indexes.len(),
            dir_entries,
            section_bytes,
            scan.pixmaps.len(),
            pixmap_paths,
        )
    }

    /// Normalizes `foo`, `foo.png`, `foo.svg`, `foo.xpm` to
    /// `(base, Option<explicit ext>)`. Explicit ext pins the filename.
    #[must_use]
    pub fn normalize_name(name: &str) -> (String, Option<String>) {
        if let Some((base, ext)) = split_icon_filename(name) {
            if Self::EXTENSIONS.contains(&ext.as_str()) {
                return (base, Some(ext));
            }
        }
        (name.to_owned(), None)
    }

    /// Memory-only lookup: normalized -> files -> global exact-size first
    /// in chain order, then per-theme best (nearest size, directory order,
    /// format order) in theme-rank order -> pixmaps. Returned files were
    /// recorded at build (`indexed_hit` means the file exists in snapshot).
    #[must_use]
    pub fn lookup(&self, name: &str, physical: u32) -> (Vec<PathBuf>, bool) {
        let (base, explicit) = Self::normalize_name(name);
        let Some(entries) = self.files().get(&base) else {
            let mut tail = Vec::new();
            if let Some(found) = self.pixmaps().get(&base) {
                tail.extend(child_filter(found, explicit.as_deref()));
            }
            let hit = !tail.is_empty();
            return (tail, hit);
        };
        // Per-theme best first (current-theme any-size candidate recorded),
        // then global exact-first to mirror resolver precedence: exact-size
        // (distance 0) matches anywhere in the chain win in chain order,
        // remaining per-theme bests follow in theme-rank order.
        let want = explicit.as_deref();
        let mut theme_best: HashMap<u32, (&IndexedIconFile, u64, u32)> = HashMap::new();
        for file in entries {
            if !want.is_none_or(|want| self.files().ext(*file).eq_ignore_ascii_case(want)) {
                continue;
            }
            let distance = directory_distance(self, *file, physical.max(1));
            let meta = self.files().meta(*file);
            let better = match theme_best.get(&meta.theme_rank) {
                None => true,
                Some((cur, cur_d, cur_dir)) => {
                    (distance, meta.directory_rank, file.format_rank)
                        < (*cur_d, *cur_dir, cur.format_rank)
                }
            };
            if better {
                theme_best.insert(meta.theme_rank, (file, distance, meta.directory_rank));
            }
        }
        let mut best: Vec<(u32, &IndexedIconFile, u64)> = theme_best
            .into_iter()
            .map(|(rank, (file, distance, _))| (rank, file, distance))
            .collect();
        best.sort_by_key(|(rank, _, _)| *rank);
        // Global exact-first mirrors resolver precedence: exact-size
        // (distance 0) matches anywhere in the chain win in chain order,
        // remaining per-theme bests follow in theme-rank order.
        let mut ordered = Vec::new();
        for &(_, file, distance) in &best {
            if distance == 0 {
                ordered.push(self.files().path(&base, *file));
            }
        }
        for &(_, file, distance) in &best {
            if distance != 0 {
                ordered.push(self.files().path(&base, *file));
            }
        }
        let mut out = ordered;
        if out.is_empty() {
            if let Some(found) = self.pixmaps().get(&base) {
                out.extend(child_filter(found, explicit.as_deref()));
            }
        }
        let hit = !out.is_empty();
        (out, hit)
    }

    /// At most one watch-set mtime check per 5s. When the watched directory
    /// mtimes changed, returns a replacement snapshot whose `generation` is
    /// one past `self.generation`. The owner swaps it in and passes the new
    /// generation to its caches (`IconThemeCaches::note_generation_bump`),
    /// so generation-keyed entries from the old snapshot miss.
    #[must_use]
    pub fn needs_rebuild(&self) -> Option<IconLookupIndex> {
        let mut guard = self.last_mtime_check.lock().ok()?;
        let now = std::time::Instant::now();
        if let Some(last) = *guard {
            if now.duration_since(last).as_secs() < 5 {
                return None;
            }
        }
        *guard = Some(now);
        let current = current_watch_mtime(&self.roots, &self.data_dirs, None);
        if current == self.watch_mtime {
            return None;
        }
        let mut next = Self::build(
            &self.theme_names,
            &self.data_dirs,
            self.generation.saturating_add(1),
        );
        next.last_mtime_check = std::sync::Mutex::new(Some(now));
        Some(next)
    }

    /// (hits, misses) for the name memo.
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(std::sync::atomic::Ordering::Relaxed),
            self.misses.load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    #[must_use]
    pub fn memo_len(&self) -> usize {
        self.memo.lock().map(|m| m.len).unwrap_or(0)
    }

    pub(super) fn memo_get(&self, name: &str, physical: u32) -> Option<Vec<PathBuf>> {
        let hit = self.memo.lock().ok()?.get(name, physical).cloned();
        if hit.is_some() {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        hit
    }

    pub(super) fn memo_store(&self, name: &str, physical: u32, paths: Vec<PathBuf>) {
        if let Ok(mut memo) = self.memo.lock() {
            memo.insert(name, physical, paths, self.memo_cap);
        }
    }

    /// Memory-only candidate paths for `name` at `physical` px.
    /// Freezes application lookup precedence: normalizes explicit
    /// extensions (never appends a second ext), returns global exact-size
    /// matches first in chain order, then per-theme best, then pixmaps
    /// recorded at build. No FS walk, no probe cap.
    #[must_use]
    pub fn candidates(&self, name: &str, physical: u32) -> Vec<PathBuf> {
        if let Some(hit) = self.memo_get(name, physical) {
            return hit;
        }
        let (ordered, _) = self.lookup(name, physical);
        self.memo_store(name, physical, ordered.clone());
        ordered
    }
}
