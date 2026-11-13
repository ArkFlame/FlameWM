//! Tab walk over a list of focus stops, shared by the usage card and the notification centre.
//! Tab moves focus forward and Shift+Tab backward, wrapping over the stops. Precedent: WAI-ARIA
//! APG dialog (focus stays inside the popup and Tab cycles) and the KDE Plasma tray popup (Tab
//! walks its focusable controls).

/// Focus after a typed Tab; `None` when the key is not Tab or there are no stops.
#[must_use]
pub fn tab_focus_after(payload: &str, focus: Option<usize>, stops: usize) -> Option<usize> {
    if stops == 0 {
        return None;
    }
    let (modifiers, key) = crate::launcher::parse_typed_key(payload)?;
    // With Shift held, X reports the key as ISO_Left_Tab: that is Shift+Tab, so it walks backward.
    if key != "Tab" && key != "ISO_Left_Tab" {
        return None;
    }
    let backward =
        key == "ISO_Left_Tab" || modifiers.split('+').any(|modifier| modifier == "Shift");
    Some(match (focus, backward) {
        (None, false) => 0,
        (None, true) => stops - 1,
        (Some(index), true) => (index + stops - 1) % stops,
        (Some(index), false) => (index + 1) % stops,
    })
}

/// Focus after a typed Up or Down over a vertical list of `count` rows. Clamped at the ends, as a
/// WAI-ARIA listbox does (no wrap). `None` for other keys or an empty list. Precedent: APG listbox.
#[must_use]
pub fn list_focus_after(payload: &str, focus: Option<usize>, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let (_, key) = crate::launcher::parse_typed_key(payload)?;
    let last = count - 1;
    Some(match (key, focus) {
        ("Up", Some(index)) => index.saturating_sub(1),
        ("Down", Some(index)) => (index + 1).min(last),
        ("Up" | "Down", None) => 0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifiers: &str, name: &str) -> String {
        format!("{modifiers}\u{1f}{name}\u{1f}")
    }

    // OBSERVABLE: with Shift held X reports ISO_Left_Tab, which must walk backward like Shift+Tab.
    // MUTATION: matching only "Tab" drops the keyboard's Shift+Tab, the bug this guards.
    #[test]
    fn shift_tab_arrives_as_iso_left_tab_and_walks_backward() {
        assert_eq!(
            tab_focus_after(&key("Shift", "ISO_Left_Tab"), None, 3),
            Some(2)
        );
        assert_eq!(
            tab_focus_after(&key("Shift", "ISO_Left_Tab"), Some(2), 3),
            Some(1)
        );
        assert_eq!(tab_focus_after(&key("", "ISO_Left_Tab"), None, 3), Some(2));
        assert_eq!(tab_focus_after(&key("Shift", "Down"), None, 3), None);
    }

    // OBSERVABLE: Up and Down step one row and clamp at both ends; Tab is not a list key.
    #[test]
    fn up_and_down_walk_a_list_and_clamp_without_wrapping() {
        assert_eq!(list_focus_after(&key("", "Down"), None, 3), Some(0));
        assert_eq!(list_focus_after(&key("", "Down"), Some(1), 3), Some(2));
        assert_eq!(list_focus_after(&key("", "Down"), Some(2), 3), Some(2));
        assert_eq!(list_focus_after(&key("", "Up"), Some(0), 3), Some(0));
        assert_eq!(list_focus_after(&key("", "Up"), Some(2), 3), Some(1));
        assert_eq!(list_focus_after(&key("", "Tab"), Some(0), 3), None);
        assert_eq!(list_focus_after(&key("", "Down"), None, 0), None);
    }
}
