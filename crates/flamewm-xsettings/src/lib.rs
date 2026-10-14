//! FlameWM XSettings producer owned by the window-manager process.
//!
//! Owns the `_XSETTINGS_S0` manager selection only when no owner exists yet.
//! Refuses to replace an existing owner. The pure parts (settings table, blob
//! encoding, serial bookkeeping) live in [`blob`]; the claim sequence runs over
//! the [`manager::ManagerPort`] trait, implemented for real X11 in [`x11`].
//! [`SelectionOwner`] remains as the read-only ownership probe port.

use std::collections::BTreeMap;

use flamewm_api::settings::AppearanceMode;
use flamewm_api::{FlameError, FlameResult};

pub const XSETTINGS_SELECTION: &str = "_XSETTINGS_S0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XSettingsValue {
    Integer(i32),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XSettingsSnapshot {
    pub serial: u32,
    pub settings: BTreeMap<String, XSettingsValue>,
}

impl XSettingsSnapshot {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<XSettingsValue> {
        self.settings.get(name).cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XSettingsInput {
    pub appearance: AppearanceMode,
    pub icon_theme: String,
    pub cursor_theme: String,
    pub cursor_size: u32,
    pub font_family: String,
    /// Whether GTK's configured font should use the standard bold face.
    pub font_bold: bool,
    pub font_size_points: u32,
    pub dpi_x1024: u32,
    pub antialias: bool,
    pub hinting: bool,
    pub hint_style: String,
    pub rgba_order: String,
    /// Installed GTK3 theme name for `Net/ThemeName`; `None` keeps `theme_name_for`.
    pub gtk3_theme: Option<String>,
}

impl Default for XSettingsInput {
    fn default() -> Self {
        Self {
            appearance: AppearanceMode::Dark,
            icon_theme: "breeze-dark".to_owned(),
            // Must match the session env default (FlameWM-Breeze-Dark); "Breeze" is not bundled.
            cursor_theme: "FlameWM-Breeze-Dark".to_owned(),
            cursor_size: 24,
            font_family: "IBM Plex Sans".to_owned(),
            font_bold: false,
            font_size_points: 10,
            dpi_x1024: 96 * 1024,
            antialias: true,
            hinting: true,
            hint_style: "hintslight".to_owned(),
            rgba_order: "rgb".to_owned(),
            gtk3_theme: None,
        }
    }
}

/// Appearance policy for published XSettings.
///
/// Only an explicit `Dark` publishes the dark theme (`Flame-Dark`,
/// `Gtk/PreferDarkTheme` = 1). `Light` publishes the light theme. `System` has
/// no OS appearance query on this path, so it is published as `Light`; it is
/// deliberately not resolved to a real system preference here. A future
/// resolver must replace this function rather than patch its callers.
#[must_use]
pub fn published_appearance(mode: AppearanceMode) -> AppearanceMode {
    match mode {
        AppearanceMode::Dark => AppearanceMode::Dark,
        AppearanceMode::Light | AppearanceMode::System => AppearanceMode::Light,
    }
}

#[must_use]
pub fn theme_name_for(appearance: AppearanceMode) -> &'static str {
    if published_appearance(appearance).prefers_dark() {
        "Flame-Dark"
    } else {
        "Flame-Light"
    }
}

fn gtk3_theme_for(input: &XSettingsInput) -> &str {
    input
        .gtk3_theme
        .as_deref()
        .unwrap_or_else(|| theme_name_for(input.appearance))
}

#[must_use]
pub fn build_settings(input: &XSettingsInput) -> XSettingsSnapshot {
    let dark = published_appearance(input.appearance).prefers_dark();
    let mut settings = BTreeMap::new();
    settings.insert(
        "Net/ThemeName".to_owned(),
        XSettingsValue::Text(gtk3_theme_for(input).to_owned()),
    );
    settings.insert(
        "Net/IconThemeName".to_owned(),
        XSettingsValue::Text(input.icon_theme.clone()),
    );
    settings.insert(
        "Xft/Antialias".to_owned(),
        XSettingsValue::Integer(i32::from(input.antialias)),
    );
    settings.insert(
        "Xft/Hinting".to_owned(),
        XSettingsValue::Integer(i32::from(input.hinting)),
    );
    settings.insert(
        "Xft/HintStyle".to_owned(),
        XSettingsValue::Text(input.hint_style.clone()),
    );
    settings.insert(
        "Xft/RGBA".to_owned(),
        XSettingsValue::Text(input.rgba_order.clone()),
    );
    settings.insert(
        "Xft/DPI".to_owned(),
        XSettingsValue::Integer(input.dpi_x1024 as i32),
    );
    settings.insert(
        "Gtk/PreferDarkTheme".to_owned(),
        XSettingsValue::Integer(i32::from(dark)),
    );
    // Spec key for GTK3 dark preference. Installed libgdk's XSETTINGS table does
    // not list it, so GTK3 dark selection also relies on Net/ThemeName.
    settings.insert(
        "Gtk/ApplicationPreferDarkTheme".to_owned(),
        XSettingsValue::Integer(i32::from(dark)),
    );
    XSettingsSnapshot {
        serial: 0,
        settings,
    }
}

/// String table published alongside the integer settings above. XSettings
/// strings (theme names) travel in the settings blob; keep them next to the
/// builder so producers cannot drift from `build_settings` keys.
#[must_use]
pub fn string_table(input: &XSettingsInput) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("Net/ThemeName".to_owned(), gtk3_theme_for(input).to_owned()),
        ("Net/IconThemeName".to_owned(), input.icon_theme.clone()),
        ("Xft/HintStyle".to_owned(), input.hint_style.clone()),
        ("Xft/RGBA".to_owned(), input.rgba_order.clone()),
        (
            "Gtk/FontName".to_owned(),
            format!(
                "{}{} {}",
                input.font_family,
                if input.font_bold { " Bold" } else { "" },
                input.font_size_points
            ),
        ),
        ("Xft/CursorTheme".to_owned(), input.cursor_theme.clone()),
    ])
}

