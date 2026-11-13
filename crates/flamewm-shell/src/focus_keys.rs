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
    if key != "Tab" {
        return None;
    }
    let backward = modifiers.split('+').any(|modifier| modifier == "Shift");
    Some(match (focus, backward) {
        (None, false) => 0,
        (None, true) => stops - 1,
        (Some(index), true) => (index + stops - 1) % stops,
        (Some(index), false) => (index + 1) % stops,
    })
}
