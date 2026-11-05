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

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG AA for normal text.
    const AA_TEXT: f64 = 4.5;

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
