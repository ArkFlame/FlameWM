//! Bounded notification history with per-application grouping, Do Not Disturb, and per-app mute.
//!
//! Pure state: no D-Bus, no timers. The notification provider feeds accepted notifications in
//! through [`NotificationHistory::record`], which decides whether a transient popup may show.

use std::collections::{BTreeSet, VecDeque};

use crate::notifications::{NotificationUrgency, NotificationView};

/// Upper bound on distinct muted application names, so the mute set cannot grow without limit.
pub const MAX_MUTED_APPS: usize = 256;

/// One recorded notification. `seq` is a history-local key; freedesktop ids are reused and are
/// therefore not used to address history entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub seq: u64,
    pub received_at_ms: u64,
    pub view: NotificationView,
}

/// Entries from one application, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryGroup {
    pub app_name: String,
    pub newest_seq: u64,
    pub entries: Vec<u64>,
    pub muted: bool,
}

/// Whether a freshly recorded notification may raise a transient popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupDecision {
    Show,
    Suppress(SuppressReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressReason {
    AppMuted,
    DoNotDisturb,
}

#[derive(Debug)]
pub struct NotificationHistory {
    entries: VecDeque<HistoryEntry>,
    capacity: usize,
    next_seq: u64,
    dnd: bool,
    muted_apps: BTreeSet<String>,
}

impl NotificationHistory {
    /// `capacity` is clamped to at least one entry.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            next_seq: 1,
            dnd: false,
            muted_apps: BTreeSet::new(),
        }
    }

    /// Records a notification and returns whether it may show a popup.
    ///
    /// Every notification is stored. Popup rules: a muted application never pops up, not even
    /// for critical urgency. Under Do Not Disturb only critical notifications pop up.
    /// The oldest entry is evicted once capacity is reached.
    pub fn record(&mut self, view: NotificationView, received_at_ms: u64) -> PopupDecision {
        let decision = if self.muted_apps.contains(&view.app_name) {
            PopupDecision::Suppress(SuppressReason::AppMuted)
        } else if self.dnd && view.urgency != NotificationUrgency::Critical {
            PopupDecision::Suppress(SuppressReason::DoNotDisturb)
        } else {
            PopupDecision::Show
        };

        let seq = self.next_seq;
        self.next_seq += 1;
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(HistoryEntry {
            seq,
            received_at_ms,
            view,
        });
        decision
    }

    /// Removes one entry by `seq`. Returns false when it was already evicted or removed.
    pub fn remove(&mut self, seq: u64) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.seq == seq) else {
            return false;
        };
        self.entries.remove(index);
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub const fn set_dnd(&mut self, enabled: bool) {
        self.dnd = enabled;
    }

    #[must_use]
    pub const fn dnd(&self) -> bool {
        self.dnd
    }

    /// Returns false when the mute set is full and `app_name` is not already muted.
    pub fn set_app_muted(&mut self, app_name: &str, muted: bool) -> bool {
        if !muted {
            self.muted_apps.remove(app_name);
            return true;
        }
        if self.muted_apps.contains(app_name) {
            return true;
        }
        if self.muted_apps.len() >= MAX_MUTED_APPS {
            return false;
        }
        self.muted_apps.insert(app_name.to_owned())
    }

    #[must_use]
    pub fn is_app_muted(&self, app_name: &str) -> bool {
        self.muted_apps.contains(app_name)
    }

    /// Entries newest first.
    pub fn entries(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter().rev()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Groups by application name. Groups are ordered by their newest entry, newest first.
    #[must_use]
    pub fn groups(&self) -> Vec<HistoryGroup> {
        let mut groups: Vec<HistoryGroup> = Vec::new();
        for entry in self.entries.iter().rev() {
            let app = entry.view.app_name.as_str();
            if let Some(group) = groups.iter_mut().find(|group| group.app_name == app) {
                group.entries.push(entry.seq);
            } else {
                groups.push(HistoryGroup {
                    app_name: app.to_owned(),
                    newest_seq: entry.seq,
                    entries: vec![entry.seq],
                    muted: self.muted_apps.contains(app),
                });
            }
        }
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(app: &str, summary: &str, urgency: NotificationUrgency) -> NotificationView {
        NotificationView {
            id: 0,
            app_name: app.to_owned(),
            summary: summary.to_owned(),
            body: String::new(),
            urgency,
            progress: None,
        }
    }

    #[test]
    fn history_is_bounded_and_evicts_oldest() {
        let mut history = NotificationHistory::new(3);
        for index in 0..5 {
            history.record(
                view("mail", &format!("m{index}"), NotificationUrgency::Normal),
                index,
            );
        }
        assert_eq!(history.len(), 3);
        let summaries: Vec<&str> = history
            .entries()
            .map(|entry| entry.view.summary.as_str())
            .collect();
        assert_eq!(summaries, ["m4", "m3", "m2"]);
    }

    #[test]
    fn capacity_zero_is_clamped_to_one() {
        let mut history = NotificationHistory::new(0);
        history.record(view("a", "1", NotificationUrgency::Normal), 0);
        history.record(view("a", "2", NotificationUrgency::Normal), 1);
        assert_eq!(history.len(), 1);
        assert_eq!(history.entries().next().unwrap().view.summary, "2");
    }

    #[test]
    fn groups_by_app_newest_first_with_counts() {
        let mut history = NotificationHistory::new(10);
        history.record(view("chat", "c1", NotificationUrgency::Normal), 0); // seq 1
        history.record(view("mail", "m1", NotificationUrgency::Normal), 1); // seq 2
        history.record(view("chat", "c2", NotificationUrgency::Normal), 2); // seq 3

        let groups = history.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].app_name, "chat");
        assert_eq!(groups[0].newest_seq, 3);
        assert_eq!(groups[0].entries, vec![3, 1]);
        assert_eq!(groups[1].app_name, "mail");
        assert_eq!(groups[1].entries, vec![2]);
        assert!(!groups[0].muted);
    }

    #[test]
    fn dnd_suppresses_all_but_critical_popups_but_keeps_history() {
        let mut history = NotificationHistory::new(10);
        history.set_dnd(true);
        assert!(history.dnd());

        assert_eq!(
            history.record(view("a", "low", NotificationUrgency::Low), 0),
            PopupDecision::Suppress(SuppressReason::DoNotDisturb)
        );
        assert_eq!(
            history.record(view("a", "normal", NotificationUrgency::Normal), 1),
            PopupDecision::Suppress(SuppressReason::DoNotDisturb)
        );
        assert_eq!(
            history.record(view("a", "crit", NotificationUrgency::Critical), 2),
            PopupDecision::Show
        );
        assert_eq!(history.len(), 3);

        history.set_dnd(false);
        assert_eq!(
            history.record(view("a", "normal", NotificationUrgency::Normal), 3),
            PopupDecision::Show
        );
    }

    #[test]
    fn muted_app_never_pops_up_even_critical_but_is_recorded() {
        let mut history = NotificationHistory::new(10);
        assert!(history.set_app_muted("spam", true));
        assert!(history.is_app_muted("spam"));

        assert_eq!(
            history.record(view("spam", "x", NotificationUrgency::Critical), 0),
            PopupDecision::Suppress(SuppressReason::AppMuted)
        );
        assert_eq!(
            history.record(view("other", "y", NotificationUrgency::Normal), 1),
            PopupDecision::Show
        );
        assert_eq!(history.len(), 2);

        let groups = history.groups();
        let spam = groups
            .iter()
            .find(|group| group.app_name == "spam")
            .unwrap();
        assert!(spam.muted);

        assert!(history.set_app_muted("spam", false));
        assert_eq!(
            history.record(view("spam", "z", NotificationUrgency::Normal), 2),
            PopupDecision::Show
        );
    }

    #[test]
    fn mute_set_is_bounded() {
        let mut history = NotificationHistory::new(1);
        for index in 0..MAX_MUTED_APPS {
            assert!(history.set_app_muted(&format!("app{index}"), true));
        }
        assert!(!history.set_app_muted("one-more", true));
        // Re-muting an existing app is not growth and still succeeds.
        assert!(history.set_app_muted("app0", true));
    }

    #[test]
    fn remove_and_clear_address_entries_by_seq() {
        let mut history = NotificationHistory::new(10);
        history.record(view("a", "1", NotificationUrgency::Normal), 0); // seq 1
        history.record(view("a", "2", NotificationUrgency::Normal), 1); // seq 2
        assert!(history.remove(1));
        assert!(!history.remove(1));
        assert_eq!(history.len(), 1);
        history.clear();
        assert!(history.is_empty());
        assert!(history.groups().is_empty());
    }
}