/// Full published setting table: the integer/theme keys from `build_settings`
/// plus the canonical GTK font and cursor keys. `Xft/CursorTheme` from
/// `string_table` is intentionally not published (not a canonical XSettings key).
#[must_use]
pub fn published_settings(input: &XSettingsInput) -> BTreeMap<String, XSettingsValue> {
    let mut settings = build_settings(input).settings;
    let strings = string_table(input);
    if let Some(font) = strings.get("Gtk/FontName") {
        settings.insert("Gtk/FontName".to_owned(), XSettingsValue::Text(font.clone()));
    }
    settings.insert(
        "Gtk/CursorThemeName".to_owned(),
        XSettingsValue::Text(input.cursor_theme.clone()),
    );
    settings.insert(
        "Gtk/CursorThemeSize".to_owned(),
        XSettingsValue::Integer(i32::try_from(input.cursor_size).unwrap_or(i32::MAX)),
    );
    settings
}

pub mod blob;
pub mod manager;
mod x11;

pub use blob::{PublishedSetting, PublishedState, encode_blob, next_publication};
pub use manager::{
    ManagerOwnership, ManagerPort, SelectionClear, XSettingsManager, acquire_manager,
    ownership_after_clear,
};
pub use x11::{X11ManagerPort, X11XSettingsManager, claim_x11_manager};

/// Port the engine implements: report whether the selection is already owned.
pub trait SelectionOwner {
    fn selection_owner_exists(&self, selection: &str) -> bool;
}

