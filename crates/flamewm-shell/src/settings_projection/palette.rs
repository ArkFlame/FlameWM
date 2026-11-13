//! Palette and accent token projection shared by shell surfaces.

use super::*;

/// Neutral tokens (`--panel`, `--surface`, `--sidebar`, `--text`, `--muted`), the overlays and every
/// fixed `--c-<hex>` chrome tone follow the appearance. Dark writes equal the shipped `:root`
/// values. Tokens a surface does not compile are skipped.
pub(super) fn project_palette_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    let palette = flamewm_ui_core::glass::palette_from_settings(snapshot);
    for (name, color) in [
        ("--panel", palette.panel),
        ("--surface", palette.surface),
        ("--sidebar", palette.sidebar),
        ("--text", palette.text),
        ("--muted", palette.muted),
    ] {
        let _ = document.color_variable(name, rgb(color.0, 255));
    }
    let _ = flamewm_ui_document::appearance_tokens::project_overlay_tokens(
        document,
        flamewm_ui_core::glass::overlays_from_settings(snapshot),
    );
    let appearance = if flamewm_ui_core::glass::appearance_from_settings(snapshot).light {
        Appearance::Light
    } else {
        Appearance::Dark
    };
    // highContrast: opaque chrome surfaces (`--c-1c2740` start menu, dock and panel tones) take the
    // palette surface, so the fixed tones follow the same set as `--panel`. Translucent scrims keep
    // their alpha.
    let high_contrast = flamewm_ui_core::glass::high_contrast_from_settings(snapshot);
    for tone in FIXED_TONES {
        let color = tone.for_appearance(appearance);
        let (value, alpha) =
            if high_contrast && tone.role == ToneRole::Surface && color.alpha == 0xff {
                (palette.surface.0, 0xff)
            } else {
                (color.color.0, color.alpha)
            };
        let _ = document.color_variable(tone.name, rgb(value, alpha));
    }
    Ok(())
}

/// `ui/shell/flamewm.css` accent-derived tokens with the alpha each keeps in its `:root` default.
pub(super) const ACCENT_TOKEN_ALPHAS: [(&str, u8); 8] = [
    ("--accent", 0xff),
    ("--accent-soft", ACCENT_SOFT_ALPHA),
    ("--accent-strong", 0xdd),
    ("--desktop-hover", 0x26),
    ("--desktop-hover-border", 0x4d),
    ("--desktop-selected", 0x47),
    ("--desktop-selected-border", 0xa6),
    ("--snap-fill", 0x33),
];

/// Projects every accent-derived token from the live `accent`/`accentOpacity` pair.
pub(super) fn project_accent_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    project_palette_tokens(document, snapshot)?;
    for (name, alpha) in ACCENT_TOKEN_ALPHAS {
        document.color_variable(name, accent_tint(snapshot, alpha))?;
    }
    // Only surfaces that draw a preview/selection outline compile this variable in.
    let _ = document.color_variable(
        "--preview-outline",
        UiColor {
            a: flamewm_ui_core::style::PREVIEW_OUTLINE_ALPHA,
            ..accent_color(snapshot)
        },
    );
    Ok(())
}

pub(super) fn accent_color(snapshot: &SettingsSnapshot) -> UiColor {
    let accent = rgb(flamewm_ui_core::accent::accent(snapshot).0, 255);
    let opacity = percent_setting(snapshot, "accentOpacity", DEFAULT_ACCENT_OPACITY);
    UiColor {
        a: opacity_alpha(opacity),
        ..accent
    }
}

/// Derived accent token: keeps the stylesheet's base alpha, scaled by `accentOpacity`.
pub(super) fn accent_tint(snapshot: &SettingsSnapshot, base_alpha: u8) -> UiColor {
    let accent = accent_color(snapshot);
    UiColor {
        a: ((u16::from(base_alpha) * u16::from(accent.a)) / 255) as u8,
        ..accent
    }
}

pub(super) fn opacity_alpha(percent: u8) -> u8 {
    ((u16::from(percent) * 255) / 100) as u8
}

pub(super) fn rgb(value: u32, alpha: u8) -> UiColor {
    UiColor {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
        a: alpha,
    }
}
