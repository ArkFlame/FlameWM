//! Notification center projection: history groups to template nodes, and center action parsing.
//!
//! Pure functions only. The notification host owns the surface and the provider, and applies
//! the nodes produced here to `ui/shell/notification-center.html`.

use std::time::{Duration, Instant};

use flamewm_integrations_linux::notification_history::NotificationHistory;

/// Action the panel status cluster dispatches to toggle the center.
pub const OPEN_ACTION: &str = "notification.center.open";

/// Action the render layer sends the grabbed surface for a root press outside it. The press is
/// only observed, never consumed, so it still reaches the window beneath.
pub const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";

/// Window in which the opener release that follows an outside-press close is the same click.
pub const CLOSE_LATCH_TTL: Duration = Duration::from_millis(300);

/// True when an outside-press release closes the open center. Matches the quick popups' rule.
#[must_use]
pub fn outside_press_closes(action: &str, inside: bool, open: bool) -> bool {
    open && !inside && action == OUTSIDE_RELEASE_ACTION
}

/// Consumes the opener release that is the same click as an outside-press close, so the bell
/// does not reopen the center it just dismissed. The latch is single-use and expires.
#[must_use]
pub fn latch_consumes_open(closed_at: &mut Option<Instant>, now: Instant) -> bool {
    let Some(at) = closed_at.take() else {
        return false;
    };
    now.saturating_duration_since(at) <= CLOSE_LATCH_TTL
}

/// True for the Escape key release, which closes the open center whichever surface had focus.
#[must_use]
pub fn is_escape_key(action: &str, text: Option<&str>) -> bool {
    action == "keyboard.input" && text == Some("\u{1b}")
}
/// Fixed template slots. Extra groups and items are counted, not shown.
pub const MAX_GROUP_SLOTS: usize = 5;
pub const MAX_ITEM_SLOTS: usize = 3;
const MAX_TEXT_CHARS: usize = 160;

/// A user command from the center surface. Indices are zero-based slots in the last projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Clear,
    ToggleDnd,
    ToggleGroupMute(usize),
    DismissItem { group: usize, item: usize },
}

impl Command {
    /// Parses a template `data-action` value. Slot numbers in actions are one-based.
    #[must_use]
    pub fn parse(action: &str) -> Option<Self> {
        match action {
            "notification.center.clear" => return Some(Self::Clear),
            "notification.center.dnd" => return Some(Self::ToggleDnd),
            _ => {}
        }
        let rest = action.strip_prefix("notification.center.")?;
        let parts: Vec<&str> = rest.split('.').collect();
        match parts.as_slice() {
            ["group", group, "mute"] => Some(Self::ToggleGroupMute(slot(group, MAX_GROUP_SLOTS)?)),
            ["item", group, item, "dismiss"] => Some(Self::DismissItem {
                group: slot(group, MAX_GROUP_SLOTS)?,
                item: slot(item, MAX_ITEM_SLOTS)?,
            }),
            _ => None,
        }
    }
}

