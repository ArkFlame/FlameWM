//! Notification center projection: history groups to template nodes, and center action parsing.
//!
//! Pure functions only. The notification host owns the surface and the provider, and applies
//! the nodes produced here to `ui/shell/notification-center.html`.

use std::time::{Duration, Instant};

use flamewm_integrations_linux::notification_history::NotificationHistory;
use flamewm_integrations_linux::notifications::{NotificationAction, NotificationUrgency};

/// Width of a row's progress track in px: the row content width at the 380 px centre.
pub const CENTER_METER_WIDTH: f32 = 296.0;

/// Action the panel status cluster dispatches to toggle the center.
pub const OPEN_ACTION: &str = "notification.center.open";

/// Action the render layer sends the grabbed surface for a root press outside it. The press is
/// only observed, never consumed, so it still reaches the window beneath.
pub const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";

/// Window in which the opener release that follows an outside-press close is the same click.
pub const CLOSE_LATCH_TTL: Duration = Duration::from_millis(300);

/// True when a secondary-button release on a notification card dismisses it instead of invoking
/// the body action. Primary and other buttons keep their existing behaviour.
#[must_use]
pub fn is_secondary_dismiss(action: &str, button: u32) -> bool {
    action == DISMISS_ACTION
        && flamewm_ui_x11::PointerButton::from_raw_x(button)
            == flamewm_ui_x11::PointerButton::Secondary
}

/// Body action of a toast card; a primary release invokes `default` or dismisses.
pub const DISMISS_ACTION: &str = "notification.dismiss";

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
/// Action buttons per item row. Clients may send more; the extra actions are not offered here.
pub const MAX_ITEM_ACTION_SLOTS: usize = 2;
const MAX_TEXT_CHARS: usize = 160;

/// A user command from the center surface. Indices are zero-based slots in the last projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Clear,
    ToggleDnd,
    ToggleGroupMute(usize),
    DismissItem {
        group: usize,
        item: usize,
    },
    InvokeItemAction {
        group: usize,
        item: usize,
        action: usize,
    },
    /// Keyboard Enter on a row: invokes the row's `default` action when it has one.
    InvokeItemDefault {
        group: usize,
        item: usize,
    },
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
            ["item", group, item, "action", action] => Some(Self::InvokeItemAction {
                group: slot(group, MAX_GROUP_SLOTS)?,
                item: slot(item, MAX_ITEM_SLOTS)?,
                action: slot(action, MAX_ITEM_ACTION_SLOTS)?,
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
    /// D-Bus id of the notification; only meaningful while that notification is active.
    pub notification_id: u32,
    pub summary: String,
    pub body: String,
    /// Client actions, at most [`MAX_ITEM_ACTION_SLOTS`]. Empty once the toast is gone.
    pub actions: Vec<NotificationAction>,
    /// `value` hint, 0..=100, when the client sent one.
    pub progress: Option<u8>,
    pub urgency: NotificationUrgency,
    /// The client sent a `default` action; Enter on the focused row invokes it.
    pub default_action: bool,
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
                            notification_id: entry.view.id,
                            summary: clip(&entry.view.summary),
                            body: clip(&entry.view.body),
                            actions: entry
                                .view
                                .actions
                                .iter()
                                .take(MAX_ITEM_ACTION_SLOTS)
                                .cloned()
                                .collect(),
                            progress: entry.view.progress,
                            urgency: entry.view.urgency,
                            default_action: entry.view.default_action,
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

/// Drops action buttons of items whose notification is no longer active, since the client can
/// no longer be answered for them.
pub fn hide_inactive_actions(view: &mut CenterView, is_active: impl Fn(u32) -> bool) {
    for item in view
        .groups
        .iter_mut()
        .flat_map(|group| group.items.iter_mut())
    {
        if !is_active(item.notification_id) {
            item.actions.clear();
        }
    }
}

/// One template write. Ids are the slot ids in `notification-center.html`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Text {
        id: String,
        value: String,
    },
    Visible {
        id: String,
        shown: bool,
    },
    /// A progress track: hidden without a value, else its fill width and `aria-valuenow`.
    Meter {
        id: String,
        value: Option<u8>,
    },
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

/// Every template write needed to show `view`, with no focused row.
#[must_use]
pub fn nodes(view: &CenterView) -> Vec<Node> {
    nodes_with_focus(view, None)
}

/// Every template write needed to show `view`; `focus` indexes rows in Tab order (see
/// [`item_at`]). Unused slots are hidden.
#[must_use]
pub fn nodes_with_focus(view: &CenterView, focus: Option<usize>) -> Vec<Node> {
    let mut out = vec![
        text("nc-dnd-state", if view.dnd { "On" } else { "Off" }),
        visible("nc-empty", view.groups.is_empty()),
    ];
    // Tab-order index of the first row of the current group.
    let mut base = 0;
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
            out.push(visible(
                format!("{i}-urgent-critical"),
                item.is_some_and(|item| item.urgency == NotificationUrgency::Critical),
            ));
            out.push(visible(
                format!("{i}-urgent-low"),
                item.is_some_and(|item| item.urgency == NotificationUrgency::Low),
            ));
            out.push(Node::Meter {
                id: format!("{i}-progress"),
                value: item.and_then(|item| item.progress),
            });
            out.push(visible(
                format!("{i}-focus"),
                item.is_some() && focus == Some(base + item_slot - 1),
            ));
            if let Some(item) = item {
                out.push(text(format!("{i}-summary"), item.summary.clone()));
                out.push(text(format!("{i}-body"), item.body.clone()));
            }
            // The row wrapper is toggled as a whole: a leaf button hidden under a visible row
            // is not repainted by the document, so the wrapper carries the transition.
            let has_actions = item.is_some_and(|item| !item.actions.is_empty());
            out.push(visible(format!("{i}-actions"), has_actions));
            for action_slot in 1..=MAX_ITEM_ACTION_SLOTS {
                let action = item.and_then(|item| item.actions.get(action_slot - 1));
                let id = format!("{i}-action-{action_slot}");
                out.push(visible(&id, action.is_some()));
                if let Some(action) = action {
                    out.push(text(&id, clip(&action.label)));
                }
            }
        }
        let hidden = group.count.saturating_sub(group.items.len());
        out.push(visible(format!("{g}-more"), hidden > 0));
        out.push(text(format!("{g}-more"), format!("+{hidden} more")));
        base += group.items.len();
    }
    out
}

