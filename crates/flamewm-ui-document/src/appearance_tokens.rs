//! Per-appearance token projection shared by every surface that ships `ui/` CSS.
//!
//! Stylesheets keep the Dark values in `:root`; these writes swap them for the snapshot's
//! appearance. Overlays (`--rim`, `--hover-fill`, `--pressed-fill`) invert on light glass, so a
//! white overlay never ends up invisible there.

use flamewm_api::settings::SettingsSnapshot;
use flamewm_skin::palette::{AlphaColor, Overlays, Palette, Rgb, on_accent};
use flamewm_ui_core::glass;

use crate::template::{UiColor, UiDocumentAccess};

fn color(rgb: Rgb, alpha: u8) -> UiColor {
    UiColor {
        r: (rgb.0 >> 16) as u8,
        g: (rgb.0 >> 8) as u8,
        b: rgb.0 as u8,
        a: alpha,
    }
}

fn overlay_color(overlay: AlphaColor) -> UiColor {
    color(overlay.color, overlay.alpha)
}

/// `--rim`, `--hover-fill`, `--pressed-fill` for one appearance's overlays.
#[must_use]
pub fn overlay_token_values(overlays: Overlays) -> [(&'static str, UiColor); 3] {
    [
        ("--rim", overlay_color(overlays.rim)),
        ("--hover-fill", overlay_color(overlays.hover)),
        ("--pressed-fill", overlay_color(overlays.pressed)),
    ]
}

/// `--text`, `--text-secondary` and `--muted` for one appearance's palette. `--text-secondary` is
/// the text ink at 85% alpha, the value the stylesheets' Dark `:root` already used.
#[must_use]
pub fn text_token_values(palette: Palette) -> [(&'static str, UiColor); 3] {
    [
        ("--text", color(palette.text, 255)),
        ("--text-secondary", color(palette.text, 0xd9)),
        ("--muted", color(palette.muted, 255)),
    ]
}

/// Label ink for text drawn on the accent fill (`--on-accent`), chosen by contrast so the label stays
/// readable for any accent. Selected segments and nav rows use the same value.
#[must_use]
pub fn on_accent_color(accent: UiColor) -> UiColor {
    let rgb = Rgb((u32::from(accent.r) << 16) | (u32::from(accent.g) << 8) | u32::from(accent.b));
    color(on_accent(rgb), 255)
}

/// Writes `--on-accent` for the given accent colour.
pub fn project_on_accent_token(
    document: &mut impl UiDocumentAccess,
    accent: UiColor,
) -> Result<(), String> {
    document.color_variable("--on-accent", on_accent_color(accent))
}

/// Writes the overlay tokens. Surfaces with their own surface model call this alone.
pub fn project_overlay_tokens(
    document: &mut impl UiDocumentAccess,
    overlays: Overlays,
) -> Result<(), String> {
    for (name, value) in overlay_token_values(overlays) {
        document.color_variable(name, value)?;
    }
    Ok(())
}

/// Settings nav-row fills (`--nav-hover`, `--nav-pressed`). Dark keeps the shipped 7% and 12%
/// white; Light uses black at the same weight, because white vanishes on light glass.
#[must_use]
pub fn nav_row_token_values(light: bool) -> [(&'static str, UiColor); 2] {
    if light {
        [
            ("--nav-hover", color(Rgb(0), 0x14)),
            ("--nav-pressed", color(Rgb(0), 0x26)),
        ]
    } else {
        [
            ("--nav-hover", color(Rgb(0x00ff_ffff), 0x12)),
            ("--nav-pressed", color(Rgb(0x00ff_ffff), 0x1f)),
        ]
    }
}

/// Writes the settings nav-row fills for the snapshot's appearance. Only documents that declare
/// `--nav-hover` and `--nav-pressed` call this.
pub fn project_nav_row_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    for (name, value) in nav_row_token_values(glass::appearance_from_settings(snapshot).light) {
        document.color_variable(name, value)?;
    }
    Ok(())
}

/// Writes the text tokens. Surfaces whose text is not appearance-driven skip this.
pub fn project_text_tokens(
    document: &mut impl UiDocumentAccess,
    palette: Palette,
) -> Result<(), String> {
    for (name, value) in text_token_values(palette) {
        document.color_variable(name, value)?;
    }
    Ok(())
}

/// Writes text and overlay tokens together.
pub fn project_appearance_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    project_text_tokens(document, glass::palette_from_settings(snapshot))?;
    project_overlay_tokens(document, glass::overlays_from_settings(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_row_fills_keep_the_dark_alphas_and_go_black_on_light() {
        let dark = nav_row_token_values(false);
        assert_eq!(dark[0].1, color(Rgb(0x00ff_ffff), 0x12));
        assert_eq!(dark[1].1, color(Rgb(0x00ff_ffff), 0x1f));
        let light = nav_row_token_values(true);
        assert_eq!(light[0].1, color(Rgb(0), 0x14));
        assert_eq!(light[1].1, color(Rgb(0), 0x26));
    }
    use flamewm_skin::palette::{Appearance, check_overlay_contrast};

    fn value(values: &[(&str, UiColor)], name: &str) -> UiColor {
        values.iter().find(|(n, _)| *n == name).expect(name).1
    }

    #[test]
    fn dark_overlays_are_the_shipped_white_alphas() {
        let values = overlay_token_values(Overlays::DARK);
        assert_eq!(
            value(&values, "--hover-fill"),
            UiColor {
                r: 255,
                g: 255,
                b: 255,
                a: 0x40
            }
        );
        assert_eq!(
            value(&values, "--pressed-fill"),
            UiColor {
                r: 255,
                g: 255,
                b: 255,
                a: 0x26
            }
        );
        assert_eq!(
            value(&values, "--rim"),
            UiColor {
                r: 255,
                g: 255,
                b: 255,
                a: 0x7f
            }
        );
    }

    #[test]
    fn light_overlays_invert_to_black_alphas() {
        let values = overlay_token_values(Overlays::LIGHT);
        let light = Overlays::LIGHT;
        assert_eq!(
            value(&values, "--hover-fill"),
            UiColor {
                r: 0,
                g: 0,
                b: 0,
                a: light.hover.alpha
            }
        );
        assert_eq!(
            value(&values, "--pressed-fill"),
            UiColor {
                r: 0,
                g: 0,
                b: 0,
                a: light.pressed.alpha
            }
        );
        assert_ne!(
            value(&values, "--hover-fill").a,
            value(&values, "--pressed-fill").a
        );
    }

    #[test]
    fn light_text_tokens_follow_the_light_palette() {
        let palette = Palette::for_appearance(Appearance::Light);
        let values = text_token_values(palette);
        assert_eq!(value(&values, "--text"), color(palette.text, 255));
    }

    #[test]
    fn light_text_secondary_follows_the_light_ink() {
        let palette = Palette::for_appearance(Appearance::Light);
        let values = text_token_values(palette);
        let secondary = value(&values, "--text-secondary");
        assert_eq!((secondary.r, secondary.g, secondary.b), (0x18, 0x1b, 0x1e));
        assert_eq!(secondary.a, 0xd9);
    }

    #[test]
    fn on_accent_ink_follows_accent_contrast() {
        let default_accent = UiColor {
            r: 0x3d,
            g: 0xae,
            b: 0xe9,
            a: 255,
        };
        let ink = on_accent_color(default_accent);
        assert_eq!((ink.r, ink.g, ink.b, ink.a), (0x18, 0x1b, 0x1e, 255));
        let black = UiColor {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        assert_eq!(on_accent_color(black).r, 255);
    }

    #[test]
    fn overlay_contrast_holds_for_both_appearances() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            assert_eq!(check_overlay_contrast(appearance), Ok(()));
        }
    }
}
