//! Byte-bounded LRU of A8 text coverage masks used by the text shadow path.
//!
//! The cached mask is colour-independent white-glyph coverage. Colour, scale
//! (already folded into `size_bits`) and the clip are applied after lookup, so
//! none of them belong in the key. The font family is part of the key because
//! it selects the Xft face that produced the coverage.
//!
//! Lookups borrow their key through `TextMaskRef`, so a cache hit allocates
//! nothing. Only an insert copies the family and text into an owned
//! `TextMaskKey`.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Upper bound on the summed mask bytes held by one cache.
pub const TEXT_MASK_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// Read-only view of the key fields. Owned and borrowed keys both implement it,
/// and the map hashes and compares through it so they always agree.
pub trait MaskKeyView {
    fn family(&self) -> &str;
    fn text(&self) -> &str;
    fn size_bits(&self) -> u32;
    fn weight(&self) -> u16;
    fn pad(&self) -> u32;
}

fn hash_view<H: Hasher>(view: &dyn MaskKeyView, state: &mut H) {
    view.family().hash(state);
    view.text().hash(state);
    view.size_bits().hash(state);
    view.weight().hash(state);
    view.pad().hash(state);
}

impl<'a> Hash for dyn MaskKeyView + 'a {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_view(self, state);
    }
}

impl<'a> PartialEq for dyn MaskKeyView + 'a {
    fn eq(&self, other: &Self) -> bool {
        self.family() == other.family()
            && self.text() == other.text()
            && self.size_bits() == other.size_bits()
            && self.weight() == other.weight()
            && self.pad() == other.pad()
    }
}

impl<'a> Eq for dyn MaskKeyView + 'a {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMaskKey {
    family: String,
    text: String,
    size_bits: u32,
    weight: u16,
    pad: u32,
}

impl TextMaskKey {
    pub fn new(family: &str, text: &str, size: f32, weight: u16, pad: u32) -> Self {
        Self {
            family: family.to_string(),
            text: text.to_string(),
            size_bits: size.to_bits(),
            weight,
            pad,
        }
    }
}

impl MaskKeyView for TextMaskKey {
    fn family(&self) -> &str {
        &self.family
    }
    fn text(&self) -> &str {
        &self.text
    }
    fn size_bits(&self) -> u32 {
        self.size_bits
    }
    fn weight(&self) -> u16 {
        self.weight
    }
    fn pad(&self) -> u32 {
        self.pad
    }
}

impl Hash for TextMaskKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_view(self, state);
    }
}

impl<'a> Borrow<dyn MaskKeyView + 'a> for TextMaskKey {
    fn borrow(&self) -> &(dyn MaskKeyView + 'a) {
        self
    }
}

/// Borrowed lookup key. Build one per paint and pass it to `get_into`; no copy.
#[derive(Clone, Copy, Debug)]
pub struct TextMaskRef<'a> {
    family: &'a str,
    text: &'a str,
    size_bits: u32,
    weight: u16,
    pad: u32,
}

impl<'a> TextMaskRef<'a> {
    pub fn new(family: &'a str, text: &'a str, size: f32, weight: u16, pad: u32) -> Self {
        Self {
            family,
            text,
            size_bits: size.to_bits(),
            weight,
            pad,
        }
    }
}

impl MaskKeyView for TextMaskRef<'_> {
    fn family(&self) -> &str {
        self.family
    }
    fn text(&self) -> &str {
        self.text
    }
    fn size_bits(&self) -> u32 {
        self.size_bits
    }
    fn weight(&self) -> u16 {
        self.weight
    }
    fn pad(&self) -> u32 {
        self.pad
    }
}

#[derive(Debug)]
struct Entry {
    width: u32,
    height: u32,
    mask: Vec<u8>,
    stamp: u64,
}

#[derive(Debug)]
pub struct TextMaskCache {
    entries: HashMap<TextMaskKey, Entry>,
    bytes: usize,
    budget: usize,
    clock: u64,
}

impl Default for TextMaskCache {
    fn default() -> Self {
        Self::with_budget(TEXT_MASK_CACHE_BYTES)
    }
}