/// Rows across all shown groups, in Tab order.
#[must_use]
pub fn item_count(view: &CenterView) -> usize {
    view.groups.iter().map(|group| group.items.len()).sum()
}

/// `(group, item)` slots of the row at Tab-order `index`.
#[must_use]
pub fn item_at(view: &CenterView, index: usize) -> Option<(usize, usize)> {
    let mut rest = index;
    for (group_index, group) in view.groups.iter().enumerate() {
        if rest < group.items.len() {
            return Some((group_index, rest));
        }
        rest -= group.items.len();
    }
    None
}

/// What a key does on the centre. `Focus` moves the Tab stop; the others act on a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Focus(usize),
    Dismiss { group: usize, item: usize },
    InvokeDefault { group: usize, item: usize },
}

/// Maps a `__flamewm.ui.key` payload to a centre action. Tab and Shift+Tab walk the rows; Delete
/// dismisses the focused row; Enter invokes its `default` action, and does nothing without one.
#[must_use]
pub fn key_action(payload: &str, focus: Option<usize>, view: &CenterView) -> Option<KeyAction> {
    if let Some(next) = crate::focus_keys::tab_focus_after(payload, focus, item_count(view)) {
        return Some(KeyAction::Focus(next));
    }
    let (_, key) = crate::launcher::parse_typed_key(payload)?;
    let index = focus.filter(|index| *index < item_count(view))?;
    let (group, item) = item_at(view, index)?;
    match key {
        "Delete" => Some(KeyAction::Dismiss { group, item }),
        "Return" | "Enter" => view.groups[group].items[item]
            .default_action
            .then_some(KeyAction::InvokeDefault { group, item }),
        _ => None,
    }
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
    use flamewm_integrations_linux::notifications::{
        NotificationAction, NotificationUrgency, NotificationView,
    };

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
                actions: Vec::new(),
                default_action: false,
                image: None,
                icon_name: None,
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
        assert_eq!(
            Command::parse("notification.center.item.2.3.action.2"),
            Some(Command::InvokeItemAction {
                group: 1,
                item: 2,
                action: 1
            })
        );
        assert_eq!(
            Command::parse("notification.center.item.2.3.action.3"),
            None
        );
        assert_eq!(Command::parse("notification.center.group.6.mute"), None);
        assert_eq!(Command::parse("notification.center.item.1.4.dismiss"), None);
        assert_eq!(Command::parse("notification.center.group.0.mute"), None);
        assert_eq!(Command::parse("notification.center.bogus"), None);
        assert_eq!(Command::parse("start.toggle"), None);
    }

    #[test]
    fn secondary_release_on_a_card_dismisses_and_primary_is_unchanged() {
        assert!(is_secondary_dismiss(DISMISS_ACTION, 3));
        assert!(!is_secondary_dismiss(DISMISS_ACTION, 1));
        assert!(!is_secondary_dismiss(DISMISS_ACTION, 2));
        assert!(!is_secondary_dismiss("notification.action.1", 3));
        assert!(!is_secondary_dismiss(OPEN_ACTION, 3));
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
        assert!(latch_consumes_open(
            &mut latch,
            now + Duration::from_millis(40)
        ));
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
    fn action_buttons_project_and_hide_once_the_toast_is_gone() {
        let mut history = NotificationHistory::new(10);
        let action = |key: &str, label: &str| NotificationAction {
            key: key.to_owned(),
            label: label.to_owned(),
        };
        history.record(
            NotificationView {
                id: 7,
                app_name: "chat".to_owned(),
                summary: "hi".to_owned(),
                body: String::new(),
                urgency: NotificationUrgency::Normal,
                progress: None,
                actions: vec![
                    action("reply", "Reply"),
                    action("later", "Later"),
                    action("extra", "Extra"),
                ],
                default_action: false,
                image: None,
                icon_name: None,
            },
            0,
        );
        let mut center = view(&history);
        assert_eq!(center.groups[0].items[0].notification_id, 7);
        assert_eq!(
            center.groups[0].items[0].actions.len(),
            MAX_ITEM_ACTION_SLOTS
        );
        let projected = nodes(&center);
        assert!(projected.contains(&text("nc-item-1-1-action-1", "Reply")));
        assert!(projected.contains(&text("nc-item-1-1-action-2", "Later")));
        assert!(projected.contains(&visible("nc-item-1-1-action-2", true)));
        assert!(projected.contains(&visible("nc-item-1-1-actions", true)));
        assert!(projected.contains(&visible("nc-item-1-2-action-1", false)));

        hide_inactive_actions(&mut center, |_| false);
        assert!(center.groups[0].items[0].actions.is_empty());
        assert!(nodes(&center).contains(&visible("nc-item-1-1-action-1", false)));
        assert!(nodes(&center).contains(&visible("nc-item-1-1-actions", false)));
    }

    #[test]
    fn every_projected_id_exists_in_template() {
        let mut history = NotificationHistory::new(10);
        record(&mut history, "chat", "hi");
        let mut ids: Vec<String> = nodes(&view(&history))
            .into_iter()
            .map(|node| match node {
                Node::Text { id, .. } | Node::Visible { id, .. } | Node::Meter { id, .. } => id,
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
            "notification.center.item.1.1.action.1",
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
    fn action_buttons_carry_a_text_child_for_projection() {
        // The document rejects text writes to an element without a text child, which would
        // abort the whole projection. Every action button therefore carries a zero-width text.
        for row in 1..=MAX_GROUP_SLOTS {
            for item in 1..=MAX_ITEM_SLOTS {
                for action in 1..=MAX_ITEM_ACTION_SLOTS {
                    let open = format!("<button id=\"nc-item-{row}-{item}-action-{action}\"");
                    let start = TEMPLATE
                        .find(&open)
                        .unwrap_or_else(|| panic!("missing {open}"));
                    let end = start + TEMPLATE[start..].find("</button>").expect("button end");
                    assert!(
                        TEMPLATE[start..end].ends_with("&#8203;"),
                        "{open} has no text child"
                    );
                }
            }
        }
    }

    #[test]
    fn clip_truncates_on_character_boundaries() {
        assert_eq!(clip("short"), "short");
        let long: String = "🙂".repeat(MAX_TEXT_CHARS + 1);
        assert_eq!(clip(&long).chars().count(), MAX_TEXT_CHARS + 1);
        assert!(clip(&long).ends_with('…'));
    }
}

#[cfg(test)]
mod keyboard_tests {
    use super::*;

    fn item(
        summary: &str,
        urgency: NotificationUrgency,
        progress: Option<u8>,
        default: bool,
    ) -> CenterItem {
        CenterItem {
            seq: 0,
            notification_id: 1,
            summary: summary.to_owned(),
            body: String::new(),
            actions: Vec::new(),
            progress,
            urgency,
            default_action: default,
        }
    }

    /// Group 1 holds rows 0 (default action) and 1 (critical, progress 40); group 2 holds row 2.
    fn two_groups() -> CenterView {
        let group = |app: &str, items| CenterGroup {
            app_name: app.to_owned(),
            muted: false,
            count: 1,
            items,
        };
        CenterView {
            dnd: false,
            groups: vec![
                group(
                    "a",
                    vec![
                        item("a1", NotificationUrgency::Normal, None, true),
                        item("a2", NotificationUrgency::Critical, Some(40), false),
                    ],
                ),
                group("b", vec![item("b1", NotificationUrgency::Low, None, false)]),
            ],
        }
    }

    fn key(name: &str) -> String {
        format!("\u{1f}{name}\u{1f}")
    }

    // OBSERVABLE: Tab walks every row across groups and wraps. MUTATION: a per-group walk stops at 2.
    #[test]
    fn tab_walks_rows_across_groups() {
        let view = two_groups();
        assert_eq!(
            key_action(&key("Tab"), None, &view),
            Some(KeyAction::Focus(0))
        );
        assert_eq!(
            key_action(&key("Tab"), Some(1), &view),
            Some(KeyAction::Focus(2))
        );
        assert_eq!(
            key_action(&key("Tab"), Some(2), &view),
            Some(KeyAction::Focus(0))
        );
    }

    // OBSERVABLE: Delete dismisses the focused row; Enter invokes only a row with a default action.
    #[test]
    fn delete_dismisses_and_enter_needs_a_default_action() {
        let view = two_groups();
        assert_eq!(
            key_action(&key("Delete"), Some(2), &view),
            Some(KeyAction::Dismiss { group: 1, item: 0 })
        );
        assert_eq!(
            key_action(&key("Return"), Some(0), &view),
            Some(KeyAction::InvokeDefault { group: 0, item: 0 })
        );
        assert_eq!(key_action(&key("Return"), Some(1), &view), None);
        assert_eq!(key_action(&key("Delete"), None, &view), None);
        assert_eq!(key_action(&key("Delete"), Some(9), &view), None);
    }

    // OBSERVABLE: urgency markers, the progress track and the focus ring project onto the row ids
    // the template defines.
    #[test]
    fn urgency_progress_and_focus_project_to_row_nodes() {
        let nodes = nodes_with_focus(&two_groups(), Some(1));
        assert!(nodes.contains(&visible("nc-item-1-2-urgent-critical", true)));
        assert!(nodes.contains(&visible("nc-item-1-1-urgent-critical", false)));
        assert!(nodes.contains(&visible("nc-item-2-1-urgent-low", true)));
        assert!(nodes.contains(&Node::Meter {
            id: "nc-item-1-2-progress".to_owned(),
            value: Some(40),
        }));
        assert!(nodes.contains(&Node::Meter {
            id: "nc-item-1-1-progress".to_owned(),
            value: None,
        }));
        assert!(nodes.contains(&visible("nc-item-1-2-focus", true)));
        assert!(nodes.contains(&visible("nc-item-1-1-focus", false)));
        assert!(nodes.contains(&visible("nc-item-2-1-focus", false)));
    }

    // OBSERVABLE: the template exposes the progress track as a progressbar.
    #[test]
    fn centre_progress_track_is_a_progressbar() {
        let template = include_str!("../../../ui/shell/notification-center.html");
        assert!(template
            .contains(r#"id="nc-item-1-2-progress" class="nc-progress" role="progressbar""#));
    }
}

#[cfg(test)]
mod toast_markup_tests {
    // Precedent: WAI-ARIA 1.2 status role for a non-modal, polite announcement (a toast that
    // does not take focus), as used by the shell's network status line.
    #[test]
    fn toast_card_is_a_polite_status_region() {
        let html = include_str!("../../../ui/shell/notification.html");
        assert!(html.contains(r#"id="notification-card" class="notification-card" role="status""#));
    }

    // Precedent: WAI-ARIA 1.2 log role for a history list that grows in order; the centre list is
    // a log with a name, not an unnamed scroll region.
    #[test]
    fn centre_list_is_a_named_log_region() {
        let html = include_str!("../../../ui/shell/notification-center.html");
        assert!(html.contains(r#"id="nc-list" class="nc-list" data-scroll="nc-list" role="log" aria-label="Notifications""#));
    }
}
