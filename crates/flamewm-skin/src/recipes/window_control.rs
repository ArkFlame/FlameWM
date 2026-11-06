//! Titlebar window-control style: the single colour owner for minimize, maximize/restore and
//! close, shared by the Wayland chrome and the X11 frame.
//!
//! Hover and pressed are inverted for minimize/maximize: a light fill with a dark glyph. Close
//! takes the red fill with a light glyph. The rest state keeps the light glyph on the titlebar.

use crate::palette::{LIGHT_TEXT, Palette, Rgb, TEXT};
use crate::recipes::window_chrome::WINDOW_CHROME;

/// Pressed fill for minimize/maximize on Dark ink: one step darker than the hover fill.
pub const INVERTED_PRESSED_FILL: Rgb = Rgb(0x00c7_c9cb);
/// Pressed fill for minimize/maximize on Light ink: one step lighter than the dark hover fill.
pub const LIGHT_INVERTED_PRESSED_FILL: Rgb = Rgb(0x0034_3a40);
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

/// Colours for one control. `ink` is the titlebar's text palette (`ChromeTokens::ink`): the rest
/// glyph takes its text ink, and minimize/maximize hover and pressed invert against it. Close keeps
/// its red fill with a light glyph whatever the ink.
#[must_use]
pub const fn control_colors(kind: ControlKind, state: ControlState, ink: Palette) -> ControlColors {
    let light = ink.text.0 == LIGHT_TEXT.0;
    match (kind, state) {
        (_, ControlState::Rest) => ControlColors {
            background: None,
            glyph: ink.text,
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
            background: Some(ink.text),
            glyph: ink.panel,
        },
        (_, ControlState::Pressed) => ControlColors {
            background: Some(if light {
                LIGHT_INVERTED_PRESSED_FILL
            } else {
                INVERTED_PRESSED_FILL
            }),
            glyph: ink.panel,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_and_max_hover_invert_to_white_fill_with_dark_glyph() {
        for kind in [ControlKind::Minimize, ControlKind::Maximize] {
            let hover = control_colors(kind, ControlState::Hover, Palette::RWR_0_0_9);
            assert_eq!(hover.background, Some(Rgb(0x00f1_f2f3)));
            assert_eq!(hover.glyph, Rgb(0x001c_1c1c));
        }
    }

    #[test]
    fn light_ink_inverts_min_and_max_against_dark_text() {
        let ink = crate::palette::Palette::LIGHT;
        let hover = control_colors(ControlKind::Minimize, ControlState::Hover, ink);
        assert_eq!(hover.background, Some(ink.text));
        assert_eq!(hover.glyph, ink.panel);
        assert_eq!(
            control_colors(ControlKind::Maximize, ControlState::Rest, ink).glyph,
            ink.text
        );
    }

    #[test]
    fn close_hover_is_red_with_light_glyph() {
        let hover = control_colors(ControlKind::Close, ControlState::Hover, Palette::RWR_0_0_9);
        assert_eq!(hover.background, Some(Rgb(0x00e8_1123)));
        assert_eq!(hover.glyph, Rgb(0x00f1_f2f3));
    }

    #[test]
    fn pressed_states_are_distinct_from_hover() {
        for kind in [
            ControlKind::Minimize,
            ControlKind::Maximize,
            ControlKind::Close,
        ] {
            let hover = control_colors(kind, ControlState::Hover, Palette::RWR_0_0_9);
            let pressed = control_colors(kind, ControlState::Pressed, Palette::RWR_0_0_9);
            assert_ne!(hover.background, pressed.background, "{kind:?}");
        }
        assert_eq!(
            control_colors(
                ControlKind::Minimize,
                ControlState::Pressed,
                Palette::RWR_0_0_9
            )
            .background,
            Some(INVERTED_PRESSED_FILL)
        );
        assert_eq!(
            control_colors(
                ControlKind::Close,
                ControlState::Pressed,
                Palette::RWR_0_0_9
            )
            .background,
            Some(CLOSE_PRESSED_FILL)
        );
    }

    #[test]
    fn rest_has_no_fill_and_light_glyph_for_every_kind() {
        for kind in [
            ControlKind::Minimize,
            ControlKind::Maximize,
            ControlKind::Close,
        ] {
            assert_eq!(
                control_colors(kind, ControlState::Rest, Palette::RWR_0_0_9),
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
        assert_eq!(
            ControlState::from_pointer(true, true),
            ControlState::Pressed
        );
        assert_eq!(
            ControlState::from_pointer(false, true),
            ControlState::Pressed
        );
    }
}
