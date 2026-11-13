//! Keyboard operation of the Wi-Fi card rows. Tab and Shift+Tab walk the visible network rows with
//! wrap; Up and Down move the same row ring; Enter activates the focused row through the click path
//! (connect, or open the secret prompt the backend asks for). Escape closes through the generic
//! keyboard path. While the filter or the secret field has focus, these keys belong to the field.
//! Precedent: WAI-ARIA APG listbox (Up and Down move the active option, focus stays in the list)
//! and the KDE Plasma tray popup Tab walk, shared with `focus_keys`. KDE is not vendored under
//! `.vendor/`; idea only.

use flamewm_shell::focus_keys::tab_focus_after;

use super::QuickControlSnapshot;

/// Visible network rows on the card (the projection shows at most `NETWORK_SLOT_COUNT`).
pub(crate) fn network_row_count(snapshot: &QuickControlSnapshot) -> usize {
    crate::taskbar::status::network::project_visible_rows(
        &snapshot.system,
        &snapshot.network_query,
        snapshot.network_scroll_offset,
    )
    .len()
    .min(crate::taskbar::status::network::NETWORK_SLOT_COUNT)
}

/// Focus after a typed Tab, Shift+Tab, Up or Down over `rows` rows; `None` for any other key.
#[must_use]
pub(crate) fn network_focus_after_key(
    payload: &str,
    focus: Option<usize>,
    rows: usize,
) -> Option<usize> {
    if rows == 0 {
        return None;
    }
    let (_, key) = flamewm_shell::launcher::parse_typed_key(payload)?;
    match key {
        "Tab" | "ISO_Left_Tab" => tab_focus_after(payload, focus, rows),
        "Down" => Some(focus.map_or(0, |index| (index + 1) % rows)),
        "Up" => Some(focus.map_or(rows - 1, |index| (index + rows - 1) % rows)),
        _ => None,
    }
}

/// Row index to activate when a typed Enter arrives on a focused row; `None` otherwise. Space is
/// not an activation key on this card: it types into the filter or the secret field.
#[must_use]
pub(crate) fn network_row_activation(
    text: Option<&str>,
    focus: Option<usize>,
    rows: usize,
) -> Option<usize> {
    let index = focus.filter(|index| *index < rows)?;
    matches!(text, Some("\n" | "\r")).then_some(index)
}

/// Action of an activated row: the same `network.connect.<n>` the row's click dispatches.
#[must_use]
pub(crate) fn network_row_action(index: usize) -> String {
    format!("network.connect.{}", index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifiers: &str, name: &str) -> String {
        format!("{modifiers}\u{1f}{name}\u{1f}")
    }

    // OBSERVABLE: Tab walks the rows with wrap, Shift+Tab backward, and Up/Down move the same ring.
    // MUTATION: dropping the wrap, the backward branch or an arrow fails one of these asserts.
    #[test]
    fn tab_and_arrows_walk_the_rows_with_wrap() {
        assert_eq!(network_focus_after_key(&key("", "Tab"), None, 3), Some(0));
        assert_eq!(
            network_focus_after_key(&key("", "Tab"), Some(2), 3),
            Some(0)
        );
        assert_eq!(
            network_focus_after_key(&key("Shift", "Tab"), None, 3),
            Some(2)
        );
        assert_eq!(
            network_focus_after_key(&key("", "ISO_Left_Tab"), Some(0), 3),
            Some(2)
        );
        assert_eq!(network_focus_after_key(&key("", "Down"), None, 3), Some(0));
        assert_eq!(
            network_focus_after_key(&key("", "Down"), Some(2), 3),
            Some(0)
        );
        assert_eq!(network_focus_after_key(&key("", "Up"), None, 3), Some(2));
        assert_eq!(network_focus_after_key(&key("", "Up"), Some(0), 3), Some(2));
        assert_eq!(network_focus_after_key(&key("", "Left"), Some(0), 3), None);
        assert_eq!(network_focus_after_key(&key("", "Tab"), None, 0), None);
    }

    // OBSERVABLE: Enter activates the focused row only; Space and other text never activate.
    // MUTATION: activating on Space (it types into the field) or on unfocused rows fails here.
    #[test]
    fn enter_activates_only_the_focused_row() {
        for text in ["\n", "\r"] {
            assert_eq!(network_row_activation(Some(text), Some(1), 3), Some(1));
            assert_eq!(network_row_activation(Some(text), None, 3), None);
            assert_eq!(network_row_activation(Some(text), Some(3), 3), None);
        }
        assert_eq!(network_row_activation(Some(" "), Some(1), 3), None);
        assert_eq!(network_row_activation(Some("x"), Some(1), 3), None);
    }

    // OBSERVABLE: the activated row dispatches the same action its click does.
    #[test]
    fn activation_dispatches_the_row_click_action() {
        assert_eq!(network_row_action(0), "network.connect.1");
        assert_eq!(network_row_action(7), "network.connect.8");
    }

    // REGRESSION: every projected row id exists in network.html, and the row rule keeps a 1px
    // border box, or the focus ring never paints.
    #[test]
    fn network_focus_ids_exist_in_html_with_a_ring_border_box() {
        let html = include_str!("../../../../../ui/shell/network.html");
        for slot in 1..=crate::taskbar::status::network::NETWORK_SLOT_COUNT {
            assert!(
                html.contains(&format!("id=\"network-slot-{slot}\"")),
                "slot {slot}"
            );
        }
        assert!(html.contains(
            ".network-row { width:100%; height:48px; box-sizing:border-box; display:flex; align-items:center; gap:10px; padding:0px var(--sp-3); background:transparent; border:1px solid transparent;"
        ));
    }
}
