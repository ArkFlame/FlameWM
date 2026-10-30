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

/// Longest `assistant.apiKey` the Settings field accepts, in characters.
pub const ASSISTANT_API_KEY_MAX_CHARS: usize = 200;

/// Validates `assistant.apiKey`. Empty is valid and means the built-in default key; otherwise the
/// value must start with `ark_`, contain no whitespace or control characters, and be at most
/// `ASSISTANT_API_KEY_MAX_CHARS` long. Returns the inline message, or `None` when valid.
#[must_use]
pub fn assistant_api_key_error(value: &str) -> Option<&'static str> {
    if value.is_empty() {
        return None;
    }
    let shape_ok = value.starts_with("ark_")
        && !value.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        && value.chars().count() <= ASSISTANT_API_KEY_MAX_CHARS;
    if shape_ok {
        None
    } else {
        Some("Use an ARK AI key: starts with ark_, no spaces, up to 200 characters")
    }
}

#[cfg(test)]
mod assistant_api_key_tests {
    use super::{assistant_api_key_error, ASSISTANT_API_KEY_MAX_CHARS};

    #[test]
    fn empty_and_ark_keys_are_valid() {
        assert_eq!(assistant_api_key_error(""), None);
        assert_eq!(assistant_api_key_error("ark_23bRexample"), None);
        let longest = format!("ark_{}", "a".repeat(ASSISTANT_API_KEY_MAX_CHARS - 4));
        assert_eq!(assistant_api_key_error(&longest), None);
    }

    #[test]
    fn wrong_prefix_spaces_controls_and_overlong_values_are_invalid() {
        assert!(assistant_api_key_error("sk_23bRexample").is_some());
        assert!(assistant_api_key_error("ARK_23bR").is_some());
        assert!(assistant_api_key_error("ark_23 bR").is_some());
        assert!(assistant_api_key_error("ark_23bR\t").is_some());
        assert!(assistant_api_key_error("ark_23bR\u{7}").is_some());
        let long = format!("ark_{}", "a".repeat(ASSISTANT_API_KEY_MAX_CHARS - 3));
        assert!(assistant_api_key_error(&long).is_some());
    }
}
