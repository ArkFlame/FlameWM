use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Boolean(bool),
    Integer(i64),
    Percent(u8),
    Rgb(u32),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppearanceMode {
    System,
    Light,
    Dark,
}

impl Default for AppearanceMode {
    fn default() -> Self {
        Self::Dark
    }
}

impl AppearanceMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    #[must_use]
    pub const fn prefers_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSnapshot {
    pub revision: u64,
    pub values: BTreeMap<String, SettingValue>,
}

impl SettingsSnapshot {
    #[must_use]
    pub fn new(revision: u64) -> Self {
        Self {
            revision,
            values: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsChange {
    pub key: String,
    pub value: Option<SettingValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsTransaction {
    pub expected_revision: u64,
    pub changes: Vec<SettingsChange>,
    pub reset_section: Option<String>,
}

impl SettingsTransaction {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.reset_section.is_none()
    }
}

/// Longest `assistant.hermesUrl` the Settings field accepts, in characters.
pub const HERMES_URL_MAX_CHARS: usize = 2048;

/// Validates `assistant.hermesUrl`. Empty is valid and means auto-detect; otherwise the value must be
/// an http or https URL without whitespace. Returns the inline message, or `None` when valid.
#[must_use]
pub fn hermes_url_error(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return None;
    }
    let scheme_ok = value.starts_with("http://") || value.starts_with("https://");
    if scheme_ok && !value.contains(char::is_whitespace) && value.chars().count() <= HERMES_URL_MAX_CHARS {
        None
    } else {
        Some("Use an http or https URL without spaces, or leave empty to auto-detect")
    }
}

#[cfg(test)]
mod hermes_url_tests {
    use super::hermes_url_error;

    #[test]
    fn empty_and_http_urls_are_valid() {
        assert_eq!(hermes_url_error(""), None);
        assert_eq!(hermes_url_error("http://127.0.0.1:8642"), None);
        assert_eq!(hermes_url_error("https://hermes.local/api"), None);
    }

    #[test]
    fn other_schemes_spaces_and_overlong_values_are_invalid() {
        assert!(hermes_url_error("127.0.0.1:8642").is_some());
        assert!(hermes_url_error("ftp://host").is_some());
        assert!(hermes_url_error("http://host /x").is_some());
        assert!(hermes_url_error("http://host\t").is_some());
        let long = format!("http://{}", "a".repeat(super::HERMES_URL_MAX_CHARS));
        assert!(hermes_url_error(&long).is_some());
    }
}
