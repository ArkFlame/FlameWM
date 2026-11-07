//! Fixed shell chrome tones: every `--c-<hex>` token of `ui/shell/flamewm.css` (plus `--strong`)
//! with a Light counterpart. One table; `settings_projection` writes `for_appearance` per token.
//! The Dark column equals the shipped `:root` declarations, so Dark rendering is unchanged.

use crate::palette::{AlphaColor, Appearance, Rgb};

/// What a tone is drawn for. Sets the contrast it must keep in both themes (see tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneRole {
    /// Chrome background that carries body text: `text` and `muted` reach 4.5:1 on it.
    Surface,
    /// Hover fill on chrome: body text reaches 4.5:1 on it and it stays 1.15:1 off the bar.
    Hover,
    /// Text colour: 4.5:1 on every neutral base.
    Ink,
    /// De-emphasised text (out-of-month calendar days): 2.5:1 on the bar.
    Dim,
    /// Icon, line or border: 3:1 on every neutral base.
    Mark,
    /// Control track fill: 1.5:1 on the bar, so the track reads against its panel.
    Track,
    /// Fixed hue in both themes (accent swatches, pure white and black).
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedTone {
    /// CSS custom property. `--c-<hex>` names spell their own Dark value.
    pub name: &'static str,
    pub role: ToneRole,
    pub dark: AlphaColor,
    pub light: AlphaColor,
}

impl FixedTone {
    #[must_use]
    pub const fn for_appearance(&self, appearance: Appearance) -> AlphaColor {
        match appearance {
            Appearance::Dark => self.dark,
            Appearance::Light => self.light,
        }
    }
}

const fn t(name: &'static str, role: ToneRole, dark: AlphaColor, light: AlphaColor) -> FixedTone {
    FixedTone {
        name,
        role,
        dark,
        light,
    }
}

const fn o(rgb: u32) -> AlphaColor {
    AlphaColor {
        color: Rgb(rgb),
        alpha: 0xff,
    }
}

const fn a(rgb: u32, alpha: u8) -> AlphaColor {
    AlphaColor {
        color: Rgb(rgb),
        alpha,
    }
}

