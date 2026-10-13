//! Freedesktop `index.theme` parser and inheritance expansion (J2).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconDirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconDir {
    pub size: u32,
    pub scale: u32,
    pub dir_type: IconDirType,
    pub min_size: u32,
    pub max_size: u32,
    pub threshold: u32,
}

impl Default for IconDir {
    fn default() -> Self {
        Self {
            size: 0,
            scale: 1,
            dir_type: IconDirType::Threshold,
            min_size: 0,
            max_size: 0,
            threshold: 2,
        }
    }
}

impl IconDir {
    /// Distance from `requested` to this directory. Lower is better; 0 is exact.
    #[must_use]
    pub fn distance(&self, requested: u32) -> u64 {
        let req = u64::from(requested);
        match self.dir_type {
            IconDirType::Fixed => {
                let nominal = u64::from(self.size) * u64::from(self.scale.max(1));
                req.abs_diff(nominal)
            }
            IconDirType::Scalable => {
                let (mut lo, mut hi) = (u64::from(self.min_size), u64::from(self.max_size));
                if lo == 0 && hi == 0 {
                    let nominal = u64::from(self.size) * u64::from(self.scale.max(1));
                    return req.abs_diff(nominal);
                }
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                if req < lo {
                    lo - req
                } else if req > hi {
                    req - hi
                } else {
                    0
                }
            }
            IconDirType::Threshold => {
                let nominal = u64::from(self.size) * u64::from(self.scale.max(1));
                let t = u64::from(self.threshold);
                if req.abs_diff(nominal) <= t {
                    0
                } else if req < nominal {
                    nominal - t - req
                } else {
                    req - (nominal + t)
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct IconThemeIndex {
    pub name: String,
    pub inherits: Vec<String>,
    pub directories: Vec<String>,
    pub dirs: HashMap<String, IconDir>,
}

/// Bounded per-resolver theme lookup caches (J03 warm path).
///
/// All entries are scoped by `theme_generation`; a generation bump makes old
/// entries miss without clearing live-raster bytes. No raw map is exposed;
/// use [`IconThemeCaches::stats`] / `memory_estimate_bytes`.
#[derive(Clone, Debug, Default)]
pub struct IconThemeCaches {
    exists: HashMap<(PathBuf, u64), bool>,
    hits: u64,
    misses: u64,
}

impl IconThemeCaches {
    fn exists(&mut self, path: &Path, generation: u64) -> Option<bool> {
        let key = (path.to_path_buf(), generation);
        if let Some(hit) = self.exists.get(&key) {
            self.hits += 1;
            return Some(*hit);
        }
        self.misses += 1;
        None
    }

    pub(crate) fn cached_exists(&mut self, path: &Path, generation: u64) -> Option<bool> {
        self.exists(path, generation)
    }

    pub(crate) fn store_exists(&mut self, path: &Path, generation: u64, exists: bool) {
        bound_insert(
            &mut self.exists,
            (path.to_path_buf(), generation),
            exists,
            4096,
        );
    }

    /// (hits, misses) across exists lookups.
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.exists.len()
    }

    #[must_use]
    pub fn memory_estimate_bytes(&self) -> usize {
        let mut bytes = 0;
        for ((path, _), _) in &self.exists {
            bytes += path.as_os_str().len() + 16;
        }
        bytes
    }

    fn clear_generation(&mut self) {
        // Generations are key-scoped so no clear is required; this hook keeps
        // the bound small if a host cycles generations rapidly.
        while self.len() > 4096 {
            if let Some(first) = self.exists.keys().next().cloned() {
                self.exists.remove(&first);
            } else {
                break;
            }
        }
    }

    pub(crate) fn note_generation_bump(&mut self) {
        self.clear_generation();
    }
}

fn bound_insert<K: std::hash::Hash + Eq + Clone, V>(
    map: &mut HashMap<K, V>,
    key: K,
    value: V,
    cap: usize,
) {
    if map.len() >= cap && !map.contains_key(&key) {
        // Bounded: drop an arbitrary entry rather than growing without limit.
        if let Some(first) = map.keys().next().cloned() {
            map.remove(&first);
        }
    }
    map.insert(key, value);
}

impl IconThemeIndex {
    #[must_use]
    pub fn parse(name: &str, text: &str) -> Self {
        let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut current = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                current = line[1..line.len() - 1].trim().to_owned();
                sections.entry(current.clone()).or_default();
                continue;
            }
            if current.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                sections
                    .entry(current.clone())
                    .or_default()
                    .insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
        let header = sections.get("Icon Theme").cloned().unwrap_or_default();
        let directories = split_list(header.get("Directories").map(String::as_str).unwrap_or(""));
        let inherits = split_list(header.get("Inherits").map(String::as_str).unwrap_or(""));
        let mut dirs = HashMap::new();
        for dir in &directories {
            if let Some(section) = sections.get(dir.as_str()) {
                dirs.insert(dir.clone(), parse_dir(section));
            }
        }
        // ScaledDirectories entries share the same section format.
        if let Some(scaled) = header.get("ScaledDirectories") {
            for dir in split_list(scaled) {
                if !dirs.contains_key(&dir) {
                    if let Some(section) = sections.get(dir.as_str()) {
                        let mut entry = parse_dir(section);
                        if entry.scale == 1 {
                            entry.scale = 2;
                        }
                        dirs.insert(dir.clone(), entry);
                    }
                }
            }
        }
        Self {
            name: name.to_owned(),
            inherits,
            directories,
            dirs,
        }
    }

    #[must_use]
    pub fn load(theme_dir: &Path) -> Option<Self> {
        let name = theme_dir
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = fs::read_to_string(theme_dir.join("index.theme")).ok()?;
        Some(Self::parse(&name, &text))
    }
}

fn parse_dir(section: &HashMap<String, String>) -> IconDir {
    let size = section
        .get("Size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let scale = section
        .get("Scale")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let dir_type = match section.get("Type").map(String::as_str).unwrap_or("") {
        "Fixed" => IconDirType::Fixed,
        "Scalable" => IconDirType::Scalable,
        _ => IconDirType::Threshold,
    };
    let threshold = section
        .get("Threshold")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let min_size = section
        .get("MinSize")
        .and_then(|v| v.parse().ok())
        .unwrap_or(size);
    let max_size = section
        .get("MaxSize")
        .and_then(|v| v.parse().ok())
        .unwrap_or(size);
    IconDir {
        size,
        scale: scale.max(1),
        dir_type,
        min_size,
        max_size,
        threshold,
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Splits `foo.png` -> `(foo, png)`; bare `foo` -> `None` (caller treats as
/// base with no explicit ext). Never yields a double extension.
fn split_icon_filename(name: &str) -> Option<(String, String)> {
    let (base, ext) = name.rsplit_once('.')?;
    if base.is_empty() || ext.is_empty() {
        return None;
    }
    if !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some((base.to_owned(), ext.to_ascii_lowercase()))
}

/// Mirrors `themes::import::MAX_DEPTH`; bounds walks through symlinked dirs.
const THEME_WALK_MAX_DEPTH: usize = 32;

fn walk_theme_dir(
    base: &Path,
    files: &mut HashMap<String, Vec<IndexedIconFile>>,
    theme_rank: usize,
    directory_rank: usize,
) {
    // One build-time walk per unindexed theme dir; lookup stays memory-only.
    // Symlinked dirs are followed, but each canonical dir is visited once and
    // depth is capped, so a symlink loop cannot make the walk unbounded.
    let mut visited = HashSet::new();
    if let Ok(canonical) = fs::canonicalize(base) {
        visited.insert(canonical);
    }
    let mut stack = vec![(base.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            let path = item.path();
            if path.is_dir() {
                if depth + 1 > THEME_WALK_MAX_DEPTH {
                    continue;
                }
                if let Ok(canonical) = fs::canonicalize(&path) {
                    if visited.insert(canonical) {
                        stack.push((path, depth + 1));
                    }
                }
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            let Some((base_name, ext)) = split_icon_filename(&name) else {
                continue;
            };
            // Unindexed theme: accept only supported extensions.
            let Some(format_rank) = IconLookupIndex::EXTENSIONS.iter().position(|e| *e == ext) else {
                continue;
            };
            files.entry(base_name).or_default().push(IndexedIconFile {
                path,
                theme_rank,
                directory_rank,
                distance: 0,
                format_rank,
            });
        }
    }
}

fn explicit_matches(path: &Path, explicit: Option<&str>) -> bool {
    match explicit {
        None => true,
        Some(want) => path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|have| have.eq_ignore_ascii_case(want)),
    }
}

fn child_filter(paths: &[PathBuf], explicit: Option<&str>) -> Vec<PathBuf> {
    // Explicit ext pins exact filename; otherwise prefer format order.
    let mut scored: Vec<(&PathBuf, usize)> = paths
        .iter()
        .filter(|p| explicit_matches(p, explicit))
        .map(|p| {
            let rank = p
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| {
                    IconLookupIndex::EXTENSIONS
                        .iter()
                        .position(|x| x.eq_ignore_ascii_case(e))
                })
                .unwrap_or(usize::MAX);
            (p, rank)
        })
        .collect();
    scored.sort_by_key(|(_, r)| *r);
    scored.into_iter().map(|(p, _)| p.clone()).collect()
}

fn directory_distance(index: &IconLookupIndex, file: &IndexedIconFile, physical: u32) -> u64 {
    let theme = match index.ordered_themes.get(file.theme_rank) {
        Some(t) => t,
        None => return u64::MAX / 2,
    };
    let dirs = match index.indexes.get(theme).and_then(|o| o.as_ref()) {
        Some(d) => d,
        None => return u64::MAX / 2,
    };
    let dir_name = match dirs.directories.get(file.directory_rank) {
        Some(d) => d,
        None => return u64::MAX / 2,
    };
    dirs.dirs
        .get(dir_name)
        .map(|e| e.distance(physical))
        .unwrap_or(u64::MAX / 2)
}

/// Depth below each search root that is watched for new icons.
const WATCH_DEPTH: usize = 3;
/// Upper bound on directories stat-ed per watch check (across all depths).
const WATCH_DIR_CAP: usize = 2048;
/// Per-depth share of `WATCH_DIR_CAP`, so a wide shallow level cannot starve
/// the deeper category levels of every theme.
const WATCH_LEVEL_CAP: usize = WATCH_DIR_CAP / (WATCH_DEPTH + 1);
/// Set once per process so a capped watch set logs a single diagnostic line.
static WATCH_CAP_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Directories whose mtime changes when an icon is added or removed:
/// every search root plus theme subdirectories (`theme/size/category`) up to
/// `WATCH_DEPTH`. Walks level by level; each level keeps at most
/// `WATCH_LEVEL_CAP` dirs, interleaved round-robin across parents so every
/// theme's shallow levels are covered before any one theme consumes a level.
/// Data dirs are stat-only and not counted against the cap.
fn watch_dirs(roots: &[PathBuf], data_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = data_dirs.to_vec();
    let mut level: Vec<PathBuf> = roots.to_vec();
    let mut capped = false;
    for depth in 0..=WATCH_DEPTH {
        if level.len() > WATCH_LEVEL_CAP {
            level.truncate(WATCH_LEVEL_CAP);
            capped = true;
        }
        if depth == WATCH_DEPTH {
            out.append(&mut level);
            break;
        }
        let groups: Vec<Vec<PathBuf>> = level.iter().map(|dir| child_dirs(dir)).collect();
        out.append(&mut level);
        level = interleave_by_parent(&groups);
    }
    if capped {
        log_watch_cap_once();
    }
    out
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

/// Round-robin merge of per-parent child lists: first child of every parent,
/// then second child of every parent, and so on.
fn interleave_by_parent(groups: &[Vec<PathBuf>]) -> Vec<PathBuf> {
    let rounds = groups.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::new();
    for index in 0..rounds {
        for group in groups {
            if let Some(path) = group.get(index) {
                out.push(path.clone());
            }
        }
    }
    out
}

fn log_watch_cap_once() {
    if !WATCH_CAP_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "flamewm-integrations-linux: icon watch set capped at {WATCH_LEVEL_CAP} dirs per depth (total cap {WATCH_DIR_CAP}); deeper icon dir changes may not trigger a refresh"
        );
    }
}

fn current_watch_mtime(roots: &[PathBuf], data_dirs: &[PathBuf]) -> Option<std::time::SystemTime> {
    let mut latest: Option<std::time::SystemTime> = None;
    for top in &watch_dirs(roots, data_dirs) {
        let mtime = fs::metadata(top).and_then(|m| m.modified()).ok();
        latest = match (latest, mtime) {
            (None, m) => m,
            (Some(a), None) => Some(a),
            (Some(a), Some(b)) => Some(a.max(b)),
        };
    }
    latest
}

/// Value of `key` inside `[section]` of an INI-style text (KDE `kdeglobals`,
/// GTK `settings.ini`). Empty values count as absent.
#[must_use]
pub fn parse_ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            current = name.trim().to_owned();
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

fn expand(
    name: &str,
    loader: &dyn Fn(&str) -> Option<IconThemeIndex>,
    chain: &mut Vec<String>,
    visiting: &mut Vec<String>,
) {
    if name.is_empty() || chain.iter().any(|item| item == name) {
        return;
    }
    if visiting.iter().any(|item| item == name) {
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
fn theme_dir(root: &Path, theme: &str) -> Option<PathBuf> {
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
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if !home.as_os_str().is_empty() {
            roots.push(home.join(".icons"));
        }
    }
    for data_dir in data_dirs {
        roots.push(data_dir.join("icons"));
    }
    // Roots are always non-empty (`join` appends a component), so the
    // empty-path filter in `unique_paths` never drops anything here.
    unique_paths(roots)
}

/// Generation-scoped shared lookup snapshot.
///
/// Built once per `(theme_names, data_dirs, generation)` and shared by all
/// resolver workers. Holds search roots, parsed `index.theme` per theme in
/// the inheritance chain, per-theme roots, a real `base-name -> existing
/// files` index (each declared dir read once at build), pixmaps recorded at
/// build, and a bounded `name -> paths` memo so repeated lookups are
/// memory-only. Interior memo uses a `Mutex`; snapshots are replaced (never
/// mutated) on generation bumps.
#[derive(Debug)]
pub struct IndexedIconFile {
    pub path: PathBuf,
    pub theme_rank: usize,
    pub directory_rank: usize,
    pub distance: u64,
    pub format_rank: usize,
}

#[derive(Debug)]
pub struct IconLookupIndex {
    /// Theme generation this snapshot was built for.
    pub generation: u64,
    /// Deduped search roots (`$HOME/.icons`, `$XDG_DATA_HOME/icons`, ...).
    pub roots: Vec<PathBuf>,
    /// Ordered theme chain (seeds + inheritance, Breeze before hicolor).
    pub ordered_themes: Vec<String>,
    /// Parsed `index.theme` per theme (`None` = no index on disk).
    pub indexes: HashMap<String, Option<IconThemeIndex>>,
    /// Per-theme roots that actually carry the theme directory.
    pub theme_roots: HashMap<String, Vec<PathBuf>>,
    /// Real name -> existing-files index: base icon name -> files found by
    /// reading each declared directory once at build.
    pub files: HashMap<String, Vec<IndexedIconFile>>,
    /// Pixmaps recorded at build (sibling `pixmaps` + `/usr/share/pixmaps`).
    pub pixmaps: HashMap<String, Vec<PathBuf>>,
    /// Top-level roots `mtime` snapshot + last check (5s cooldown owner).
    pub watch_mtime: Option<std::time::SystemTime>,
    pub last_mtime_check: std::sync::Mutex<Option<std::time::Instant>>,
    /// Build inputs needed for atomic rebuild on invalidation.
    pub theme_names: Vec<String>,
    pub data_dirs: Vec<PathBuf>,
    memo: std::sync::Mutex<HashMap<(String, u32), Vec<PathBuf>>>,
    /// Cap for the name memo (bounded legacy fallback below counts too).
    pub memo_cap: usize,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

#[cfg(test)]
thread_local! {
    /// Test seam: `index.theme` reads performed by the current thread.
    static INDEX_THEME_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
            HashMap::new(),
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
        files: HashMap<String, Vec<IndexedIconFile>>,
        pixmaps: HashMap<String, Vec<PathBuf>>,
        theme_names: Vec<String>,
        data_dirs: Vec<PathBuf>,
        generation: u64,
    ) -> Self {
        Self {
            generation,
            roots,
            ordered_themes,
            indexes,
            theme_roots,
            files,
            pixmaps,
            watch_mtime: None,
            last_mtime_check: std::sync::Mutex::new(None),
            theme_names,
            data_dirs,
            memo: std::sync::Mutex::new(HashMap::new()),
            memo_cap: 2048,
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Builds a snapshot from live theme state (does FS I/O once).
    #[must_use]
    pub fn build(theme_names: &[String], data_dirs: &[PathBuf], generation: u64) -> Self {
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
            let mut found = None;
            for root in &roots {
                let Some(dir) = theme_dir(root, theme) else {
                    continue;
                };
                if dir.join("index.theme").is_file() {
                    if let Ok(text) = fs::read_to_string(dir.join("index.theme")) {
                        #[cfg(test)]
                        INDEX_THEME_READS.with(|c| c.set(c.get() + 1));
                        found = Some(IconThemeIndex::parse(theme, &text));
                        break;
                    }
                }
            }
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
        // Real name -> existing-files index: read each declared dir once,
        // record actual supported files only.
        let mut files: HashMap<String, Vec<IndexedIconFile>> = HashMap::new();
        for (theme_rank, theme) in ordered.iter().enumerate() {
            let parsed = parsed.borrow();
            let Some(Some(index)) = parsed.get(theme) else {
                continue;
            };
            let Some(roots_for_theme) = theme_roots.get(theme) else {
                continue;
            };
            for (directory_rank, dir) in index.directories.iter().enumerate() {
                if index.dirs.get(dir).is_none() {
                    continue;
                };
                for root in roots_for_theme {
                    let Some(theme_path) = theme_dir(root, theme) else {
                        continue;
                    };
                    let dir_path = theme_path.join(dir);
                    let Ok(read) = fs::read_dir(&dir_path) else {
                        continue;
                    };
                    for item in read.flatten() {
                        let name = item.file_name().to_string_lossy().into_owned();
                        let Some((base, ext)) = split_icon_filename(&name) else {
                            continue;
                        };
                        let Some(format_rank) = Self::EXTENSIONS.iter().position(|e| *e == ext)
                        else {
                            continue;
                        };
                        files.entry(base).or_default().push(IndexedIconFile {
                            path: item.path(),
                            theme_rank,
                            directory_rank,
                            distance: 0,
                            format_rank,
                        });
                    }
                }
            }
        }
        // Store per-file distance lazily at lookup (size-dependent).
        for (theme_rank, theme) in ordered.iter().enumerate() {
            if parsed
                .borrow()
                .get(theme)
                .is_some_and(|entry| entry.is_some())
            {
                continue;
            }
            let Some(roots_for_theme) = theme_roots.get(theme) else {
                continue;
            };
            for root in roots_for_theme {
                let base = root.join(theme);
                walk_theme_dir(&base, &mut files, theme_rank, usize::MAX / 4);
            }
        }
        let mut pixmaps: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut pixmap_dirs: Vec<PathBuf> = Vec::new();
        for root in &roots {
            if let Some(parent) = root.parent() {
                let dir = parent.join("pixmaps");
                if !pixmap_dirs.contains(&dir) {
                    pixmap_dirs.push(dir);
                }
            }
        }
        for data_dir in data_dirs {
            let dir = data_dir.join("pixmaps");
            if !pixmap_dirs.contains(&dir) {
                pixmap_dirs.push(dir);
            }
        }
        let usr = Path::new("/usr/share/pixmaps");
        if !pixmap_dirs.contains(&usr.to_path_buf()) {
            pixmap_dirs.push(usr.to_path_buf());
        }
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
        let mut index = Self::new_full(
            roots,
            ordered,
            parsed.into_inner(),
            theme_roots,
            files,
            pixmaps,
            theme_names.to_vec(),
            data_dirs.to_vec(),
            generation,
        );
        index.watch_mtime = current_watch_mtime(&index.roots, &index.data_dirs);
        index
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
        let Some(entries) = self.files.get(&base) else {
            let mut tail = Vec::new();
            if let Some(found) = self.pixmaps.get(&base) {
                tail.extend(child_filter(found, explicit.as_deref()));
            }
            let hit = !tail.is_empty();
            return (tail, hit);
        };
        // Per-theme best first (current-theme any-size candidate recorded),
        // then global exact-first to mirror resolver precedence: exact-size
        // (distance 0) matches anywhere in the chain win in chain order,
        // remaining per-theme bests follow in theme-rank order.
        let mut theme_best: HashMap<usize, (&IndexedIconFile, u64)> = HashMap::new();
        for file in entries {
            if !explicit_matches(&file.path, explicit.as_deref()) {
                continue;
            }
            let distance = directory_distance(self, file, physical.max(1));
            let better = match theme_best.get(&file.theme_rank) {
                None => true,
                Some((cur, cur_d)) => {
                    (distance, file.directory_rank, file.format_rank)
                        < (*cur_d, cur.directory_rank, cur.format_rank)
                }
            };
            if better {
                theme_best.insert(file.theme_rank, (file, distance));
            }
        }
        let mut best: Vec<(usize, &IndexedIconFile, u64)> = theme_best
            .into_iter()
            .map(|(rank, (file, distance))| (rank, file, distance))
            .collect();
        best.sort_by_key(|(rank, _, _)| *rank);
        // Global exact-first mirrors resolver precedence: exact-size
        // (distance 0) matches anywhere in the chain win in chain order,
        // remaining per-theme bests follow in theme-rank order.
        let mut ordered = Vec::new();
        for (_, file, distance) in &best {
            if *distance == 0 {
                ordered.push(file.path.clone());
            }
        }
        for (_, file, distance) in &best {
            if *distance != 0 {
                ordered.push(file.path.clone());
            }
        }
        let mut out = ordered;
        if out.is_empty() {
            if let Some(found) = self.pixmaps.get(&base) {
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
        let current = current_watch_mtime(&self.roots, &self.data_dirs);
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
        self.memo.lock().map(|m| m.len()).unwrap_or(0)
    }

    fn memo_get(&self, name: &str, physical: u32) -> Option<Vec<PathBuf>> {
        let hit = self
            .memo
            .lock()
            .ok()?
            .get(&(name.to_owned(), physical))
            .cloned();
        if hit.is_some() {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        hit
    }

    fn memo_store(&self, name: &str, physical: u32, paths: Vec<PathBuf>) {
        if let Ok(mut memo) = self.memo.lock() {
            if memo.len() >= self.memo_cap && !memo.contains_key(&(name.to_owned(), physical)) {
                if let Some(first) = memo.keys().next().cloned() {
                    memo.remove(&first);
                }
            }
            memo.insert((name.to_owned(), physical), paths);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_paths_drops_empty_and_repeats_keeping_first_order() {
        let input = vec![
            PathBuf::from("/b"),
            PathBuf::new(),
            PathBuf::from("/a"),
            PathBuf::from("/b"),
        ];
        assert_eq!(
            unique_paths(input),
            vec![PathBuf::from("/b"), PathBuf::from("/a")]
        );
    }

    #[test]
    fn parses_directories_and_inherits() {
        let text = "[Icon Theme]\nName=Test\nDirectories=16x16/apps,scalable/apps\nInherits=Breeze,hicolor\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[scalable/apps]\nSize=48\nType=Scalable\nMinSize=16\nMaxSize=512\n";
        let index = IconThemeIndex::parse("Test", text);
        assert_eq!(index.inherits, vec!["Breeze", "hicolor"]);
        assert_eq!(index.directories.len(), 2);
        assert_eq!(index.dirs["16x16/apps"].dir_type, IconDirType::Fixed);
        assert_eq!(index.dirs["scalable/apps"].min_size, 16);
    }

    #[test]
    fn cycle_terminates() {
        let loader = |name: &str| {
            let inherits = match name {
                "A" => vec!["B".to_owned()],
                "B" => vec!["A".to_owned()],
                _ => Vec::new(),
            };
            Some(IconThemeIndex {
                name: name.to_owned(),
                inherits,
                ..IconThemeIndex::default()
            })
        };
        let chain = inheritance_chain("A", &loader);
        assert_eq!(chain.first().map(String::as_str), Some("A"));
        assert!(chain.contains(&"B".to_owned()));
        assert_eq!(chain.last().map(String::as_str), Some("hicolor"));
        assert!(chain.iter().filter(|n| *n == "A").count() == 1);
    }

    #[test]
    fn configured_theme_parses_kdeglobals_style_text() {
        let text = "[General]\nColorScheme=BreezeDark\n\n[Icons]\nTheme=Papirus-Dark\n\n[KDE]\nLookAndFeelPackage=org.kde.breeze.desktop\n";
        assert_eq!(
            parse_ini_value(text, "Icons", "Theme").as_deref(),
            Some("Papirus-Dark")
        );
        assert_eq!(parse_ini_value(text, "General", "Theme"), None);
        assert_eq!(parse_ini_value("[Icons]\nTheme=\n", "Icons", "Theme"), None);
        let gtk = "[Settings]\ngtk-icon-theme-name = Adwaita\n";
        assert_eq!(
            parse_ini_value(gtk, "Settings", "gtk-icon-theme-name").as_deref(),
            Some("Adwaita")
        );
    }

    #[test]
    fn shallow_watch_detects_nested_icon_directory_change() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-watch-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        let nested = root.join("Theme/16x16/apps");
        fs::create_dir_all(&nested).expect("nested theme dirs");
        let before = current_watch_mtime(std::slice::from_ref(&root), &[]);
        // Force a distinct mtime on the nested directory so the check does
        // not depend on filesystem timestamp granularity.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        fs::write(nested.join("new-app.png"), b"x").expect("add icon");
        fs::File::open(&nested)
            .and_then(|dir| dir.set_modified(later))
            .expect("set nested mtime");
        let after = current_watch_mtime(std::slice::from_ref(&root), &[]);
        let _ = fs::remove_dir_all(&root);
        assert_ne!(before, after);
        assert_eq!(after, Some(later));
    }

    #[test]
    fn watch_dirs_covers_depth_three_under_last_theme_past_cap() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-watch-cap-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        // First theme alone holds more than WATCH_DIR_CAP directories.
        for i in 0..2100 {
            fs::create_dir_all(root.join("A").join(format!("s{i}"))).expect("wide theme dirs");
        }
        // Last theme: a depth-3 category directory (theme/size/category).
        let category = root.join("B").join("16x16").join("apps");
        fs::create_dir_all(&category).expect("last theme category dir");
        let watched = watch_dirs(std::slice::from_ref(&root), &[]);
        let _ = fs::remove_dir_all(&root);
        assert!(
            watched.contains(&category),
            "depth-3 category under last theme must be watched past the cap"
        );
    }

    #[test]
    fn warm_candidates_hit_memo_without_new_miss() {
        let index = IconLookupIndex::new(
            Vec::new(),
            vec!["hicolor".to_owned()],
            HashMap::new(),
            HashMap::new(),
            7,
        );
        let first = index.candidates("firefox", 48);
        let (_, cold_misses) = index.stats();
        let second = index.candidates("firefox", 48);
        let (hits, misses) = index.stats();
        assert_eq!(first, second);
        assert_eq!(misses, cold_misses, "warm call must not miss again");
        assert_eq!(hits, 1, "warm call must be served from the memo");
        assert_eq!(index.memo_len(), 1);
    }

    #[test]
    fn needs_rebuild_bumps_generation_when_watch_mtime_is_stale() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-rebuild-gen-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("watch root");
        let mut index = IconLookupIndex::new(
            vec![root.clone()],
            Vec::new(),
            HashMap::new(),
            HashMap::new(),
            3,
        );
        index.watch_mtime = Some(std::time::UNIX_EPOCH);
        let next = index.needs_rebuild();
        let throttled = index.needs_rebuild();
        let _ = fs::remove_dir_all(&root);
        let next = next.expect("stale watch mtime must produce a rebuilt snapshot");
        assert_eq!(next.generation, 4);
        assert!(throttled.is_none(), "second check inside 5s must be throttled");
    }

    fn theme_fixture(data: &Path, name: &str, index: &str, icon: Option<&str>) {
        let base = data.join("icons").join(name);
        fs::create_dir_all(base.join("16x16/apps")).expect("theme fixture dirs");
        fs::write(base.join("index.theme"), index).expect("index.theme fixture");
        if let Some(icon) = icon {
            fs::write(base.join("16x16/apps").join(icon), b"").expect("icon fixture");
        }
    }

    const FIXED_16: &str = "[Icon Theme]\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n";

    #[cfg(unix)]
    #[test]
    fn unindexed_theme_symlink_loop_terminates() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-loop-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&data);
        // No index.theme: forces the build-time walk for this theme.
        let apps = data.join("icons/loopy/16x16/apps");
        fs::create_dir_all(&apps).expect("loop fixture dirs");
        fs::write(apps.join("loop-app.png"), b"").expect("icon fixture");
        std::os::unix::fs::symlink(data.join("icons/loopy"), apps.join("up")).expect("loop link");
        std::os::unix::fs::symlink(&apps, apps.join("self")).expect("self link");
        let names = vec!["loopy".to_owned()];
        let index = IconLookupIndex::build(&names, std::slice::from_ref(&data), 1);
        let _ = fs::remove_dir_all(&data);
        let (paths, hit) = index.lookup("loop-app", 16);
        assert!(hit);
        assert!(paths[0].ends_with("loopy/16x16/apps/loop-app.png"));
    }

    #[cfg(unix)]
    #[test]
    fn unindexed_theme_valid_symlinked_subdir_is_indexed() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-symlink-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&data);
        // Target lives outside the theme; only the in-theme symlink reaches it.
        let real = data.join("shared/real");
        fs::create_dir_all(&real).expect("symlink fixture dirs");
        fs::create_dir_all(data.join("icons/linked")).expect("theme dir");
        fs::write(real.join("linked-app.png"), b"").expect("icon fixture");
        std::os::unix::fs::symlink(&real, data.join("icons/linked/alias")).expect("dir link");
        let names = vec!["linked".to_owned()];
        let index = IconLookupIndex::build(&names, std::slice::from_ref(&data), 1);
        let _ = fs::remove_dir_all(&data);
        let (paths, hit) = index.lookup("linked-app", 16);
        assert!(hit, "valid symlinked subdir must stay indexed");
        assert!(paths.iter().any(|p| p.ends_with("linked/alias/linked-app.png")));
    }

    #[test]
    fn selected_theme_inherits_before_breeze_then_hicolor() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-chain-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&data);
        let mcmojave = "[Icon Theme]\nName=McMojave\nDirectories=16x16/apps\nInherits=Numix-Circle-Light,hicolor,breeze\n\n[16x16/apps]\nSize=16\nType=Fixed\n";
        theme_fixture(&data, "McMojave-circle", mcmojave, None);
        theme_fixture(&data, "Numix-Circle-Light", FIXED_16, Some("chain-app.png"));
        theme_fixture(&data, "Breeze", FIXED_16, Some("chain-app.png"));
        theme_fixture(&data, "hicolor", FIXED_16, Some("chain-app.png"));
        let names = vec!["McMojave-circle".to_owned(), "Breeze".to_owned()];
        let index = IconLookupIndex::build(&names, std::slice::from_ref(&data), 1);
        let _ = fs::remove_dir_all(&data);
        let (paths, hit) = index.lookup("chain-app", 16);
        assert!(hit);
        assert_eq!(index.ordered_themes.first().map(String::as_str), Some("McMojave-circle"));
        // Inherited Numix must outrank Breeze, which the old ordering put first.
        assert!(paths[0].ends_with("Numix-Circle-Light/16x16/apps/chain-app.png"));
    }

    #[test]
    fn theme_dir_lookup_is_case_insensitive_for_breeze() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-case-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&data);
        // Host ships lowercase `breeze`; the chain names `Breeze`.
        theme_fixture(&data, "breeze", FIXED_16, Some("case-app.png"));
        let names = vec!["Breeze".to_owned()];
        let index = IconLookupIndex::build(&names, std::slice::from_ref(&data), 1);
        let _ = fs::remove_dir_all(&data);
        let (paths, hit) = index.lookup("case-app", 16);
        assert!(hit, "Breeze must resolve to the installed breeze directory");
        assert!(paths[0].ends_with("breeze/16x16/apps/case-app.png"));
    }

    #[test]
    fn missing_selected_theme_falls_back_to_breeze_not_blank() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-missing-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&data);
        theme_fixture(&data, "Breeze", FIXED_16, Some("fallback-app.png"));
        let names = vec!["McMojave-circle".to_owned(), "Breeze".to_owned()];
        let index = IconLookupIndex::build(&names, std::slice::from_ref(&data), 1);
        let (paths, hit) = index.lookup("fallback-app", 16);
        let (missing, missing_hit) = index.lookup("never-installed-icon", 16);
        let _ = fs::remove_dir_all(&data);
        assert!(hit, "Breeze must serve when the selected theme is absent");
        assert!(paths[0].ends_with("Breeze/16x16/apps/fallback-app.png"));
        assert!(missing.is_empty() && !missing_hit, "intentionally missing icon stays empty");
    }

    #[test]
    fn build_parses_each_chained_index_theme_once() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-parse-once-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&data);
        theme_fixture(&data, "Breeze", FIXED_16, Some("once-app.png"));
        let mine = data.join("icons").join("Mine");
        fs::create_dir_all(&mine).expect("selected theme dir");
        fs::write(
            mine.join("index.theme"),
            "[Icon Theme]\nName=Mine\nInherits=Breeze\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
        )
        .expect("selected index.theme");
        let names = vec!["Mine".to_owned()];
        INDEX_THEME_READS.with(|c| c.set(0));
        let index = IconLookupIndex::build(&names, std::slice::from_ref(&data), 1);
        let reads = INDEX_THEME_READS.with(|c| c.get());
        let (paths, hit) = index.lookup("once-app", 16);
        let _ = fs::remove_dir_all(&data);
        assert_eq!(
            reads, 2,
            "Mine and Breeze each parse index.theme once per build"
        );
        assert!(hit && paths[0].ends_with("Breeze/16x16/apps/once-app.png"));
    }
}
