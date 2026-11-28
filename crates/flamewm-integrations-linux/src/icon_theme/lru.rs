//! Pure bounded LRU cache: dependency-free recency and eviction.
//!
//! Step 1 of the icon-theme pipeline remaster. This module has no filesystem,
//! theme, or rendering dependency, and no current call site uses it yet, so it
//! changes no lookup behavior and no pixel. The wiring plan is recorded in
//! `docs/swarm/wave2/w44-remaster-icon-theme-pipeline-design.md`.
//!
//! Recency is recorded lazily: `order` holds `(key, stamp)` records stamped on
//! every `get`/`insert`. A record is live only while its stamp matches the
//! entry's current stamp. Stale records are skipped during eviction and
//! compacted once they outnumber live entries, which keeps every operation
//! amortized O(1) without a pointer-linked list or unsafe code.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// A bounded least-recently-used cache.
///
/// `get` and `insert` refresh recency; `peek` and `contains` do not. Inserting
/// a new key at capacity evicts the least recently used live entry.
#[derive(Clone, Debug)]
pub struct BoundedLru<K, V> {
    capacity: usize,
    entries: HashMap<K, (V, u64)>,
    order: VecDeque<(K, u64)>,
    clock: u64,
}

impl<K: Clone + Eq + Hash, V> BoundedLru<K, V> {
    /// Creates a cache holding at most `capacity` entries (minimum one).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
            clock: 0,
        }
    }

    fn stamp(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    fn is_live(&self, key: &K, stamp: u64) -> bool {
        self.entries
            .get(key)
            .is_some_and(|(_, live)| *live == stamp)
    }

    /// Reads a value and refreshes its recency, or `None` on a miss.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let stamp = self.stamp();
        match self.entries.get_mut(key) {
            Some(entry) => entry.1 = stamp,
            None => return None,
        }
        self.order.push_back((key.clone(), stamp));
        self.compact();
        self.entries.get(key).map(|(value, _)| value)
    }

    /// Reads a value without changing recency.
    #[must_use]
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|(value, _)| value)
    }

    /// Membership test without changing recency.
    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Inserts or replaces a value, refreshing recency. Returns the replaced
    /// value, if any. A new key evicts the least recently used live entry.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let stamp = self.stamp();
        if let Some(entry) = self.entries.get_mut(&key) {
            let old = std::mem::replace(&mut entry.0, value);
            entry.1 = stamp;
            self.order.push_back((key.clone(), stamp));
            self.compact();
            return Some(old);
        }
        while self.entries.len() >= self.capacity {
            let Some((old_key, old_stamp)) = self.order.pop_front() else {
                break;
            };
            if self.is_live(&old_key, old_stamp) {
                self.entries.remove(&old_key);
            }
        }
        self.order.push_back((key.clone(), stamp));
        self.entries.insert(key, (value, stamp));
        self.compact();
        None
    }

    /// Removes one entry and returns its value.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.entries.remove(key).map(|(value, _)| value)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Drops stale recency records once they dominate, bounding `order` to
    /// roughly twice the live entry count.
    fn compact(&mut self) {
        if self.order.len() <= 2 * self.entries.len() + 16 {
            return;
        }
        let entries = &self.entries;
        self.order
            .retain(|(key, stamp)| entries.get(key).is_some_and(|(_, live)| *live == *stamp));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> BoundedLru<&'static str, u32> {
        BoundedLru::new(2)
    }

    #[test]
    fn evicts_least_recently_used_at_capacity() {
        let mut cache = small();
        cache.insert("a", 1);
        cache.insert("b", 2);
        cache.insert("c", 3);

        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), Some(&2));
        assert_eq!(cache.get(&"c"), Some(&3));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn get_refreshes_recency() {
        let mut cache = small();
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.get(&"a"), Some(&1));

        cache.insert("c", 3);
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.get(&"b"), None);
    }

    #[test]
    fn peek_and_contains_do_not_refresh_recency() {
        let mut cache = small();
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert!(cache.contains(&"a"));
        assert_eq!(cache.peek(&"a"), Some(&1));

        cache.insert("c", 3);
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), Some(&2));
        assert_eq!(cache.get(&"c"), Some(&3));
    }

    #[test]
    fn replace_keeps_len_and_refreshes_recency() {
        let mut cache = small();
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.insert("a", 9), Some(1));
        assert_eq!(cache.len(), 2);

        cache.insert("c", 3);
        assert_eq!(cache.get(&"a"), Some(&9));
        assert_eq!(cache.get(&"b"), None);
    }

    #[test]
    fn stale_recency_records_are_compacted_without_changing_order() {
        let mut cache = BoundedLru::new(3);
        for key in ["a", "b", "c"] {
            cache.insert(key, 0);
        }
        for _ in 0..100 {
            assert!(cache.get(&"a").is_some());
        }
        assert!(cache.order.len() <= 2 * cache.len() + 16 + 1);

        // Recency is b (oldest), c, a. The next insert must evict b.
        cache.insert("d", 0);
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some(&0));
        assert_eq!(cache.get(&"c"), Some(&0));
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn zero_capacity_is_clamped_to_one() {
        let mut cache = BoundedLru::new(0);
        assert_eq!(cache.capacity(), 1);
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), Some(&2));
    }

    #[test]
    fn remove_and_clear_are_recency_safe() {
        let mut cache = small();
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.remove(&"a"), Some(1));

        cache.insert("c", 3);
        assert_eq!(cache.get(&"b"), Some(&2));
        assert_eq!(cache.get(&"a"), None);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.get(&"b"), None);
    }

    #[test]
    fn values_are_generic() {
        let mut cache: BoundedLru<u8, String> = BoundedLru::new(2);
        cache.insert(1, "one".to_owned());
        assert_eq!(cache.get(&1).map(String::as_str), Some("one"));
    }
}
