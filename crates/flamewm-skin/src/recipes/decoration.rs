//! Window decoration policy and typed titlebar style tokens (single owner).
//!
//! Eligibility: only a `Floating` window gets the rounded mask. Snapped,
//! maximized, fullscreen windows are rectangular.

use super::window_chrome::WINDOW_CHROME;
use crate::palette::Rgb;

/// Corner radius of the `Rounded` style for floating windows.
pub const ROUNDED_RADIUS: u16 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DecorationStyle {
    #[default]
    Rounded,
    Squared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ButtonShape {
    /// Today's paint: close gets a red disk on hover/press, others the svg scheme.
    #[default]
    Default,
    /// No filled backing: hover/press fill matches the titlebar.
    Flat,
    Circle,
    Square,
}

/// Per-button overrides. `None` keeps the skin default for that slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ButtonTokens {
    pub shape: ButtonShape,
    pub glyph: Option<Rgb>,
    pub hover: Option<Rgb>,
    pub pressed: Option<Rgb>,
}

/// Typed chrome style tokens. Index order: minimize, maximize, close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChromeTokens {
    pub style: DecorationStyle,
    pub titlebar_bg: Rgb,
    pub buttons: [ButtonTokens; 3],
}

impl Default for ChromeTokens {
    fn default() -> Self {
        Self {
            style: DecorationStyle::Rounded,
            titlebar_bg: WINDOW_CHROME.titlebar_background,
            buttons: [ButtonTokens::default(); 3],
        }
    }
}

/// Corner radius for a frame: 0 unless the window is `Floating`.
#[must_use]
pub const fn corner_radius(floating: bool, style: DecorationStyle) -> u16 {
    if !floating {
        return 0;
    }
    match style {
        DecorationStyle::Rounded => ROUNDED_RADIUS,
        DecorationStyle::Squared => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squared_is_zero_in_every_state() {
        for floating in [false, true] {
            assert_eq!(corner_radius(floating, DecorationStyle::Squared), 0);
        }
    }

    #[test]
    fn rounded_is_twelve_for_floating_only() {
        assert_eq!(corner_radius(true, DecorationStyle::Rounded), 12);
        assert_eq!(
            corner_radius(true, DecorationStyle::Rounded),
            ROUNDED_RADIUS
        );
        assert_eq!(
            corner_radius(false, DecorationStyle::Rounded),
            0,
            "snapped/max/fullscreen"
        );
    }

    #[test]
    fn default_tokens_reproduce_skin_titlebar_and_buttons() {
        let tokens = ChromeTokens::default();
        assert_eq!(tokens.titlebar_bg, crate::palette::PANEL);
        assert_eq!(tokens.style, DecorationStyle::Rounded);
        assert_eq!(tokens.buttons, [ButtonTokens::default(); 3]);
    }
}
