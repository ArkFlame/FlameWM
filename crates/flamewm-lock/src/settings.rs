use std::path::PathBuf;

use crate::clock::ClockFormat;

/// Lock screen look settings. Every field falls back to its default when the
/// key is absent or its value is invalid, so older settings files keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockSettings {
    pub background_color: [u8; 3],
    /// Saved custom image (`lockBackgroundImage`); decoded only in `LockBackgroundMode::Custom`.
    pub background_image: Option<PathBuf>,
    pub background_mode: LockBackgroundMode,
    /// Avatar picture inside the avatar ring (`avatarImage`); `None` shows the default icon.
    pub avatar_image: Option<PathBuf>,
    pub clock_format: ClockFormat,
    pub show_date: bool,
    pub show_username: bool,
    pub accent: String,
    pub font_family: String,
    pub saver: SaverConfig,
    /// Background blur, 0..=100 (`lockBackgroundBlurPercent`).
    pub blur_percent: u8,
    /// Background darkening, 0..=80 (`lockBackgroundDimPercent`).
    pub dim_percent: u8,
}

/// Idle policy after the lock is shown (`screensaver*` keys). Minutes of `0` mean never.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaverConfig {
    pub mode: SaverMode,
    pub after_lock_minutes: u32,
    pub num_ribbons: usize,
    /// Ribbon width in tenths of a pixel (px = tenths / 10).
    pub ribbon_width_tenths: u32,
    /// Parsed and tolerated, but inert: Ribbons always spans the whole root window
    /// (see `ribbons.rs`), so this value does not change what is rendered.
    pub span_monitors: bool,
    pub display_off_minutes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaverMode {
    Ribbons,
    /// No animation: the display is turned off after the idle timer.
    None,
}

impl SaverConfig {
    pub fn ribbons(&self) -> bool {
        self.mode == SaverMode::Ribbons
    }
}

impl Default for SaverConfig {
    fn default() -> Self {
        Self {
            mode: SaverMode::Ribbons,
            after_lock_minutes: 1,
            num_ribbons: 6,
            ribbon_width_tenths: 110,
            span_monitors: false,
            display_off_minutes: 20,
        }
    }
}

pub const DEFAULT_BACKGROUND: [u8; 3] = [0x23, 0x26, 0x29];
pub const DEFAULT_ACCENT: &str = "#3daee9";
pub const DEFAULT_FONT_FAMILY: &str = "Noto Sans";

impl Default for LockSettings {
    fn default() -> Self {
        Self {
            background_color: DEFAULT_BACKGROUND,
            background_image: None,
            background_mode: LockBackgroundMode::Default,
            avatar_image: None,
            clock_format: ClockFormat::H24,
            show_date: true,
            show_username: true,
            accent: DEFAULT_ACCENT.to_string(),
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            saver: SaverConfig::default(),
            blur_percent: DEFAULT_BLUR_PERCENT,
            dim_percent: DEFAULT_DIM_PERCENT,
        }
    }
}

impl LockSettings {
    /// Builds settings from a key lookup; `lookup` returns the raw text value.
    pub fn from_lookup<'a>(lookup: impl Fn(&str) -> Option<&'a str>) -> Self {
        let defaults = Self::default();
        let background_color = lookup("lockBackgroundColor")
            .and_then(parse_hex_color)
            .unwrap_or(defaults.background_color);
        let background_image = absolute_path(&lookup, "lockBackgroundImage");
        // Files written before `lockBackgroundMode` existed derive the mode from the saved values.
        let background_mode = lookup("lockBackgroundMode")
            .and_then(LockBackgroundMode::parse)
            .unwrap_or_else(|| {
                if background_image.is_some() {
                    LockBackgroundMode::Custom
                } else if background_color != DEFAULT_BACKGROUND {
                    LockBackgroundMode::Solid
                } else {
                    LockBackgroundMode::Default
                }
            });
        Self {
            background_color,
            background_image,
            background_mode,
            avatar_image: absolute_path(&lookup, "avatarImage"),
            clock_format: lookup("lockClockFormat")
                .and_then(ClockFormat::parse)
                .unwrap_or(defaults.clock_format),
            show_date: lookup("lockShowDate")
                .and_then(parse_bool)
                .unwrap_or(defaults.show_date),
            show_username: lookup("lockShowUsername")
                .and_then(parse_bool)
                .unwrap_or(defaults.show_username),
            accent: lookup("accent")
                .filter(|value| parse_hex_color(value).is_some())
                .map_or(defaults.accent, str::to_string),
            font_family: lookup("fontFamily")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map_or(defaults.font_family, str::to_string),
            saver: saver_from_lookup(&lookup),
            blur_percent: percent_from_lookup(&lookup, "lockBackgroundBlurPercent", 100, DEFAULT_BLUR_PERCENT),
            dim_percent: percent_from_lookup(&lookup, "lockBackgroundDimPercent", 80, DEFAULT_DIM_PERCENT),
        }
    }
}

