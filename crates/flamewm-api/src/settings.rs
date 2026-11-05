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

/// Product default accent shared by every settings consumer. Must equal the picker's blue swatch
/// (`accent_for_swatch("blue")` in flamewm-settings); a test there enforces it.
pub const DEFAULT_ACCENT: &str = "#3DAEE9";
pub const DEFAULT_ACCENT_RGB: u32 = 0x003d_aee9;

/// Resolve the shared `accent` setting to opaque `0x00RRGGBB`.
///
/// Settings normally stores a six-digit hex colour. The named values are retained for snapshots
/// written by older versions; all presentation and WM consumers use this resolver so fallback,
/// aliases and accepted hex forms cannot drift.
#[must_use]
pub fn accent_rgb(snapshot: &SettingsSnapshot) -> u32 {
    let Some(SettingValue::Text(value)) = snapshot.values.get("accent") else {
        return DEFAULT_ACCENT_RGB;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "red" => return 0x00ef_4048,
        "blue" => return 0x003d_aee9,
        "purple" => return 0x009b_59b6,
        "teal" => return 0x001a_bc9c,
        "green" => return 0x0027_ae60,
        "amber" => return 0x00f3_9c12,
        _ => {}
    }
    let hex = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).unwrap_or(DEFAULT_ACCENT_RGB)
    } else {
        DEFAULT_ACCENT_RGB
    }
}

#[cfg(test)]
mod accent_tests {
    use super::{DEFAULT_ACCENT_RGB, SettingValue, SettingsSnapshot, accent_rgb};

    fn snapshot(value: &str) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("accent".into(), SettingValue::Text(value.into()));
        snapshot
    }

    #[test]
    fn accent_uses_one_default_and_accepts_hash_bare_hex_and_case() {
        assert_eq!(accent_rgb(&SettingsSnapshot::new(0)), DEFAULT_ACCENT_RGB);
        assert_eq!(accent_rgb(&snapshot("#12aBcD")), 0x0012_abcd);
        assert_eq!(accent_rgb(&snapshot("123456")), 0x0012_3456);
        assert_eq!(accent_rgb(&snapshot(" #EF4048 ")), 0x00ef_4048);
    }

    #[test]
    fn accent_keeps_legacy_names_and_invalid_values_fall_back() {
        assert_eq!(accent_rgb(&snapshot("red")), 0x00ef_4048);
        assert_eq!(accent_rgb(&snapshot("blue")), 0x003d_aee9);
        assert_eq!(accent_rgb(&snapshot("nonsense")), DEFAULT_ACCENT_RGB);
        assert_eq!(accent_rgb(&snapshot("#GG0000")), DEFAULT_ACCENT_RGB);
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

/// Keyboard layouts `keyboardLayout` accepts: xkb layout names as `xkbcommon` spells them, one
/// Settings segment each. The list is the single source for the platform validation and the
/// compositor's keymap, so a saved value is always one the seat can compile.
///
/// A fixed list rather than free text: a layout name that xkb does not ship would make the seat
/// compile an empty keymap and leave the session without a keyboard.
pub const KEYBOARD_LAYOUTS: &[&str] = &[
    "us", "gb", "de", "fr", "es", "it", "pt", "br", "nl", "be", "ch", "at", "se", "no", "dk", "fi",
    "pl", "cz", "hu", "ro", "ru", "ua", "tr", "gr", "ca", "jp", "kr",
];

/// The layouts Settings offers as one-click segments.
///
/// A strict subset of [`KEYBOARD_LAYOUTS`]: the accepted set stays open so a saved profile from a
/// fuller keyboard picker still validates, while the panel shows the common ones instead of 27
/// buttons.
pub const KEYBOARD_LAYOUT_CHOICES: &[&str] = &["us", "gb", "de", "fr", "es", "it", "pt", "br"];

/// Layout used when `keyboardLayout` is unset, and the fallback for a value outside
/// [`KEYBOARD_LAYOUTS`].
pub const DEFAULT_KEYBOARD_LAYOUT: &str = KEYBOARD_LAYOUTS[0];

/// Inclusive bounds of `pointerSpeed`, in percent of libinput's normalized acceleration range.
///
/// The compositor maps the value to libinput with `speed = percent / 100.0`, so [`POINTER_SPEED_MIN`]
/// is libinput's slowest setting, 0 is the device default and [`POINTER_SPEED_MAX`] its fastest.
pub const POINTER_SPEED_MIN: i8 = -100;

/// Inclusive upper bound of `pointerSpeed`; see [`POINTER_SPEED_MIN`].
pub const POINTER_SPEED_MAX: i8 = 100;

/// The `pointerSpeed` values Settings offers as one-click segments.
///
/// A subset of the accepted range, like [`KEYBOARD_LAYOUT_CHOICES`]: five steps are coarse but they
/// are exactly the granularity a user can tell apart, and libinput itself snaps a requested speed to
/// the closest step it implements. Any value inside [`POINTER_SPEED_MIN`]`..=`[`POINTER_SPEED_MAX`]
/// still validates, so a hand-edited profile is never rejected for being finer than the panel.
pub const POINTER_SPEED_CHOICES: [i64; 5] = [-100, -50, 0, 50, 100];

/// Node id of one `pointerSpeed` segment, the single source for the markup and its projection.
#[must_use]
pub fn pointer_speed_node(value: i64) -> String {
    if value < 0 {
        format!("pointer-speed-minus-{}", value.unsigned_abs())
    } else {
        format!("pointer-speed-{value}")
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
        && !value
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        && value.chars().count() <= ASSISTANT_API_KEY_MAX_CHARS;
    if shape_ok {
        None
    } else {
        Some("Use an ARK AI key: starts with ark_, no spaces, up to 200 characters")
    }
}

#[cfg(test)]
mod assistant_api_key_tests {
    use super::{ASSISTANT_API_KEY_MAX_CHARS, assistant_api_key_error};

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
