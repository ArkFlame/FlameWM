//! Alt+Tab thumbnail policy: target size, cache key and a bounded LRU of captured pixels.
//!
//! Pure: no X state, no pixels, no allocation beyond the cache itself. The capture path (a later
//! slice) asks `thumb_size` for the target rect, keys its pixels by `ThumbKey`, and stores them in
//! `ThumbCache`, which evicts the least recently shown entry once `MAX_THUMBS` is reached. Captures
//! are lazy (taken when the switcher opens) and freed on close or eviction; nothing stays resident
//! while idle (see docs/backlog/alttab.md).

use std::collections::VecDeque;

/// Thumbnail box in device pixels; the card renders at most this size.
pub(crate) const THUMB_MAX_W: u32 = 160;
pub(crate) const THUMB_MAX_H: u32 = 100;

/// Bytes per thumbnail pixel (32-bit `ZPixmap` after conversion).
pub(crate) const THUMB_BYTES_PER_PIXEL: u64 = 4;

/// Upper bound on resident thumbnails. `24 * 160 * 100 * 4` = `1_536_000` bytes (about 1.46 MiB),
/// under the 2 MiB transient-peak budget in `docs/competitors/vela/GAP_PLAN.md`.
pub(crate) const MAX_THUMBS: usize = 24;

/// Aspect-fit a source window size into the thumbnail box. Never upscales: a window smaller than
/// the box keeps its own size. Returns `None` for an empty source (nothing to capture).
pub(crate) fn thumb_size(src_w: u32, src_h: u32) -> Option<(u32, u32)> {
    if src_w == 0 || src_h == 0 {
        return None;
    }
    if src_w <= THUMB_MAX_W && src_h <= THUMB_MAX_H {
        return Some((src_w, src_h));
    }
    // Scale by the tighter axis, rounding down, keeping at least one pixel per axis.
    let scale_w = u64::from(THUMB_MAX_W) * u64::from(src_h);
    let scale_h = u64::from(THUMB_MAX_H) * u64::from(src_w);
    if scale_w <= scale_h {
        // Width is the tighter bound.
        let h = (u64::from(src_h) * u64::from(THUMB_MAX_W) / u64::from(src_w)).max(1);
        Some((THUMB_MAX_W, u32::try_from(h).unwrap_or(THUMB_MAX_H)))
    } else {
        let w = (u64::from(src_w) * u64::from(THUMB_MAX_H) / u64::from(src_h)).max(1);
        Some((u32::try_from(w).unwrap_or(THUMB_MAX_W), THUMB_MAX_H))
    }
}

/// Bytes one thumbnail of `(w, h)` occupies.
pub(crate) fn thumb_bytes(w: u32, h: u32) -> u64 {
    u64::from(w) * u64::from(h) * THUMB_BYTES_PER_PIXEL
}

/// Cache key: a window plus its source size. A resize changes the key, so stale pixels miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThumbKey {
    pub(crate) window: u32,
    pub(crate) src_w: u32,
    pub(crate) src_h: u32,
}

/// Bounded LRU. Front is least recently used, back most recently used.
#[derive(Debug)]
pub(crate) struct ThumbCache<V> {
    entries: VecDeque<(ThumbKey, V)>,
    capacity: usize,
}

impl<V> Default for ThumbCache<V> {
    fn default() -> Self {
        Self::with_capacity(MAX_THUMBS)
    }
}

impl<V> ThumbCache<V> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Looks up `key` and marks it most recently used.
    pub(crate) fn get(&mut self, key: ThumbKey) -> Option<&V> {
        let index = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(index)?;
        self.entries.push_back(entry);
        self.entries.back().map(|(_, v)| v)
    }

    /// Inserts or replaces `key`; returns the value evicted to stay within capacity, if any.
    pub(crate) fn insert(&mut self, key: ThumbKey, value: V) -> Option<V> {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(index);
        }
        self.entries.push_back((key, value));
        if self.entries.len() > self.capacity {
            return self.entries.pop_front().map(|(_, v)| v);
        }
        None
    }

    /// Drops every entry for `window` (window closed or unmapped); returns how many went.
    pub(crate) fn remove_window(&mut self, window: u32) -> usize {
        let before = self.entries.len();
        self.entries.retain(|(k, _)| k.window != window);
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_window_keeps_its_size() {
        assert_eq!(thumb_size(80, 60), Some((80, 60)));
        assert_eq!(thumb_size(160, 100), Some((160, 100)));
    }

    #[test]
    fn large_window_fits_the_box_keeping_aspect() {
        // 1280x800 is 16:10: the box is 160x100 exactly.
        assert_eq!(thumb_size(1280, 800), Some((160, 100)));
        // 1920x1080 is 16:9: width bound, height 90.
        assert_eq!(thumb_size(1920, 1080), Some((160, 90)));
        // Tall window: height bound, width 50.
        assert_eq!(thumb_size(500, 1000), Some((50, 100)));
    }

    #[test]
    fn extreme_aspect_keeps_one_pixel() {
        assert_eq!(thumb_size(10_000, 1), Some((160, 1)));
        assert_eq!(thumb_size(1, 10_000), Some((1, 100)));
    }

    #[test]
    fn empty_source_has_no_thumbnail() {
        assert_eq!(thumb_size(0, 600), None);
        assert_eq!(thumb_size(800, 0), None);
    }

    #[test]
    fn resident_budget_stays_under_two_mib() {
        let per = thumb_bytes(THUMB_MAX_W, THUMB_MAX_H);
        assert_eq!(per, 64_000);
        assert!(per * MAX_THUMBS as u64 <= 2 * 1024 * 1024);
    }

    fn key(window: u32) -> ThumbKey {
        ThumbKey {
            window,
            src_w: 1280,
            src_h: 800,
        }
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut cache = ThumbCache::with_capacity(2);
        assert_eq!(cache.insert(key(1), "a"), None);
        assert_eq!(cache.insert(key(2), "b"), None);
        assert_eq!(cache.get(key(1)), Some(&"a")); // 1 is now most recent
        assert_eq!(cache.insert(key(3), "c"), Some("b"));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(key(2)), None);
        assert_eq!(cache.get(key(1)), Some(&"a"));
    }

    #[test]
    fn resize_is_a_new_key() {
        let mut cache = ThumbCache::with_capacity(4);
        cache.insert(key(7), 1);
        let resized = ThumbKey {
            src_w: 640,
            ..key(7)
        };
        assert_eq!(cache.get(resized), None);
        assert_eq!(cache.get(key(7)), Some(&1));
    }

    #[test]
    fn reinsert_replaces_without_growing() {
        let mut cache = ThumbCache::with_capacity(2);
        cache.insert(key(1), 1);
        assert_eq!(cache.insert(key(1), 2), None);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(key(1)), Some(&2));
    }

    #[test]
    fn remove_window_drops_all_its_sizes() {
        let mut cache = ThumbCache::with_capacity(8);
        cache.insert(key(1), 1);
        cache.insert(
            ThumbKey {
                src_w: 640,
                ..key(1)
            },
            2,
        );
        cache.insert(key(2), 3);
        assert_eq!(cache.remove_window(1), 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn default_cache_is_bounded_by_max_thumbs() {
        let mut cache: ThumbCache<u32> = ThumbCache::default();
        for w in 0..(MAX_THUMBS as u32 + 5) {
            cache.insert(key(w), w);
        }
        assert_eq!(cache.len(), MAX_THUMBS);
        assert_eq!(cache.get(key(0)), None);
    }
}