/// Lock background source (`lockBackgroundMode`): the bundled image, the plain colour, or the
/// saved custom image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockBackgroundMode {
    Default,
    Solid,
    Custom,
}

impl LockBackgroundMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "solid" => Some(Self::Solid),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

pub const DEFAULT_BLUR_PERCENT: u8 = 40;
pub const DEFAULT_DIM_PERCENT: u8 = 25;

/// Integer percentage clamped to `0..=max`; missing or unparsable values give `default`.
fn percent_from_lookup<'a>(lookup: &impl Fn(&str) -> Option<&'a str>, key: &str, max: u8, default: u8) -> u8 {
    lookup(key)
        .and_then(|text| text.trim().parse::<i64>().ok())
        .map_or(default, |value| value.clamp(0, i64::from(max)) as u8)
}

/// Trimmed `key` value when it is an absolute path; relative or missing values give `None`.
fn absolute_path<'a>(lookup: &impl Fn(&str) -> Option<&'a str>, key: &str) -> Option<PathBuf> {
    lookup(key)
        .map(str::trim)
        .filter(|path| path.starts_with('/'))
        .map(PathBuf::from)
}

fn saver_from_lookup<'a>(lookup: &impl Fn(&str) -> Option<&'a str>) -> SaverConfig {
    let defaults = SaverConfig::default();
    let number = |key: &str| lookup(key).and_then(|text| text.trim().parse::<u32>().ok());
    SaverConfig {
        mode: match lookup("screensaverMode").map(str::trim) {
            Some("none") => SaverMode::None,
            Some("ribbons") => SaverMode::Ribbons,
            _ => defaults.mode,
        },
        after_lock_minutes: number("screensaverAfterLockMinutes").unwrap_or(defaults.after_lock_minutes),
        num_ribbons: number("screensaverNumRibbons").map_or(defaults.num_ribbons, |n| n as usize),
        ribbon_width_tenths: number("screensaverRibbonWidthTenths")
            .unwrap_or(defaults.ribbon_width_tenths),
        span_monitors: lookup("screensaverSpanMonitors")
            .and_then(parse_bool)
            .unwrap_or(defaults.span_monitors),
        display_off_minutes: number("screensaverDisplayOffMinutes")
            .unwrap_or(defaults.display_off_minutes),
    }
}

