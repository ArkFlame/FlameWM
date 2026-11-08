//! Titlebar double-click action. The default keeps the historical behaviour (toggle maximize).
//! Configured by `FLAMEWM_TITLE_DOUBLE_CLICK` (`maximize` or `off`); unknown values fall back to
//! the default so a typo never disables the titlebar.

use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleDoubleClick {
    /// Toggle maximize (default, the historical behaviour).
    #[default]
    Maximize,
    /// A double-click does nothing; single clicks still focus and drag.
    Off,
}

impl TitleDoubleClick {
    /// Parses a configured value, ignoring case and surrounding whitespace.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "maximize" => Some(Self::Maximize),
            "off" | "none" => Some(Self::Off),
            _ => None,
        }
    }

    /// Reads `FLAMEWM_TITLE_DOUBLE_CLICK`; unset or invalid values give the default.
    #[must_use]
    pub fn from_env() -> Self {
        env::var("FLAMEWM_TITLE_DOUBLE_CLICK")
            .ok()
            .and_then(|value| Self::parse(&value))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::TitleDoubleClick;

    #[test]
    fn parses_known_values_case_insensitively() {
        assert_eq!(
            TitleDoubleClick::parse("maximize"),
            Some(TitleDoubleClick::Maximize)
        );
        assert_eq!(
            TitleDoubleClick::parse(" MAXIMIZE "),
            Some(TitleDoubleClick::Maximize)
        );
        assert_eq!(TitleDoubleClick::parse("off"), Some(TitleDoubleClick::Off));
        assert_eq!(TitleDoubleClick::parse("None"), Some(TitleDoubleClick::Off));
    }

    #[test]
    fn unknown_values_are_rejected_and_default_is_maximize() {
        assert_eq!(TitleDoubleClick::parse("shade"), None);
        assert_eq!(TitleDoubleClick::parse(""), None);
        assert_eq!(TitleDoubleClick::default(), TitleDoubleClick::Maximize);
    }
}
