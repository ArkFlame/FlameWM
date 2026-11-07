//! Remembered window geometry per app id, and the debounce that decides when it is written out.
//!
//! The file format is one `app-id = x y width height` line per app, the same `key = value` family
//! as the settings file. Memory is bounded: the oldest-remembered app is evicted first.

use std::time::{Duration, Instant};

use flamewm_api::Rect;

/// Most app ids remembered at once.
pub const GEOMETRY_CAPACITY: usize = 256;
/// Quiet period after the last change before the memory is written.
pub const PERSIST_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GeometryMemory {
    entries: Vec<(String, Rect)>,
}

impl GeometryMemory {
    /// Parses remembered geometry. Malformed lines are skipped: this cache is never worth
    /// refusing to start over.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut memory = Self::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((app_id, value)) = line.split_once('=') else {
                continue;
            };
            let numbers = value
                .split_whitespace()
                .map(str::parse::<i32>)
                .collect::<Result<Vec<_>, _>>();
            if let Ok([x, y, width, height]) = numbers.as_deref() {
                memory.remember(app_id.trim(), Rect::new(*x, *y, *width, *height));
            }
        }
        memory
    }

    #[must_use]
    pub fn recall(&self, app_id: &str) -> Option<Rect> {
        self.entries
            .iter()
            .find(|(id, _)| id == app_id)
            .map(|(_, rect)| *rect)
    }

    /// Stores `rect` for `app_id`. Returns true when the memory changed and needs a write.
    pub fn remember(&mut self, app_id: &str, rect: Rect) -> bool {
        if app_id.is_empty() || rect.width <= 0 || rect.height <= 0 {
            return false;
        }
        match self.entries.iter().position(|(id, _)| id == app_id) {
            Some(index) if self.entries[index].1 == rect => return false,
            Some(index) => {
                self.entries.remove(index);
            }
            None if self.entries.len() >= GEOMETRY_CAPACITY => {
                self.entries.remove(0);
            }
            None => {}
        }
        self.entries.push((app_id.to_owned(), rect));
        true
    }

    #[must_use]
    pub fn serialize(&self) -> String {
        self.entries
            .iter()
            .map(|(id, rect)| {
                format!(
                    "{id} = {} {} {} {}\n",
                    rect.x, rect.y, rect.width, rect.height
                )
            })
            .collect()
    }
}

/// Trailing debounce: every change pushes the write out to `PERSIST_DELAY` after it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PersistDebounce {
    due: Option<Instant>,
}

impl PersistDebounce {
    pub fn note_change(&mut self, now: Instant) {
        self.due = Some(now + PERSIST_DELAY);
    }

    /// True once when the quiet period has elapsed, and clears the pending write.
    pub fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(at) if now >= at => {
                self.due = None;
                true
            }
            _ => false,
        }
    }

    /// Caps a reactor wait so the pending write happens on time.
    #[must_use]
    pub fn cap_wait(&self, now: Instant, wait: Option<Duration>) -> Option<Duration> {
        let Some(at) = self.due else {
            return wait;
        };
        let left = at.saturating_duration_since(now);
        Some(wait.map_or(left, |wait| wait.min(left)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32) -> Rect {
        Rect::new(x, 10, 640, 480)
    }

    #[test]
    fn remember_recall_and_unchanged_is_not_a_change() {
        let mut memory = GeometryMemory::default();
        assert!(memory.remember("firefox", rect(1)));
        assert!(!memory.remember("firefox", rect(1)));
        assert_eq!(memory.recall("firefox"), Some(rect(1)));
        assert!(memory.remember("firefox", rect(2)));
        assert_eq!(memory.recall("firefox"), Some(rect(2)));
        assert_eq!(memory.recall("chromium"), None);
    }

    #[test]
    fn invalid_entries_are_refused() {
        let mut memory = GeometryMemory::default();
        assert!(!memory.remember("", rect(1)));
        assert!(!memory.remember("app", Rect::new(0, 0, 0, 10)));
    }

    #[test]
    fn capacity_evicts_the_oldest_app() {
        let mut memory = GeometryMemory::default();
        for index in 0..GEOMETRY_CAPACITY {
            memory.remember(&format!("app{index}"), rect(index as i32));
        }
        memory.remember("new", rect(-1));
        assert_eq!(memory.entries.len(), GEOMETRY_CAPACITY);
        assert_eq!(memory.recall("app0"), None);
        assert_eq!(memory.recall("app1"), Some(rect(1)));
        assert_eq!(memory.recall("new"), Some(rect(-1)));
    }

    #[test]
    fn serialize_round_trips_and_skips_malformed_lines() {
        let mut memory = GeometryMemory::default();
        memory.remember("org.a.B", Rect::new(-5, 7, 800, 600));
        memory.remember("xterm", rect(3));
        let text = format!("# header\nbroken line\nbad = 1 2 3\n{}", memory.serialize());
        assert_eq!(GeometryMemory::parse(&text), memory);
    }

    #[test]
    fn debounce_waits_for_quiet_period_and_caps_waits() {
        let start = Instant::now();
        let mut debounce = PersistDebounce::default();
        assert_eq!(debounce.cap_wait(start, None), None);
        assert_eq!(
            debounce.cap_wait(start, Some(Duration::from_secs(9))),
            Some(Duration::from_secs(9))
        );
        debounce.note_change(start);
        let later = start + Duration::from_millis(300);
        debounce.note_change(later);
        assert!(!debounce.take_due(start + PERSIST_DELAY));
        assert_eq!(debounce.cap_wait(later, None), Some(PERSIST_DELAY));
        assert!(debounce.take_due(later + PERSIST_DELAY));
        assert!(!debounce.take_due(later + PERSIST_DELAY * 2));
    }
}
