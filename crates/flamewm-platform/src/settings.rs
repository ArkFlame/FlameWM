use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use flamewm_api::panels::PanelEdge;
use flamewm_api::settings::AppearanceMode;
use flamewm_api::shortcuts::{
    ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

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
            icon_theme: "*:-HighContrast".to_owned(),
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
            match key {
                "revision" => snapshot.revision = parse(value, "revision")?,
                "accent" => snapshot.accent = value.to_owned(),
                "iconTheme" => snapshot.icon_theme = value.to_owned(),
                "appearance" => {
                    snapshot.appearance = AppearanceMode::parse(value)
                        .ok_or_else(|| FlameError::invalid("bad appearance"))?;
                }
                "wallpaper" => snapshot.wallpaper = value.to_owned(),
                "DesktopSelectionFillOpacity" => {
                    snapshot.desktop_selection_fill_opacity = parse(value, key)?;
                }
                "WindowSnapPreviewFillOpacity" => {
                    snapshot.window_snap_preview_fill_opacity = parse(value, key)?;
                }
                "fontFamily" => snapshot.font_family = value.to_owned(),
                "fontBold" => snapshot.font_bold = parse_bool(value, key)?,
                "fontSizeOffset" => snapshot.font_size_offset = parse(value, key)?,
                "taskbarColor" => snapshot.taskbar_color = value.to_owned(),
                "taskbarOpacity" => snapshot.taskbar_opacity = parse(value, key)?,
                "taskbarHeight" => snapshot.taskbar_height = parse(value, key)?,
                "startButtonText" => snapshot.start_button_text = value.to_owned(),
                "customStartIcon" => snapshot.custom_start_icon = value.to_owned(),
                "stickyNotesEnabled" => snapshot.sticky_notes_enabled = parse_bool(value, key)?,
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
        let mut lines = vec![
            format!("revision={}", snapshot.revision),
            format!("accent={}", snapshot.accent),
            format!("iconTheme={}", snapshot.icon_theme),
            format!("appearance={}", snapshot.appearance.as_str()),
            format!("wallpaper={}", snapshot.wallpaper),
            format!(
                "DesktopSelectionFillOpacity={}",
                snapshot.desktop_selection_fill_opacity
            ),
            format!(
                "WindowSnapPreviewFillOpacity={}",
                snapshot.window_snap_preview_fill_opacity
            ),
            format!("fontFamily={}", snapshot.font_family),
            format!("fontBold={}", if snapshot.font_bold { 1 } else { 0 }),
            format!("fontSizeOffset={}", snapshot.font_size_offset),
            format!("taskbarColor={}", snapshot.taskbar_color),
            format!("taskbarOpacity={}", snapshot.taskbar_opacity),
            format!("taskbarHeight={}", snapshot.taskbar_height),
            format!("startButtonText={}", snapshot.start_button_text),
            format!("customStartIcon={}", snapshot.custom_start_icon),
            format!(
                "stickyNotesEnabled={}",
                if snapshot.sticky_notes_enabled { 1 } else { 0 }
            ),
        ];
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
    if value.len() == 7
        && value.starts_with('#')
        && value.chars().skip(1).all(|c| c.is_ascii_hexdigit())
    {
        return Ok(());
    }
    invalid_owned(format!(
        "{name} must be #RRGGBB{}",
        if allow_empty { " or empty" } else { "" }
    ))
}

fn is_stable_key(key: &str) -> bool {
    matches!(
        key,
        "revision"
            | "accent"
            | "iconTheme"
            | "appearance"
            | "wallpaper"
            | "DesktopSelectionFillOpacity"
            | "WindowSnapPreviewFillOpacity"
            | "fontFamily"
            | "fontBold"
            | "fontSizeOffset"
            | "taskbarColor"
            | "taskbarOpacity"
            | "taskbarHeight"
            | "startButtonText"
            | "customStartIcon"
            | "stickyNotesEnabled"
    ) || key.starts_with("scale.")
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
    fn wallpaper_path_with_newline_is_rejected() {
        let mut settings = ProductSettings::default();
        settings.wallpaper = "bad\npath".to_owned();
        assert_eq!(
            settings.validate().expect_err("control character").code,
            ErrorCode::InvalidArgument
        );
    }
}
