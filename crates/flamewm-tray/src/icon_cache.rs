//! Bounded, keyed cache for decoded tray icon pixmaps. Pure: no bus, no thread, no I/O.
//!
//! Ported shape: `.vendor/vela-main/shell/src/tray.h:88,103-106` keeps one keyed
//! `QHash<QString, QImage>` of tray images instead of rebuilding buffers per projection.
//! Here the key is a stable content fingerprint of an [`IconPixmap`] rather than an
//! "address/serial" string, so identical icons from different items share one entry.

use std::collections::{HashMap, VecDeque};

use crate::item::IconPixmap;

/// Absolute budget for retained decoded tray icon bytes.
pub const ICON_CACHE_BUDGET_BYTES: usize = 4 * 1024 * 1024;

const ENTRY_OVERHEAD_BYTES: usize = 64;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Stable identity of a decoded icon: dimensions plus a content fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IconCacheKey {
    /// Pixel width of the pixmap.
    pub width: u32,
    /// Pixel height of the pixmap.
    pub height: u32,
    /// FNV-1a fingerprint of the unchanged ARGB bytes.
    pub fingerprint: u64,
}

impl IconCacheKey {
    /// Computes the key without taking ownership of the buffer.
    #[must_use]
    pub fn for_pixmap(pixmap: &IconPixmap) -> Self {
        let mut fingerprint = FNV_OFFSET;
        for byte in &pixmap.argb {
            fingerprint ^= u64::from(*byte);
            fingerprint = fingerprint.wrapping_mul(FNV_PRIME);
        }
        Self {
            width: pixmap.width,
            height: pixmap.height,
            fingerprint,
        }
    }
}

fn entry_cost(pixmap: &IconPixmap) -> usize {
    pixmap.argb.len().saturating_add(ENTRY_OVERHEAD_BYTES)
}

/// Byte-bounded LRU of decoded icons. An entry larger than the whole budget is not
/// retained; the caller still receives its key so it can render the pixmap it owns.
#[derive(Debug)]
pub struct IconCache {
    entries: HashMap<IconCacheKey, IconPixmap>,
    lru: VecDeque<IconCacheKey>,
    bytes: usize,
    budget: usize,
}

impl Default for IconCache {
    fn default() -> Self {
        Self::with_budget(ICON_CACHE_BUDGET_BYTES)
    }
}

