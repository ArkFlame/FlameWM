//! Short-TTL listing cache. Keys are canonical URL strings. Mutations invalidate the target
//! and its parent so the UI never shows a stale directory after its own write.

use crate::entry::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 256;

pub struct ListCache {
    ttl: Duration,
    map: HashMap<String, (Instant, Arc<Vec<Entry>>)>,
    /// Bumped on every invalidation. A listing captured under an older generation is stale.
    generation: u64,
}

impl ListCache {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            map: HashMap::new(),
            generation: 0,
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn get(&mut self, key: &str) -> Option<Arc<Vec<Entry>>> {
        let (at, v) = self.map.get(key)?;
        if at.elapsed() > self.ttl {
            self.map.remove(key);
            return None;
        }
        Some(Arc::clone(v))
    }

    /// Store a listing captured at `generation`; dropped if any invalidation happened since.
    pub fn put(&mut self, key: String, entries: Vec<Entry>, generation: u64) {
        if generation != self.generation {
            return;
        }
        if self.map.len() >= MAX_ENTRIES {
            let ttl = self.ttl;
            self.map.retain(|_, (at, _)| at.elapsed() <= ttl);
            if self.map.len() >= MAX_ENTRIES {
                self.map.clear();
            }
        }
        self.map.insert(key, (Instant::now(), Arc::new(entries)));
    }

    pub fn invalidate(&mut self, key: &str) {
        self.generation = self.generation.wrapping_add(1);
        self.map.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_and_invalidates() {
        let mut c = ListCache::new(Duration::from_millis(50));
        c.put("sftp://h:22/a".into(), Vec::new(), c.generation());
        assert!(c.get("sftp://h:22/a").is_some());
        c.invalidate("sftp://h:22/a");
        assert!(c.get("sftp://h:22/a").is_none());
        c.put("k".into(), Vec::new(), c.generation());
        std::thread::sleep(Duration::from_millis(70));
        assert!(c.get("k").is_none());
    }

    #[test]
    fn stale_generation_put_is_dropped() {
        let mut c = ListCache::new(Duration::from_secs(60));
        let started = c.generation();
        c.invalidate("other");
        c.put("k".into(), Vec::new(), started);
        assert!(
            c.get("k").is_none(),
            "listing started before invalidation must not cache"
        );
        c.put("k".into(), Vec::new(), c.generation());
        assert!(c.get("k").is_some());
    }
}
