//! Icon raster cache: decoded RGBA rasters under a count and byte LRU budget.
use super::*;

// Retain a large working set across taskbar, Start, and launcher views.
// The icon resolver has already rasterized every image to its requested
// physical display size; no extra downscale or lossy preview is necessary.
pub(super) const ICON_CACHE_ENTRY_BUDGET: usize = 2048;

/// Maximum retained RGBA8 icon pixel payload (32 MiB, not a fixed allocation).
pub(super) const ICON_CACHE_BYTE_BUDGET: usize = 32 * 1024 * 1024;

pub type IconCacheKey = (String, u32, u32);

/// Decoded icon raster. The pixel buffer is shared: the cache and every
/// document showing the icon hold one allocation, so a hit clones an `Arc`.
#[derive(Clone)]
pub struct IconRaster {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
}

impl From<RuntimeImage> for IconRaster {
    fn from(image: RuntimeImage) -> Self {
        Self {
            source: image.source,
            width: image.width,
            height: image.height,
            pixels: Arc::new(image.pixels),
        }
    }
}

pub(super) type SizeKey = (u32, u32);

pub(super) struct IconCacheEntry {
    raster: IconRaster,
    /// Stamp of the most recent insert/get; matches this entry's newest `order` record.
    stamp: u64,
}

/// LRU via lazy recency records: `order` holds `(name, size, stamp)` triples
/// pushed on every insert/get. A record is live only if its stamp equals the
/// entry's current stamp; stale records are skipped on eviction and compacted
/// away once they outnumber live entries, keeping every operation amortized O(1).
/// Entries nest by name so lookups borrow `&str` without allocating.
pub(super) struct IconCache {
    entry_budget: usize,
    byte_budget: usize,
    bytes: usize,
    len: usize,
    clock: u64,
    entries: HashMap<Arc<str>, HashMap<SizeKey, IconCacheEntry>>,
    order: VecDeque<(Arc<str>, SizeKey, u64)>,
}

impl IconCache {
    fn new(entry_budget: usize) -> Self {
        Self::with_budgets(entry_budget, ICON_CACHE_BYTE_BUDGET)
    }

    fn with_budgets(entry_budget: usize, byte_budget: usize) -> Self {
        Self {
            entry_budget: entry_budget.max(1),
            byte_budget,
            bytes: 0,
            len: 0,
            clock: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn next_stamp(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
        self.len = 0;
    }

    fn is_live(&self, name: &str, size: SizeKey, stamp: u64) -> bool {
        self.entries
            .get(name)
            .and_then(|sizes| sizes.get(&size))
            .is_some_and(|entry| entry.stamp == stamp)
    }

    fn remove(&mut self, name: &str, size: SizeKey) {
        let Some(sizes) = self.entries.get_mut(name) else {
            return;
        };
        if let Some(entry) = sizes.remove(&size) {
            self.bytes -= entry.raster.pixels.len();
            self.len -= 1;
        }
        if sizes.is_empty() {
            self.entries.remove(name);
        }
    }

    /// Membership test that does not touch LRU recency.
    #[cfg(test)]
    fn contains(&self, name: &str, size: SizeKey) -> bool {
        self.entries
            .get(name)
            .is_some_and(|sizes| sizes.contains_key(&size))
    }

    fn get(&mut self, name: &str, size: SizeKey) -> Option<IconRaster> {
        let stamp = self.next_stamp();
        let key = self.entries.get_key_value(name)?.0.clone();
        let entry = self.entries.get_mut(name)?.get_mut(&size)?;
        entry.stamp = stamp;
        let raster = entry.raster.clone();
        self.order.push_back((key, size, stamp));
        self.compact_order();
        Some(raster)
    }

    fn insert(&mut self, name: &str, size: SizeKey, raster: IconRaster) {
        self.remove(name, size);
        let bytes = raster.pixels.len();
        if bytes > self.byte_budget {
            // An oversized raster is never retained; evicting others for it
            // would only churn the cache.
            return;
        }
        while self.len >= self.entry_budget || self.bytes + bytes > self.byte_budget {
            let Some((old_name, old_size, stamp)) = self.order.pop_front() else {
                break;
            };
            if self.is_live(&old_name, old_size, stamp) {
                self.remove(&old_name, old_size);
            }
        }
        let stamp = self.next_stamp();
        // Multiple physical sizes of one icon name share its Arc<str> key.
        // Avoid allocating a duplicate string for each HiDPI variant.
        let key: Arc<str> = self
            .entries
            .get_key_value(name)
            .map_or_else(|| Arc::from(name), |(existing, _)| Arc::clone(existing));
        self.bytes += bytes;
        self.len += 1;
        self.order.push_back((key.clone(), size, stamp));
        self.entries
            .entry(key)
            .or_default()
            .insert(size, IconCacheEntry { raster, stamp });
        self.compact_order();
    }

    /// Drops stale recency records once they dominate, bounding `order` to
    /// roughly twice the live entry count.
    fn compact_order(&mut self) {
        if self.order.len() <= 2 * self.len + 16 {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(name, size, stamp)| {
            entries
                .get(&**name)
                .and_then(|sizes| sizes.get(size))
                .is_some_and(|entry| entry.stamp == *stamp)
        });
    }
}

pub(super) fn icon_cache() -> &'static std::sync::Mutex<IconCache> {
    static CACHE: OnceLock<std::sync::Mutex<IconCache>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(IconCache::new(ICON_CACHE_ENTRY_BUDGET)))
}

