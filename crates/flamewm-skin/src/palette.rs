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
pub const PANEL: Rgb = Rgb(0x001c_1c1c);
pub const SURFACE: Rgb = Rgb(0x0020_2020);
pub const SIDEBAR: Rgb = Rgb(0x001c_1c1c);
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
pub const LIGHT_PANEL: Rgb = Rgb(0x00e4_e6e8);
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

/// WCAG AA contrast for body text.
pub const BODY_MIN_CONTRAST: f64 = 4.5;
/// WCAG AA contrast for large text and non-text ink.
pub const LARGE_MIN_CONTRAST: f64 = 3.0;

/// Mean colour of backdrop samples (for example a 16x16 wallpaper downsample), averaged per
/// sRGB channel. Empty input returns black. Computed once per backdrop change, never per frame.
#[must_use]
pub fn mean_backdrop(samples: &[Rgb]) -> Rgb {
    if samples.is_empty() {
        return Rgb(0);
    }
    let count = samples.len() as u64;
    let channel = |shift: u32| {
        let sum: u64 = samples
            .iter()
            .map(|s| u64::from((s.0 >> shift) & 0xff))
            .sum();
        (((sum + count / 2) / count) as u32) << shift
    };
    Rgb(channel(16) | channel(8) | channel(0))
}

/// Colour a glass surface shows: `tint` at `alpha` (0..=255) over the backdrop sample.
#[must_use]
pub fn effective_backdrop(tint: Rgb, alpha: u8, backdrop: Rgb) -> Rgb {
    composite(AlphaColor { color: tint, alpha }, backdrop)
}

/// Least scrim alpha (0..=255) that lets `ink` reach `target` over `base`. The scrim is black when
/// `ink` is lighter than `base` and white otherwise, so it always pushes the base away from the ink.
/// Contrast only grows with scrim alpha, so a binary search finds the floor. `Some(0)` when `ink`
/// already meets `target`; `None` when even a full scrim cannot.
#[must_use]
pub fn scrim_alpha_floor(ink: Rgb, base: Rgb, target: f64) -> Option<u8> {
    let scrim = if relative_luminance(ink) > relative_luminance(base) {
        Rgb(0)
    } else {
        Rgb(0x00ff_ffff)
    };
    scrim_alpha_floor_with(ink, base, scrim, target)
}

/// [`scrim_alpha_floor`] with the scrim colour fixed by the caller, so several inks share one scrim.
#[must_use]
pub fn scrim_alpha_floor_with(ink: Rgb, base: Rgb, scrim: Rgb, target: f64) -> Option<u8> {
    let meets = |alpha: u8| {
        contrast_ratio(
            ink,
            composite(
                AlphaColor {
                    color: scrim,
                    alpha,
                },
                base,
            ),
        ) >= target
    };
    if !meets(u8::MAX) {
        return None;
    }
    let (mut lo, mut hi) = (0_u8, u8::MAX);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if meets(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Some(lo)
}

/// Ink for one glass surface: the text palette chosen against the effective backdrop, and the scrim
/// floor each ink colour needs to reach `target` over that backdrop. The caller raises the surface
/// tint to `max(user_alpha, scrim)`, so the opacity setting still applies above the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlassInk {
    pub palette: Palette,
    /// Scrim floor for the body text colour.
    pub text_scrim: Option<u8>,
    /// Scrim floor for the muted colour, which is the least contrasting token.
    pub muted_scrim: Option<u8>,
}