fn slot(text: &str, slots: usize) -> Option<usize> {
    let number: usize = text.parse().ok()?;
    (1..=slots).contains(&number).then(|| number - 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CenterItem {
    pub seq: u64,
    pub summary: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CenterGroup {
    pub app_name: String,
    pub muted: bool,
    pub count: usize,
    /// Newest first, at most [`MAX_ITEM_SLOTS`].
    pub items: Vec<CenterItem>,
}

/// What the center shows, derived from history. Resolves commands to history keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CenterView {
    pub dnd: bool,
    /// Groups beyond [`MAX_GROUP_SLOTS`] are dropped from view.
    pub groups: Vec<CenterGroup>,
}

#[must_use]
pub fn view(history: &NotificationHistory) -> CenterView {
    let groups = history
        .groups()
        .into_iter()
        .take(MAX_GROUP_SLOTS)
        .map(|group| {
            let items = group
                .entries
                .iter()
                .take(MAX_ITEM_SLOTS)
                .filter_map(|seq| {
                    history
                        .entries()
                        .find(|entry| entry.seq == *seq)
                        .map(|entry| CenterItem {
                            seq: entry.seq,
                            summary: clip(&entry.view.summary),
                            body: clip(&entry.view.body),
                        })
                })
                .collect();
            CenterGroup {
                app_name: group.app_name,
                muted: group.muted,
                count: group.entries.len(),
                items,
            }
        })
        .collect();
    CenterView {
        dnd: history.dnd(),
        groups,
    }
}

/// One template write. Ids are the slot ids in `notification-center.html`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Text { id: String, value: String },
    Visible { id: String, shown: bool },
}

fn text(id: impl Into<String>, value: impl Into<String>) -> Node {
    Node::Text {
        id: id.into(),
        value: value.into(),
    }
}

fn visible(id: impl Into<String>, shown: bool) -> Node {
    Node::Visible {
        id: id.into(),
        shown,
    }
}

/// Every template write needed to show `view`. Unused slots are hidden.
#[must_use]
pub fn nodes(view: &CenterView) -> Vec<Node> {
    let mut out = vec![
        text("nc-dnd-state", if view.dnd { "On" } else { "Off" }),
        visible("nc-empty", view.groups.is_empty()),
    ];
    for group_slot in 1..=MAX_GROUP_SLOTS {
        let group = view.groups.get(group_slot - 1);
        let g = format!("nc-group-{group_slot}");
        out.push(visible(&g, group.is_some()));
        let Some(group) = group else {
            for item_slot in 1..=MAX_ITEM_SLOTS {
                out.push(visible(format!("nc-item-{group_slot}-{item_slot}"), false));
            }
            out.push(visible(format!("{g}-more"), false));
            continue;
        };
        out.push(text(format!("{g}-app"), clip(&group.app_name)));
        out.push(text(format!("{g}-count"), group.count.to_string()));
        out.push(text(
            format!("{g}-mute"),
            if group.muted { "Unmute" } else { "Mute" },
        ));
        for item_slot in 1..=MAX_ITEM_SLOTS {
            let item = group.items.get(item_slot - 1);
            let i = format!("nc-item-{group_slot}-{item_slot}");
            out.push(visible(&i, item.is_some()));
            if let Some(item) = item {
                out.push(text(format!("{i}-summary"), item.summary.clone()));
                out.push(text(format!("{i}-body"), item.body.clone()));
            }
        }
        let hidden = group.count.saturating_sub(group.items.len());
        out.push(visible(format!("{g}-more"), hidden > 0));
        out.push(text(format!("{g}-more"), format!("+{hidden} more")));
    }
    out
}

fn clip(value: &str) -> String {
    let mut chars = value.chars();
    let text: String = chars.by_ref().take(MAX_TEXT_CHARS).collect();
    if chars.next().is_some() {
        format!("{text}…")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_integrations_linux::notification_history::NotificationHistory;
    use flamewm_integrations_linux::notifications::{NotificationUrgency, NotificationView};

    const TEMPLATE: &str = include_str!("../../../ui/shell/notification-center.html");

    fn record(history: &mut NotificationHistory, app: &str, summary: &str) {
        history.record(
            NotificationView {
                id: 0,
                app_name: app.to_owned(),
                summary: summary.to_owned(),
                body: String::new(),
                urgency: NotificationUrgency::Normal,
                progress: None,
            },
            0,
        );
    }

    #[test]
    fn parses_template_actions_one_based() {
        assert_eq!(
            Command::parse("notification.center.clear"),
            Some(Command::Clear)
        );
        assert_eq!(
            Command::parse("notification.center.dnd"),
            Some(Command::ToggleDnd)
        );
        assert_eq!(
            Command::parse("notification.center.group.1.mute"),
            Some(Command::ToggleGroupMute(0))
        );
        assert_eq!(
            Command::parse("notification.center.item.5.3.dismiss"),
            Some(Command::DismissItem { group: 4, item: 2 })
        );
        assert_eq!(Command::parse("notification.center.group.6.mute"), None);
        assert_eq!(Command::parse("notification.center.item.1.4.dismiss"), None);
        assert_eq!(Command::parse("notification.center.group.0.mute"), None);
        assert_eq!(Command::parse("notification.center.bogus"), None);
        assert_eq!(Command::parse("start.toggle"), None);
    }

    #[test]
    fn outside_press_closes_only_an_open_center_on_an_outside_release() {
        assert!(outside_press_closes(OUTSIDE_RELEASE_ACTION, false, true));
        assert!(!outside_press_closes(OUTSIDE_RELEASE_ACTION, true, true));
        assert!(!outside_press_closes(OUTSIDE_RELEASE_ACTION, false, false));
        assert!(!outside_press_closes("keyboard.input", false, true));
    }

    #[test]
    fn close_latch_consumes_one_opener_release_inside_the_window() {
        let now = Instant::now();
        let mut latch = Some(now);
        assert!(latch_consumes_open(&mut latch, now + Duration::from_millis(40)));
        assert_eq!(latch, None, "the latch is single-use");
        assert!(!latch_consumes_open(&mut latch, now));

        let mut stale = Some(now);
        assert!(!latch_consumes_open(
            &mut stale,
            now + CLOSE_LATCH_TTL + Duration::from_millis(1)
        ));
        assert_eq!(stale, None, "an expired latch is dropped");
    }

    #[test]
    fn only_the_escape_key_text_closes_the_center() {
        assert!(is_escape_key("keyboard.input", Some("\u{1b}")));
        assert!(!is_escape_key("keyboard.input", Some("a")));
        assert!(!is_escape_key("keyboard.input", None));
        assert!(!is_escape_key("notification.center.open", Some("\u{1b}")));
    }

    #[test]
    fn view_groups_newest_first_and_bounds_slots() {
        let mut history = NotificationHistory::new(50);
        history.set_dnd(true);
        history.set_app_muted("mute-me", true);
        for index in 0..6 {
            record(&mut history, &format!("app{index}"), "y");
        }
        record(&mut history, "mail", "m1");
        record(&mut history, "mute-me", "x");
        for index in 0..4 {
            record(&mut history, "chat", &format!("c{index}"));
        }

        let center = view(&history);
        assert!(center.dnd);
        assert_eq!(center.groups.len(), MAX_GROUP_SLOTS);
        let chat = center
            .groups
            .iter()
            .find(|group| group.app_name == "chat")
            .expect("chat group");
        assert_eq!(chat.count, 4);
        assert_eq!(chat.items.len(), MAX_ITEM_SLOTS);
        assert_eq!(chat.items[0].summary, "c3");
        assert!(!chat.muted);
    }

    #[test]
    fn nodes_hide_empty_center_and_report_overflow() {
        let empty = nodes(&CenterView::default());
        assert!(empty.contains(&visible("nc-empty", true)));
        assert!(empty.contains(&text("nc-dnd-state", "Off")));

        let mut history = NotificationHistory::new(10);
        for index in 0..4 {
            record(&mut history, "chat", &format!("c{index}"));
        }
        let projected = nodes(&view(&history));
        assert!(projected.contains(&visible("nc-empty", false)));
        assert!(projected.contains(&text("nc-group-1-count", "4")));
        assert!(projected.contains(&text("nc-group-1-mute", "Mute")));
        assert!(projected.contains(&text("nc-group-1-more", "+1 more")));
        assert!(projected.contains(&visible("nc-group-1-more", true)));
        assert!(projected.contains(&visible("nc-group-2", false)));
    }

    #[test]
    fn every_projected_id_exists_in_template() {
        let mut history = NotificationHistory::new(10);
        record(&mut history, "chat", "hi");
        let mut ids: Vec<String> = nodes(&view(&history))
            .into_iter()
            .map(|node| match node {
                Node::Text { id, .. } | Node::Visible { id, .. } => id,
            })
            .collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            assert!(
                TEMPLATE.contains(&format!("id=\"{id}\"")),
                "template lacks #{id}"
            );
        }
        for action in [
            "notification.center.clear",
            "notification.center.dnd",
            "notification.center.group.1.mute",
            "notification.center.item.1.1.dismiss",
        ] {
            assert!(TEMPLATE.contains(action), "template lacks {action}");
        }
    }

    /// Declarations of the first `{ ... }` rule whose selector is exactly `selector`.
    fn rule(css: &str, selector: &str) -> String {
        let start = css
            .find(&format!("{selector} {{"))
            .unwrap_or_else(|| panic!("template lacks rule {selector}"));
        let body = &css[start..];
        body[..body.find('}').expect("rule end")].to_owned()
    }

    #[test]
    fn center_text_ellipsizes_and_button_labels_center() {
        for selector in [".nc-item-summary", ".nc-item-body", ".nc-group-app"] {
            let body = rule(TEMPLATE, selector);
            assert!(body.contains("white-space:nowrap"), "{selector}: {body}");
            assert!(
                body.contains("text-overflow:ellipsis"),
                "{selector}: {body}"
            );
        }
        for selector in [".nc-clear", ".nc-group-mute"] {
            let body = rule(TEMPLATE, selector);
            assert!(body.contains("display:flex"), "{selector}: {body}");
            assert!(body.contains("align-items:center"), "{selector}: {body}");
        }
        assert!(rule(TEMPLATE, ".nc-toggle").contains("justify-content:space-between"));
    }

    #[test]
    fn clip_truncates_on_character_boundaries() {
        assert_eq!(clip("short"), "short");
        let long: String = "🙂".repeat(MAX_TEXT_CHARS + 1);
        assert_eq!(clip(&long).chars().count(), MAX_TEXT_CHARS + 1);
        assert!(clip(&long).ends_with('…'));
    }
}
