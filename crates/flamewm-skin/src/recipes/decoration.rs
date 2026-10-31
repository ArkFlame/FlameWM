//! Window decoration policy and typed titlebar style tokens (single owner).
//!
//! Eligibility: `Floating` and snapped (layout) windows get the rounded mask.
//! Maximized and fullscreen windows are rectangular.

use crate::palette::Rgb;

/// Corner radius of the `Rounded` style for floating windows.
pub const ROUNDED_RADIUS: u16 = 24;

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

/// Settings default for `surfaceColor` (shared Surfaces RGB).
pub const DEFAULT_SURFACE_RGB: Rgb = Rgb(0x0033_3333);
/// Settings default for `surfaceOpacity` 65% (Tahoe dark) as an 8-bit alpha (`65 * 255 / 100`).
pub const DEFAULT_SURFACE_ALPHA: u8 = 165;
/// Settings default for `surfaceBlurPercent`.
pub const DEFAULT_SURFACE_BLUR_PERCENT: u8 = 80;

/// Typed chrome style tokens. Index order: minimize, maximize, close.
/// `titlebar_bg` is the straight RGB of `surfaceColor`; `titlebar_alpha` is the
/// `surfaceOpacity` percent mapped to 0..=255 (`percent * 255 / 100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChromeTokens {
    pub style: DecorationStyle,
    pub titlebar_bg: Rgb,
    pub titlebar_alpha: u8,
    /// `surfaceBlurPercent` (0..=100): blur of what is behind the titlebar.
    pub titlebar_blur_percent: u8,
    /// Shared one-pixel inner glass contour (RRGGBB + straight alpha).
    pub surface_border_rgb: Rgb,
    pub surface_border_alpha: u8,
    /// Bumped when the backdrop source (wallpaper) changes so frames repaint.
    pub backdrop_revision: u32,
    pub buttons: [ButtonTokens; 3],
}

impl Default for ChromeTokens {
    fn default() -> Self {
        Self {
            style: DecorationStyle::Rounded,
            titlebar_bg: DEFAULT_SURFACE_RGB,
            titlebar_alpha: DEFAULT_SURFACE_ALPHA,
            titlebar_blur_percent: DEFAULT_SURFACE_BLUR_PERCENT,
            surface_border_rgb: Rgb(0x00ff_ffff),
            surface_border_alpha: 128,
            backdrop_revision: 0,
            buttons: [ButtonTokens::default(); 3],
        }
    }
}

/// Corner radius for a frame: 0 unless the window is rounded-eligible
/// (`Floating` or snapped; the caller passes that flag).
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
    fn rounded_is_twenty_four_for_floating_only() {
        assert_eq!(corner_radius(true, DecorationStyle::Rounded), 24);
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
        assert_eq!(tokens.titlebar_bg, DEFAULT_SURFACE_RGB);
        assert_eq!(tokens.titlebar_alpha, DEFAULT_SURFACE_ALPHA);
        assert_eq!(tokens.style, DecorationStyle::Rounded);
        assert_eq!(tokens.buttons, [ButtonTokens::default(); 3]);
    }
}
