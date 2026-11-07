//! Theme directory walking and inheritance reads.

use super::*;

/// Mirrors `themes::import::MAX_DEPTH`; bounds walks through symlinked dirs.
pub(super) const THEME_WALK_MAX_DEPTH: usize = 32;

/// One directory entry as the index build needs it. `is_dir` follows symlinks,
/// like `Path::is_dir`, but avoids a stat for entries the kernel already typed.
#[derive(Debug)]
pub(super) struct DirItem {
    pub(super) name: OsString,
    pub(super) path: PathBuf,
    pub(super) is_dir: bool,
}

/// Directory reads shared by the index builds of one scan (see
/// [`IconLookupIndex::build_shared`]). Each directory is read once per scan.
/// Never used for rebuild checks, which must see the disk.
#[derive(Debug, Default)]
pub struct DirListings {
    pub(super) by_dir: std::sync::Mutex<HashMap<PathBuf, Arc<[DirItem]>>>,
}

impl DirListings {
    pub(super) fn items(&self, dir: &Path) -> Arc<[DirItem]> {
        if let Some(hit) = self
            .by_dir
            .lock()
            .ok()
            .and_then(|map| map.get(dir).cloned())
        {
            return hit;
        }
        let items: Arc<[DirItem]> = read_dir_items(dir).into();
        if let Ok(mut map) = self.by_dir.lock() {
            map.insert(dir.to_path_buf(), Arc::clone(&items));
        }
        items
    }
}

pub(super) fn read_dir_items(dir: &Path) -> Vec<DirItem> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .map(|item| {
            let is_dir = match item.file_type() {
                Ok(kind) if !kind.is_symlink() => kind.is_dir(),
                _ => item.path().is_dir(),
            };
            DirItem {
                name: item.file_name(),
                path: item.path(),
                is_dir,
            }
        })
        .collect()
}

pub(super) fn dir_items(dir: &Path, listings: Option<&DirListings>) -> Arc<[DirItem]> {
    match listings {
        Some(listings) => listings.items(dir),
        None => read_dir_items(dir).into(),
    }
}

pub(super) fn walk_theme_dir(
    base: &Path,
    files: &mut IconFileBuilder,
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
            let rel = dir.strip_prefix(base).unwrap_or(Path::new(""));
            files.push(theme_rank, directory_rank, rel, &dir, &item.file_name());
        }
    }
}

pub(super) fn explicit_matches(path: &Path, explicit: Option<&str>) -> bool {
    match explicit {
        None => true,
        Some(want) => path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|have| have.eq_ignore_ascii_case(want)),
    }
}

pub(super) fn child_filter(paths: &[PathBuf], explicit: Option<&str>) -> Vec<PathBuf> {
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

pub(super) fn read_theme_inherits(roots: &[PathBuf], theme: &str) -> Option<IconThemeIndex> {
    roots.iter().find_map(|root| {
        let index_file = theme_dir(root, theme)?.join("index.theme");
        if !index_file.is_file() {
            return None;
        }
        let file = fs::File::open(index_file).ok()?;
        #[cfg(test)]
        INDEX_THEME_READS.with(|c| c.set(c.get() + 1));
        IconThemeIndex::parse_inherits(theme, std::io::BufReader::new(file))
    })
}

pub(super) fn read_theme_index(roots: &[PathBuf], theme: &str) -> Option<IconThemeIndex> {
    roots.iter().find_map(|root| {
        let index_file = theme_dir(root, theme)?.join("index.theme");
        if !index_file.is_file() {
            return None;
        }
        let text = fs::read_to_string(index_file).ok()?;
        #[cfg(test)]
        INDEX_THEME_READS.with(|c| c.set(c.get() + 1));
        Some(IconThemeIndex::parse(theme, &text))
    })
}
