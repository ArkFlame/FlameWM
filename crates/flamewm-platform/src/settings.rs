use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flamewm_api::panels::PanelEdge;
use flamewm_api::settings::{AppearanceMode, SettingValue};
use flamewm_api::shortcuts::{
    ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::chrome::parse_hex_rgb;

const KNOWN_HOTKEYS: [&str; 8] = [
    ACTION_TOGGLE_START_MENU,
    ACTION_WORKSPACE_LEFT,
    ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP,
    ACTION_WORKSPACE_DOWN,
    ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_MAXIMIZE,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSettings {
    pub revision: u64,
    pub accent: String,
    pub icon_theme: String,
    pub appearance: AppearanceMode,
    /// Empty means use the desktop/theme default wallpaper.
    pub wallpaper: String,
    pub desktop_selection_fill_opacity: u8,
    pub window_snap_preview_fill_opacity: u8,
    pub font_family: String,
    pub font_bold: bool,
    pub font_size_offset: i8,
    pub taskbar_color: String,
    pub taskbar_opacity: u8,
    pub taskbar_height: u16,
    pub start_button_text: String,
    pub custom_start_icon: String,
    pub sticky_notes_enabled: bool,
    pub start_menu_color: String,
    pub start_menu_opacity: u8,
    pub accent_opacity: u8,
    pub decoration: String,
    pub titlebar_color: String,
    pub titlebar_minimize_style: String,
    pub titlebar_maximize_style: String,
    pub titlebar_close_style: String,
    pub titlebar_minimize_color: String,
    pub titlebar_minimize_hover_color: String,
    pub titlebar_minimize_pressed_color: String,
    pub titlebar_maximize_color: String,
    pub titlebar_maximize_hover_color: String,
    pub titlebar_maximize_pressed_color: String,
    pub titlebar_close_color: String,
    pub titlebar_close_hover_color: String,
    pub titlebar_close_pressed_color: String,
    pub cursor_theme: String,
    pub animations_enabled: bool,
    pub animation_speed: u16,
    pub start_animation: String,
    pub start_animation_enabled: bool,
    pub window_open_animation_enabled: bool,
    pub minimize_animation_enabled: bool,
    pub maximize_animation_enabled: bool,
    pub per_output_scale: BTreeMap<String, u16>,
    pub panel_edges: BTreeMap<String, PanelEdge>,
    pub hotkeys: BTreeMap<String, String>,
    pub unknown_keys: BTreeMap<String, String>,
}

impl Default for ProductSettings {
    fn default() -> Self {
        Self {
            revision: 1,
            accent: "#EF4048".to_owned(),
            icon_theme: "McMojave-circle".to_owned(),
            appearance: AppearanceMode::Dark,
            wallpaper: String::new(),
            desktop_selection_fill_opacity: 20,
            window_snap_preview_fill_opacity: 20,
            font_family: "IBM Plex Sans".to_owned(),
            font_bold: false,
            font_size_offset: 0,
            taskbar_color: "#191b1d".to_owned(),
            taskbar_opacity: 99,
            taskbar_height: 44,
            start_button_text: String::new(),
            custom_start_icon: String::new(),
            sticky_notes_enabled: true,
            start_menu_color: "#191b1d".to_owned(),
            start_menu_opacity: 99,
            accent_opacity: 100,
            decoration: "rounded".to_owned(),
            titlebar_color: "#1b1e20".to_owned(),
            titlebar_minimize_style: "default".to_owned(),
            titlebar_maximize_style: "default".to_owned(),
            titlebar_close_style: "default".to_owned(),
            titlebar_minimize_color: String::new(),
            titlebar_minimize_hover_color: String::new(),
            titlebar_minimize_pressed_color: String::new(),
            titlebar_maximize_color: String::new(),
            titlebar_maximize_hover_color: String::new(),
            titlebar_maximize_pressed_color: String::new(),
            titlebar_close_color: String::new(),
            titlebar_close_hover_color: String::new(),
            titlebar_close_pressed_color: String::new(),
            cursor_theme: String::new(),
            animations_enabled: true,
            animation_speed: 100,
            start_animation: "slide-up".to_owned(),
            start_animation_enabled: true,
            window_open_animation_enabled: true,
            minimize_animation_enabled: true,
            maximize_animation_enabled: true,
            per_output_scale: BTreeMap::new(),
            panel_edges: BTreeMap::new(),
            hotkeys: BTreeMap::new(),
            unknown_keys: BTreeMap::new(),
        }
    }
}

impl ProductSettings {
    pub fn validate(&self) -> FlameResult<()> {
        validate_color(&self.taskbar_color, false, "taskbarColor")?;
        validate_color(&self.accent, true, "accent")?;
        if self
            .wallpaper
            .chars()
            .any(|c| c == '\n' || c == '\r' || c == '\0')
        {
            return invalid("wallpaper path contains invalid control characters");
        }
        if self.desktop_selection_fill_opacity > 60 {
            return invalid("DesktopSelectionFillOpacity must be 0..60");
        }
        if self.window_snap_preview_fill_opacity > 60 {
            return invalid("WindowSnapPreviewFillOpacity must be 0..60");
        }
        if !(-2..=4).contains(&self.font_size_offset) {
            return invalid("fontSizeOffset must be -2..4");
        }
        if self.taskbar_opacity > 100 {
            return invalid("taskbarOpacity must be 0..100");
        }
        if !(34..=72).contains(&self.taskbar_height) {
            return invalid("taskbarHeight must be 34..72");
        }
        validate_color(&self.start_menu_color, false, "startMenuColor")?;
        validate_color(&self.titlebar_color, false, "titlebarColor")?;
        for (value, name) in [
            (&self.titlebar_minimize_color, "titlebarMinimizeColor"),
            (
                &self.titlebar_minimize_hover_color,
                "titlebarMinimizeHoverColor",
            ),
            (
                &self.titlebar_minimize_pressed_color,
                "titlebarMinimizePressedColor",
            ),
            (&self.titlebar_maximize_color, "titlebarMaximizeColor"),
            (
                &self.titlebar_maximize_hover_color,
                "titlebarMaximizeHoverColor",
            ),
            (
                &self.titlebar_maximize_pressed_color,
                "titlebarMaximizePressedColor",
            ),
            (&self.titlebar_close_color, "titlebarCloseColor"),
            (&self.titlebar_close_hover_color, "titlebarCloseHoverColor"),
            (
                &self.titlebar_close_pressed_color,
                "titlebarClosePressedColor",
            ),
        ] {
            validate_color(value, true, name)?;
        }
        if self.start_menu_opacity > 100 {
            return invalid("startMenuOpacity must be 0..100");
        }
        if self.accent_opacity > 100 {
            return invalid("accentOpacity must be 0..100");
        }
        if !matches!(self.decoration.as_str(), "rounded" | "ios") {
            return invalid("decoration must be rounded or ios");
        }
        for style in [
            &self.titlebar_minimize_style,
            &self.titlebar_maximize_style,
            &self.titlebar_close_style,
        ] {
            if !matches!(style.as_str(), "default" | "flat" | "circle" | "square") {
                return invalid("titlebar button style must be default/flat/circle/square");
            }
        }
        if self.cursor_theme.len() > 64
            || self
                .cursor_theme
                .chars()
                .any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return invalid("cursorTheme must be at most 64 characters without path separators");
        }
        if !(25..=200).contains(&self.animation_speed) {
            return invalid("animationSpeed must be 25..200");
        }
        if self.start_animation != "slide-up" {
            return invalid("startAnimation must be slide-up");
        }
        for output in self.panel_edges.keys() {
            if output.is_empty()
                || output
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '=')
            {
                return invalid("panel.edge output id is invalid");
            }
        }
        for (output, scale) in &self.per_output_scale {
            if output.is_empty()
                || output
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '=')
            {
                return invalid("perOutputScale output id is invalid");
            }
            if !matches!(*scale, 100 | 125 | 150 | 175 | 200) {
                return invalid("perOutputScale supports only 100/125/150/175/200");
            }
        }
        for action in self.hotkeys.keys() {
            if !KNOWN_HOTKEYS.contains(&action.as_str()) {
                return invalid("unknown hotkey action id");
            }
        }
        for key in self.unknown_keys.keys() {
            if is_stable_key(key) {
                return invalid("unknown key duplicates stable key");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    current: ProductSettings,
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self {
            current: ProductSettings::default(),
        }
    }
}

impl ConfigStore {
    #[must_use]
    pub fn current(&self) -> &ProductSettings {
        &self.current
    }

    pub fn parse_snapshot(&self, text: &str) -> FlameResult<ProductSettings> {
        let mut snapshot = self.current.clone();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return invalid_owned(format!("missing '=' in line: {line}"));
            };
            let key = key.trim();
            let value = value.trim();
            if let Some(row) = stable_setting(key) {
                (row.parse)(&mut snapshot, value, key)?;
                continue;
            }
            match key {
                "revision" => snapshot.revision = parse(value, "revision")?,
                _ if key.starts_with("scale.") => {
                    let output = key.trim_start_matches("scale.");
                    snapshot
                        .per_output_scale
                        .insert(output.to_owned(), parse(value, key)?);
                }
                _ if key.starts_with("panel.edge.") => {
                    let output = key.trim_start_matches("panel.edge.");
                    match parse_panel_edge(value) {
                        Some(edge) => snapshot.panel_edges.insert(output.to_owned(), edge),
                        None => return invalid("panel.edge must be bottom/top/left/right"),
                    };
                }
                _ if key.starts_with("hotkey.") => {
                    let action = key.trim_start_matches("hotkey.");
                    snapshot.hotkeys.insert(action.to_owned(), value.to_owned());
                }
                _ => {
                    snapshot
                        .unknown_keys
                        .insert(key.to_owned(), value.to_owned());
                }
            }
        }
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn apply_snapshot(&mut self, snapshot: ProductSettings) -> FlameResult<()> {
        snapshot.validate()?;
        if snapshot.revision <= self.current.revision {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "settings revision is stale",
            ));
        }
        self.current = snapshot;
        Ok(())
    }

    /// Adopts persisted per-output scales without moving the settings revision.
    pub fn adopt_scales(&mut self, snapshot: &ProductSettings) {
        self.current
            .per_output_scale
            .clone_from(&snapshot.per_output_scale);
    }

    /// Adopts persisted panel edges without moving the settings revision.
    pub fn adopt_panel_edges(&mut self, snapshot: &ProductSettings) {
        self.current.panel_edges.clone_from(&snapshot.panel_edges);
    }

    #[must_use]
    pub fn serialize(snapshot: &ProductSettings) -> String {
        let defaults = ProductSettings::default();
        let mut lines = vec![format!("revision={}", snapshot.revision)];
        // Base rows are always written. Optional rows are written only when they differ from
        // defaults, so settings files that never set them keep their existing byte-for-byte shape.
        for row in STABLE_SETTINGS {
            let value = (row.get)(snapshot);
            if row.base || value != (row.get)(&defaults) {
                lines.push(format!("{}={}", row.key, wire_text(&value, row.base)));
            }
        }
        lines.extend(
            snapshot
                .per_output_scale
                .iter()
                .map(|(output, scale)| format!("scale.{output}={scale}")),
        );
        lines.extend(
            snapshot
                .panel_edges
                .iter()
                .map(|(output, edge)| format!("panel.edge.{output}={}", panel_edge_name(*edge))),
        );
        lines.extend(
            snapshot
                .hotkeys
                .iter()
                .map(|(action, binding)| format!("hotkey.{action}={binding}")),
        );
        lines.extend(
            snapshot
                .unknown_keys
                .iter()
                .map(|(key, value)| format!("{key}={value}")),
        );
        lines.push(String::new());
        lines.join("\n")
    }

    /// Linux-first tmp+fsync+rename persistence matching the native ConfigStore contract.
    pub fn persist(path: &Path, snapshot: &ProductSettings) -> FlameResult<()> {
        snapshot.validate()?;
        if path.as_os_str().is_empty() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "settings persistence path is empty",
            ));
        }
        let parent = path
            .parent()
            .filter(|candidate| !candidate.as_os_str().is_empty());
        if let Some(parent) = parent {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let temp = temp_path(path);
        let write_result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temp)?;
            file.write_all(Self::serialize(snapshot).as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp);
            return Err(io_error(error));
        }
        if let Some(parent) = parent {
            if let Ok(directory) = File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }
}

