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
    revision: u64,
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
            revision: 0,
        }
    }

    /// Records a notification and returns whether it may show a popup.
    ///
    /// Every notification is stored. Popup rules: a muted application never pops up, not even
    /// for critical urgency. Under Do Not Disturb only critical notifications pop up.
    /// The oldest entry is evicted once capacity is reached.
    pub fn record(&mut self, view: NotificationView, received_at_ms: u64) -> PopupDecision {
        let decision = self.popup_decision(&view.app_name, view.urgency);
        self.push(view, received_at_ms);
        decision
    }

    /// The popup rule for a notification, without recording it.
    #[must_use]
    pub fn popup_decision(&self, app_name: &str, urgency: NotificationUrgency) -> PopupDecision {
        if self.muted_apps.contains(app_name) {
            PopupDecision::Suppress(SuppressReason::AppMuted)
        } else if self.dnd && urgency != NotificationUrgency::Critical {
            PopupDecision::Suppress(SuppressReason::DoNotDisturb)
        } else {
            PopupDecision::Show
        }
    }

    fn push(&mut self, view: NotificationView, received_at_ms: u64) {
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
        self.revision += 1;
    }

    /// Removes one entry by `seq`. Returns false when it was already evicted or removed.
    pub fn remove(&mut self, seq: u64) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.seq == seq) else {
            return false;
        };
        self.entries.remove(index);
        self.revision += 1;
        true
    }

    pub fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.entries.clear();
            self.revision += 1;
        }
    }

    pub fn set_dnd(&mut self, enabled: bool) {
        if self.dnd != enabled {
            self.dnd = enabled;
            self.revision += 1;
        }
    }

    /// Monotonic counter bumped by every observable change; lets consumers skip re-projection.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn dnd(&self) -> bool {
        self.dnd
    }

    /// Returns false when the mute set is full and `app_name` is not already muted.
    pub fn set_app_muted(&mut self, app_name: &str, muted: bool) -> bool {
        if !muted {
            if self.muted_apps.remove(app_name) {
                self.revision += 1;
            }
            return true;
        }
        if self.muted_apps.contains(app_name) {
            return true;
        }
        if self.muted_apps.len() >= MAX_MUTED_APPS {
            return false;
        }
        self.muted_apps.insert(app_name.to_owned());
        self.revision += 1;
        true
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

/// Largest history file read at startup; anything bigger is treated as corrupt and ignored.
pub const MAX_HISTORY_FILE_BYTES: u64 = 512 * 1024;
const HISTORY_HEADER: &str = "flamewm-notification-history 1";
/// Per-field character cap applied when persisting, bounding each record line.
const MAX_PERSISTED_TEXT_CHARS: usize = 2048;

impl NotificationHistory {
    /// Serializes entries (oldest first), DND, and mutes as line-oriented, tab-separated text.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = String::new();
        out.push_str(HISTORY_HEADER);
        out.push('\n');
        out.push_str(if self.dnd { "dnd\t1\n" } else { "dnd\t0\n" });
        for app in &self.muted_apps {
            out.push_str("mute\t");
            out.push_str(&escape(app));
            out.push('\n');
        }
        for entry in &self.entries {
            out.push_str("entry\t");
            out.push_str(&entry.received_at_ms.to_string());
            out.push('\t');
            out.push_str(urgency_code(entry.view.urgency));
            for field in [&entry.view.app_name, &entry.view.summary, &entry.view.body] {
                out.push('\t');
                out.push_str(&escape(field));
            }
            out.push('\n');
        }
        out
    }

    /// Parses [`Self::encode`] output. Unknown or malformed lines are skipped. The newest
    /// `capacity` entries and at most [`MAX_MUTED_APPS`] mutes survive.
    #[must_use]
    pub fn decode(text: &str, capacity: usize) -> Self {
        let mut history = Self::new(capacity);
        let mut lines = text.lines();
        if lines.next() != Some(HISTORY_HEADER) {
            return history;
        }
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["dnd", value] => history.dnd = *value == "1",
                ["mute", app] => {
                    history.set_app_muted(&unescape(app), true);
                }
                ["entry", received, urgency, app, summary, body] => {
                    let (Ok(received_at_ms), Some(urgency)) =
                        (received.parse::<u64>(), parse_urgency(urgency))
                    else {
                        continue;
                    };
                    history.push(
                        NotificationView {
                            id: 0,
                            app_name: unescape(app),
                            summary: unescape(summary),
                            body: unescape(body),
                            urgency,
                            progress: None,
                        },
                        received_at_ms,
                    );
                }
                _ => {}
            }
        }
        history.revision = 0;
        history
    }

    /// Loads from `path`. A missing, oversized, or unreadable file yields an empty history.
    #[must_use]
    pub fn load_file(path: &std::path::Path, capacity: usize) -> Self {
        let Ok(metadata) = std::fs::metadata(path) else {
            return Self::new(capacity);
        };
        if metadata.len() > MAX_HISTORY_FILE_BYTES {
            return Self::new(capacity);
        }
        match std::fs::read_to_string(path) {
            Ok(text) => Self::decode(&text, capacity),
            Err(_) => Self::new(capacity),
        }
    }

    /// Writes atomically via a sibling temporary file and rename.
    pub fn save_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, self.encode())?;
        std::fs::rename(&temporary, path)
    }
}

