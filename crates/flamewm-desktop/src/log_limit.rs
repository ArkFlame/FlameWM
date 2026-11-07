//! Rate-limited stderr reporting for failures that can repeat every tick or event.
//!
//! A key prints on its first occurrence, then at most once per [`WINDOW`]. A later print
//! reports how many repeats were suppressed, so a persistent failure stays visible without
//! flooding the log.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub(crate) const WINDOW: Duration = Duration::from_secs(60);

struct Slot {
    last: Instant,
    suppressed: u64,
}

static SLOTS: Mutex<Option<HashMap<&'static str, Slot>>> = Mutex::new(None);

/// `Some(n)` when this occurrence should print, with `n` repeats suppressed since the last print;
/// `None` when it is inside the window and must stay silent.
fn admit(key: &'static str, now: Instant) -> Option<u64> {
    let mut guard = SLOTS.lock().unwrap_or_else(PoisonError::into_inner);
    let map = guard.get_or_insert_with(HashMap::new);
    match map.get_mut(key) {
        None => {
            map.insert(key, Slot { last: now, suppressed: 0 });
            Some(0)
        }
        Some(slot) if now.saturating_duration_since(slot.last) >= WINDOW => {
            let suppressed = std::mem::take(&mut slot.suppressed);
            slot.last = now;
            Some(suppressed)
        }
        Some(slot) => {
            slot.suppressed += 1;
            None
        }
    }
}

/// Prints `message` to stderr subject to the per-`key` window.
pub(crate) fn emit(key: &'static str, message: std::fmt::Arguments<'_>) {
    match admit(key, Instant::now()) {
        Some(0) => eprintln!("{message}"),
        Some(suppressed) => eprintln!("{message} (+{suppressed} repeats suppressed)"),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_inside_the_window_are_suppressed_then_counted() {
        let base = Instant::now();
        let key = "log-limit-test-window";
        assert_eq!(admit(key, base), Some(0));
        assert_eq!(admit(key, base + Duration::from_secs(1)), None);
        assert_eq!(admit(key, base + Duration::from_secs(2)), None);
        assert_eq!(admit(key, base + WINDOW), Some(2));
        assert_eq!(admit(key, base + WINDOW + Duration::from_secs(1)), None);
    }

    #[test]
    fn keys_are_independent() {
        let base = Instant::now();
        assert_eq!(admit("log-limit-test-a", base), Some(0));
        assert_eq!(admit("log-limit-test-b", base), Some(0));
    }
}
