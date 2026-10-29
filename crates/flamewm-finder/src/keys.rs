//! Key handling as pure data. The window translates each key press into a [`Command`]; the rules
//! live here so they can be tested without an X11 session.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Close the open overlay, or the window when none is open.
    Escape,
    /// Open the selected file with its default application.
    Open,
    /// Show the selected file's folder.
    Reveal,
    /// Toggle the quick-look preview of the selected file.
    Preview,
    /// Toggle the details panel of the selected file.
    Details,
    /// Move to the next result (Tab), or the next tile in grid view.
    Next,
    /// Move to the previous result (Shift+Tab), or the previous tile in grid view.
    Previous,
    /// Arrow keys: one row up or down in the list, a row of tiles in grid view.
    Up,
    Down,
    /// Arrow keys: one tile left or right in grid view; no effect in the list.
    Left,
    Right,
    /// Switch between the list and the grid of tiles.
    ToggleView,
    /// Show or hide the keyboard help.
    Help,
    PageDown,
    PageUp,
    First,
    Last,
    Backspace,
    Insert(String),
}

/// Maps one key press to a command. `navigating` is true once the user has moved into the result
/// list; Space previews only then, otherwise it types a space.
#[must_use]
pub fn command(
    key: &str,
    text: Option<&str>,
    mods: Modifiers,
    navigating: bool,
) -> Option<Command> {
    let key = key.to_lowercase();
    match key.as_str() {
        "escape" | "esc" => return Some(Command::Escape),
        "return" | "enter" | "kp_enter" => {
            return Some(if mods.shift {
                Command::Reveal
            } else {
                Command::Open
            });
        }
        "up" => return Some(Command::Up),
        "down" => return Some(Command::Down),
        "left" => return Some(Command::Left),
        "right" => return Some(Command::Right),
        "f1" => return Some(Command::Help),
        "tab" => {
            return Some(if mods.shift {
                Command::Previous
            } else {
                Command::Next
            });
        }
        "page_up" | "prior" => return Some(Command::PageUp),
        "page_down" | "next" => return Some(Command::PageDown),
        "home" => return Some(Command::First),
        "end" => return Some(Command::Last),
        "backspace" => return Some(Command::Backspace),
        "space" if navigating && !mods.ctrl && !mods.alt => return Some(Command::Preview),
        "i" if mods.ctrl => return Some(Command::Details),
        "g" if mods.ctrl && !mods.shift && !mods.alt => return Some(Command::ToggleView),
        _ => {}
    }
    if mods.ctrl || mods.alt {
        return None;
    }
    let typed = text?;
    if typed.is_empty() || typed.chars().any(char::is_control) {
        return None;
    }
    Some(Command::Insert(typed.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };

    #[test]
    fn enter_opens_and_shift_enter_reveals() {
        assert_eq!(command("return", None, NONE, false), Some(Command::Open));
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(command("return", None, shift, false), Some(Command::Reveal));
    }

    #[test]
    fn arrows_and_tab_walk_the_list() {
        assert_eq!(command("down", None, NONE, false), Some(Command::Down));
        assert_eq!(command("up", None, NONE, false), Some(Command::Up));
        assert_eq!(command("left", None, NONE, false), Some(Command::Left));
        assert_eq!(command("right", None, NONE, false), Some(Command::Right));
        assert_eq!(command("tab", None, NONE, false), Some(Command::Next));
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(command("tab", None, shift, false), Some(Command::Previous));
        assert_eq!(
            command("page_down", None, NONE, true),
            Some(Command::PageDown)
        );
        assert_eq!(command("home", None, NONE, true), Some(Command::First));
    }

    #[test]
    fn space_previews_only_after_navigating_otherwise_types() {
        assert_eq!(
            command("space", Some(" "), NONE, true),
            Some(Command::Preview)
        );
        assert_eq!(
            command("space", Some(" "), NONE, false),
            Some(Command::Insert(" ".to_owned()))
        );
    }

    #[test]
    fn ctrl_g_toggles_the_view_and_f1_opens_help() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        assert_eq!(
            command("g", Some("g"), ctrl, false),
            Some(Command::ToggleView)
        );
        assert_eq!(command("f1", None, NONE, false), Some(Command::Help));
        // Ctrl+Shift+G is not the view toggle, and it does not type a letter either.
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..NONE
        };
        assert_eq!(command("g", Some("G"), ctrl_shift, false), None);
    }

    #[test]
    fn escape_always_closes_and_backspace_edits() {
        assert_eq!(command("escape", None, NONE, true), Some(Command::Escape));
        assert_eq!(
            command("backspace", None, NONE, false),
            Some(Command::Backspace)
        );
    }

    #[test]
    fn ctrl_i_toggles_details_and_other_ctrl_keys_do_not_type() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        assert_eq!(command("i", Some("i"), ctrl, false), Some(Command::Details));
        assert_eq!(command("c", Some("c"), ctrl, false), None);
    }

    #[test]
    fn printable_text_is_inserted_and_control_text_is_not() {
        assert_eq!(
            command("a", Some("é"), NONE, false),
            Some(Command::Insert("é".to_owned()))
        );
        assert_eq!(command("unknown", Some("\u{7f}"), NONE, false), None);
        assert_eq!(command("unknown", None, NONE, false), None);
    }
}