pub(super) fn cached_image(name: &str, w: u32, h: u32) -> Option<IconRaster> {
    icon_cache().lock().ok()?.get(name, (w, h))
}

#[cfg(test)]
pub(super) fn cached_contains(name: &str, w: u32, h: u32) -> bool {
    icon_cache()
        .lock()
        .is_ok_and(|cache| cache.contains(name, (w, h)))
}

pub fn note_icon_result(name: &str, w: u32, h: u32, raster: impl Into<IconRaster>) {
    if let Ok(mut cache) = icon_cache().lock() {
        cache.insert(name, (w, h), raster.into());
    }
    result_counter().increment();
}

/// Drops every caller-side cached raster so a generation bump (theme or
/// palette change) never retains an old icon. Worker service keys already
/// carry the generation; this keeps the projection cache consistent with
/// them without changing call-site signatures.
pub fn invalidate_icon_cache() {
    if let Ok(mut cache) = icon_cache().lock() {
        cache.clear();
    }
}

pub fn note_loader_stats(loader: &IconLoader) {
    let bumps = [
        (queue_full_counter(), loader.queue_full_drops),
        (stale_counter(), loader.stale_drops),
        (high_counter(), loader.high_enqueued),
        (low_counter(), loader.low_enqueued),
        (coalesced_counter(), loader.coalesced),
        (dropped_low_counter(), loader.dropped_low),
    ];
    for (counter, n) in bumps {
        if n > 0 {
            counter.increment_by(n);
        }
    }
}

