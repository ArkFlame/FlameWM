//! Diffing of the toplevel facts that external taskbars and switchers read.
//!
//! A protocol client must hear about a change exactly once and only when something changed, so the
//! compositor keeps the last facts it sent per window and sends only the fields this diff reports.

/// The facts a toplevel listing exposes for one window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToplevelFacts {
    pub title: String,
    pub app_id: String,
}

/// Which facts changed between two snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToplevelDelta {
    pub title: bool,
    pub app_id: bool,
}

impl ToplevelDelta {
    /// Whether nothing changed, so no event (not even `done`) should be sent.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.title && !self.app_id
    }
}

/// Compares the last sent facts with the current ones.
#[must_use]
pub fn diff(previous: &ToplevelFacts, next: &ToplevelFacts) -> ToplevelDelta {
    ToplevelDelta {
        title: previous.title != next.title,
        app_id: previous.app_id != next.app_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(title: &str, app_id: &str) -> ToplevelFacts {
        ToplevelFacts {
            title: title.to_owned(),
            app_id: app_id.to_owned(),
        }
    }

    #[test]
    fn identical_facts_produce_no_delta() {
        let delta = diff(&facts("term", "foot"), &facts("term", "foot"));
        assert!(delta.is_empty());
    }

    #[test]
    fn title_only_change_reports_title_alone() {
        let delta = diff(&facts("a", "foot"), &facts("b", "foot"));
        assert_eq!(
            delta,
            ToplevelDelta {
                title: true,
                app_id: false
            }
        );
        assert!(!delta.is_empty());
    }

    #[test]
    fn app_id_only_change_reports_app_id_alone() {
        let delta = diff(&facts("a", "foot"), &facts("a", "kitty"));
        assert_eq!(
            delta,
            ToplevelDelta {
                title: false,
                app_id: true
            }
        );
    }

    #[test]
    fn both_changes_are_reported_together() {
        let delta = diff(&facts("", ""), &facts("Firefox", "firefox"));
        assert_eq!(
            delta,
            ToplevelDelta {
                title: true,
                app_id: true
            }
        );
    }

    #[test]
    fn default_snapshot_against_empty_strings_is_empty() {
        assert!(diff(&ToplevelFacts::default(), &facts("", "")).is_empty());
    }
}
