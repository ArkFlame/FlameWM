//! Keyboard activation for the taskbar overflow menu, following the WAI-ARIA menu-button
//! pattern: Enter or Space on an open menu activates the first item. Escape closes through the
//! shared route arm. Arrow navigation is not wired: shell popups receive typed text only.

/// Row action of the first overflow row; rows are named `task.overflow.N` in `task-menu.html`.
const FIRST_ROW_ACTION: &str = "task.overflow.0";

/// Number of rows the open overflow menu shows; zero when another menu, or none, is open.
pub(crate) fn overflow_menu_rows(menu: Option<&flamewm_shell::ContextMenuState>) -> usize {
    match menu.map(|menu| &menu.kind) {
        Some(flamewm_shell::ContextMenuKind::TaskOverflow { entries }) => entries.len(),
        _ => 0,
    }
}

/// Row action that a typed Enter or Space activates on the open overflow menu. `None` when no
/// menu row exists or the text is another key.
pub(crate) fn overflow_activation_row(text: Option<&str>, rows: usize) -> Option<&'static str> {
    if rows == 0 {
        return None;
    }
    match text? {
        "\n" | "\r" | " " => Some(FIRST_ROW_ACTION),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_space_activate_the_first_row_only_while_rows_exist() {
        for text in ["\n", "\r", " "] {
            assert_eq!(
                overflow_activation_row(Some(text), 3),
                Some("task.overflow.0"),
                "key {text:?}"
            );
        }
        assert_eq!(overflow_activation_row(Some("\n"), 0), None);
        assert_eq!(overflow_activation_row(Some(" "), 0), None);
    }

    #[test]
    fn other_keys_and_missing_text_do_not_activate() {
        assert_eq!(overflow_activation_row(Some("\u{1b}"), 3), None);
        assert_eq!(overflow_activation_row(Some("a"), 3), None);
        assert_eq!(overflow_activation_row(None, 3), None);
    }
}
