//! Keyboard operation of the AI usage card. The card is a grabbed status popup whose only focus
//! stop is the Close button: provider rows are read-only meters. Tab moves focus forward and
//! Shift+Tab backward, wrapping over the stops; Enter or Space on a focused stop activates it.
//! Precedent: WAI-ARIA APG dialog (focus stays inside, Tab cycles) and the KDE Plasma tray popup
//! (Tab walks its focusable controls), and the audio popup's Tab order in `quick_controls`.

/// Focus stops on the card, in Tab order.
pub(crate) const USAGE_FOCUS_STOPS: usize = 1;

/// Focus after a typed Tab; `None` when the key is not Tab or the card has no stops.
pub(crate) fn usage_focus_after_tab(
    payload: &str,
    focus: Option<usize>,
    stops: usize,
) -> Option<usize> {
    if stops == 0 {
        return None;
    }
    let (modifiers, key) = flamewm_shell::launcher::parse_typed_key(payload)?;
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

/// True when a typed Enter or Space activates the focused stop.
pub(crate) fn usage_activates(text: Option<&str>, focus: Option<usize>) -> bool {
    focus.is_some_and(|index| index < USAGE_FOCUS_STOPS) && matches!(text, Some("\n" | "\r" | " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifiers: &str, name: &str) -> String {
        format!("{modifiers}\u{1f}{name}\u{1f}")
    }

    // OBSERVABLE: Tab reaches the first stop, Shift+Tab the last, and both wrap.
    // MUTATION: dropping the wrap or the Shift branch fails one of these asserts.
    #[test]
    fn tab_moves_focus_forward_and_shift_tab_backward_with_wrap() {
        assert_eq!(usage_focus_after_tab(&key("", "Tab"), None, 1), Some(0));
        assert_eq!(usage_focus_after_tab(&key("", "Tab"), Some(0), 1), Some(0));
        assert_eq!(
            usage_focus_after_tab(&key("Shift", "Tab"), None, 3),
            Some(2)
        );
        assert_eq!(usage_focus_after_tab(&key("", "Tab"), Some(2), 3), Some(0));
        assert_eq!(
            usage_focus_after_tab(&key("Shift", "Tab"), Some(0), 3),
            Some(2)
        );
        assert_eq!(usage_focus_after_tab(&key("", "Tab"), Some(0), 0), None);
        assert_eq!(usage_focus_after_tab(&key("", "Down"), None, 1), None);
    }

    #[test]
    fn enter_and_space_activate_only_a_focused_stop() {
        for text in ["\n", "\r", " "] {
            assert!(usage_activates(Some(text), Some(0)));
            assert!(!usage_activates(Some(text), None));
        }
        assert!(!usage_activates(Some("x"), Some(0)));
        assert!(!usage_activates(None, Some(0)));
    }
}
