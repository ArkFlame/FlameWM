use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod control;
pub mod file_chooser;
pub mod runtime;
pub mod view;

#[cfg(test)]
mod controller;
#[cfg(test)]
mod projection;

use flamewm_api::panels::PanelEdge;
use flamewm_api::settings::SettingValue;
use flamewm_api::{ModeId, OutputId};
use flamewm_control_core::ControlError;
use flamewm_settings_core::{ControlTransport, SettingsClient, SettingsPage};
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess, UiKeyEvent, UiKeyPhase, UiWindowConfig,
};

const DONATE_URI: &str = "https://paypal.me/LinsaFTW";
const SOURCE_URI: &str = "https://github.com/ArkFlame/flamewm";
const WEBSITE_URI: &str = "https://wm.arkflame.com";

#[must_use]
pub fn normal_window_config(width: u32, height: u32) -> UiWindowConfig {
    UiWindowConfig {
        width,
        height,
        x: 0,
        y: 0,
        title: "System Settings".to_owned(),
    }
}

pub struct SettingsApplication<T: ControlTransport> {
    client: SettingsClient<T>,
    wallpaper_chooser: Option<file_chooser::WallpaperChooserWorker>,
}

impl<T: ControlTransport> SettingsApplication<T> {
    pub fn new(transport: T) -> Self {
        Self {
            client: SettingsClient::new(transport),
            wallpaper_chooser: None,
        }
    }

    pub fn client(&self) -> &SettingsClient<T> {
        &self.client
    }
    pub fn client_mut(&mut self) -> &mut SettingsClient<T> {
        &mut self.client
    }

    pub fn refresh(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.client.refresh_all().map_err(control_error)?;
        self.sync(document)
    }

