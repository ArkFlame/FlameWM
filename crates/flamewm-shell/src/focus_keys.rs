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
}
