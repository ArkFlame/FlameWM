//! Keyboard operation of the media card. The card is a status popup with three transport stops in
//! Tab order: previous, play or pause, next. Tab moves focus forward and Shift+Tab backward,
//! wrapping over the stops; Enter or Space on a focused stop activates it once and keeps focus
//! on that stop. A stop with no active player drops the action (see `context_system_request`).
//! Escape closes the card through the generic keyboard path.
//! Precedent: WAI-ARIA APG toolbar and dialog (Tab moves between controls, Enter and Space
//! activate the focused control) and the KDE Plasma applet popup, whose Tab walks its controls.
//! Tab walk shared with the usage card and the notification centre; see `focus_keys`.

use super::*;

/// Transport stops on the card, in Tab order.
pub(crate) const MEDIA_FOCUS_STOPS: usize = 3;

/// Action dispatched by each stop, indexed by Tab-order position.
pub(crate) const MEDIA_STOP_ACTIONS: [&str; MEDIA_FOCUS_STOPS] =
    ["media.previous", "media.playpause", "media.next"];

/// Tab walk shared with the usage card and the notification centre.
pub(crate) use flamewm_shell::focus_keys::tab_focus_after as media_focus_after_tab;

/// Action of the focused stop when a typed Enter or Space activates it; `None` otherwise.
#[must_use]
pub(crate) fn media_activation(text: Option<&str>, focus: Option<usize>) -> Option<&'static str> {
    let index = focus.filter(|index| *index < MEDIA_FOCUS_STOPS)?;
    matches!(text, Some("\n" | "\r" | " ")).then_some(MEDIA_STOP_ACTIONS[index])
}

impl ShellLoop {
    /// Projects the focus ring onto the media card and redraws it.
    pub(crate) fn project_media_focus(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let focus = self.media_focus;
        runtime.with_document(self.surfaces.media, |document| {
            crate::projection::project_media_focus(document, focus)
        })?;
        runtime.redraw(self.surfaces.media)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modifiers: &str, name: &str) -> String {
        format!("{modifiers}\u{1f}{name}\u{1f}")
    }

    // OBSERVABLE: Tab walks previous, play/pause, next and wraps; Shift+Tab walks backward.
    // MUTATION: dropping the wrap or the Shift branch fails one of these asserts.
    #[test]
    fn tab_walks_the_three_transport_stops_with_wrap() {
        assert_eq!(
            media_focus_after_tab(&key("", "Tab"), None, MEDIA_FOCUS_STOPS),
            Some(0)
        );
        assert_eq!(
            media_focus_after_tab(&key("", "Tab"), Some(0), MEDIA_FOCUS_STOPS),
            Some(1)
        );
        assert_eq!(
            media_focus_after_tab(&key("", "Tab"), Some(2), MEDIA_FOCUS_STOPS),
            Some(0)
        );
        assert_eq!(
            media_focus_after_tab(&key("Shift", "Tab"), None, MEDIA_FOCUS_STOPS),
            Some(2)
        );
        assert_eq!(
            media_focus_after_tab(&key("Shift", "Tab"), Some(0), MEDIA_FOCUS_STOPS),
            Some(2)
        );
        assert_eq!(
            media_focus_after_tab(&key("", "Down"), None, MEDIA_FOCUS_STOPS),
            None
        );
    }

    // OBSERVABLE: Enter and Space on a focused stop dispatch that stop's media action, once per
    // typed key. Unfocused or out-of-range focus and other text activate nothing.
    // MUTATION: mapping the wrong index to an action, or activating without focus, fails here.
    #[test]
    fn enter_and_space_activate_only_the_focused_stop() {
        for text in ["\n", "\r", " "] {
            assert_eq!(
                media_activation(Some(text), Some(0)),
                Some("media.previous")
            );
            assert_eq!(
                media_activation(Some(text), Some(1)),
                Some("media.playpause")
            );
            assert_eq!(media_activation(Some(text), Some(2)), Some("media.next"));
            assert_eq!(media_activation(Some(text), None), None);
            assert_eq!(media_activation(Some(text), Some(MEDIA_FOCUS_STOPS)), None);
        }
        assert_eq!(media_activation(Some("x"), Some(0)), None);
        assert_eq!(media_activation(None, Some(0)), None);
    }
}
