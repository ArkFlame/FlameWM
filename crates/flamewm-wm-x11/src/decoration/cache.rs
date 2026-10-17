//! Decoration cache: WM-owned icon/raster caches, no per-frame rediscovery.
//!
//! Native icon rasters once per slot edge, catalog `Icon=` rasters once per
//! icon name. Thread-unsafe by design: owned by the single-threaded WM event
//! loop. All caches are bounded LRU with fixed capacities; bounds are kept in
//! production.

#[cfg(test)]
use std::collections::{HashMap, VecDeque};

#[cfg(test)]
const TITLE_MEASURE_CACHE_CAPACITY: usize = 1_024;
#[cfg(test)]
const ICON_RASTER_CACHE_CAPACITY: usize = 128;

/// WM-owned frame caches: icon rasters, title measures, profiler hook.
/// Title measures cache the canonical Xft estimate
/// (`external_text_measure`, IBM Plex Sans) so layout never re-measures
/// per frame; native pixmap bytes track `external_pixmap_byte_estimate`.
/// Bounded LRU: icon rasters cap at 128 entries, title measures at 1024.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct DecorationCache {
    icon_rasters: HashMap<String, flamewm_integrations_linux::icons::Rgba8Raster>,
    icon_raster_order: VecDeque<String>,
    title_measures: HashMap<(String, u32), i32>,
    title_measure_order: VecDeque<(String, u32)>,
    image_cache_bytes: usize,
}

#[cfg(test)]
impl DecorationCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn icon_raster(
        &mut self,
        key: &str,
    ) -> Option<flamewm_integrations_linux::icons::Rgba8Raster> {
        let raster = self.icon_rasters.get(key).cloned()?;
        touch(&mut self.icon_raster_order, &key.to_owned());
        Some(raster)
    }

    pub fn insert_icon_raster(&mut self, key: String, raster: flamewm_integrations_linux::icons::Rgba8Raster) {
        lru_insert(&mut self.icon_rasters, &mut self.icon_raster_order, ICON_RASTER_CACHE_CAPACITY, key, raster);
    }

    #[must_use]
    pub fn title_measure(&mut self, title: &str, size_bits: u32) -> Option<i32> {
        let key = (title.to_owned(), size_bits);
        let width = self.title_measures.get(&key).copied()?;
        touch(&mut self.title_measure_order, &key);
        Some(width)
    }

    pub fn insert_title_measure(&mut self, title: String, size_bits: u32, width: i32) {
        lru_insert(&mut self.title_measures, &mut self.title_measure_order, TITLE_MEASURE_CACHE_CAPACITY, (title, size_bits), width);
    }

    /// Native pixmap bytes for the live-target image cache (w*h*4 per
    /// cached entry, canonical `external_pixmap_byte_estimate`).
    #[must_use]
    pub fn image_cache_bytes(&self) -> usize {
        self.image_cache_bytes
    }

    pub fn add_image_cache_bytes(&mut self, bytes: usize) {
        self.image_cache_bytes = self.image_cache_bytes.saturating_add(bytes);
    }

    pub fn clear_image_cache_bytes(&mut self) {
        self.image_cache_bytes = 0;
    }

    #[must_use]
    pub fn title_measure_count(&self) -> usize {
        self.title_measures.len()
    }

    #[must_use]
    pub fn icon_raster_count(&self) -> usize {
        self.icon_rasters.len()
    }
}

/// Convert a worker-produced straight RGBA8 catalog raster to ARGB32.
/// Resolution and decoding belong to `IconService`, never the WM event loop.
#[must_use]
pub fn catalog_icon_image(
    raster: &flamewm_integrations_linux::icons::Rgba8Raster,
) -> Option<crate::chrome::IconImage> {
    if raster.width == 0 || raster.height == 0 {
        return None;
    }
    let pixel_count = usize::try_from(raster.width)
        .ok()?
        .checked_mul(usize::try_from(raster.height).ok()?)?;
    if raster.pixels.len() != pixel_count.saturating_mul(4) {
        return None;
    }
    // Straight RGBA8 -> cached ARGB32 `IconImage` (pure memory convert).
    // Miss keeps the empty slot, never brand artwork.
    let argb = raster
        .pixels
        .chunks_exact(4)
        .map(|pixel| {
            u32::from(pixel[3]) << 24
                | u32::from(pixel[0]) << 16
                | u32::from(pixel[1]) << 8
                | u32::from(pixel[2])
        })
        .collect::<Vec<u32>>();
    Some(crate::chrome::IconImage {
        width: raster.width,
        height: raster.height,
        argb,
    })
}

