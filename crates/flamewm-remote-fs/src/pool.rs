//! Connection pool. ssh's ControlMaster holds the real socket; the pool decides when a
//! connection must be (re)established or re-verified, and evicts idle masters.

use crate::transport::ConnKey;
use crate::Error;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Transport operations the pool needs. Implemented by `SshConnector`; tests use fakes.
pub trait Connector: Send + Sync {
    fn establish(&self, key: &ConnKey) -> Result<(), Error>;
    fn is_alive(&self, key: &ConnKey) -> bool;
    fn close(&self, key: &ConnKey);
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    verified: Instant,
    used: Instant,
}

pub struct Pool<C: Connector> {
    conn: C,
    slots: Mutex<HashMap<ConnKey, Slot>>,
    /// Re-check liveness only after this interval since the last successful check.
    verify_after: Duration,
    /// Close masters unused for this long.
    idle: Duration,
}

impl<C: Connector> Pool<C> {
    pub fn new(conn: C, verify_after: Duration, idle: Duration) -> Self {
        Self { conn, slots: Mutex::new(HashMap::new()), verify_after, idle }
    }

    /// Ensure a live connection for `key`, reusing the existing master when verified recently.
    pub fn ensure(&self, key: &ConnKey) -> Result<(), Error> {
        self.ensure_at(key, Instant::now())
    }

    fn ensure_at(&self, key: &ConnKey, now: Instant) -> Result<(), Error> {
        for stale in self.take_idle(now) {
            self.conn.close(&stale);
        }
        let known = {
            let mut slots = self.slots.lock().map_err(|_| Error::Io("pool poisoned".into()))?;
            match slots.get_mut(key) {
                Some(slot) => {
                    slot.used = now;
                    if now.duration_since(slot.verified) < self.verify_after {
                        return Ok(());
                    }
                    true
                }
                None => false,
            }
        };
        if known && self.conn.is_alive(key) {
            self.touch(key, now, true)?;
            return Ok(());
        }
        // Establish outside the lock so other hosts are not blocked by a slow handshake.
        self.conn.establish(key)?;
        self.touch(key, now, true)
    }

    fn touch(&self, key: &ConnKey, now: Instant, verified: bool) -> Result<(), Error> {
        let mut slots = self.slots.lock().map_err(|_| Error::Io("pool poisoned".into()))?;
        let slot = slots.entry(key.clone()).or_insert(Slot { verified: now, used: now });
        slot.used = now;
        if verified {
            slot.verified = now;
        }
        Ok(())
    }

    /// Forget a connection after a transport failure so the next `ensure` re-establishes it.
    pub fn drop_key(&self, key: &ConnKey) {
        if let Ok(mut slots) = self.slots.lock() {
            slots.remove(key);
        }
        self.conn.close(key);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.lock().map(|s| s.len()).unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn take_idle(&self, now: Instant) -> Vec<ConnKey> {
        let Ok(mut slots) = self.slots.lock() else { return Vec::new() };
        let stale: Vec<ConnKey> = slots
            .iter()
            .filter(|(_, s)| now.duration_since(s.used) > self.idle)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &stale {
            slots.remove(k);
        }
        stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Fake {
        establishes: AtomicUsize,
        alive: AtomicUsize,
        closes: AtomicUsize,
        fail_next: std::sync::atomic::AtomicBool,
    }

    impl Connector for std::sync::Arc<Fake> {
        fn establish(&self, _: &ConnKey) -> Result<(), Error> {
            self.establishes.fetch_add(1, Ordering::SeqCst);
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(Error::ConnectionLost("refused".into()));
            }
            Ok(())
        }
        fn is_alive(&self, _: &ConnKey) -> bool {
            self.alive.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn close(&self, _: &ConnKey) {
            self.closes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn key(h: &str) -> ConnKey {
        ConnKey { user: None, host: h.into(), port: 22 }
    }

    #[test]
    fn reuses_connection_within_verify_window() {
        let fake = std::sync::Arc::new(Fake::default());
        let pool = Pool::new(fake.clone(), Duration::from_secs(30), Duration::from_secs(300));
        for _ in 0..10 {
            pool.ensure(&key("a")).unwrap();
        }
        assert_eq!(fake.establishes.load(Ordering::SeqCst), 1);
        assert_eq!(fake.alive.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn rechecks_after_verify_window_without_reconnecting_when_alive() {
        let fake = std::sync::Arc::new(Fake::default());
        let pool = Pool::new(fake.clone(), Duration::from_secs(30), Duration::from_secs(300));
        let t0 = Instant::now();
        pool.ensure_at(&key("a"), t0).unwrap();
        pool.ensure_at(&key("a"), t0 + Duration::from_secs(31)).unwrap();
        assert_eq!(fake.alive.load(Ordering::SeqCst), 1);
        assert_eq!(fake.establishes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn drop_forces_reestablish_and_failures_propagate() {
        let fake = std::sync::Arc::new(Fake::default());
        let pool = Pool::new(fake.clone(), Duration::from_secs(30), Duration::from_secs(300));
        pool.ensure(&key("a")).unwrap();
        pool.drop_key(&key("a"));
        assert_eq!(fake.closes.load(Ordering::SeqCst), 1);
        fake.fail_next.store(true, Ordering::SeqCst);
        assert!(matches!(pool.ensure(&key("a")), Err(Error::ConnectionLost(_))));
        assert!(pool.is_empty());
        pool.ensure(&key("a")).unwrap();
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn idle_masters_are_evicted_and_closed() {
        let fake = std::sync::Arc::new(Fake::default());
        let pool = Pool::new(fake.clone(), Duration::from_secs(30), Duration::from_secs(300));
        let t0 = Instant::now();
        pool.ensure_at(&key("a"), t0).unwrap();
        pool.ensure_at(&key("b"), t0 + Duration::from_secs(200)).unwrap();
        pool.ensure_at(&key("c"), t0 + Duration::from_secs(400)).unwrap();
        assert_eq!(fake.closes.load(Ordering::SeqCst), 1, "only 'a' is idle >300s");
        assert_eq!(pool.len(), 2);
    }
}