/// Light `Surface` tones are a ramp from `#c4c9ce` (darkest Dark well) to white, ordered like the
/// Dark tones they replace. `Ink` runs from `#181b1e` (brightest Dark text) to `#566069`.
pub const FIXED_TONES: &[FixedTone] = &[
    t(
        "--c-00000080",
        ToneRole::Surface,
        a(0x0000_0000, 0x80),
        a(0x0000_0000, 0x33),
    ),
    t(
        "--c-050607",
        ToneRole::Surface,
        o(0x0005_0607),
        o(0x00c9_ced2),
    ),
    t(
        "--c-0c0d0e",
        ToneRole::Surface,
        o(0x000c_0d0e),
        o(0x00d0_d4d8),
    ),
    t(
        "--c-111315",
        ToneRole::Surface,
        o(0x0011_1315),
        o(0x00d5_d9dc),
    ),
    t(
        "--c-151719",
        ToneRole::Surface,
        o(0x0015_1719),
        o(0x00d9_dcdf),
    ),
    t(
        "--c-17191b",
        ToneRole::Surface,
        o(0x0017_191b),
        o(0x00da_dde1),
    ),
    t(
        "--c-181a1c",
        ToneRole::Surface,
        o(0x0018_1a1c),
        o(0x00db_dee1),
    ),
    t(
        "--c-191b1d",
        ToneRole::Surface,
        o(0x0019_1b1d),
        o(0x00dc_dfe2),
    ),
    t(
        "--c-1c2740",
        ToneRole::Surface,
        o(0x001c_2740),
        o(0x00eb_edee),
    ),
    t(
        "--c-1e1e1e",
        ToneRole::Surface,
        o(0x001e_1e1e),
        o(0x00df_e2e4),
    ),
    t(
        "--c-202224",
        ToneRole::Surface,
        o(0x0020_2224),
        o(0x00e2_e5e7),
    ),
    t(
        "--c-232629",
        ToneRole::Surface,
        o(0x0023_2629),
        o(0x00e6_e8ea),
    ),
    t(
        "--c-24272a",
        ToneRole::Surface,
        o(0x0024_272a),
        o(0x00e7_e9eb),
    ),
    t(
        "--c-25282b",
        ToneRole::Surface,
        o(0x0025_282b),
        o(0x00e8_eaec),
    ),
    t(
        "--c-25314a",
        ToneRole::Surface,
        o(0x0025_314a),
        o(0x00f4_f5f6),
    ),
    t(
        "--c-27292b",
        ToneRole::Surface,
        o(0x0027_292b),
        o(0x00e9_ebec),
    ),
    t(
        "--c-272a2d",
        ToneRole::Surface,
        o(0x0027_2a2d),
        o(0x00ea_ebed),
    ),
    t(
        "--c-2a2d30",
        ToneRole::Surface,
        o(0x002a_2d30),
        o(0x00ec_eeef),
    ),
    t(
        "--c-2a2d31",
        ToneRole::Surface,
        o(0x002a_2d31),
        o(0x00ed_eef0),
    ),
    t(
        "--c-2a2e31",
        ToneRole::Surface,
        o(0x002a_2e31),
        o(0x00ed_eef0),
    ),
    t(
        "--c-443087",
        ToneRole::Surface,
        o(0x0044_3087),
        o(0x00d2_cdec),
    ),
    t(
        "--c-292c2f",
        ToneRole::Hover,
        o(0x0029_2c2f),
        o(0x00cd_d2d8),
    ),
    t(
        "--c-303438",
        ToneRole::Hover,
        o(0x0030_3438),
        o(0x00c9_ced4),
    ),
    t(
        "--c-383c40",
        ToneRole::Hover,
        o(0x0038_3c40),
        o(0x00c0_c6cd),
    ),
    t(
        "--c-3b3f43",
        ToneRole::Hover,
        o(0x003b_3f43),
        o(0x00bc_c2c9),
    ),
    t(
        "--c-7f878e26",
        ToneRole::Hover,
        a(0x007f_878e, 0x26),
        a(0x0000_0000, 0x26),
    ),
    t(
        "--c-ffffff1f",
        ToneRole::Hover,
        a(0x00ff_ffff, 0x1f),
        a(0x0000_0000, 0x1f),
    ),
    t("--c-569cd6", ToneRole::Ink, o(0x0056_9cd6), o(0x001d_5a94)),
    t("--c-8f969c", ToneRole::Ink, o(0x008f_969c), o(0x0052_5b64)),
    t("--c-949ca3", ToneRole::Ink, o(0x0094_9ca3), o(0x004e_5760)),
    t("--c-9ca3a9", ToneRole::Ink, o(0x009c_a3a9), o(0x004a_525a)),
    t("--c-9fa6ac", ToneRole::Ink, o(0x009f_a6ac), o(0x0048_5058)),
    t("--c-abb2b8", ToneRole::Ink, o(0x00ab_b2b8), o(0x0041_484f)),
    t("--c-aeb4ba", ToneRole::Ink, o(0x00ae_b4ba), o(0x003f_464d)),
    t("--c-aeb9cc", ToneRole::Ink, o(0x00ae_b9cc), o(0x003a_4148)),
    t("--c-c9cdd1", ToneRole::Ink, o(0x00c9_cdd1), o(0x0030_353a)),
    t("--c-ce9178", ToneRole::Ink, o(0x00ce_9178), o(0x009a_4a22)),
    t("--c-d2d6da", ToneRole::Ink, o(0x00d2_d6da), o(0x002a_2f34)),
    t("--c-d4d4d4", ToneRole::Ink, o(0x00d4_d4d4), o(0x002b_3035)),
    t("--c-d5d8da", ToneRole::Ink, o(0x00d5_d8da), o(0x0029_2e33)),
    t("--c-dfe2e4", ToneRole::Ink, o(0x00df_e2e4), o(0x0023_272b)),
    t("--c-e7eaed", ToneRole::Ink, o(0x00e7_eaed), o(0x001e_2125)),
    t("--c-f0f2f3", ToneRole::Ink, o(0x00f0_f2f3), o(0x0019_1c1f)),
    t("--c-ff9c9f", ToneRole::Ink, o(0x00ff_9c9f), o(0x00b4_2b33)),
    t("--strong", ToneRole::Ink, o(0x00ff_ffff), o(0x0018_1b1e)),
    t("--c-62686d", ToneRole::Dim, o(0x0062_686d), o(0x006b_737c)),
    t("--c-87919b", ToneRole::Mark, o(0x0087_919b), o(0x005f_6670)),
    t("--c-df703e", ToneRole::Mark, o(0x00df_703e), o(0x00b9_531f)),
    t(
        "--c-42474b",
        ToneRole::Track,
        o(0x0042_474b),
        o(0x007d_848c),
    ),
    t(
        "--c-44494e",
        ToneRole::Track,
        o(0x0044_494e),
        o(0x007d_848c),
    ),
    t(
        "--c-555a5f",
        ToneRole::Track,
        o(0x0055_5a5f),
        o(0x007d_848c),
    ),
    t("--c-000000", ToneRole::Keep, o(0x0000_0000), o(0x0000_0000)),
    t("--c-1abc9c", ToneRole::Keep, o(0x001a_bc9c), o(0x001a_bc9c)),
    t("--c-3daee9", ToneRole::Keep, o(0x003d_aee9), o(0x003d_aee9)),
    t("--c-54d56b", ToneRole::Keep, o(0x0054_d56b), o(0x0054_d56b)),
    t("--c-9b59b6", ToneRole::Keep, o(0x009b_59b6), o(0x009b_59b6)),
    t("--c-ef4048", ToneRole::Keep, o(0x00ef_4048), o(0x00ef_4048)),
    t("--c-f39c12", ToneRole::Keep, o(0x00f3_9c12), o(0x00f3_9c12)),
    t("--c-ffffff", ToneRole::Keep, o(0x00ff_ffff), o(0x00ff_ffff)),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{Palette, composite, contrast_ratio};

    const SHELL_CSS: &str = include_str!("../../../ui/shell/flamewm.css");
    const BODY_MIN: f64 = 4.5;
    const ICON_MIN: f64 = 3.0;
    const DIM_MIN: f64 = 2.5;
    const TRACK_MIN: f64 = 1.5;
    const HOVER_MIN: f64 = 1.15;

    /// CSS hex spelling of a tone: six digits, plus two alpha digits when not opaque.
    fn hex(color: AlphaColor) -> String {
        if color.alpha == 0xff {
            format!("{:06x}", color.color.0)
        } else {
            format!("{:06x}{:02x}", color.color.0, color.alpha)
        }
    }

    /// `(name, value)` for every declaration of the first `:root` block.
    fn root_declarations(css: &str) -> Vec<(&str, &str)> {
        let body = &css[css.find('{').expect("root block") + 1..];
        let body = &body[..body.find('}').expect("root block end")];
        body.split(';')
            .filter_map(|decl| decl.split_once(':'))
            .map(|(name, value)| (name.trim(), value.trim()))
            .collect()
    }

    /// Every `--c-<hex>` name used through `var(...)` anywhere in the sheet.
    fn used_c_tokens(css: &str) -> Vec<&str> {
        css.match_indices("var(--c-")
            .map(|(at, _)| &css[at + 4..])
            .map(|rest| &rest[..rest.find([')', ',']).expect("var close")])
            .collect()
    }

    #[test]
    fn every_css_tone_has_a_table_entry_in_both_themes() {
        let declared = root_declarations(SHELL_CSS);
        for (name, _) in declared.iter().filter(|(name, _)| name.starts_with("--c-")) {
            assert!(
                FIXED_TONES.iter().any(|tone| tone.name == *name),
                "{name} missing"
            );
        }
        for name in used_c_tokens(SHELL_CSS) {
            assert!(
                FIXED_TONES.iter().any(|tone| tone.name == name),
                "var({name}) missing"
            );
        }
        for tone in FIXED_TONES {
            assert!(
                declared.iter().any(|(name, _)| *name == tone.name),
                "{} not declared in :root",
                tone.name
            );
        }
    }

    #[test]
    fn dark_column_is_the_shipped_root_value_and_c_names_spell_it() {
        let declared = root_declarations(SHELL_CSS);
        for tone in FIXED_TONES {
            let (_, value) = declared
                .iter()
                .find(|(name, _)| *name == tone.name)
                .expect("declared");
            assert_eq!(
                value.trim_start_matches('#'),
                hex(tone.dark),
                "{} Dark",
                tone.name
            );
            if let Some(spelled) = tone.name.strip_prefix("--c-") {
                assert_eq!(spelled, hex(tone.dark), "{} name", tone.name);
            }
        }
    }

    #[test]
    fn names_are_unique() {
        for (i, tone) in FIXED_TONES.iter().enumerate() {
            assert!(
                FIXED_TONES[i + 1..]
                    .iter()
                    .all(|other| other.name != tone.name)
            );
        }
    }

    /// Colour a tone paints over `base`, after alpha compositing.
    fn painted(color: AlphaColor, base: Rgb) -> Rgb {
        composite(color, base)
    }

    /// Contrast of the tone's paint against each neutral base it can sit on.
    fn ratios(color: AlphaColor, bases: &[(&'static str, Rgb)]) -> Vec<(&'static str, f64)> {
        bases
            .iter()
            .map(|&(name, base)| (name, contrast_ratio(painted(color, base), base)))
            .collect()
    }

    fn check(tone: &FixedTone, appearance: Appearance) -> Vec<String> {
        let palette = Palette::for_appearance(appearance);
        let color = tone.for_appearance(appearance);
        let bases = [
            ("panel", palette.panel),
            ("surface", palette.surface),
            ("sidebar", palette.sidebar),
        ];
        let mut failures = Vec::new();
        let mut require = |ok: bool, what: String| {
            if !ok {
                failures.push(format!("{appearance:?} {} {what}", tone.name));
            }
        };
        let floor = |min: f64, ratios: Vec<(&str, f64)>, require: &mut dyn FnMut(bool, String)| {
            for (name, ratio) in ratios {
                require(ratio >= min, format!("on {name} {ratio:.2} < {min}"));
            }
        };
        match tone.role {
            ToneRole::Surface => {
                let bar = painted(color, palette.panel);
                let text = contrast_ratio(palette.text, bar);
                let muted = contrast_ratio(palette.muted, bar);
                require(
                    text >= BODY_MIN && muted >= BODY_MIN,
                    format!("text {text:.2} muted {muted:.2}"),
                );
            }
            ToneRole::Hover => {
                let text = contrast_ratio(palette.text, painted(color, palette.panel));
                require(text >= BODY_MIN, format!("text {text:.2}"));
                // Nav rows and titlebar buttons sit on the panel or sidebar base, not the surface.
                let bars: Vec<_> = bases
                    .iter()
                    .filter(|(name, _)| *name != "surface")
                    .copied()
                    .collect();
                floor(HOVER_MIN, ratios(color, &bars), &mut require);
            }
            ToneRole::Ink => floor(BODY_MIN, ratios(color, &bases), &mut require),
            ToneRole::Dim => floor(DIM_MIN, ratios(color, &bases), &mut require),
            ToneRole::Mark => floor(ICON_MIN, ratios(color, &bases), &mut require),
            ToneRole::Track => floor(TRACK_MIN, ratios(color, &bases), &mut require),
            ToneRole::Keep => require(tone.light == tone.dark, "differs across themes".into()),
        }
        failures
    }

    #[test]
    fn contrast_contract_holds_in_both_themes() {
        let failures: Vec<String> = [Appearance::Dark, Appearance::Light]
            .into_iter()
            .flat_map(|appearance| {
                FIXED_TONES
                    .iter()
                    .flat_map(move |tone| check(tone, appearance))
            })
            .collect();
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn hover_tones_stay_distinct_from_their_bar_in_both_themes() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let bar = Palette::for_appearance(appearance).panel;
            for tone in FIXED_TONES
                .iter()
                .filter(|tone| tone.role == ToneRole::Hover)
            {
                let ratio = contrast_ratio(painted(tone.for_appearance(appearance), bar), bar);
                assert!(
                    ratio >= HOVER_MIN,
                    "{appearance:?} {} {ratio:.2}",
                    tone.name
                );
            }
        }
    }
}
