//! Byte-bounded LRU of finished text-shadow images.
//!
//! A value is the blurred, premultiplied ARGB32 image of one text run, placed at its origin (the
//! pad is a function of the blur radius, which is part of the key). It holds no position and no
//! clip: placement and clipping are applied after lookup, so a run that moves reuses its image.
//!
//! The key covers every input that changes the stored pixels: font family and text (the glyph
//! coverage), size and weight (the Xft face), blur radius (the kernel and pad) and colour (the
//! RGB and the alpha scale). Two runs that differ in any of these never share an image.
//!
//! Capacity is [`TEXT_SHADOW_CACHE_BYTES`] of image bytes (4 bytes per pixel). When an insert
//! would exceed it, least-recently-used entries are dropped until it fits. An image larger than
//! the whole budget is returned to the caller but not retained.

use std::cell::Cell;
use std::collections::HashMap;
use std::os::raw::c_ulong;
use std::rc::Rc;

/// Upper bound on the summed ARGB32 image bytes one cache holds (16 MiB).
///
/// Sized to the settings window's working set: 552 distinct shadows, 14.3 MiB of images, measured
/// on one startup. A 4 MiB cap evicted in the repeating paint order and missed 1186 of 2936
/// draws. Each cached image also has a server-side pixmap of the same size, so this is up to
/// 16 MiB on the client and 16 MiB on the X server per process.
pub const TEXT_SHADOW_CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Identity of one shadow image. `size` and `radius` are stored as bit patterns, so keys compare
/// exactly as the pixels were computed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShadowKey {
    family: String,
    text: String,
    size_bits: u32,
    weight: u16,
    radius_bits: u32,
    color: [u8; 4],
}

impl ShadowKey {
    pub fn new(
        family: &str,
        text: &str,
        size: f32,
        weight: u16,
        radius: f32,
        color: [u8; 4],
    ) -> Self {
        Self {
            family: family.to_owned(),
            text: text.to_owned(),
            size_bits: size.to_bits(),
            weight,
            radius_bits: radius.to_bits(),
            color,
        }
    }
}

/// Premultiplied ARGB32 pixels of one shadow, row-major, origin at the image's top-left.
#[derive(Debug, PartialEq, Eq)]
pub struct ShadowImage {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u32>,
    /// Server-side copy of `argb`, set once on first draw: (pixmap, accounted bytes). The owner
    /// frees it when this image leaves the cache, so a repeated run blits without re-uploading.
    pub server: Cell<Option<(c_ulong, usize)>>,
}

impl ShadowImage {
    pub fn new(width: u32, height: u32, argb: Vec<u32>) -> Self {
        Self {
            width,
            height,
            argb,
            server: Cell::new(None),
        }
    }

    fn bytes(&self) -> usize {
        self.argb.len() * std::mem::size_of::<u32>()
    }
}

struct Entry {
    image: Rc<ShadowImage>,
    last_used: u64,
}

pub struct TextShadowCache {
    entries: HashMap<ShadowKey, Entry>,
    bytes: usize,
    tick: u64,
    capacity: usize,
}

