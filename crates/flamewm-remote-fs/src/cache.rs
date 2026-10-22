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
}

impl ListCache {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self { ttl, map: HashMap::new() }
    }

    pub fn get(&mut self, key: &str) -> Option<Arc<Vec<Entry>>> {
        let (at, v) = self.map.get(key)?;
        if at.elapsed() > self.ttl {
            self.map.remove(key);
            return None;
        }
        Some(Arc::clone(v))
    }

    pub fn put(&mut self, key: String, entries: Vec<Entry>) {
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
        self.map.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_and_invalidates() {
        let mut c = ListCache::new(Duration::from_millis(50));
        c.put("sftp://h:22/a".into(), Vec::new());
        assert!(c.get("sftp://h:22/a").is_some());
        c.invalidate("sftp://h:22/a");
        assert!(c.get("sftp://h:22/a").is_none());
        c.put("k".into(), Vec::new());
        std::thread::sleep(Duration::from_millis(70));
        assert!(c.get("k").is_none());
    }
}
