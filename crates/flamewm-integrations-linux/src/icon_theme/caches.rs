//! Per-scan caches for icon theme lookups.

use super::*;

/// Bounded per-resolver theme lookup caches (J03 warm path).
///
/// All entries are scoped by `theme_generation`; a generation bump makes old
/// entries miss without clearing live-raster bytes. No raw map is exposed;
/// use [`IconThemeCaches::stats`] / `memory_estimate_bytes`.
#[derive(Clone, Debug, Default)]
pub struct IconThemeCaches {
    /// Generation -> path -> exists. Nested so lookups borrow `&Path` with no allocation.
    pub(super) exists: HashMap<u64, HashMap<PathBuf, bool>>,
    pub(super) exists_len: usize,
    pub(super) hits: u64,
    pub(super) misses: u64,
}

pub(super) const EXISTS_CAP: usize = 4096;

impl IconThemeCaches {
    pub(super) fn exists(&mut self, path: &Path, generation: u64) -> Option<bool> {
        if let Some(hit) = self.exists.get(&generation).and_then(|m| m.get(path)) {
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
        if let Some(slot) = self
            .exists
            .get_mut(&generation)
            .and_then(|m| m.get_mut(path))
        {
            *slot = exists;
            return;
        }
        if self.exists_len >= EXISTS_CAP {
            self.evict_one_exists();
        }
        self.exists
            .entry(generation)
            .or_default()
            .insert(path.to_path_buf(), exists);
        self.exists_len += 1;
    }

    /// Drops one arbitrary entry (same policy as the former flat `bound_insert`).
    pub(super) fn evict_one_exists(&mut self) {
        let Some(generation) = self.exists.keys().next().copied() else {
            return;
        };
        let Some(inner) = self.exists.get_mut(&generation) else {
            return;
        };
        if let Some(path) = inner.keys().next().cloned() {
            inner.remove(&path);
            self.exists_len -= 1;
        }
        if inner.is_empty() {
            self.exists.remove(&generation);
        }
    }

    /// (hits, misses) across exists lookups.
    #[must_use]
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.exists_len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exists_len == 0
    }

    #[must_use]
    pub fn memory_estimate_bytes(&self) -> usize {
        self.exists
            .values()
            .flat_map(|m| m.keys())
            .map(|path| path.as_os_str().len() + 16)
            .sum()
    }

    pub(super) fn clear_generation(&mut self) {
        // Generations are key-scoped so no clear is required; this hook keeps
        // the bound small if a host cycles generations rapidly.
        while self.len() > EXISTS_CAP {
            self.evict_one_exists();
        }
    }

    pub(crate) fn note_generation_bump(&mut self) {
        self.clear_generation();
    }
}

/// Name memo keyed `name -> physical -> paths`. The nested layout lets lookups
/// borrow `&str` (no per-call `String` key); `len` counts `(name, physical)` pairs.
#[derive(Debug, Default)]
pub(super) struct NameMemo {
    pub(super) by_name: HashMap<String, HashMap<u32, Vec<PathBuf>>>,
    pub(super) len: usize,
}

impl NameMemo {
    pub(super) fn get(&self, name: &str, physical: u32) -> Option<&Vec<PathBuf>> {
        self.by_name.get(name)?.get(&physical)
    }

    /// Same policy as `bound_insert`: at capacity, drop one arbitrary pair first.
    pub(super) fn insert(&mut self, name: &str, physical: u32, paths: Vec<PathBuf>, cap: usize) {
        if let Some(slot) = self
            .by_name
            .get_mut(name)
            .and_then(|m| m.get_mut(&physical))
        {
            *slot = paths;
            return;
        }
        if self.len >= cap {
            if let Some(outer) = self.by_name.keys().next().cloned() {
                if let Some(inner) = self.by_name.get_mut(&outer) {
                    if let Some(first) = inner.keys().next().copied() {
                        inner.remove(&first);
                        self.len -= 1;
                    }
                    if inner.is_empty() {
                        self.by_name.remove(&outer);
                    }
                }
            }
        }
        self.by_name
            .entry(name.to_owned())
            .or_default()
            .insert(physical, paths);
        self.len += 1;
    }
}