/// Typed bridge between one `ProductSettings` field and its file text, API value, and apply input.
trait Slot: Sized + Clone {
    fn to_value(&self) -> SettingValue;
    fn from_text(text: &str, key: &str) -> FlameResult<Self>;
    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self>;
}

impl Slot for String {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.clone())
    }

    fn from_text(text: &str, _key: &str) -> FlameResult<Self> {
        Ok(text.to_owned())
    }

    fn from_value(value: &SettingValue, _key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Text(text) => Ok(text.clone()),
            _ => invalid("setting requires text"),
        }
    }
}

impl Slot for bool {
    fn to_value(&self) -> SettingValue {
        SettingValue::Boolean(*self)
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        parse_bool(text, key)
    }

    fn from_value(value: &SettingValue, _key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Boolean(on) => Ok(*on),
            _ => invalid("setting requires boolean"),
        }
    }
}

impl Slot for u8 {
    fn to_value(&self) -> SettingValue {
        SettingValue::Percent(*self)
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        parse(text, key)
    }

    fn from_value(value: &SettingValue, _key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Percent(percent) => Ok(*percent),
            _ => invalid("setting requires percent"),
        }
    }
}

macro_rules! integer_slot {
    ($($ty:ty),*) => {$(
        impl Slot for $ty {
            fn to_value(&self) -> SettingValue {
                SettingValue::Integer(i64::from(*self))
            }

            fn from_text(text: &str, key: &str) -> FlameResult<Self> {
                parse(text, key)
            }

            fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
                match value {
                    SettingValue::Integer(number) => (*number).try_into().map_err(|_| {
                        FlameError::new(ErrorCode::InvalidArgument, format!("{key} is out of range"))
                    }),
                    _ => invalid("setting requires integer"),
                }
            }
        }
    )*};
}