impl Default for TextShadowCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextShadowCache {
    pub fn new() -> Self {
        Self::with_capacity(TEXT_SHADOW_CACHE_BYTES)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            tick: 0,
            capacity,
        }
    }

    /// Looks up `key` and marks it most recently used.
    pub fn get(&mut self, key: &ShadowKey) -> Option<Rc<ShadowImage>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = tick;
        Some(Rc::clone(&entry.image))
    }

    pub fn contains(&self, key: &ShadowKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Stores `image` under `key` and returns it. Entries it displaces (a replaced key, or
    /// least-recently-used ones dropped for space) are pushed to `displaced` so the owner can
    /// free their server copies. An image larger than the whole capacity is returned without
    /// being retained.
    pub fn insert(
        &mut self,
        key: ShadowKey,
        image: ShadowImage,
        displaced: &mut Vec<Rc<ShadowImage>>,
    ) -> Rc<ShadowImage> {
        let image = Rc::new(image);
        let size = image.bytes();
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.image.bytes();
            displaced.push(old.image);
        }
        if size > self.capacity {
            return image;
        }
        while self.bytes + size > self.capacity {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.image.bytes();
                displaced.push(evicted.image);
            }
        }
        self.tick += 1;
        self.bytes += size;
        self.entries.insert(
            key,
            Entry {
                image: Rc::clone(&image),
                last_used: self.tick,
            },
        );
        image
    }

    /// Removes every entry and returns the images, for teardown.
    pub fn take_all(&mut self) -> Vec<Rc<ShadowImage>> {
        self.bytes = 0;
        self.entries.drain().map(|(_, entry)| entry.image).collect()
    }

    /// Summed image bytes currently retained.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, fill: u32) -> ShadowImage {
        ShadowImage::new(w, h, vec![fill; w as usize * h as usize])
    }

    fn key(text: &str) -> ShadowKey {
        ShadowKey::new("Sans", text, 13.0, 400, 2.0, [0, 0, 0, 0x80])
    }

    #[test]
    fn hit_returns_the_stored_pixels() {
        let mut cache = TextShadowCache::with_capacity(1 << 20);
        cache.insert(key("On"), image(4, 3, 0x8010_2030), &mut Vec::new());
        let hit = cache.get(&key("On")).expect("stored key hits");
        assert_eq!((hit.width, hit.height), (4, 3));
        assert_eq!(hit.argb, image(4, 3, 0x8010_2030).argb);
        assert!(cache.get(&key("Off")).is_none());
    }

    #[test]
    fn key_distinguishes_text_family_size_weight_radius_and_colour() {
        let base = key("On");
        let variants = [
            ShadowKey::new("Serif", "On", 13.0, 400, 2.0, [0, 0, 0, 0x80]),
            ShadowKey::new("Sans", "Off", 13.0, 400, 2.0, [0, 0, 0, 0x80]),
            ShadowKey::new("Sans", "On", 14.0, 400, 2.0, [0, 0, 0, 0x80]),
            ShadowKey::new("Sans", "On", 13.0, 700, 2.0, [0, 0, 0, 0x80]),
            ShadowKey::new("Sans", "On", 13.0, 400, 3.0, [0, 0, 0, 0x80]),
            ShadowKey::new("Sans", "On", 13.0, 400, 2.0, [255, 0, 0, 0x80]),
            ShadowKey::new("Sans", "On", 13.0, 400, 2.0, [0, 0, 0, 0x40]),
        ];
        for variant in &variants {
            assert_ne!(&base, variant);
        }
        let mut cache = TextShadowCache::with_capacity(1 << 20);
        cache.insert(base.clone(), image(1, 1, 1), &mut Vec::new());
        for variant in &variants {
            assert!(cache.get(variant).is_none(), "{variant:?} must miss");
        }
    }

    #[test]
    fn eviction_keeps_bytes_within_capacity_and_drops_least_recently_used() {
        // Each 4x4 image is 64 bytes; the budget holds exactly three.
        let mut cache = TextShadowCache::with_capacity(3 * 64);
        cache.insert(key("a"), image(4, 4, 1), &mut Vec::new());
        cache.insert(key("b"), image(4, 4, 2), &mut Vec::new());
        cache.insert(key("c"), image(4, 4, 3), &mut Vec::new());
        assert!(cache.get(&key("a")).is_some(), "touch a so b is oldest");
        cache.insert(key("d"), image(4, 4, 4), &mut Vec::new());
        assert!(cache.bytes() <= 3 * 64);
        assert_eq!(cache.len(), 3);
        assert!(cache.get(&key("b")).is_none(), "b was least recently used");
        assert!(cache.get(&key("a")).is_some());
        assert!(cache.get(&key("c")).is_some());
        assert!(cache.get(&key("d")).is_some());
    }

    #[test]
    fn oversized_image_is_returned_but_not_retained() {
        let mut cache = TextShadowCache::with_capacity(64);
        let mut displaced = Vec::new();
        let returned = cache.insert(key("big"), image(8, 8, 9), &mut displaced);
        assert!(displaced.is_empty());
        assert_eq!(returned.argb.len(), 64);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn replacing_a_key_does_not_double_count_bytes() {
        let mut cache = TextShadowCache::with_capacity(1 << 20);
        cache.insert(key("x"), image(4, 4, 1), &mut Vec::new());
        let mut displaced = Vec::new();
        cache.insert(key("x"), image(4, 4, 2), &mut displaced);
        assert_eq!(displaced.len(), 1, "the replaced image is handed back");
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 64);
    }

    #[test]
    fn default_capacity_is_sixteen_mebibytes() {
        assert_eq!(TextShadowCache::new().capacity, 16 * 1024 * 1024);
    }
}