    pub fn handle_action(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.phase != UiActionPhase::Release || !event.inside {
            return Ok(());
        }
        let action = event.action.as_str();
        if let Some(page) = action.strip_prefix("settings.page.").and_then(page) {
            self.client.set_page(page);
        } else if let Some(name) = action.strip_prefix("settings.accent.") {
            let accent = match name {
                "red" => "#EF4048",
                "blue" => "#3DAEE9",
                "purple" => "#9B59B6",
                "teal" => "#1ABC9C",
                "reset" => "#EF4048",
                _ => return Err(format!("unknown accent '{name}'")),
            };
            self.client
                .apply_setting("accent", SettingValue::Text(accent.to_owned()))
                .map_err(control_error)?;
        } else if action == "settings.icon-theme.next" {
            self.client
                .apply_setting(
                    "iconTheme",
                    SettingValue::Text("*:-HighContrast".to_owned()),
                )
                .map_err(control_error)?;
        } else if action == "settings.wallpaper.reset" {
            self.client.set_wallpaper("").map_err(control_error)?;
        } else if action == "settings.wallpaper.choose" {
            if self.wallpaper_chooser.is_some() {
                return Err("wallpaper chooser is already open".to_owned());
            }
            self.wallpaper_chooser = Some(file_chooser::WallpaperChooserWorker::start()?);
        } else if let Some(slot) = action
            .strip_prefix("settings.display.select.")
            .and_then(|value| value.parse::<usize>().ok())
        {
            let output_id = self
                .client
                .displays()
                .and_then(|snapshot| snapshot.outputs.get(slot))
                .map(|output| output.id.clone());
            if let Some(output_id) = output_id {
                self.client.select_output(&output_id);
            }
        } else if let Some(key) = action.strip_prefix("settings.toggle.") {
            let enabled = self
                .client
                .settings()
                .and_then(|snapshot| snapshot.values.get(key))
                .and_then(|value| match value {
                    SettingValue::Boolean(value) => Some(!value),
                    _ => None,
                })
                .ok_or_else(|| format!("toggle '{key}' has no authoritative boolean value"))?;
            self.client
                .apply_setting(key, SettingValue::Boolean(enabled))
                .map_err(control_error)?;
        } else if let Some(step) = action
            .strip_prefix("settings.selection-opacity.")
            .and_then(step)
        {
            self.step_percent("DesktopSelectionFillOpacity", step, 60)?;
        } else if let Some(step) = action.strip_prefix("settings.snap-opacity.").and_then(step) {
            self.step_percent("WindowSnapPreviewFillOpacity", step, 60)?;
        } else if let Some(step) = action
            .strip_prefix("settings.taskbar-opacity.")
            .and_then(step)
        {
            self.set_taskbar_value("opacity", step)?;
        } else if action == "settings.taskbar.position" {
            self.cycle_panel_edge()?;
        } else if let Some(edge) = action.strip_prefix("settings.taskbar.position.") {
            self.set_panel_edge(edge)?;
        } else if action == "settings.taskbar.color" {
            self.cycle_taskbar_color()?;
        } else if action == "settings.taskbar.size" {
            self.set_taskbar_value("size", 1)?;
        } else if let Some(step) = action.strip_prefix("settings.font-size.").and_then(step) {
            self.step_integer("fontSizeOffset", step, -2, 4)?;
        } else if action == "settings.font.family" {
            self.cycle_font_family()?;
        } else if let Some(slot) = action.strip_prefix("settings.hotkey.") {
            self.handle_hotkey_action(slot)?;
        } else if let Some(percent) = action
            .strip_prefix("settings.display.scale.")
            .and_then(|value| value.parse::<u16>().ok())
        {
            self.client
                .set_shell_scale(percent)
                .map_err(control_error)?;
        } else if action == "settings.display.keep" {
            self.client.keep_display_mode().map_err(control_error)?;
        } else if action == "settings.display.revert" {
            self.client.revert_display_mode().map_err(control_error)?;
        } else if let Some(raw_mode) = action
            .strip_prefix("settings.display.begin.")
            .or_else(|| action.strip_prefix("settings.display.mode."))
        {
            let mode = raw_mode
                .parse::<u64>()
                .map(ModeId)
                .map_err(|_| format!("invalid display mode '{raw_mode}'"))?;
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("system clock before Unix epoch: {error}"))?
                .as_millis()
                .min(u128::from(u64::MAX)) as u64;
            self.client
                .begin_display_mode(mode, now_ms)
                .map_err(control_error)?;
        } else if let Some(uri) = about_uri(action) {
            open_uri(uri)?;
        }
        self.sync(document)
    }

    pub fn handle_key_event(
        &mut self,
        event: &UiKeyEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.phase == UiKeyPhase::Release {
            let binding = if event.modifiers.is_empty() {
                event.key.clone()
            } else {
                format!("{}+{}", event.modifiers.join("+"), event.key)
            };
            if self.client.capture_hotkey(&binding) {
                self.client.commit_hotkey_capture().map_err(control_error)?;
            }
        }
        self.sync(document)
    }

    pub fn poll_background(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let result = self
            .wallpaper_chooser
            .as_mut()
            .and_then(file_chooser::WallpaperChooserWorker::poll);
        let Some(result) = result else {
            return Ok(());
        };
        self.wallpaper_chooser = None;
        match result? {
            Some(path) => self
                .client
                .set_wallpaper(path.to_string_lossy().into_owned()),
            None => Ok(()),
        }
        .map_err(control_error)?;
        self.sync(document)
    }

    fn step_percent(&mut self, key: &str, direction: i8, maximum: u8) -> Result<(), String> {
        let current = self.percent_setting(key)?;
        let next = i16::from(current) + i16::from(direction) * 5;
        let next = next.clamp(0, i16::from(maximum)) as u8;
        self.client
            .apply_setting(key, SettingValue::Percent(next))
            .map_err(control_error)
    }

    fn step_integer(
        &mut self,
        key: &str,
        direction: i8,
        minimum: i64,
        maximum: i64,
    ) -> Result<(), String> {
        let current = self.integer_setting(key)?;
        let next = (current + i64::from(direction)).clamp(minimum, maximum);
        self.client
            .apply_setting(key, SettingValue::Integer(next))
            .map_err(control_error)
    }

    fn set_taskbar_value(&mut self, kind: &str, direction: i8) -> Result<(), String> {
        let color = self.text_setting("taskbarColor")?;
        let opacity = self.percent_setting("taskbarOpacity")?;
        let height = self.integer_setting("taskbarHeight")?;
        let (opacity, height) = match kind {
            "opacity" => (
                (i16::from(opacity) + i16::from(direction) * 5).clamp(0, 100) as u8,
                height,
            ),
            "size" => {
                let sizes = [34_i64, 44, 52, 64, 72];
                (opacity, cycle_value(&sizes, height, direction))
            }
            _ => return Err(format!("unknown taskbar setting '{kind}'")),
        };
        self.client
            .set_taskbar_appearance(color, opacity, height as u16)
            .map_err(control_error)
    }

    fn cycle_taskbar_color(&mut self) -> Result<(), String> {
        const COLORS: [&str; 3] = ["#191B1D", "#25282B", "#3D4145"];
        let current = self.text_setting("taskbarColor")?;
        let next = cycle_text(&COLORS, &current);
        self.client
            .apply_setting("taskbarColor", SettingValue::Text(next.to_owned()))
            .map_err(control_error)
    }

    fn cycle_font_family(&mut self) -> Result<(), String> {
        const FAMILIES: [&str; 3] = ["IBM Plex Sans", "Noto Sans", "DejaVu Sans"];
        let current = self.text_setting("fontFamily")?;
        let next = cycle_text(&FAMILIES, &current);
        self.client
            .apply_setting("fontFamily", SettingValue::Text(next.to_owned()))
            .map_err(control_error)
    }

    fn cycle_panel_edge(&mut self) -> Result<(), String> {
        let (output, edge) = self.selected_panel()?;
        self.client
            .set_panel_edge(output, next_edge(edge))
            .map_err(control_error)
    }

    fn set_panel_edge(&mut self, value: &str) -> Result<(), String> {
        let edge = match value {
            "bottom" => PanelEdge::Bottom,
            "top" => PanelEdge::Top,
            "left" => PanelEdge::Left,
            "right" => PanelEdge::Right,
            _ => return Err(format!("unknown panel edge '{value}'")),
        };
        let (output, _) = self.selected_panel()?;
        self.client
            .set_panel_edge(output, edge)
            .map_err(control_error)
    }

    fn selected_panel(&self) -> Result<(OutputId, PanelEdge), String> {
        let output = self
            .client
            .ui()
            .selected_output
            .clone()
            .ok_or_else(|| "no authoritative display output is selected".to_owned())?;
        let edge = self
            .client
            .panels()
            .and_then(|snapshot| snapshot.panels.iter().find(|panel| panel.output == output))
            .map(|panel| panel.edge)
            .ok_or_else(|| "selected output has no authoritative panel".to_owned())?;
        Ok((output, edge))
    }

    fn handle_hotkey_action(&mut self, slot: &str) -> Result<(), String> {
        let (action, operation) = match slot.split_once('.') {
            Some((slot, operation @ ("clear" | "reset"))) => (hotkey_action(slot), Some(operation)),
            _ => (hotkey_action(slot), None),
        };
        let action = action.ok_or_else(|| format!("unknown hotkey row '{slot}'"))?;
        match operation {
            Some("clear") => {
                self.client.begin_hotkey_capture(action);
                self.client.capture_hotkey("Escape");
                self.client.commit_hotkey_capture().map_err(control_error)
            }
            Some("reset") => self.client.reset_hotkey(action).map_err(control_error),
            None => {
                self.client.begin_hotkey_capture(action);
                Ok(())
            }
            _ => Err(format!("unknown hotkey operation '{slot}'")),
        }
    }

    fn percent_setting(&self, key: &str) -> Result<u8, String> {
        match self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
        {
            Some(SettingValue::Percent(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative percent value"
            )),
        }
    }

    fn integer_setting(&self, key: &str) -> Result<i64, String> {
        match self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
        {
            Some(SettingValue::Integer(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative integer value"
            )),
        }
    }

    fn text_setting(&self, key: &str) -> Result<String, String> {
        match self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
        {
            Some(SettingValue::Text(value)) => Ok(value.clone()),
            _ => Err(format!("setting '{key}' has no authoritative text value")),
        }
    }

    fn sync(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        document.position("nav-selection", 8.0, nav_top(self.client.ui().page))?;
        if let Some(SettingValue::Text(accent)) = self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get("accent"))
        {
            document.background("nav-selection", accent_color(accent)?)?;
        }
        for (name, page) in [
            ("appearance", SettingsPage::Appearance),
            ("desktop", SettingsPage::Desktop),
            ("taskbar", SettingsPage::Taskbar),
            ("displays", SettingsPage::Displays),
            ("fonts", SettingsPage::Fonts),
            ("hotkeys", SettingsPage::Hotkeys),
            ("about", SettingsPage::About),
        ] {
            document.visible(&format!("page-{name}"), self.client.ui().page == page)?;
        }
        for slot in 0..4 {
            let id = format!("display-output-{slot}");
            let Some(output) = self
                .client
                .displays()
                .and_then(|snapshot| snapshot.outputs.get(slot))
            else {
                document.visible(&id, false)?;
                continue;
            };
            document.visible(&id, true)?;
            document.text(
                &format!("display-output-name-{slot}"),
                output.connector.clone(),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                &format!("display-output-size-{slot}"),
                format!(
                    "{} x {}  |  {}%",
                    resolution.width, resolution.height, output.shell_scale_percent
                ),
            )?;
            let selected = self.client.ui().selected_output.as_ref() == Some(&output.id);
            document.visible(&format!("display-output-selected-{slot}"), selected)?;
            document.visible(&format!("display-output-unselected-{slot}"), !selected)?;
        }
        if let Some(output) = self.client.ui().selected_output.as_ref().and_then(|id| {
            self.client
                .displays()
                .and_then(|snapshot| snapshot.outputs.iter().find(|output| &output.id == id))
        }) {
            document.text(
                "display-selected-title",
                format!("Selected: {}", output.connector),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                "display-selected-resolution",
                format!("{} x {}", resolution.width, resolution.height),
            )?;
            document.text(
                "display-selected-scale",
                format!("{}%", output.shell_scale_percent),
            )?;
        } else {
            for id in [
                "display-selected-title",
                "display-selected-resolution",
                "display-selected-scale",
            ] {
                document.text(id, "\u{200b}".to_owned())?;
            }
        }
        for (key, on_id, off_id) in [
            (
                "stickyNotesEnabled",
                "sticky-toggle-on",
                "sticky-toggle-off",
            ),
            ("fontBold", "font-toggle-on", "font-toggle-off"),
        ] {
            let enabled = self
                .client
                .settings()
                .and_then(|snapshot| snapshot.values.get(key))
                .and_then(|value| match value {
                    SettingValue::Boolean(value) => Some(*value),
                    _ => None,
                });
            document.visible(on_id, enabled == Some(true))?;
            document.visible(off_id, enabled == Some(false))?;
        }
        if let Some(SettingValue::Text(family)) = self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get("fontFamily"))
        {
            document.font_family(family.clone())?;
        }
        Ok(())
    }
}

