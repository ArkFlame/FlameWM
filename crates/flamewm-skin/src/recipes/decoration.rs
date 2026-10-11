//! Window decoration policy and typed titlebar style tokens (single owner).
//!
//! Eligibility: only a `Floating` window gets the rounded mask (and the iOS
//! glass recipe). Snapped, maximized, fullscreen windows are rectangular.

use super::window_chrome::WINDOW_CHROME;
use crate::palette::Rgb;

/// Corner radius of the iOS recipe (Rounded keeps `chrome.radius`).
pub const IOS_RADIUS: u16 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DecorationStyle {
    #[default]
    Rounded,
    Ios,
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
        DecorationStyle::Rounded => WINDOW_CHROME.metrics.radius,
        DecorationStyle::Ios => IOS_RADIUS,
    }
}

/// Glass highlight/edge only for a `Floating` iOS window.
#[must_use]
pub const fn glass_enabled(floating: bool, style: DecorationStyle) -> bool {
    floating && matches!(style, DecorationStyle::Ios)
}

/// Opaque glass tints `(top highlight, inner edge)` precomputed against the
/// titlebar color. Not a blur: no backdrop is sampled.
#[must_use]
pub fn glass_tints(titlebar: Rgb) -> (Rgb, Rgb) {
    (blend(titlebar, 0xffffff, 90), blend(titlebar, 0xffffff, 30))
}

/// Per-channel mix of `base` toward `over` by `weight/255`.
fn blend(base: Rgb, over: u32, weight: u32) -> Rgb {
    let mut out = 0_u32;
    for shift in [16, 8, 0] {
        let a = (base.0 >> shift) & 0xff;
        let b = (over >> shift) & 0xff;
        let c = (a * (255 - weight) + b * weight + 127) / 255;
        out |= c << shift;
    }
    Rgb(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_truth_table_only_floating_is_rounded() {
        for style in [DecorationStyle::Rounded, DecorationStyle::Ios] {
            assert_eq!(corner_radius(false, style), 0, "snapped/max/fullscreen");
            assert!(!glass_enabled(false, style));
        }
        assert_eq!(
            corner_radius(true, DecorationStyle::Rounded),
            WINDOW_CHROME.metrics.radius
        );
        assert_eq!(corner_radius(true, DecorationStyle::Ios), IOS_RADIUS);
        assert!(IOS_RADIUS > WINDOW_CHROME.metrics.radius);
        assert!(!glass_enabled(true, DecorationStyle::Rounded));
        assert!(glass_enabled(true, DecorationStyle::Ios));
    }

    #[test]
    fn default_tokens_reproduce_skin_titlebar_and_buttons() {
        let tokens = ChromeTokens::default();
        assert_eq!(tokens.titlebar_bg, crate::palette::PANEL);
        assert_eq!(tokens.style, DecorationStyle::Rounded);
        assert_eq!(tokens.buttons, [ButtonTokens::default(); 3]);
    }

    #[test]
    fn glass_tints_are_lighter_than_bar_and_highlight_is_brightest() {
        let (top, edge) = glass_tints(crate::palette::PANEL);
        assert_ne!(top, crate::palette::PANEL);
        assert_ne!(edge, crate::palette::PANEL);
        assert!(top.0 > edge.0);
        assert_eq!(blend(Rgb(0), 0xffffff, 255), Rgb(0xffffff));
        assert_eq!(blend(Rgb(0x123456), 0xffffff, 0), Rgb(0x123456));
    }
}
