//! Keyboard operation of the taskbar overflow menu, following the WAI-ARIA menu-button pattern:
//! Down/Up/Home/End move the highlighted row, Enter or Space activates it. Escape closes through
//! the shared route arm. Precedent: WAI-ARIA APG menu button (arrow, Home, End, Enter, Space).
//! Focus return to the trigger is not wired: shell surfaces have no keyboard focus model.

/// Number of rows the open overflow menu shows; zero when another menu, or none, is open.
pub(crate) fn overflow_menu_rows(menu: Option<&flamewm_shell::ContextMenuState>) -> usize {
    match menu.map(|menu| &menu.kind) {
        Some(flamewm_shell::ContextMenuKind::TaskOverflow { entries }) => entries.len(),
        _ => 0,
    }
}

/// Highlighted row after a typed navigation key on the open overflow menu. `None` when the key is
/// not a menu navigation key or no row exists.
pub(crate) fn overflow_menu_step(payload: &str, rows: usize, highlight: usize) -> Option<usize> {
    if rows == 0 {
        return None;
    }
    let (_, key) = flamewm_shell::launcher::parse_typed_key(payload)?;
    match key {
        "Down" => Some((highlight + 1).min(rows - 1)),
        "Up" => Some(highlight.saturating_sub(1)),
        "Home" => Some(0),
        "End" => Some(rows - 1),
        _ => None,
    }
}

/// Row action that a typed Enter or Space activates on the open overflow menu. `None` when no
/// menu row exists or the text is another key.
pub(crate) fn overflow_activation_row(
    text: Option<&str>,
    rows: usize,
    highlight: usize,
) -> Option<String> {
    if rows == 0 {
        return None;
    }
    match text? {
        "\n" | "\r" | " " => Some(format!("task.overflow.{}", highlight.min(rows - 1))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_space_activate_the_highlighted_row_while_rows_exist() {
        for text in ["\n", "\r", " "] {
            assert_eq!(
                overflow_activation_row(Some(text), 3, 0).as_deref(),
                Some("task.overflow.0"),
                "key {text:?}"
            );
            assert_eq!(
                overflow_activation_row(Some(text), 3, 2).as_deref(),
                Some("task.overflow.2"),
                "key {text:?}"
            );
        }
        assert_eq!(overflow_activation_row(Some("\n"), 0, 0), None);
        assert_eq!(overflow_activation_row(Some(" "), 0, 0), None);
    }

    #[test]
    fn other_keys_and_missing_text_do_not_activate() {
        assert_eq!(overflow_activation_row(Some("\u{1b}"), 3, 0), None);
        assert_eq!(overflow_activation_row(Some("a"), 3, 0), None);
        assert_eq!(overflow_activation_row(None, 3, 0), None);
    }

    /// Typed-key payload `modifiers US key US text`, as the shell receives `__flamewm.ui.key`.
    fn key(name: &str) -> String {
        format!("\u{1f}{name}\u{1f}")
    }

    #[test]
    fn arrows_move_and_clamp_within_rows() {
        assert_eq!(overflow_menu_step(&key("Down"), 3, 0), Some(1));
        assert_eq!(overflow_menu_step(&key("Down"), 3, 2), Some(2));
        assert_eq!(overflow_menu_step(&key("Up"), 3, 0), Some(0));
        assert_eq!(overflow_menu_step(&key("Up"), 3, 2), Some(1));
        assert_eq!(overflow_menu_step(&key("End"), 3, 0), Some(2));
        assert_eq!(overflow_menu_step(&key("Home"), 3, 2), Some(0));
    }

    #[test]
    fn navigation_needs_rows_and_a_navigation_key() {
        assert_eq!(overflow_menu_step(&key("Down"), 0, 0), None);
        assert_eq!(overflow_menu_step(&key("a"), 3, 0), None);
        assert_eq!(overflow_menu_step(&key("Escape"), 3, 0), None);
    }
}