fn step(value: &str) -> Option<i8> {
    match value {
        "down" => Some(-1),
        "up" => Some(1),
        _ => None,
    }
}

fn cycle_value(values: &[i64], current: i64, direction: i8) -> i64 {
    let index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    let next = if direction < 0 {
        index.saturating_sub(1)
    } else {
        (index + 1).min(values.len().saturating_sub(1))
    };
    values[next]
}

fn cycle_text<'a>(values: &'a [&'a str], current: &str) -> &'a str {
    let index = values
        .iter()
        .position(|value| value.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    values[(index + 1) % values.len()]
}

fn next_edge(edge: PanelEdge) -> PanelEdge {
    match edge {
        PanelEdge::Bottom => PanelEdge::Top,
        PanelEdge::Top => PanelEdge::Left,
        PanelEdge::Left => PanelEdge::Right,
        PanelEdge::Right => PanelEdge::Bottom,
    }
}

fn hotkey_action(slot: &str) -> Option<&'static str> {
    match slot {
        "start" => Some(flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU),
        "left" => Some(flamewm_api::shortcuts::ACTION_WORKSPACE_LEFT),
        "right" => Some(flamewm_api::shortcuts::ACTION_WORKSPACE_RIGHT),
        "up" => Some(flamewm_api::shortcuts::ACTION_WORKSPACE_UP),
        "down" => Some(flamewm_api::shortcuts::ACTION_WORKSPACE_DOWN),
        _ => None,
    }
}

fn nav_top(page: SettingsPage) -> f32 {
    let index = match page {
        SettingsPage::Appearance => 0,
        SettingsPage::Desktop => 1,
        SettingsPage::Taskbar => 2,
        SettingsPage::Displays => 3,
        SettingsPage::Fonts => 4,
        SettingsPage::Hotkeys => 5,
        SettingsPage::About => 6,
    };
    72.0 + index as f32 * 40.0
}

fn accent_color(value: &str) -> Result<UiColor, String> {
    let digits = value
        .strip_prefix('#')
        .filter(|digits| digits.len() == 6)
        .ok_or_else(|| format!("accent is not a #RRGGBB value: '{value}'"))?;
    let red = u8::from_str_radix(&digits[0..2], 16).map_err(|_| "invalid accent red".to_owned())?;
    let green =
        u8::from_str_radix(&digits[2..4], 16).map_err(|_| "invalid accent green".to_owned())?;
    let blue =
        u8::from_str_radix(&digits[4..6], 16).map_err(|_| "invalid accent blue".to_owned())?;
    Ok(UiColor {
        r: red,
        g: green,
        b: blue,
        a: 56,
    })
}

fn page(value: &str) -> Option<SettingsPage> {
    match value {
        "appearance" => Some(SettingsPage::Appearance),
        "desktop" => Some(SettingsPage::Desktop),
        "taskbar" => Some(SettingsPage::Taskbar),
        "displays" => Some(SettingsPage::Displays),
        "fonts" => Some(SettingsPage::Fonts),
        "hotkeys" => Some(SettingsPage::Hotkeys),
        "about" => Some(SettingsPage::About),
        _ => None,
    }
}

fn control_error(error: ControlError) -> String {
    format!("{}: {}", error.name, error.message)
}

fn about_uri(action: &str) -> Option<&'static str> {
    match action {
        "about.donate" => Some(DONATE_URI),
        "about.source" => Some(SOURCE_URI),
        "about.website" => Some(WEBSITE_URI),
        _ => None,
    }
}

fn open_uri(uri: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(uri)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open {uri}: {error}"))
}
