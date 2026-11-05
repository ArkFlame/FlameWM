//! Titlebar window-control style: the single colour owner for minimize, maximize/restore and
//! close, shared by the Wayland chrome and the X11 frame.
//!
//! Hover and pressed are inverted for minimize/maximize: a light fill with a dark glyph. Close
//! takes the red fill with a light glyph. The rest state keeps the light glyph on the titlebar.

use crate::palette::{PANEL, Rgb, TEXT};
use crate::recipes::window_chrome::WINDOW_CHROME;

/// Pressed fill for minimize/maximize: one step darker than the hover fill.
pub const INVERTED_PRESSED_FILL: Rgb = Rgb(0x00c7_c9cb);
/// Pressed fill for close: one step darker than `WINDOW_CHROME.close_hover`.
pub const CLOSE_PRESSED_FILL: Rgb = Rgb(0x00c4_2b1c);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    Minimize,
    /// Maximize and restore share one style.
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ControlState {
    #[default]
    Rest,
    Hover,
    Pressed,
}

impl ControlState {
    /// Pressed wins over hover; a pointer on a control that is armed is pressed.
    #[must_use]
    pub const fn from_pointer(hovered: bool, pressed: bool) -> Self {
        if pressed {
            Self::Pressed
        } else if hovered {
            Self::Hover
        } else {
            Self::Rest
        }
    }
}

/// Fill behind the glyph (`None` at rest: the titlebar shows through) and the glyph colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlColors {
    pub background: Option<Rgb>,
    pub glyph: Rgb,
}

#[must_use]
pub const fn control_colors(kind: ControlKind, state: ControlState) -> ControlColors {
    match (kind, state) {
        (_, ControlState::Rest) => ControlColors {
            background: None,
            glyph: TEXT,
        },
        (ControlKind::Close, ControlState::Hover) => ControlColors {
            background: Some(WINDOW_CHROME.close_hover),
            glyph: TEXT,
        },
        (ControlKind::Close, ControlState::Pressed) => ControlColors {
            background: Some(CLOSE_PRESSED_FILL),
            glyph: TEXT,
        },
        (_, ControlState::Hover) => ControlColors {
            background: Some(TEXT),
            glyph: PANEL,
        },
        (_, ControlState::Pressed) => ControlColors {
            background: Some(INVERTED_PRESSED_FILL),
            glyph: PANEL,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_and_max_hover_invert_to_white_fill_with_dark_glyph() {
        for kind in [ControlKind::Minimize, ControlKind::Maximize] {
            let hover = control_colors(kind, ControlState::Hover);
            assert_eq!(hover.background, Some(Rgb(0x00f1_f2f3)));
            assert_eq!(hover.glyph, Rgb(0x001b_1e20));
        }
    }

    #[test]
    fn close_hover_is_red_with_light_glyph() {
        let hover = control_colors(ControlKind::Close, ControlState::Hover);
        assert_eq!(hover.background, Some(Rgb(0x00e8_1123)));
        assert_eq!(hover.glyph, Rgb(0x00f1_f2f3));
    }

    #[test]
    fn pressed_states_are_distinct_from_hover() {
        for kind in [ControlKind::Minimize, ControlKind::Maximize, ControlKind::Close] {
            let hover = control_colors(kind, ControlState::Hover);
            let pressed = control_colors(kind, ControlState::Pressed);
            assert_ne!(hover.background, pressed.background, "{kind:?}");
        }
        assert_eq!(
            control_colors(ControlKind::Minimize, ControlState::Pressed).background,
            Some(INVERTED_PRESSED_FILL)
        );
        assert_eq!(
            control_colors(ControlKind::Close, ControlState::Pressed).background,
            Some(CLOSE_PRESSED_FILL)
        );
    }

    #[test]
    fn rest_has_no_fill_and_light_glyph_for_every_kind() {
        for kind in [ControlKind::Minimize, ControlKind::Maximize, ControlKind::Close] {
            assert_eq!(
                control_colors(kind, ControlState::Rest),
                ControlColors {
                    background: None,
                    glyph: Rgb(0x00f1_f2f3),
                }
            );
        }
    }

    #[test]
    fn pointer_state_maps_pressed_over_hover_over_rest() {
        assert_eq!(ControlState::from_pointer(false, false), ControlState::Rest);
        assert_eq!(ControlState::from_pointer(true, false), ControlState::Hover);
        assert_eq!(ControlState::from_pointer(true, true), ControlState::Pressed);
        assert_eq!(ControlState::from_pointer(false, true), ControlState::Pressed);
    }
}
