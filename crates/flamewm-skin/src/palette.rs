//! Exact colors from `examples/flamewm-v8/flamewm.css`, line 1 and component rules.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlphaColor {
    pub color: Rgb,
    pub alpha: u8,
}

pub const ACCENT: Rgb = Rgb(0x00ef_4048);
pub const DESKTOP: Rgb = Rgb(0x0000_0000);
pub const PANEL: Rgb = Rgb(0x001b_1e20);
pub const SURFACE: Rgb = Rgb(0x0020_2326);
pub const SIDEBAR: Rgb = Rgb(0x001b_1e20);
pub const TEXT: Rgb = Rgb(0x00f1_f2f3);
pub const MUTED: Rgb = Rgb(0x00ae_b4bb);
pub const STICKY_BACKGROUND: Rgb = Rgb(0x00ff_e56b);
pub const STICKY_TEXT: Rgb = Rgb(0x0017_1717);

pub const ACCENT_SOFT: AlphaColor = AlphaColor {
    color: ACCENT,
    alpha: 0x38,
};
pub const DESKTOP_HOVER: AlphaColor = AlphaColor {
    color: ACCENT,
    alpha: 0x26,
};
pub const DESKTOP_SELECTED: AlphaColor = AlphaColor {
    color: ACCENT,
    alpha: 0x47,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Rgb,
    pub desktop: Rgb,
    pub panel: Rgb,
    pub surface: Rgb,
    pub sidebar: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub sticky_background: Rgb,
    pub sticky_text: Rgb,
}

impl Palette {
    pub const RWR_0_0_9: Self = Self {
        accent: ACCENT,
        desktop: DESKTOP,
        panel: PANEL,
        surface: SURFACE,
        sidebar: SIDEBAR,
        text: TEXT,
        muted: MUTED,
        sticky_background: STICKY_BACKGROUND,
        sticky_text: STICKY_TEXT,
    };
}

impl Default for Palette {
    fn default() -> Self {
        Self::RWR_0_0_9
    }
}

/// Shell appearance mode that selects a palette. `Dark` is the shipped default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

/// Light-mode counterparts of the dark text and surface tokens. Each text token is checked
/// against the surfaces it sits on by the `contrast` tests below.
pub const LIGHT_SURFACE: Rgb = Rgb(0x00f4_f5f7);
pub const LIGHT_PANEL: Rgb = Rgb(0x00e4_e7eb);
pub const LIGHT_TEXT: Rgb = Rgb(0x0018_1b1e);
pub const LIGHT_MUTED: Rgb = Rgb(0x0043_4a52);

impl Palette {
    pub const LIGHT: Self = Self {
        accent: ACCENT,
        desktop: DESKTOP,
        panel: LIGHT_PANEL,
        surface: LIGHT_SURFACE,
        sidebar: LIGHT_PANEL,
        text: LIGHT_TEXT,
        muted: LIGHT_MUTED,
        sticky_background: STICKY_BACKGROUND,
        sticky_text: STICKY_TEXT,
    };

    /// Palette for a shell appearance. `Dark` returns the unchanged shipped palette.
    #[must_use]
    pub const fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::RWR_0_0_9,
            Appearance::Light => Self::LIGHT,
        }
    }
}

/// Translucent overlay tokens that invert per appearance. White overlays are the Dark values and
/// vanish on light glass, so Light uses black overlays of the same role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overlays {
    pub rim: AlphaColor,
    pub hover: AlphaColor,
    pub pressed: AlphaColor,
}

impl Overlays {
    pub const DARK: Self = Self {
        rim: AlphaColor {
            color: Rgb(0x00ff_ffff),
            alpha: 0x7f,
        },
        hover: AlphaColor {
            color: Rgb(0x00ff_ffff),
            alpha: 0x40,
        },
        pressed: AlphaColor {
            color: Rgb(0x00ff_ffff),
            alpha: 0x26,
        },
    };
    pub const LIGHT: Self = Self {
        rim: AlphaColor {
            color: Rgb(0),
            alpha: 0x33,
        },
        hover: AlphaColor {
            color: Rgb(0),
            alpha: 0x26,
        },
        pressed: AlphaColor {
            color: Rgb(0),
            alpha: 0x40,
        },
    };

