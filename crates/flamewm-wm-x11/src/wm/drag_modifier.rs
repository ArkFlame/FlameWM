//! Modifier for modifier+drag move (primary button) and modifier+Ctrl+right-drag resize.
//! Configured by `FLAMEWM_DRAG_MODIFIER` (`super` or `alt`). The default `super` keeps the
//! historical behaviour. Unknown values fall back to the default.

use std::env;

/// X11 modifier bit: Mod4 (Super / Windows key).
const MOD_SUPER: u16 = 0x0040;
/// X11 modifier bit: Mod1 (Alt).
const MOD_ALT: u16 = 0x0008;
/// X11 modifier bit: Control.
const MOD_CONTROL: u16 = 0x0004;
const BUTTON_PRIMARY: u8 = 1;
const BUTTON_RIGHT: u8 = 3;

/// Modifier + primary button begins the canonical titlebar move regardless of client-area
/// coordinates. Other buttons or modifiers replay unchanged, preserving selection and menus.
#[must_use]
pub fn is_meta_move_press(detail: u8, state: u16, modifier: u16) -> bool {
    detail == BUTTON_PRIMARY && state & modifier != 0
}

/// Control + modifier + right button starts a resize (GNOME/KDE convention). Left stays move.
/// Right is the chosen button because Control+modifier+left is still matched by the move rule.
#[must_use]
pub fn is_meta_resize_press(detail: u8, state: u16, modifier: u16) -> bool {
    detail == BUTTON_RIGHT && state & MOD_CONTROL != 0 && state & modifier != 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragModifier {
    /// Super (Mod4) plus drag, the historical default.
    #[default]
    Super,
    /// Alt (Mod1) plus drag. Alt+click is then consumed by the WM and not replayed to the client.
    Alt,
}

impl DragModifier {
    /// Modifier bit tested against the event state.
    #[must_use]
    pub const fn mask(self) -> u16 {
        match self {
            Self::Super => MOD_SUPER,
            Self::Alt => MOD_ALT,
        }
    }

    /// Parses a configured value, ignoring case and surrounding whitespace.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "super" | "meta" => Some(Self::Super),
            "alt" => Some(Self::Alt),
            _ => None,
        }
    }

    /// Reads `FLAMEWM_DRAG_MODIFIER`; unset or invalid values give the default.
    #[must_use]
    pub fn from_env() -> Self {
        env::var("FLAMEWM_DRAG_MODIFIER")
            .ok()
            .and_then(|value| Self::parse(&value))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{DragModifier, MOD_ALT, MOD_SUPER, is_meta_move_press, is_meta_resize_press};

    #[test]
    fn masks_match_x11_modifier_bits() {
        assert_eq!(DragModifier::Super.mask(), MOD_SUPER);
        assert_eq!(DragModifier::Alt.mask(), MOD_ALT);
        assert_eq!(DragModifier::Super.mask(), 0x0040);
        assert_eq!(DragModifier::Alt.mask(), 0x0008);
    }

    #[test]
    fn parses_known_values_and_rejects_others() {
        assert_eq!(DragModifier::parse("Super"), Some(DragModifier::Super));
        assert_eq!(DragModifier::parse(" meta "), Some(DragModifier::Super));
        assert_eq!(DragModifier::parse("ALT"), Some(DragModifier::Alt));
        assert_eq!(DragModifier::parse("ctrl"), None);
        assert_eq!(DragModifier::default(), DragModifier::Super);
    }

    #[test]
    fn super_drag_is_the_default_move_and_alt_drag_moves_only_with_alt() {
        assert!(is_meta_move_press(1, 0x0040, MOD_SUPER));
        assert!(!is_meta_move_press(1, 0x0008, MOD_SUPER));
        assert!(is_meta_move_press(1, 0x0008, MOD_ALT));
        assert!(!is_meta_move_press(1, 0x0040, MOD_ALT));
        assert!(!is_meta_move_press(2, 0x0008, MOD_ALT));
    }

    #[test]
    fn modifier_right_drag_resizes_only_with_control_and_modifier() {
        assert!(is_meta_resize_press(3, 0x0004 | 0x0008, MOD_ALT));
        assert!(!is_meta_resize_press(3, 0x0008, MOD_ALT));
        assert!(!is_meta_resize_press(3, 0x0004 | 0x0040, MOD_ALT));
        assert!(is_meta_resize_press(3, 0x0004 | 0x0040, MOD_SUPER));
    }
}