impl TextMaskCache {
    pub fn with_budget(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            budget,
            clock: 0,
        }
    }

    /// Copy a cached mask into `out` and return its dimensions.
    pub fn get_into(&mut self, key: &dyn MaskKeyView, out: &mut Vec<u8>) -> Option<(u32, u32)> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.stamp = self.clock;
        out.clear();
        out.extend_from_slice(&entry.mask);
        Some((entry.width, entry.height))
    }

    /// Store a mask. Masks larger than the whole budget are not retained.
    pub fn insert(&mut self, key: TextMaskKey, width: u32, height: u32, mask: &[u8]) {
        if mask.len() > self.budget {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.mask.len();
        }
        self.clock += 1;
        self.bytes += mask.len();
        self.entries.insert(
            key,
            Entry {
                width,
                height,
                mask: mask.to_vec(),
                stamp: self.clock,
            },
        );
        while self.bytes > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.stamp)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.mask.len();
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> TextMaskKey {
        TextMaskKey::new("Sans", text, 14.0, 400, 4)
    }

    fn probe(text: &str) -> TextMaskRef<'_> {
        TextMaskRef::new("Sans", text, 14.0, 400, 4)
    }

    #[test]
    fn hit_returns_stored_mask_and_dims() {
        let mut cache = TextMaskCache::with_budget(1024);
        cache.insert(key("abc"), 2, 3, &[1, 2, 3, 4, 5, 6]);
        let mut out = vec![9; 2];
        assert_eq!(cache.get_into(&probe("abc"), &mut out), Some((2, 3)));
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(cache.get_into(&probe("abd"), &mut out), None);
    }

    #[test]
    fn owned_and_borrowed_keys_hash_and_compare_alike() {
        use std::collections::hash_map::DefaultHasher;
        fn digest<T: Hash + ?Sized>(value: &T) -> u64 {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        }
        let owned = key("abc");
        let borrowed = probe("abc");
        assert_eq!(digest(&owned), digest(&borrowed as &dyn MaskKeyView));
        assert!(*(&owned as &dyn MaskKeyView) == *(&borrowed as &dyn MaskKeyView));
    }

    #[test]
    fn bench_mask_cache_hits() {
        let mut cache = TextMaskCache::with_budget(1 << 20);
        let mut out = Vec::new();
        let (mut hits, mut misses) = (0u32, 0u32);
        let words = ["alpha", "beta", "gamma", "delta"];
        for _ in 0..25 {
            for word in words {
                if cache.get_into(&probe(word), &mut out).is_some() {
                    hits += 1;
                } else {
                    misses += 1;
                    cache.insert(key(word), 2, 2, &[0; 4]);
                }
            }
        }
        eprintln!("BENCH mask_cache_hits={hits} mask_cache_misses={misses}");
        assert_eq!((hits, misses), (96, 4));
    }

    #[test]
    fn key_distinguishes_family_size_weight_pad() {
        let mut cache = TextMaskCache::with_budget(1024);
        cache.insert(key("abc"), 1, 1, &[7]);
        let mut out = Vec::new();
        assert!(
            cache
                .get_into(&TextMaskRef::new("Mono", "abc", 14.0, 400, 4), &mut out)
                .is_none()
        );
        assert!(
            cache
                .get_into(&TextMaskRef::new("Sans", "abc", 15.0, 400, 4), &mut out)
                .is_none()
        );
        assert!(
            cache
                .get_into(&TextMaskRef::new("Sans", "abc", 14.0, 700, 4), &mut out)
                .is_none()
        );
        assert!(
            cache
                .get_into(&TextMaskRef::new("Sans", "abc", 14.0, 400, 5), &mut out)
                .is_none()
        );
        assert!(cache.get_into(&probe("abc"), &mut out).is_some());
    }

    #[test]
    fn evicts_least_recently_used_to_stay_within_budget() {
        let mut cache = TextMaskCache::with_budget(20);
        cache.insert(key("a"), 1, 8, &[0; 8]);
        cache.insert(key("b"), 1, 8, &[0; 8]);
        let mut out = Vec::new();
        // Touch "a" so "b" becomes the least recently used entry.
        assert!(cache.get_into(&probe("a"), &mut out).is_some());
        cache.insert(key("c"), 1, 8, &[0; 8]);
        assert!(cache.bytes() <= 20);
        assert!(cache.get_into(&probe("b"), &mut out).is_none());
        assert!(cache.get_into(&probe("a"), &mut out).is_some());
        assert!(cache.get_into(&probe("c"), &mut out).is_some());
    }

    #[test]
    fn oversize_mask_is_not_retained() {
        let mut cache = TextMaskCache::with_budget(4);
        cache.insert(key("big"), 3, 3, &[0; 9]);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn reinsert_replaces_bytes_without_leaking_budget() {
        let mut cache = TextMaskCache::with_budget(100);
        cache.insert(key("x"), 1, 4, &[0; 4]);
        cache.insert(key("x"), 1, 6, &[0; 6]);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 6);
        cache.clear();
        assert_eq!(cache.bytes(), 0);
        assert!(cache.is_empty());
    }
}