integer_slot!(i8, u16);

impl Slot for AppearanceMode {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.as_str().to_owned())
    }

    fn from_text(text: &str, _key: &str) -> FlameResult<Self> {
        AppearanceMode::parse(text).ok_or_else(|| FlameError::invalid("bad appearance"))
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        match value {
            SettingValue::Text(text) => Self::from_text(text, key),
            _ => invalid("setting requires text"),
        }
    }
}

/// One typed key. The table below is the single source for file parse, serialize, API snapshot,
/// apply, and section reset. `base` keys are always written (bools as 1/0); other keys are written
/// only when they differ from defaults (bools as true/false).
pub(crate) struct StableSetting {
    pub(crate) key: &'static str,
    pub(crate) section: &'static str,
    pub(crate) base: bool,
    pub(crate) get: fn(&ProductSettings) -> SettingValue,
    pub(crate) set:
        fn(&mut ProductSettings, &ProductSettings, Option<&SettingValue>, &str) -> FlameResult<()>,
    pub(crate) parse: fn(&mut ProductSettings, &str, &str) -> FlameResult<()>,
}

macro_rules! stable_settings {
    ($(($base:literal, $section:ident, $key:literal, $field:ident)),* $(,)?) => {
        pub(crate) static STABLE_SETTINGS: &[StableSetting] = &[$(
            StableSetting {
                key: $key,
                section: stringify!($section),
                base: $base,
                get: |settings| Slot::to_value(&settings.$field),
                set: |settings, defaults, value, key| {
                    settings.$field = resolve(value, &defaults.$field, key)?;
                    Ok(())
                },
                parse: |settings, text, key| {
                    settings.$field = Slot::from_text(text, key)?;
                    Ok(())
                },
            },
        )*];
    };
}

