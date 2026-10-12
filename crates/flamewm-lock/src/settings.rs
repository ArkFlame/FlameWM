use std::path::PathBuf;

use crate::clock::ClockFormat;

/// Lock screen look settings. Every field falls back to its default when the
/// key is absent or its value is invalid, so older settings files keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockSettings {
    pub background_color: [u8; 3],
    pub background_image: Option<PathBuf>,
    pub clock_format: ClockFormat,
    pub show_date: bool,
    pub show_username: bool,
    pub accent: String,
    pub font_family: String,
}

pub const DEFAULT_BACKGROUND: [u8; 3] = [0x23, 0x26, 0x29];
pub const DEFAULT_ACCENT: &str = "#3daee9";
pub const DEFAULT_FONT_FAMILY: &str = "Noto Sans";

impl Default for LockSettings {
    fn default() -> Self {
        Self {
            background_color: DEFAULT_BACKGROUND,
            background_image: None,
            clock_format: ClockFormat::H24,
            show_date: true,
            show_username: true,
            accent: DEFAULT_ACCENT.to_string(),
            font_family: DEFAULT_FONT_FAMILY.to_string(),
        }
    }
}

impl LockSettings {
    /// Builds settings from a key lookup; `lookup` returns the raw text value.
    pub fn from_lookup<'a>(lookup: impl Fn(&str) -> Option<&'a str>) -> Self {
        let defaults = Self::default();
        Self {
            background_color: lookup("lockBackgroundColor")
                .and_then(parse_hex_color)
                .unwrap_or(defaults.background_color),
            background_image: lookup("lockBackgroundImage")
                .map(str::trim)
                .filter(|path| path.starts_with('/'))
                .map(PathBuf::from),
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
        }
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
