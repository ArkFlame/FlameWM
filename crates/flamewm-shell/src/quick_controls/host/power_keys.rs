//! Keyboard operation of the power (battery) card. The card has six focus stops in Tab order:
//! power profile, brightness down, brightness up, microphone mute, microphone down, microphone
//! up. Tab moves focus forward and Shift+Tab backward, wrapping over the stops; Enter or Space on
//! a focused stop activates it once and keeps focus there. Escape closes the card through the
//! generic keyboard path.
//! Precedent: WAI-ARIA APG toolbar and dialog (Tab moves between controls, Enter and Space
//! activate the focused control) and the KDE Plasma tray popup, whose Tab walks its controls, as
//! cited in `focus_keys`. KDE Plasma is not vendored under `.vendor/`; the Tab walk is shared with
//! the usage and media cards and the notification centre through `focus_keys`.

/// Focus stops on the card, in Tab order; the element id of each is the focus-ring target.
pub(crate) const POWER_STOP_IDS: [&str; 6] = [
    "battery-profile",
    "battery-brightness-down",
    "battery-brightness-up",
    "battery-input-mute",
    "battery-input-down",
    "battery-input-up",
];

/// Action dispatched by each stop, indexed by Tab-order position.
pub(crate) const POWER_STOP_ACTIONS: [&str; 6] = [
    "power.profile.cycle",
    "brightness.down",
    "brightness.up",
    "input.mute",
    "input.down",
    "input.up",
];

/// Tab walk shared with the usage and media cards and the notification centre.
pub(crate) use flamewm_shell::focus_keys::tab_focus_after as power_focus_after_tab;

/// Focus after a typed Tab on the power card; `None` for any other key.
#[must_use]
pub(crate) fn power_key_action(payload: &str, focus: Option<usize>) -> Option<usize> {
    power_focus_after_tab(payload, focus, POWER_STOP_IDS.len())
}

/// Action of the focused stop when a typed Enter or Space activates it; `None` otherwise.
#[must_use]
pub(crate) fn power_activation(text: Option<&str>, focus: Option<usize>) -> Option<&'static str> {
    let index = focus.filter(|index| *index < POWER_STOP_ACTIONS.len())?;
    matches!(text, Some("\n" | "\r" | " ")).then_some(POWER_STOP_ACTIONS[index])
}

/// Focus ring state per stop: true on the focused stop, false elsewhere.
#[must_use]
pub(crate) fn power_focus_ring(focus: Option<usize>) -> [bool; POWER_STOP_IDS.len()] {
    std::array::from_fn(|index| focus == Some(index))
}

/// Draws the focus ring on the focused stop and clears it from the rest.
pub(crate) fn project_power_focus(
    document: &mut impl flamewm_ui_x11::UiDocumentAccess,
    focus: Option<usize>,
) -> Result<(), String> {
    for (id, ringed) in POWER_STOP_IDS.iter().zip(power_focus_ring(focus)) {
        if ringed {
            document.border(id, crate::projection::task_accent())?;
        } else {
            document.border_clear(id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifiers: &str, name: &str) -> String {
        format!("{modifiers}\u{1f}{name}\u{1f}")
    }

    // OBSERVABLE: Tab walks the six power stops and wraps; Shift+Tab walks backward.
    // MUTATION: dropping the wrap, the Shift branch or the stop count fails one of these asserts.
    #[test]
    fn tab_walks_the_six_power_stops_with_wrap() {
        let stops = POWER_STOP_IDS.len();
        assert_eq!(stops, 6);
        assert_eq!(power_key_action(&key("", "Tab"), None), Some(0));
        assert_eq!(power_key_action(&key("", "Tab"), Some(0)), Some(1));
        assert_eq!(power_key_action(&key("", "Tab"), Some(5)), Some(0));
        assert_eq!(power_key_action(&key("Shift", "Tab"), None), Some(5));
        assert_eq!(power_key_action(&key("Shift", "Tab"), Some(0)), Some(5));
        assert_eq!(power_key_action(&key("", "ISO_Left_Tab"), None), Some(5));
        assert_eq!(power_key_action(&key("", "Down"), None), None);
    }

    // OBSERVABLE: Enter and Space on a focused stop dispatch that stop's action, once per typed
    // key. Unfocused or out-of-range focus and other text activate nothing.
    // MUTATION: mapping the wrong index to an action, or activating without focus, fails here.
    #[test]
    fn enter_and_space_activate_only_the_focused_stop() {
        for text in ["\n", "\r", " "] {
            for (index, action) in POWER_STOP_ACTIONS.iter().enumerate() {
                assert_eq!(power_activation(Some(text), Some(index)), Some(*action));
            }
            assert_eq!(power_activation(Some(text), None), None);
            assert_eq!(power_activation(Some(text), Some(6)), None);
        }
        assert_eq!(power_activation(Some("x"), Some(0)), None);
        assert_eq!(power_activation(None, Some(0)), None);
    }

    // OBSERVABLE: exactly the focused stop is ringed. MUTATION: ringing the index off by one or
    // leaving two stops ringed fails here.
    #[test]
    fn focus_ring_marks_only_the_focused_stop() {
        assert_eq!(power_focus_ring(None), [false; 6]);
        assert_eq!(
            power_focus_ring(Some(3)),
            [false, false, false, true, false, false]
        );
    }

    // REGRESSION: every stop id must exist in battery.html, and each stop must carry the
    // `power-stop` class whose rule gives a 1px border box, or the ring never paints.
    // MUTATION: renaming a stop id or dropping the border from the class fails here.
    #[test]
    fn power_focus_ids_exist_in_html_with_a_ring_border_box() {
        let html = include_str!("../../../../../ui/shell/battery.html");
        for id in POWER_STOP_IDS {
            assert!(
                html.contains(&format!("id=\"{id}\"")),
                "{id} missing from battery.html"
            );
        }
        assert!(
            html.contains(".power-stop { border:1px solid transparent; box-sizing:border-box;"),
            "power-stop class needs a border box"
        );
    }
}