/// Parses `#RRGGBB` (case-insensitive) into RGB bytes.
pub fn parse_hex_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    Some([value(0)?, value(2)?, value(4)?])
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<&'static str> {
        move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    #[test]
    fn empty_lookup_yields_defaults() {
        let settings = LockSettings::from_lookup(|_| None);
        assert_eq!(settings, LockSettings::default());
        assert_eq!(settings.background_color, [0x23, 0x26, 0x29]);
        assert_eq!(settings.clock_format, ClockFormat::H24);
        assert!(settings.show_date && settings.show_username);
        assert_eq!(settings.background_image, None);
        assert_eq!(settings.avatar_image, None);
        assert_eq!(settings.saver, SaverConfig::default());
    }

    #[test]
    fn screensaver_keys_parse_and_zero_means_never() {
        static PAIRS: &[(&str, &str)] = &[
            ("screensaverMode", "none"),
            ("screensaverAfterLockMinutes", "0"),
            ("screensaverNumRibbons", "9"),
            ("screensaverRibbonWidthTenths", "250"),
            ("screensaverSpanMonitors", "true"),
            ("screensaverDisplayOffMinutes", "0"),
        ];
        let saver = LockSettings::from_lookup(lookup_from(PAIRS)).saver;
        assert_eq!(saver.mode, SaverMode::None);
        assert_eq!(saver.after_lock_minutes, 0);
        assert_eq!(saver.num_ribbons, 9);
        assert_eq!(saver.ribbon_width_tenths, 250);
        assert!(saver.span_monitors);
        assert_eq!(saver.display_off_minutes, 0);
        assert!(!saver.ribbons());
    }

    #[test]
    fn background_percentages_parse_clamp_and_default() {
        let defaults = LockSettings::default();
        assert_eq!((defaults.blur_percent, defaults.dim_percent), (40, 25));
        static PAIRS: &[(&str, &str)] = &[("lockBackgroundBlurPercent", "250"), ("lockBackgroundDimPercent", "-4")];
        let clamped = LockSettings::from_lookup(lookup_from(PAIRS));
        assert_eq!((clamped.blur_percent, clamped.dim_percent), (100, 0));
        static DIM: &[(&str, &str)] = &[("lockBackgroundDimPercent", "99")];
        assert_eq!(LockSettings::from_lookup(lookup_from(DIM)).dim_percent, 80);
        static JUNK: &[(&str, &str)] = &[("lockBackgroundBlurPercent", "soft")];
        assert_eq!(LockSettings::from_lookup(lookup_from(JUNK)).blur_percent, 40);
    }

    #[test]
    fn avatar_image_must_be_an_absolute_path() {
        static ABS: &[(&str, &str)] = &[("avatarImage", " /home/u/face.png ")];
        assert_eq!(
            LockSettings::from_lookup(lookup_from(ABS)).avatar_image,
            Some(PathBuf::from("/home/u/face.png"))
        );
        static REL: &[(&str, &str)] = &[("avatarImage", "face.png")];
        assert_eq!(LockSettings::from_lookup(lookup_from(REL)).avatar_image, None);
    }

    #[test]
    fn invalid_screensaver_values_fall_back_to_defaults() {
        static PAIRS: &[(&str, &str)] = &[
            ("screensaverMode", "fireworks"),
            ("screensaverAfterLockMinutes", "-1"),
            ("screensaverDisplayOffMinutes", "soon"),
        ];
        let saver = LockSettings::from_lookup(lookup_from(PAIRS)).saver;
        assert_eq!(saver, SaverConfig::default());
    }

    #[test]
    fn background_mode_migrates_when_absent_and_explicit_mode_wins() {
        let custom = LockSettings::from_lookup(lookup_from(&[("lockBackgroundImage", "/a.png")]));
        assert_eq!(custom.background_mode, LockBackgroundMode::Custom);
        let solid = LockSettings::from_lookup(lookup_from(&[("lockBackgroundColor", "#102030")]));
        assert_eq!(solid.background_mode, LockBackgroundMode::Solid);
        let default = LockSettings::from_lookup(lookup_from(&[]));
        assert_eq!(default.background_mode, LockBackgroundMode::Default);
        let explicit = LockSettings::from_lookup(lookup_from(&[
            ("lockBackgroundImage", "/a.png"),
            ("lockBackgroundMode", "default"),
        ]));
        assert_eq!(explicit.background_mode, LockBackgroundMode::Default);
        assert_eq!(explicit.background_image, Some(PathBuf::from("/a.png")));
    }

    #[test]
    fn valid_values_are_applied() {
        static PAIRS: &[(&str, &str)] = &[
            ("lockBackgroundColor", "#102030"),
            ("lockBackgroundImage", "/usr/share/wallpapers/a.png"),
            ("lockClockFormat", "12h"),
            ("lockShowDate", "false"),
            ("lockShowUsername", "false"),
            ("accent", "#AABBCC"),
            ("fontFamily", " Inter "),
        ];
        let settings = LockSettings::from_lookup(lookup_from(PAIRS));
        assert_eq!(settings.background_color, [0x10, 0x20, 0x30]);
        assert_eq!(
            settings.background_image,
            Some(PathBuf::from("/usr/share/wallpapers/a.png"))
        );
        assert_eq!(settings.clock_format, ClockFormat::H12);
        assert!(!settings.show_date && !settings.show_username);
        assert_eq!(settings.accent, "#AABBCC");
        assert_eq!(settings.font_family, "Inter");
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        static PAIRS: &[(&str, &str)] = &[
            ("lockBackgroundColor", "232629"),
            ("lockBackgroundImage", "relative/a.png"),
            ("lockClockFormat", "25h"),
            ("lockShowDate", "yes"),
            ("accent", "blue"),
            ("fontFamily", "   "),
        ];
        let settings = LockSettings::from_lookup(lookup_from(PAIRS));
        assert_eq!(settings, LockSettings::default());
    }

    #[test]
    fn hex_color_requires_hash_and_six_digits() {
        assert_eq!(parse_hex_color("#00ff7F"), Some([0, 255, 127]));
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color("#gg0000"), None);
        assert_eq!(parse_hex_color("#1234567"), None);
    }
}