fn urgency_code(urgency: NotificationUrgency) -> &'static str {
    match urgency {
        NotificationUrgency::Low => "l",
        NotificationUrgency::Normal => "n",
        NotificationUrgency::Critical => "c",
    }
}

fn parse_urgency(code: &str) -> Option<NotificationUrgency> {
    match code {
        "l" => Some(NotificationUrgency::Low),
        "n" => Some(NotificationUrgency::Normal),
        "c" => Some(NotificationUrgency::Critical),
        _ => None,
    }
}

fn escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars().take(MAX_PERSISTED_TEXT_CHARS) {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
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

    #[test]
    fn revision_advances_only_on_observable_change() {
        let mut history = NotificationHistory::new(4);
        let start = history.revision();
        history.set_dnd(false);
        assert_eq!(history.revision(), start);
        history.set_dnd(true);
        let after_dnd = history.revision();
        assert!(after_dnd > start);
        history.set_app_muted("never", false);
        assert_eq!(history.revision(), after_dnd);
        history.record(view("a", "1", NotificationUrgency::Normal), 0);
        assert!(history.revision() > after_dnd);
    }

    #[test]
    fn encode_decode_round_trips_entries_dnd_and_mutes() {
        let mut history = NotificationHistory::new(10);
        history.set_dnd(true);
        history.set_app_muted("spam\tapp", true);
        history.record(
            NotificationView {
                id: 9,
                app_name: "chat\\x".to_owned(),
                summary: "line1\nline2".to_owned(),
                body: "tab\there\r".to_owned(),
                urgency: NotificationUrgency::Critical,
                progress: Some(40),
            },
            1234,
        );
        history.record(view("mail", "m", NotificationUrgency::Low), 5678);

        let restored = NotificationHistory::decode(&history.encode(), 10);
        assert!(restored.dnd());
        assert!(restored.is_app_muted("spam\tapp"));
        assert_eq!(restored.revision(), 0);
        let entries: Vec<_> = restored.entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].received_at_ms, 1234);
        assert_eq!(entries[1].view.app_name, "chat\\x");
        assert_eq!(entries[1].view.summary, "line1\nline2");
        assert_eq!(entries[1].view.body, "tab\there\r");
        assert_eq!(entries[1].view.urgency, NotificationUrgency::Critical);
        assert_eq!(entries[0].view.urgency, NotificationUrgency::Low);
    }

    #[test]
    fn decode_is_bounded_and_skips_malformed_lines() {
        let text = format!(
            "{HISTORY_HEADER}\nbogus\nentry\tx\tn\ta\tb\tc\nentry\t1\tq\ta\tb\tc\n\
             entry\t1\tn\ta\tb\n{}",
            (0..5)
                .map(|index| format!("entry\t{index}\tn\tapp\tm{index}\t\n"))
                .collect::<String>()
        );
        let restored = NotificationHistory::decode(&text, 3);
        let summaries: Vec<&str> = restored
            .entries()
            .map(|entry| entry.view.summary.as_str())
            .collect();
        assert_eq!(summaries, ["m4", "m3", "m2"]);
    }

    #[test]
    fn decode_rejects_unknown_header() {
        let restored = NotificationHistory::decode("other-format\nentry\t1\tn\ta\tb\tc\n", 5);
        assert!(restored.is_empty());
    }

    #[test]
    fn save_and_load_file_round_trip_and_missing_file_is_empty() {
        let dir = std::env::temp_dir().join(format!("flamewm-history-test-{}", std::process::id()));
        let path = dir.join("notification-history.tsv");
        assert!(NotificationHistory::load_file(&path, 5).is_empty());

        let mut history = NotificationHistory::new(5);
        history.record(view("app", "saved", NotificationUrgency::Normal), 7);
        history.save_file(&path).expect("save history");
        let loaded = NotificationHistory::load_file(&path, 5);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.entries().next().unwrap().view.summary, "saved");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