/// Empty transparent placeholder applied synchronously on a visible-row
/// cache miss and hidden-slot initial state. The worker result overwrites
/// it via `note_icon_result` when ready (neutral packaged/system raster,
/// never flame-red, never brand). This is the single placeholder system:
/// no second placeholder path exists.
pub(super) fn empty_image(w: u32, h: u32) -> RuntimeImage {
    let dim_w = w.max(1);
    let dim_h = h.max(1);
    RuntimeImage {
        source: "flamewm-empty".to_owned(),
        width: dim_w,
        height: dim_h,
        pixels: vec![0; dim_w as usize * dim_h as usize * 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_projection::tests::*;

    // A03: a theme change drops rasters cached under the old theme, so the next
    // Start or taskbar projection re-requests them instead of reusing old art.
    #[test]
    fn theme_change_invalidates_cached_icons() {
        let _guard = icon_test_lock().lock();
        note_icon_result("firefox", 20, 20, empty_image(20, 20));
        assert!(cached_contains("firefox", 20, 20));
        invalidate_icon_cache();
        assert!(!cached_contains("firefox", 20, 20));
    }

    #[test]
    fn icon_cache_evicts_least_recently_used_entry_at_budget() {
        let mut cache = IconCache::new(2);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(&first.0, (first.1, first.2), test_image("first").into());
        cache.insert(&second.0, (second.1, second.2), test_image("second").into());
        assert_eq!(
            cache
                .get(&first.0, (first.1, first.2))
                .expect("first cache hit")
                .source,
            "first"
        );
        cache.insert(&third.0, (third.1, third.2), test_image("third").into());

        assert!(
            cache.get(&second.0, (second.1, second.2)).is_none(),
            "least-recent entry evicted"
        );
        assert_eq!(
            cache
                .get(&first.0, (first.1, first.2))
                .expect("first retained")
                .source,
            "first"
        );
        assert_eq!(
            cache
                .get(&third.0, (third.1, third.2))
                .expect("third retained")
                .source,
            "third"
        );
    }

    #[test]
    fn icon_cache_contains_does_not_touch_lru_order() {
        let mut cache = IconCache::new(2);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(&first.0, (first.1, first.2), test_image("first").into());
        cache.insert(&second.0, (second.1, second.2), test_image("second").into());
        assert!(cache.contains(&first.0, (first.1, first.2)));
        assert!(!cache.contains(&third.0, (third.1, third.2)));
        cache.insert(&third.0, (third.1, third.2), test_image("third").into());

        // `first` was only probed, so it stays the least-recent entry.
        assert!(
            !cache.contains(&first.0, (first.1, first.2)),
            "contains must not refresh recency"
        );
        assert!(cache.contains(&second.0, (second.1, second.2)));
        assert!(cache.contains(&third.0, (third.1, third.2)));
    }

    #[test]
    fn icon_cache_get_refreshes_order_across_many_touches() {
        let mut cache = IconCache::new(3);
        let keys: Vec<IconCacheKey> = ["a", "b", "c"]
            .iter()
            .map(|name| ((*name).to_owned(), 1, 1))
            .collect();
        for key in &keys {
            cache.insert(&key.0, (key.1, key.2), test_image(&key.0).into());
        }
        // Hammer one key so stale recency records pile up and get compacted.
        for _ in 0..100 {
            assert!(cache.get(&keys[0].0, (keys[0].1, keys[0].2)).is_some());
        }
        assert!(cache.order.len() <= 2 * cache.len + 16 + 1);
        // Recency now: b (oldest), c, a. Next insert evicts b.
        cache.insert("d", (1, 1), test_image("d").into());
        assert!(
            !cache.contains(&keys[1].0, (keys[1].1, keys[1].2)),
            "least-recent live entry evicted"
        );
        assert!(cache.contains(&keys[0].0, (keys[0].1, keys[0].2)));
        assert!(cache.contains(&keys[2].0, (keys[2].1, keys[2].2)));
        assert_eq!(cache.bytes, 3 * 4);
    }

    #[test]
    fn default_icon_cache_is_bounded_for_an_idle_shell() {
        let cache = IconCache::new(ICON_CACHE_ENTRY_BUDGET);
        assert_eq!(cache.entry_budget, 2048);
        assert_eq!(cache.byte_budget, 32 * 1024 * 1024);
    }

    #[test]
    fn icon_cache_shares_name_storage_between_icon_sizes() {
        let mut cache = IconCache::with_budgets(10, 4096);
        cache.insert("browser", (16, 16), test_image("small").into());
        cache.insert("browser", (32, 32), test_image("large").into());
        let first = &cache.order[0].0;
        let second = &cache.order[1].0;
        assert!(
            Arc::ptr_eq(first, second),
            "size variants share an icon-name allocation"
        );
        assert_eq!(cache.len, 2);
        assert!(cache.get("browser", (16, 16)).is_some());
        assert!(cache.get("browser", (32, 32)).is_some());
    }

    #[test]
    fn icon_cache_byte_budget_evicts_oldest_when_exceeded() {
        // Entry budget is loose; the 8-byte byte budget must drive eviction.
        let mut cache = IconCache::with_budgets(10, 8);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(&first.0, (first.1, first.2), test_image("first").into());
        cache.insert(&second.0, (second.1, second.2), test_image("second").into());
        cache.insert(&third.0, (third.1, third.2), test_image("third").into());

        assert!(
            cache.get(&first.0, (first.1, first.2)).is_none(),
            "oldest evicted by bytes"
        );
        assert!(
            cache.get(&second.0, (second.1, second.2)).is_some(),
            "second retained"
        );
        assert!(
            cache.get(&third.0, (third.1, third.2)).is_some(),
            "third retained"
        );
        assert!(cache.bytes <= 8, "cache stays under byte budget");
    }

    #[test]
    fn icon_cache_entry_larger_than_byte_budget_is_not_retained() {
        let mut cache = IconCache::with_budgets(10, 8);
        let small = ("small".to_owned(), 1, 1);
        let huge = ("huge".to_owned(), 3, 3);
        cache.insert(&small.0, (small.1, small.2), test_image("small").into());
        cache.insert(
            &huge.0,
            (huge.1, huge.2),
            sized_test_image("huge", 3).into(),
        );

        assert!(
            cache.get(&huge.0, (huge.1, huge.2)).is_none(),
            "oversized entry not retained"
        );
        assert!(
            cache.get(&small.0, (small.1, small.2)).is_some(),
            "oversized insert evicts nothing"
        );
        assert!(cache.bytes <= 8, "cache stays under byte budget");
    }
}