impl IconCache {
    /// Creates a cache with the default [`ICON_CACHE_BUDGET_BYTES`] budget.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty cache with an explicit byte budget.
    #[must_use]
    pub fn with_budget(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            lru: VecDeque::new(),
            bytes: 0,
            budget,
        }
    }

    /// The absolute byte budget this cache enforces.
    #[must_use]
    pub const fn budget_bytes(&self) -> usize {
        self.budget
    }

    /// Exact bytes retained, including per-entry overhead. Never exceeds [`Self::budget_bytes`].
    #[must_use]
    pub const fn memory_estimate_bytes(&self) -> usize {
        self.bytes
    }

    /// Number of retained icons.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no icons are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True when `key` is currently retained.
    #[must_use]
    pub fn contains_key(&self, key: &IconCacheKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Borrows a retained icon without changing its recency.
    #[must_use]
    pub fn peek(&self, key: &IconCacheKey) -> Option<&IconPixmap> {
        self.entries.get(key)
    }

    /// Borrows a retained icon and marks it most-recently-used.
    #[must_use]
    pub fn get(&mut self, key: &IconCacheKey) -> Option<&IconPixmap> {
        if self.entries.contains_key(key) {
            self.bump(key);
        }
        self.entries.get(key)
    }

    /// Retains `pixmap` under its content key, evicting least-recently-used entries until it fits.
    /// Returns `None` when the entry is empty or larger than the whole budget; callers keep and
    /// render their own copy in that case, so admission never changes visible pixels.
    pub fn insert(&mut self, pixmap: IconPixmap) -> Option<IconCacheKey> {
        if pixmap.argb.is_empty() {
            return None;
        }
        let key = IconCacheKey::for_pixmap(&pixmap);
        let cost = entry_cost(&pixmap);
        if cost > self.budget {
            return None;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(entry_cost(&old));
            self.unbump(&key);
        }
        while self.bytes.saturating_add(cost) > self.budget {
            let Some(victim) = self.lru.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(entry_cost(&old));
            }
        }
        self.entries.insert(key, pixmap);
        self.bytes = self.bytes.saturating_add(cost);
        self.lru.push_back(key);
        Some(key)
    }

    /// Removes a retained icon and returns its bytes to the caller.
    #[must_use]
    pub fn take(&mut self, key: &IconCacheKey) -> Option<IconPixmap> {
        let pixmap = self.entries.remove(key)?;
        self.bytes = self.bytes.saturating_sub(entry_cost(&pixmap));
        self.unbump(key);
        Some(pixmap)
    }

    /// Drops every retained icon and resets the byte total.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.bytes = 0;
    }

    fn bump(&mut self, key: &IconCacheKey) {
        self.unbump(key);
        self.lru.push_back(*key);
    }

    fn unbump(&mut self, key: &IconCacheKey) {
        self.lru.retain(|existing| existing != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixmap(width: u32, height: u32, fill: u8) -> IconPixmap {
        IconPixmap {
            width,
            height,
            argb: vec![fill; (width * height * 4) as usize],
        }
    }

    #[test]
    fn key_is_stable_and_content_sensitive() {
        let a = pixmap(1, 1, 7);
        let b = pixmap(1, 1, 7);
        let c = pixmap(1, 1, 8);
        let d = pixmap(2, 1, 7);
        assert_eq!(IconCacheKey::for_pixmap(&a), IconCacheKey::for_pixmap(&b));
        assert_ne!(IconCacheKey::for_pixmap(&a), IconCacheKey::for_pixmap(&c));
        assert_ne!(IconCacheKey::for_pixmap(&a), IconCacheKey::for_pixmap(&d));
    }

    #[test]
    fn byte_budget_evicts_least_recently_used() {
        // A 1x1 icon costs 4 ARGB bytes + 64 overhead = 68 bytes.
        let mut cache = IconCache::with_budget(68 * 2);
        let first = cache.insert(pixmap(1, 1, 1)).expect("first retained");
        let second = cache.insert(pixmap(1, 1, 2)).expect("second retained");
        assert!(cache.get(&first).is_some());
        let third = cache.insert(pixmap(1, 1, 3)).expect("third retained");
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_key(&first));
        assert!(!cache.contains_key(&second));
        assert!(cache.contains_key(&third));
        assert!(cache.memory_estimate_bytes() <= cache.budget_bytes());
    }

    #[test]
    fn oversize_pixmap_is_not_retained() {
        let mut cache = IconCache::with_budget(4);
        assert_eq!(cache.insert(pixmap(1, 1, 9)), None);
        assert!(cache.is_empty());
        assert_eq!(cache.memory_estimate_bytes(), 0);
    }

    #[test]
    fn reinserting_same_content_does_not_double_count() {
        let mut cache = IconCache::new();
        let key = cache.insert(pixmap(1, 1, 5)).expect("retained");
        let again = cache.insert(pixmap(1, 1, 5)).expect("retained");
        assert_eq!(key, again);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.memory_estimate_bytes(), 68);
    }

    #[test]
    fn take_and_clear_release_bytes() {
        let mut cache = IconCache::new();
        let key = cache.insert(pixmap(1, 1, 4)).expect("retained");
        assert_eq!(cache.memory_estimate_bytes(), 68);
        let taken = cache.take(&key).expect("present");
        assert_eq!(taken.argb, vec![4, 4, 4, 4]);
        assert_eq!(cache.memory_estimate_bytes(), 0);
        let _ = cache.insert(pixmap(1, 1, 4));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.memory_estimate_bytes(), 0);
    }
}
