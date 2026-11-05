//! Per-appearance token projection shared by every surface that ships `ui/` CSS.
//!
//! Stylesheets keep the Dark values in `:root`; these writes swap them for the snapshot's
//! appearance. Overlays (`--rim`, `--hover-fill`, `--pressed-fill`) invert on light glass, so a
//! white overlay never ends up invisible there.

use flamewm_api::settings::SettingsSnapshot;
use flamewm_skin::palette::{AlphaColor, Overlays, Palette, Rgb};
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

/// `--text` and `--muted` for one appearance's palette.
#[must_use]
pub fn text_token_values(palette: Palette) -> [(&'static str, UiColor); 2] {
    [
        ("--text", color(palette.text, 255)),
        ("--muted", color(palette.muted, 255)),
    ]
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
    fn overlay_contrast_holds_for_both_appearances() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            assert_eq!(check_overlay_contrast(appearance), Ok(()));
        }
    }
}