/// Single owner for legible glass ink: polarity from the effective backdrop, then scrim floors.
#[must_use]
pub fn glass_ink(tint: Rgb, alpha: u8, backdrop: Rgb, target: f64) -> GlassInk {
    let effective = effective_backdrop(tint, alpha, backdrop);
    let palette = ink_palette_on(effective);
    GlassInk {
        palette,
        text_scrim: scrim_alpha_floor(palette.text, effective, target),
        muted_scrim: scrim_alpha_floor(palette.muted, effective, target),
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
        assert_eq!(Palette::RWR_0_0_9.surface, Rgb(0x0020_2020));
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

#[cfg(test)]
mod glass_ink_tests {
    use super::*;

    /// `glass::LIGHT`: white tint at 5%, alpha 13/255.
    const LIGHT_TINT: Rgb = Rgb(0x00ff_ffff);
    const LIGHT_ALPHA: u8 = 13;
    /// `glass::DARK`: `#eaf4ff` tint at 12%, alpha 31/255.
    const DARK_TINT: Rgb = Rgb(0x00ea_f4ff);
    const DARK_ALPHA: u8 = 31;
    /// Measured Task Manager Light backdrop samples under the rows.
    const TM_LOW: Rgb = Rgb(0x005b_6182);
    const TM_HIGH: Rgb = Rgb(0x0071_6c87);
    /// Measured bright pink wallpaper under Settings Dark sidebar labels.
    const PINK: Rgb = Rgb(0x00ff_b6c1);
    /// Bright Light wallpaper stand-in for Finder tiles. Not a captured sample.
    const BRIGHT: Rgb = Rgb(0x00f0_f0f0);

    /// Contrast of `ink` over `base` after `floor` of the scrim `ink` was given.
    fn contrast_with_floor(ink: Rgb, base: Rgb, floor: u8) -> f64 {
        let scrim = if relative_luminance(ink) > relative_luminance(base) {
            Rgb(0)
        } else {
            Rgb(0x00ff_ffff)
        };
        contrast_ratio(
            ink,
            composite(
                AlphaColor {
                    color: scrim,
                    alpha: floor,
                },
                base,
            ),
        )
    }

    #[test]
    fn mean_backdrop_averages_per_channel_and_handles_empty() {
        assert_eq!(mean_backdrop(&[]), Rgb(0));
        assert_eq!(
            mean_backdrop(&[Rgb(0x0000_0000), Rgb(0x00ff_ffff)]),
            Rgb(0x0080_8080)
        );
        assert_eq!(mean_backdrop(&[Rgb(0x0012_3456)]), Rgb(0x0012_3456));
    }

    #[test]
    fn effective_backdrop_is_the_backdrop_at_zero_and_the_tint_at_full_alpha() {
        assert_eq!(effective_backdrop(LIGHT_TINT, 0, TM_LOW), TM_LOW);
        assert_eq!(effective_backdrop(LIGHT_TINT, 255, TM_LOW), LIGHT_TINT);
    }

    #[test]
    fn task_manager_light_muted_was_1_5_to_1_8_before_the_owner() {
        // Before: the Light palette was chosen from the material alone, so muted was #434a52.
        let before_low = contrast_ratio(LIGHT_MUTED, TM_LOW);
        let before_high = contrast_ratio(LIGHT_MUTED, TM_HIGH);
        assert!((before_low - 1.49).abs() < 0.02, "{before_low}");
        assert!((before_high - 1.79).abs() < 0.02, "{before_high}");
    }

    #[test]
    fn task_manager_light_glass_gets_dark_ink_and_a_scrim_floor() {
        for backdrop in [TM_LOW, TM_HIGH] {
            let ink = glass_ink(LIGHT_TINT, LIGHT_ALPHA, backdrop, BODY_MIN_CONTRAST);
            let effective = effective_backdrop(LIGHT_TINT, LIGHT_ALPHA, backdrop);
            assert_eq!(ink.palette, Palette::RWR_0_0_9, "{backdrop:?}");
            for (token, floor) in [
                (ink.palette.text, ink.text_scrim),
                (ink.palette.muted, ink.muted_scrim),
            ] {
                let floor = floor.expect("a scrim reaches 4.5:1 on this backdrop");
                assert!(contrast_with_floor(token, effective, floor) >= BODY_MIN_CONTRAST);
                if floor > 0 {
                    assert!(contrast_with_floor(token, effective, floor - 1) < BODY_MIN_CONTRAST);
                }
            }
        }
        // The upper sample needs scrim for body text: Dark ink alone is 4.04:1 there.
        let high = glass_ink(LIGHT_TINT, LIGHT_ALPHA, TM_HIGH, BODY_MIN_CONTRAST);
        assert!(high.text_scrim.is_some_and(|floor| floor > 0));
    }

    #[test]
    fn settings_dark_sidebar_on_pink_flips_to_dark_ink() {
        // Before: Dark ink #f1f2f3 was 1.41:1 on the pink glass.
        let effective = effective_backdrop(DARK_TINT, DARK_ALPHA, PINK);
        assert!(contrast_ratio(Palette::RWR_0_0_9.text, effective) < 2.0);
        let ink = glass_ink(DARK_TINT, DARK_ALPHA, PINK, BODY_MIN_CONTRAST);
        assert_eq!(ink.palette, Palette::LIGHT);
        assert_eq!(ink.text_scrim, Some(0));
    }

    #[test]
    fn finder_bright_backdrop_takes_dark_ink() {
        let ink = glass_ink(LIGHT_TINT, LIGHT_ALPHA, BRIGHT, BODY_MIN_CONTRAST);
        assert_eq!(ink.palette, Palette::LIGHT);
    }

    #[test]
    fn scrim_floor_is_zero_when_ink_already_reaches_target() {
        assert_eq!(
            scrim_alpha_floor(LIGHT_TEXT, Rgb(0x00ff_ffff), BODY_MIN_CONTRAST),
            Some(0)
        );
    }

    #[test]
    fn scrim_floor_is_none_when_no_scrim_can_reach_target() {
        // Mid grey ink over mid grey base: even a full white or black scrim stays below 21:1.
        assert_eq!(
            scrim_alpha_floor(Rgb(0x0077_7777), Rgb(0x0077_7777), 21.0),
            None
        );
    }

    #[test]
    fn scrim_floor_is_the_least_alpha_that_reaches_target() {
        let floor = scrim_alpha_floor(Rgb(0x00ae_b4bb), TM_LOW, BODY_MIN_CONTRAST).unwrap();
        assert!(floor > 0);
        assert!(contrast_with_floor(Rgb(0x00ae_b4bb), TM_LOW, floor) >= BODY_MIN_CONTRAST);
        assert!(contrast_with_floor(Rgb(0x00ae_b4bb), TM_LOW, floor - 1) < BODY_MIN_CONTRAST);
    }
}