/// Claim `_XSETTINGS_S0` only when no owner exists. Never steals ownership.
pub fn claim_manager<O: SelectionOwner>(owner: &O) -> FlameResult<()> {
    if owner.selection_owner_exists(XSETTINGS_SELECTION) {
        return Err(FlameError::unavailable(
            "XSettings manager already owned; refusing to replace existing owner",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeOwner {
        owned: bool,
    }

    impl SelectionOwner for FakeOwner {
        fn selection_owner_exists(&self, selection: &str) -> bool {
            assert_eq!(selection, XSETTINGS_SELECTION);
            self.owned
        }
    }

    #[test]
    fn dark_is_default_and_publishes_dark_preference() {
        assert_eq!(AppearanceMode::default(), AppearanceMode::Dark);
        let snapshot = build_settings(&XSettingsInput::default());
        assert_eq!(
            snapshot.get("Gtk/PreferDarkTheme"),
            Some(XSettingsValue::Integer(1))
        );
        assert_eq!(
            snapshot.get("Gtk/ApplicationPreferDarkTheme"),
            Some(XSettingsValue::Integer(1))
        );
        assert!(snapshot.settings.contains_key("Net/ThemeName"));
        assert!(snapshot.settings.contains_key("Net/IconThemeName"));
        assert!(snapshot.settings.contains_key("Xft/DPI"));
        assert!(snapshot.settings.contains_key("Xft/Antialias"));
        assert!(snapshot.settings.contains_key("Xft/Hinting"));
        let strings = string_table(&XSettingsInput::default());
        assert_eq!(
            strings.get("Net/ThemeName").map(String::as_str),
            Some("Flame-Dark")
        );
        assert_eq!(
            strings.get("Net/IconThemeName").map(String::as_str),
            Some("breeze-dark")
        );
    }

    #[test]
    fn refuses_to_replace_existing_manager_owner() {
        let owned = FakeOwner { owned: true };
        let error = claim_manager(&owned).expect_err("must refuse existing owner");
        assert_eq!(error.code, flamewm_api::ErrorCode::Unavailable);
        let free = FakeOwner { owned: false };
        claim_manager(&free).expect("free selection may be claimed");
    }

    #[test]
    fn published_table_carries_canonical_gtk_keys() {
        let settings = published_settings(&XSettingsInput::default());
        assert_eq!(
            settings.get("Gtk/FontName"),
            Some(&XSettingsValue::Text("IBM Plex Sans 10".to_owned()))
        );
        assert_eq!(
            settings.get("Gtk/CursorThemeName"),
            Some(&XSettingsValue::Text("FlameWM-Breeze-Dark".to_owned()))
        );
        assert_eq!(
            settings.get("Gtk/CursorThemeSize"),
            Some(&XSettingsValue::Integer(24))
        );
        assert_eq!(
            settings.get("Xft/DPI"),
            Some(&XSettingsValue::Integer(96 * 1024))
        );
        assert!(!settings.contains_key("Xft/CursorTheme"));
    }

    #[test]
    fn gtk_font_name_publishes_bold_face_without_changing_family_or_size() {
        let input = XSettingsInput {
            font_family: "Noto Sans".to_owned(),
            font_bold: true,
            font_size_points: 13,
            ..XSettingsInput::default()
        };
        let expected = XSettingsValue::Text("Noto Sans Bold 13".to_owned());
        assert_eq!(
            published_settings(&input).get("Gtk/FontName"),
            Some(&expected)
        );
        assert_eq!(
            string_table(&input).get("Gtk/FontName").map(String::as_str),
            Some("Noto Sans Bold 13")
        );
    }

    #[test]
    fn system_appearance_publishes_light_policy() {
        assert_eq!(
            published_appearance(AppearanceMode::System),
            AppearanceMode::Light
        );
        assert_eq!(
            published_appearance(AppearanceMode::Dark),
            AppearanceMode::Dark
        );
        let input = XSettingsInput {
            appearance: AppearanceMode::System,
            ..XSettingsInput::default()
        };
        let snapshot = build_settings(&input);
        assert_eq!(
            snapshot.get("Gtk/PreferDarkTheme"),
            Some(XSettingsValue::Integer(0))
        );
        assert_eq!(
            snapshot.get("Gtk/ApplicationPreferDarkTheme"),
            Some(XSettingsValue::Integer(0))
        );
        assert_eq!(
            string_table(&input).get("Net/ThemeName").map(String::as_str),
            Some("Flame-Light")
        );
    }

    #[test]
    fn light_snapshot_clears_dark_preference() {
        let input = XSettingsInput {
            appearance: AppearanceMode::Light,
            ..XSettingsInput::default()
        };
        let snapshot = build_settings(&input);
        assert_eq!(
            snapshot.get("Gtk/PreferDarkTheme"),
            Some(XSettingsValue::Integer(0))
        );
        let strings = string_table(&input);
        assert_eq!(
            strings.get("Net/ThemeName").map(String::as_str),
            Some("Flame-Light")
        );
    }

    #[test]
    fn installed_gtk3_theme_overrides_flame_name_only_when_set() {
        let default = XSettingsInput::default();
        assert_eq!(
            published_settings(&default).get("Net/ThemeName"),
            Some(&XSettingsValue::Text("Flame-Dark".to_owned()))
        );
        let input = XSettingsInput {
            gtk3_theme: Some("Breeze-Dark".to_owned()),
            ..XSettingsInput::default()
        };
        assert_eq!(
            published_settings(&input).get("Net/ThemeName"),
            Some(&XSettingsValue::Text("Breeze-Dark".to_owned()))
        );
        assert_eq!(
            string_table(&input).get("Net/ThemeName").map(String::as_str),
            Some("Breeze-Dark")
        );
    }
}