#[cfg(test)]
fn touch<K: Clone + PartialEq>(order: &mut VecDeque<K>, key: &K) {
    if let Some(position) = order.iter().position(|entry| entry == key) {
        order.remove(position);
    }
    order.push_back(key.clone());
}

#[cfg(test)]
fn lru_insert<K: Clone + Eq + std::hash::Hash, V>(
    entries: &mut HashMap<K, V>,
    order: &mut VecDeque<K>,
    capacity: usize,
    key: K,
    value: V,
) {
    if entries.contains_key(&key) {
        touch(order, &key);
    } else {
        if entries.len() >= capacity {
            if let Some(oldest) = order.pop_front() {
                entries.remove(&oldest);
            }
        }
        order.push_back(key.clone());
    }
    entries.insert(key, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t24_title_measure_cache_pins_canonical_width() {
        let mut cache = DecorationCache::new();
        let size_bits = 12.0_f32.to_bits();
        assert_eq!(cache.title_measure("hello", size_bits), None);
        let (expected, _) = flamewm_render_x11::external_text_measure("hello", 12.0);
        cache.insert_title_measure("hello".to_owned(), size_bits, expected.round() as i32);
        assert_eq!(
            cache.title_measure("hello", size_bits),
            Some(expected.round() as i32)
        );
        assert_eq!(cache.title_measure_count(), 1);
    }

    #[test]
    fn icon_bytes_track_pixmap_estimate() {
        let mut cache = DecorationCache::new();
        cache.add_image_cache_bytes(flamewm_render_x11::external_pixmap_byte_estimate(2, 3));
        assert_eq!(cache.image_cache_bytes(), 24);
        cache.clear_image_cache_bytes();
        assert_eq!(cache.image_cache_bytes(), 0);
    }

    #[test]
    fn title_measure_cache_evicts_least_recently_used_entry() {
        let mut cache = DecorationCache::new();
        for index in 0..TITLE_MEASURE_CACHE_CAPACITY {
            cache.insert_title_measure(index.to_string(), 12, index as i32);
        }
        assert_eq!(cache.title_measure("0", 12), Some(0));
        cache.insert_title_measure("new".to_owned(), 12, 7);
        assert_eq!(cache.title_measure("1", 12), None);
        assert_eq!(cache.title_measure("0", 12), Some(0));
        assert_eq!(cache.title_measure_count(), TITLE_MEASURE_CACHE_CAPACITY);
    }

    #[test]
    fn icon_raster_cache_evicts_least_recently_used_entry() {
        let mut cache = DecorationCache::new();
        for index in 0..ICON_RASTER_CACHE_CAPACITY {
            cache.insert_icon_raster(index.to_string(), test_raster());
        }
        assert!(cache.icon_raster("0").is_some());
        cache.insert_icon_raster("new".to_owned(), test_raster());
        assert!(cache.icon_raster("1").is_none());
        assert!(cache.icon_raster("0").is_some());
        assert_eq!(cache.icon_raster_count(), ICON_RASTER_CACHE_CAPACITY);
    }

    #[test]
    fn worker_catalog_raster_converts_to_argb_without_resolving() {
        let raster = flamewm_integrations_linux::icons::Rgba8Raster {
            source: "worker".to_owned(),
            width: 1,
            height: 1,
            pixels: vec![0x11, 0x22, 0x33, 0x80],
            origin: flamewm_integrations_linux::icons::IconOrigin::SystemTheme,
            fallback_reason: None,
        };
        let image = catalog_icon_image(&raster).expect("valid worker raster");
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.argb, vec![0x8011_2233]);
    }

    fn test_raster() -> flamewm_integrations_linux::icons::Rgba8Raster {
        flamewm_integrations_linux::icons::Rgba8Raster {
            source: "test".to_owned(),
            width: 1,
            height: 1,
            pixels: vec![0; 4],
            origin: flamewm_integrations_linux::icons::IconOrigin::PackagedFlame,
            fallback_reason: None,
        }
    }
}