    #[must_use]
    pub const fn for_appearance(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Dark => Self::DARK,
            Appearance::Light => Self::LIGHT,
        }
    }
}

/// Source-over composite of a translucent colour onto an opaque base.
#[must_use]
pub fn composite(over: AlphaColor, base: Rgb) -> Rgb {
    let a = u32::from(over.alpha);
    let mix = |shift: u32| {
        let f = (over.color.0 >> shift) & 0xff;
        let b = (base.0 >> shift) & 0xff;
        ((f * a + b * (255 - a) + 127) / 255) << shift
    };
    Rgb(mix(16) | mix(8) | mix(0))
}

/// Minimum contrast of a hover fill against the surface it sits on (non-text state fill).
pub const HOVER_MIN_CONTRAST: f64 = 1.25;
/// Minimum contrast between the hover and pressed fills, so pressed reads as a distinct state.
pub const PRESSED_VS_HOVER_MIN_CONTRAST: f64 = 1.15;

/// Checks the hover and pressed overlays of `appearance` over the panel surface. Every surface
/// contract test calls this one owner, so the thresholds cannot drift between sheets.
pub fn check_overlay_contrast(appearance: Appearance) -> Result<(), String> {
    let base = Palette::for_appearance(appearance).panel;
    let overlays = Overlays::for_appearance(appearance);
    let hover = composite(overlays.hover, base);
    let pressed = composite(overlays.pressed, base);
    let hover_ratio = contrast_ratio(hover, base);
    if hover_ratio < HOVER_MIN_CONTRAST {
        return Err(format!(
            "{appearance:?} hover {hover_ratio:.2} < {HOVER_MIN_CONTRAST}"
        ));
    }
    let pressed_ratio = contrast_ratio(hover, pressed);
    if pressed_ratio < PRESSED_VS_HOVER_MIN_CONTRAST {
        return Err(format!(
            "{appearance:?} pressed/hover {pressed_ratio:.2} < {PRESSED_VS_HOVER_MIN_CONTRAST}"
        ));
    }
    Ok(())
}