stable_settings! {
    (true, appearance, "accent", accent),
    (true, appearance, "appearance", appearance),
    (true, desktop, "wallpaper", wallpaper),
    (true, desktop, "DesktopSelectionFillOpacity", desktop_selection_fill_opacity),
    (true, windows, "WindowSnapPreviewFillOpacity", window_snap_preview_fill_opacity),
    (true, fonts, "fontFamily", font_family),
    (true, fonts, "fontBold", font_bold),
    (true, fonts, "fontSizeOffset", font_size_offset),
    (true, taskbar, "taskbarColor", taskbar_color),
    (true, taskbar, "taskbarOpacity", taskbar_opacity),
    (true, taskbar, "taskbarHeight", taskbar_height),
    (true, taskbar, "startButtonText", start_button_text),
    (true, taskbar, "customStartIcon", custom_start_icon),
    (true, desktop, "stickyNotesEnabled", sticky_notes_enabled),
    (false, appearance, "iconTheme", icon_theme),
    (false, taskbar, "startMenuColor", start_menu_color),
    (false, taskbar, "startMenuOpacity", start_menu_opacity),
    (false, appearance, "accentOpacity", accent_opacity),
    (false, appearance, "decoration", decoration),
    (false, windows, "titlebarColor", titlebar_color),
    (false, windows, "titlebarMinimizeStyle", titlebar_minimize_style),
    (false, windows, "titlebarMaximizeStyle", titlebar_maximize_style),
    (false, windows, "titlebarCloseStyle", titlebar_close_style),
    (false, windows, "titlebarMinimizeColor", titlebar_minimize_color),
    (false, windows, "titlebarMinimizeHoverColor", titlebar_minimize_hover_color),
    (false, windows, "titlebarMinimizePressedColor", titlebar_minimize_pressed_color),
    (false, windows, "titlebarMaximizeColor", titlebar_maximize_color),
    (false, windows, "titlebarMaximizeHoverColor", titlebar_maximize_hover_color),
    (false, windows, "titlebarMaximizePressedColor", titlebar_maximize_pressed_color),
    (false, windows, "titlebarCloseColor", titlebar_close_color),
    (false, windows, "titlebarCloseHoverColor", titlebar_close_hover_color),
    (false, windows, "titlebarClosePressedColor", titlebar_close_pressed_color),
    (false, appearance, "cursorTheme", cursor_theme),
    (false, windows, "animationsEnabled", animations_enabled),
    (false, windows, "animationSpeed", animation_speed),
    (false, windows, "startAnimation", start_animation),
    (false, windows, "startAnimationEnabled", start_animation_enabled),
    (false, windows, "windowOpenAnimationEnabled", window_open_animation_enabled),
    (false, windows, "minimizeAnimationEnabled", minimize_animation_enabled),
    (false, windows, "maximizeAnimationEnabled", maximize_animation_enabled),
}

