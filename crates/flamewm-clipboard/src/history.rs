//! Bounded text history for the clipboard keeper. Newest item first; evicts oldest items
//! until both the item cap and the total byte cap hold. Pure memory, no I/O, no timers.

use std::collections::VecDeque;

/// Klipper's default history length.
pub const MAX_ITEMS: usize = 20;
/// Total byte cap across all items. Also the largest single item accepted.
pub const MAX_BYTES: usize = 256 * 1024;

pub struct ClipHistory {
    items: VecDeque<String>,
    bytes: usize,
}

impl ClipHistory {
    pub const fn new() -> Self {
        Self {
            items: VecDeque::new(),
            bytes: 0,
        }
    }

    /// Returns false when the text is empty, larger than `MAX_BYTES`, or equal to the newest item.
    pub fn push(&mut self, text: String) -> bool {
        if text.is_empty() || text.len() > MAX_BYTES || self.items.front() == Some(&text) {
            return false;
        }
        self.bytes += text.len();
        self.items.push_front(text);
        while self.items.len() > MAX_ITEMS || self.bytes > MAX_BYTES {
            let Some(old) = self.items.pop_back() else {
                break;
            };
            self.bytes -= old.len();
        }
        true
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(String::as_str)
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(tag: usize, len: usize) -> String {
        format!("{tag}{}", "x".repeat(len))
    }

    #[test]
    fn item_cap_evicts_oldest() {
        let mut history = ClipHistory::new();
        for tag in 0..=MAX_ITEMS {
            assert!(history.push(tag.to_string()));
        }
        assert_eq!(history.iter().count(), MAX_ITEMS);
        assert_eq!(history.iter().next(), Some(MAX_ITEMS.to_string().as_str()));
        assert!(!history.iter().any(|item| item == "0"));
    }

    #[test]
    fn byte_cap_evicts_until_total_fits() {
        let mut history = ClipHistory::new();
        for tag in 0..3 {
            assert!(history.push(sized(tag, 100 * 1024)));
        }
        assert_eq!(history.iter().count(), 2);
        assert!(history.bytes <= MAX_BYTES);
        assert!(history.iter().all(|item| !item.starts_with('0')));
    }

    #[test]
    fn duplicate_of_newest_is_not_added() {
        let mut history = ClipHistory::new();
        assert!(history.push("a".into()));
        assert!(!history.push("a".into()));
        assert_eq!(history.iter().count(), 1);
        assert!(history.push("b".into()));
        assert!(history.push("a".into()));
        assert_eq!(history.iter().count(), 3);
    }

    #[test]
    fn oversize_and_empty_items_are_rejected() {
        let mut history = ClipHistory::new();
        assert!(!history.push(String::new()));
        assert!(!history.push("x".repeat(MAX_BYTES + 1)));
        assert_eq!(history.iter().count(), 0);
        assert!(history.push("x".repeat(MAX_BYTES)));
        assert_eq!(history.bytes, MAX_BYTES);
    }

    #[test]
    fn clear_empties_history_and_bytes() {
        let mut history = ClipHistory::new();
        assert!(history.push("a".into()));
        history.clear();
        assert_eq!(history.iter().count(), 0);
        assert_eq!(history.bytes, 0);
    }
}