/// WCAG 2.x relative luminance of an opaque colour.
#[must_use]
pub fn relative_luminance(color: Rgb) -> f64 {
    let channel = |shift: u32| {
        let c = f64::from((color.0 >> shift) & 0xff) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// WCAG 2.x contrast ratio between two opaque colours, in `1.0..=21.0`.
#[must_use]
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Label ink for text drawn on an accent fill: white or the Light text ink, whichever has the higher
/// WCAG contrast against `accent`. Dark text wins on light accents such as the shipped `#3DAEE9`.
#[must_use]
pub fn on_accent(accent: Rgb) -> Rgb {
    const WHITE: Rgb = Rgb(0x00ff_ffff);
    if contrast_ratio(accent, WHITE) >= contrast_ratio(accent, LIGHT_TEXT) {
        WHITE
    } else {
        LIGHT_TEXT
    }
}

/// Text palette for ink drawn on a neutral `surface`: Dark or Light, whichever has the higher WCAG
/// contrast. Dark glass keeps the shipped Dark palette; light glass gets the Light text and panel.
#[must_use]
pub fn ink_palette_on(surface: Rgb) -> Palette {
    if contrast_ratio(surface, Palette::RWR_0_0_9.text)
        >= contrast_ratio(surface, Palette::LIGHT.text)
    {
        Palette::RWR_0_0_9
    } else {
        Palette::LIGHT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG AA for normal text.
    const AA_TEXT: f64 = 4.5;
    /// Shipped Dark titlebar material (`DEFAULT_SURFACE_RGB`).
    const DESKTOP_DARK_SURFACE: Rgb = Rgb(0x0033_3333);

    #[test]
    fn ink_follows_the_surface_it_sits_on() {
        assert_eq!(ink_palette_on(DESKTOP_DARK_SURFACE), Palette::RWR_0_0_9);
        assert_eq!(ink_palette_on(LIGHT_SURFACE), Palette::LIGHT);
        assert_eq!(ink_palette_on(LIGHT_PANEL), Palette::LIGHT);
    }

    #[test]
    fn default_palette_is_the_shipped_dark_palette() {
        assert_eq!(Palette::default(), Palette::RWR_0_0_9);
        assert_eq!(
            Palette::for_appearance(Appearance::Dark),
            Palette::RWR_0_0_9
        );
        assert_eq!(Palette::RWR_0_0_9.text, Rgb(0x00f1_f2f3));
        assert_eq!(Palette::RWR_0_0_9.surface, Rgb(0x0020_2326));
    }

    #[test]
    fn contrast_reference_values() {
        assert!((contrast_ratio(Rgb(0x00ff_ffff), Rgb(0)) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(Rgb(0x0077_7777), Rgb(0x0077_7777)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn dark_text_tokens_meet_aa_on_every_surface() {
        let p = Palette::RWR_0_0_9;
        for bg in [p.surface, p.panel, p.sidebar] {
            assert!(contrast_ratio(p.text, bg) >= AA_TEXT, "text on {bg:?}");
            assert!(contrast_ratio(p.muted, bg) >= AA_TEXT, "muted on {bg:?}");
        }
    }

    #[test]
    fn light_text_tokens_meet_aa_on_every_surface() {
        let p = Palette::for_appearance(Appearance::Light);
        for bg in [p.surface, p.panel, p.sidebar] {
            assert!(contrast_ratio(p.text, bg) >= AA_TEXT, "text on {bg:?}");
            assert!(contrast_ratio(p.muted, bg) >= AA_TEXT, "muted on {bg:?}");
        }
    }

    #[test]
    fn light_palette_inverts_text_against_surface() {
        let p = Palette::for_appearance(Appearance::Light);
        assert!(relative_luminance(p.surface) > relative_luminance(p.text));
        assert!(relative_luminance(p.panel) > relative_luminance(p.muted));
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    #[test]
    fn hover_and_pressed_overlays_meet_contrast_on_each_appearance() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            assert_eq!(check_overlay_contrast(appearance), Ok(()));
        }
    }

    #[test]
    fn light_overlays_are_dark_so_they_show_on_light_glass() {
        assert_eq!(Overlays::LIGHT.hover.color, Rgb(0));
        assert_eq!(Overlays::LIGHT.pressed.color, Rgb(0));
    }

    #[test]
    fn dark_overlays_are_the_shipped_white_alphas() {
        assert_eq!(Overlays::DARK.hover.alpha, 0x40);
        assert_eq!(Overlays::DARK.pressed.alpha, 0x26);
        assert_eq!(Overlays::DARK.rim.alpha, 0x7f);
    }

    #[test]
    fn composite_at_full_and_zero_alpha() {
        let white = AlphaColor {
            color: Rgb(0x00ff_ffff),
            alpha: 0xff,
        };
        assert_eq!(composite(white, Rgb(0)), Rgb(0x00ff_ffff));
        let clear = AlphaColor {
            color: Rgb(0x00ff_ffff),
            alpha: 0,
        };
        assert_eq!(composite(clear, Rgb(0x0012_3456)), Rgb(0x0012_3456));
    }

    #[test]
    fn on_accent_picks_the_higher_contrast_ink() {
        // Shipped default accent: white is 2.5:1, dark ink is well above AA.
        let default_accent = Rgb(0x003d_aee9);
        assert_eq!(on_accent(default_accent), LIGHT_TEXT);
        assert!(contrast_ratio(on_accent(default_accent), default_accent) >= 4.5);
        assert_eq!(on_accent(Rgb(0)), Rgb(0x00ff_ffff));
        assert_eq!(on_accent(Rgb(0x00ff_ffff)), LIGHT_TEXT);
        for accent in [Rgb(0x00ef_4048), Rgb(0x0021_7bc0), Rgb(0x0090_ee40)] {
            let ink = on_accent(accent);
            let other = if ink == LIGHT_TEXT {
                Rgb(0x00ff_ffff)
            } else {
                LIGHT_TEXT
            };
            assert!(contrast_ratio(ink, accent) >= contrast_ratio(other, accent));
        }
    }
}