#[must_use]
pub(crate) fn stable_setting(key: &str) -> Option<&'static StableSetting> {
    STABLE_SETTINGS.iter().find(|row| row.key == key)
}

fn resolve<T: Slot>(value: Option<&SettingValue>, default: &T, key: &str) -> FlameResult<T> {
    value.map_or_else(|| Ok(default.clone()), |value| T::from_value(value, key))
}

fn wire_text(value: &SettingValue, flag: bool) -> String {
    match value {
        SettingValue::Text(text) => text.clone(),
        SettingValue::Boolean(on) if flag => u8::from(*on).to_string(),
        SettingValue::Boolean(on) => on.to_string(),
        SettingValue::Percent(percent) => percent.to_string(),
        SettingValue::Integer(number) => number.to_string(),
        SettingValue::Rgb(rgb) => rgb.to_string(),
    }
}

fn parse<T: std::str::FromStr>(value: &str, name: &str) -> FlameResult<T> {
    value
        .parse::<T>()
        .map_err(|_| FlameError::new(ErrorCode::InvalidArgument, format!("bad {name}")))
}

#[must_use]
pub const fn panel_edge_name(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Bottom => "bottom",
        PanelEdge::Top => "top",
        PanelEdge::Left => "left",
        PanelEdge::Right => "right",
    }
}

fn parse_panel_edge(value: &str) -> Option<PanelEdge> {
    match value {
        "bottom" => Some(PanelEdge::Bottom),
        "top" => Some(PanelEdge::Top),
        "left" => Some(PanelEdge::Left),
        "right" => Some(PanelEdge::Right),
        _ => None,
    }
}

fn parse_bool(value: &str, name: &str) -> FlameResult<bool> {
    match value {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" => Ok(false),
        _ => invalid_owned(format!("bad {name}")),
    }
}

fn validate_color(value: &str, allow_empty: bool, name: &str) -> FlameResult<()> {
    if allow_empty && value.is_empty() {
        return Ok(());
    }
    if parse_hex_rgb(value).is_some() {
        return Ok(());
    }
    invalid_owned(format!(
        "{name} must be #RRGGBB{}",
        if allow_empty { " or empty" } else { "" }
    ))
}

fn is_stable_key(key: &str) -> bool {
    key == "revision"
        || stable_setting(key).is_some()
        || key.starts_with("scale.")
        || key.starts_with("panel.edge.")
        || key.starts_with("hotkey.")
}

fn temp_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.tmp", path.display()))
}

