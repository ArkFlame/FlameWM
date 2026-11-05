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
    FixedTone { name, role, dark, light }
}

const fn o(rgb: u32) -> AlphaColor {
    AlphaColor { color: Rgb(rgb), alpha: 0xff }
}

const fn a(rgb: u32, alpha: u8) -> AlphaColor {
    AlphaColor { color: Rgb(rgb), alpha }
}

/// Light `Surface` tones are a ramp from `#c4c9ce` (darkest Dark well) to white, ordered like the
/// Dark tones they replace. `Ink` runs from `#181b1e` (brightest Dark text) to `#566069`.
pub const FIXED_TONES: &[FixedTone] = &[
    t("--c-00000080", ToneRole::Surface, a(0x000000, 0x80), a(0x000000, 0x33)),
    t("--c-050607", ToneRole::Surface, o(0x050607), o(0xc9ced2)),
    t("--c-0c0d0e", ToneRole::Surface, o(0x0c0d0e), o(0xd0d4d8)),
    t("--c-111315", ToneRole::Surface, o(0x111315), o(0xd5d9dc)),
    t("--c-151719", ToneRole::Surface, o(0x151719), o(0xd9dcdf)),
    t("--c-17191b", ToneRole::Surface, o(0x17191b), o(0xdadde1)),
    t("--c-181a1c", ToneRole::Surface, o(0x181a1c), o(0xdbdee1)),
    t("--c-191b1d", ToneRole::Surface, o(0x191b1d), o(0xdcdfe2)),
    t("--c-1c2740", ToneRole::Surface, o(0x1c2740), o(0xebedee)),
    t("--c-1e1e1e", ToneRole::Surface, o(0x1e1e1e), o(0xdfe2e4)),
    t("--c-202224", ToneRole::Surface, o(0x202224), o(0xe2e5e7)),
    t("--c-232629", ToneRole::Surface, o(0x232629), o(0xe6e8ea)),
    t("--c-24272a", ToneRole::Surface, o(0x24272a), o(0xe7e9eb)),
    t("--c-25282b", ToneRole::Surface, o(0x25282b), o(0xe8eaec)),
    t("--c-25314a", ToneRole::Surface, o(0x25314a), o(0xf4f5f6)),
    t("--c-27292b", ToneRole::Surface, o(0x27292b), o(0xe9ebec)),
    t("--c-272a2d", ToneRole::Surface, o(0x272a2d), o(0xeaebed)),
    t("--c-2a2d30", ToneRole::Surface, o(0x2a2d30), o(0xeceeef)),
    t("--c-2a2d31", ToneRole::Surface, o(0x2a2d31), o(0xedeef0)),
    t("--c-2a2e31", ToneRole::Surface, o(0x2a2e31), o(0xedeef0)),
    t("--c-443087", ToneRole::Surface, o(0x443087), o(0xd2cdec)),
    t("--c-292c2f", ToneRole::Hover, o(0x292c2f), o(0xcdd2d8)),
    t("--c-303438", ToneRole::Hover, o(0x303438), o(0xc9ced4)),
    t("--c-383c40", ToneRole::Hover, o(0x383c40), o(0xc0c6cd)),
    t("--c-3b3f43", ToneRole::Hover, o(0x3b3f43), o(0xbcc2c9)),
    t("--c-7f878e26", ToneRole::Hover, a(0x7f878e, 0x26), a(0x000000, 0x26)),
    t("--c-569cd6", ToneRole::Ink, o(0x569cd6), o(0x1d5a94)),
    t("--c-8f969c", ToneRole::Ink, o(0x8f969c), o(0x525b64)),
    t("--c-949ca3", ToneRole::Ink, o(0x949ca3), o(0x4e5760)),
    t("--c-9ca3a9", ToneRole::Ink, o(0x9ca3a9), o(0x4a525a)),
    t("--c-9fa6ac", ToneRole::Ink, o(0x9fa6ac), o(0x485058)),
    t("--c-abb2b8", ToneRole::Ink, o(0xabb2b8), o(0x41484f)),
    t("--c-aeb4ba", ToneRole::Ink, o(0xaeb4ba), o(0x3f464d)),
    t("--c-aeb9cc", ToneRole::Ink, o(0xaeb9cc), o(0x3a4148)),
    t("--c-c9cdd1", ToneRole::Ink, o(0xc9cdd1), o(0x30353a)),
    t("--c-ce9178", ToneRole::Ink, o(0xce9178), o(0x9a4a22)),
    t("--c-d2d6da", ToneRole::Ink, o(0xd2d6da), o(0x2a2f34)),
    t("--c-d4d4d4", ToneRole::Ink, o(0xd4d4d4), o(0x2b3035)),
    t("--c-d5d8da", ToneRole::Ink, o(0xd5d8da), o(0x292e33)),
    t("--c-dfe2e4", ToneRole::Ink, o(0xdfe2e4), o(0x23272b)),
    t("--c-e7eaed", ToneRole::Ink, o(0xe7eaed), o(0x1e2125)),
    t("--c-f0f2f3", ToneRole::Ink, o(0xf0f2f3), o(0x191c1f)),
    t("--c-ff9c9f", ToneRole::Ink, o(0xff9c9f), o(0xb42b33)),
    t("--strong", ToneRole::Ink, o(0xffffff), o(0x181b1e)),
    t("--c-62686d", ToneRole::Dim, o(0x62686d), o(0x6b737c)),
    t("--c-87919b", ToneRole::Mark, o(0x87919b), o(0x5f6670)),
    t("--c-df703e", ToneRole::Mark, o(0xdf703e), o(0xb9531f)),
    t("--c-42474b", ToneRole::Track, o(0x42474b), o(0x7d848c)),
    t("--c-44494e", ToneRole::Track, o(0x44494e), o(0x7d848c)),
    t("--c-555a5f", ToneRole::Track, o(0x555a5f), o(0x7d848c)),
    t("--c-000000", ToneRole::Keep, o(0x000000), o(0x000000)),
    t("--c-1abc9c", ToneRole::Keep, o(0x1abc9c), o(0x1abc9c)),
    t("--c-3daee9", ToneRole::Keep, o(0x3daee9), o(0x3daee9)),
    t("--c-54d56b", ToneRole::Keep, o(0x54d56b), o(0x54d56b)),
    t("--c-9b59b6", ToneRole::Keep, o(0x9b59b6), o(0x9b59b6)),
    t("--c-ef4048", ToneRole::Keep, o(0xef4048), o(0xef4048)),
    t("--c-f39c12", ToneRole::Keep, o(0xf39c12), o(0xf39c12)),
    t("--c-ffffff", ToneRole::Keep, o(0xffffff), o(0xffffff)),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{composite, contrast_ratio, Palette};

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
            assert!(FIXED_TONES.iter().any(|tone| tone.name == *name), "{name} missing");
        }
        for name in used_c_tokens(SHELL_CSS) {
            assert!(FIXED_TONES.iter().any(|tone| tone.name == name), "var({name}) missing");
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
            assert_eq!(value.trim_start_matches('#'), hex(tone.dark), "{} Dark", tone.name);
            if let Some(spelled) = tone.name.strip_prefix("--c-") {
                assert_eq!(spelled, hex(tone.dark), "{} name", tone.name);
            }
        }
    }

    #[test]
    fn names_are_unique() {
        for (i, tone) in FIXED_TONES.iter().enumerate() {
            assert!(FIXED_TONES[i + 1..].iter().all(|other| other.name != tone.name));
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
                let bars: Vec<_> = bases.iter().filter(|(name, _)| *name != "surface").copied().collect();
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
            .flat_map(|appearance| FIXED_TONES.iter().flat_map(move |tone| check(tone, appearance)))
            .collect();
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn hover_tones_stay_distinct_from_their_bar_in_both_themes() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            let bar = Palette::for_appearance(appearance).panel;
            for tone in FIXED_TONES.iter().filter(|tone| tone.role == ToneRole::Hover) {
                let ratio = contrast_ratio(painted(tone.for_appearance(appearance), bar), bar);
                assert!(ratio >= HOVER_MIN, "{appearance:?} {} {ratio:.2}", tone.name);
            }
        }
    }
}
