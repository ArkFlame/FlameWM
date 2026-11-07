//! Suppresses identical repeated diagnostics. A condition that persists across settings sync
//! points (for example a cursor theme that cannot load) logs once instead of on every pass.

/// Remembers the last diagnostic emitted. Repeats are suppressed until the text changes or the
/// condition clears.
#[derive(Debug, Default)]
pub struct RepeatLog {
    last: Option<String>,
}

impl RepeatLog {
    #[must_use]
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Returns `true` when `message` differs from the last emitted one, and records it.
    pub fn should_emit(&mut self, message: &str) -> bool {
        if self.last.as_deref() == Some(message) {
            return false;
        }
        self.last = Some(message.to_owned());
        true
    }

    /// Forgets the last message so the next occurrence emits again.
    pub fn clear(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::RepeatLog;

    #[test]
    fn repeats_are_suppressed_until_text_changes_or_clears() {
        let mut log = RepeatLog::new();
        assert!(log.should_emit("theme lacks Default"));
        assert!(!log.should_emit("theme lacks Default"));
        assert!(!log.should_emit("theme lacks Default"));
        assert!(log.should_emit("theme missing"));
        log.clear();
        assert!(log.should_emit("theme missing"));
    }
}