fn invalid<T>(message: &'static str) -> FlameResult<T> {
    Err(FlameError::new(ErrorCode::InvalidArgument, message))
}

fn invalid_owned<T>(message: String) -> FlameResult<T> {
    Err(FlameError::new(ErrorCode::InvalidArgument, message))
}

fn io_error(error: io::Error) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_current_flamewm_product_contract() {
        let settings = ProductSettings::default();
        assert_eq!(settings.revision, 1);
        assert_eq!(settings.accent, "#EF4048");
        assert_eq!(settings.taskbar_height, 44);
        assert_eq!(settings.desktop_selection_fill_opacity, 20);
        assert_eq!(settings.window_snap_preview_fill_opacity, 20);
        assert_eq!(settings.appearance, AppearanceMode::Dark);
    }

    #[test]
    fn unset_icon_theme_defaults_to_mcmojave_and_writes_nothing() {
        let store = ConfigStore::default();
        let parsed = store.parse_snapshot("revision=2\n").expect("valid snapshot");
        assert_eq!(parsed.icon_theme, "McMojave-circle");
        assert!(!ConfigStore::serialize(&parsed).contains("iconTheme"));
    }

    #[test]
    fn explicit_icon_theme_is_kept_and_written() {
        let store = ConfigStore::default();
        let parsed = store
            .parse_snapshot("revision=2\niconTheme=*:-HighContrast\n")
            .expect("explicit icon theme");
        assert_eq!(parsed.icon_theme, "*:-HighContrast");
        let text = ConfigStore::serialize(&parsed);
        assert!(text.contains("iconTheme=*:-HighContrast\n"));
        assert_eq!(store.parse_snapshot(&text).expect("round trip"), parsed);
    }

    #[test]
    fn appearance_snapshot_round_trip_defaults_to_dark() {
        let store = ConfigStore::default();
        let parsed = store
            .parse_snapshot("revision=2\n")
            .expect("valid snapshot");
        assert_eq!(parsed.appearance, AppearanceMode::Dark);
        let text = ConfigStore::serialize(&ProductSettings::default());
        assert!(text.contains("appearance=dark\n"));
        let reparsed = store.parse_snapshot(&text).expect("round trip");
        assert_eq!(reparsed.appearance, AppearanceMode::Dark);
    }

    #[test]
    fn invalid_appearance_value_is_rejected() {
        let store = ConfigStore::default();
        assert_eq!(
            store
                .parse_snapshot("revision=2\nappearance=neon\n")
                .expect_err("bad appearance")
                .code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn parser_preserves_unknown_compatible_keys() {
        let store = ConfigStore::default();
        let parsed = store
            .parse_snapshot("revision=2\nfutureFeature=value\n")
            .expect("valid future key");
        assert_eq!(
            parsed.unknown_keys.get("futureFeature"),
            Some(&"value".to_owned())
        );
    }

    #[test]
    fn stale_revision_cannot_replace_effective_state() {
        let mut store = ConfigStore::default();
        let mut candidate = ProductSettings::default();
        candidate.revision = 1;
        assert_eq!(
            store
                .apply_snapshot(candidate)
                .expect_err("must reject stale")
                .code,
            ErrorCode::StaleRevision
        );
    }

    #[test]
    fn invalid_output_scale_is_rejected() {
        let mut settings = ProductSettings::default();
        settings.per_output_scale.insert("eDP-1".to_owned(), 133);
        assert_eq!(
            settings.validate().expect_err("invalid scale").code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn empty_persistence_path_is_rejected_before_io() {
        assert_eq!(
            ConfigStore::persist(Path::new(""), &ProductSettings::default())
                .expect_err("empty path")
                .code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn new_appearance_keys_default_and_stay_unwritten_when_unchanged() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.start_menu_color, "#191b1d");
        assert_eq!(defaults.start_menu_opacity, 99);
        assert_eq!(defaults.accent_opacity, 100);
        assert_eq!(defaults.decoration, "rounded");
        assert_eq!(defaults.titlebar_color, "#1b1e20");
        assert_eq!(defaults.titlebar_close_style, "default");
        assert!(defaults.titlebar_close_color.is_empty());
        assert!(defaults.cursor_theme.is_empty());
        assert_eq!(defaults.animation_speed, 100);
        assert_eq!(defaults.start_animation, "slide-up");
        assert!(defaults.maximize_animation_enabled);

        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("startMenuColor"));
        assert!(!text.contains("titlebar"));
        assert!(!text.contains("animation"));
    }

    #[test]
    fn old_settings_file_parses_to_defaults_and_serializes_unchanged() {
        let store = ConfigStore::default();
        let old = "revision=3\naccent=#EF4048\ntaskbarColor=#191b1d\nfutureFeature=kept\n";
        let parsed = store.parse_snapshot(old).expect("old file");
        assert_eq!(parsed.animation_speed, 100);
        assert_eq!(parsed.titlebar_color, "#1b1e20");
        let text = ConfigStore::serialize(&parsed);
        assert!(!text.contains("titlebar"));
        assert!(text.contains("futureFeature=kept\n"));
        assert_eq!(store.parse_snapshot(&text).expect("round trip"), parsed);
    }

    #[test]
    fn new_appearance_keys_round_trip_when_set() {
        let store = ConfigStore::default();
        let text = "revision=2\nstartMenuColor=#112233\nstartMenuOpacity=80\naccentOpacity=40\n\
            decoration=ios\ntitlebarColor=#A0B0C0\ntitlebarCloseStyle=circle\n\
            titlebarMinimizeHoverColor=#FFFFFF\ncursorTheme=Adwaita\nanimationsEnabled=false\n\
            animationSpeed=150\nstartAnimationEnabled=0\nfutureFeature=kept\n";
        let parsed = store.parse_snapshot(text).expect("valid new keys");
        assert_eq!(parsed.start_menu_color, "#112233");
        assert_eq!(parsed.start_menu_opacity, 80);
        assert_eq!(parsed.accent_opacity, 40);
        assert_eq!(parsed.decoration, "ios");
        assert_eq!(parsed.titlebar_close_style, "circle");
        assert_eq!(parsed.titlebar_minimize_hover_color, "#FFFFFF");
        assert_eq!(parsed.cursor_theme, "Adwaita");
        assert!(!parsed.animations_enabled);
        assert_eq!(parsed.animation_speed, 150);
        assert!(!parsed.start_animation_enabled);
        assert_eq!(
            parsed.unknown_keys.get("futureFeature"),
            Some(&"kept".to_owned())
        );

        let serialized = ConfigStore::serialize(&parsed);
        assert!(serialized.contains("animationsEnabled=false\n"));
        assert!(serialized.contains("futureFeature=kept\n"));
        assert_eq!(
            store.parse_snapshot(&serialized).expect("round trip"),
            parsed
        );
    }

    #[test]
    fn invalid_new_appearance_values_are_rejected() {
        let store = ConfigStore::default();
        for line in [
            "startMenuColor=red",
            "titlebarColor=#12345",
            "titlebarCloseColor=#GGGGGG",
            "startMenuOpacity=101",
            "accentOpacity=101",
            "decoration=neon",
            "titlebarMinimizeStyle=round",
            "cursorTheme=../evil",
            "cursorTheme=a\\b",
            "animationSpeed=24",
            "animationSpeed=201",
            "startAnimation=bounce",
            "animationsEnabled=maybe",
        ] {
            assert_eq!(
                store
                    .parse_snapshot(&format!("revision=2\n{line}\n"))
                    .expect_err(line)
                    .code,
                ErrorCode::InvalidArgument,
                "{line} must be rejected"
            );
        }
    }

    #[test]
    fn wallpaper_path_with_newline_is_rejected() {
        let mut settings = ProductSettings::default();
        settings.wallpaper = "bad\npath".to_owned();
        assert_eq!(
            settings.validate().expect_err("control character").code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn base_flags_write_digits_and_optional_flags_write_words() {
        let mut settings = ProductSettings::default();
        settings.font_bold = true;
        settings.sticky_notes_enabled = false;
        settings.animations_enabled = false;
        let text = ConfigStore::serialize(&settings);
        assert!(text.contains("fontBold=1\n"));
        assert!(text.contains("stickyNotesEnabled=0\n"));
        assert!(text.contains("animationsEnabled=false\n"));
        assert!(text.find("fontBold=") < text.find("animationsEnabled="));
        let reparsed = ConfigStore::default().parse_snapshot(&text).expect("round trip");
        assert_eq!(reparsed, settings);
    }
}
