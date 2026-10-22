use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use flamewm_api::panels::PanelEdge;
use flamewm_api::settings::{AppearanceMode, SettingValue};
use flamewm_api::shortcuts::{
    ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::chrome::parse_hex_rgb;
use crate::storage::write_atomic_text;

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

/// Generates `as_str` (wire text) and `parse` (wire text to variant) from one variant table.
macro_rules! wire_enum {
    ($ty:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        impl $ty {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            #[must_use]
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($wire => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

/// Clock notation on the lock screen (`lockClockFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockClockFormat {
    TwentyFour,
    Twelve,
}

wire_enum!(LockClockFormat { TwentyFour => "24h", Twelve => "12h" });

/// Lock-screen idle action after the Ribbons screensaver (`screensaverMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreensaverMode {
    Ribbons,
    /// Turn the display off while locked.
    None,
}

wire_enum!(ScreensaverMode { Ribbons => "ribbons", None => "none" });

/// Taskbar layout (`taskbarStyle`). `Floating` insets the bar and rounds its corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarStyle {
    Classic,
    Floating,
}

wire_enum!(TaskbarStyle { Classic => "classic", Floating => "floating" });

/// Clock notation on the taskbar (`taskbarClockFormat`). Same `24h`/`12h` wire values as the lock
/// screen, so both keys share one type.
pub type ClockFormat = LockClockFormat;

/// Image source for the desktop wallpaper (`wallpaperMode`) and the lock background
/// (`lockBackgroundMode`). `default` is the bundled image, `solid` the plain colour, and `custom`
/// the saved image path. Switching modes never erases the saved path or colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundMode {
    Default,
    Solid,
    Custom,
}

wire_enum!(BackgroundMode { Default => "default", Solid => "solid", Custom => "custom" });

/// Desktop wallpaper placement (`wallpaperFit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperFit {
    Fill,
    Fit,
    Stretch,
    Center,
    Tile,
}

wire_enum!(WallpaperFit {
    Fill => "fill",
    Fit => "fit",
    Stretch => "stretch",
    Center => "center",
    Tile => "tile",
});

/// Translucent surface policy for the taskbar or Start menu.
///
/// `blur_percent` (0..=100) maps to a blur radius of `blur_percent / 100 * 24` px at 1x. Blur is
/// only applied when `opacity_percent < 100`; at 100% the radius is always 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backdrop {
    pub opacity_percent: u8,
    pub blur_percent: u8,
    pub radius_px: f32,
}

/// Typed view of the `screensaver*` keys. Ribbon width is in pixels at 1280x720.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreensaverSettings {
    pub mode: ScreensaverMode,
    pub after_lock: Duration,
    pub num_ribbons: u8,
    pub ribbon_width_px: f32,
    pub span_monitors: bool,
    pub display_off_minutes: u16,
}

/// Animation style shared by every element that animates. Consumers reuse one implementation per
/// variant; `None` means no animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationStyle {
    None,
    Slide,
    Fade,
    MagicLamp,
    Scale,
}

impl AnimationStyle {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Slide => "slide",
            Self::Fade => "fade",
            Self::MagicLamp => "magic-lamp",
            Self::Scale => "scale",
        }
    }

    /// Parses one stable key's wire value. `slide-up` is the legacy spelling of `slide`.
    fn parse_for(key: &str, text: &str) -> Option<Self> {
        let style = match text {
            "none" => Self::None,
            "slide" | "slide-up" => Self::Slide,
            "fade" => Self::Fade,
            "magic-lamp" => Self::MagicLamp,
            "scale" => Self::Scale,
            _ => return None,
        };
        let allowed: &[Self] = match key {
            "startAnimation" => &[Self::None, Self::Slide, Self::Fade, Self::MagicLamp],
            "minimizeAnimation" => &[Self::None, Self::Fade, Self::MagicLamp],
            "maximizeAnimation" => &[Self::None, Self::Fade, Self::Scale],
            "windowOpenAnimation" => &[Self::None, Self::Scale, Self::Fade],
            "windowCloseAnimation" => &[Self::None, Self::Fade],
            _ => return None,
        };
        // Old releases erroneously serialized the maximize Scale effect as
        // "magic-lamp". Keep persisted profiles working, but normalize them to Scale.
        if key == "maximizeAnimation" && text == "magic-lamp" {
            return Some(Self::Scale);
        }
        allowed.contains(&style).then_some(style)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSettings {
    pub revision: u64,
    pub accent: String,
    pub icon_theme: String,
    pub appearance: AppearanceMode,
    /// Saved custom wallpaper path; used only when `wallpaper_mode` is `Custom`.
    pub wallpaper: String,
    pub wallpaper_mode: BackgroundMode,
    /// Solid desktop colour used when `wallpaper_mode` is `Solid`.
    pub wallpaper_color: String,
    pub wallpaper_fit: WallpaperFit,
    /// When enabled, the desktop stretches one wallpaper across all outputs.
    pub share_wallpaper: bool,
    pub desktop_selection_fill_opacity: u8,
    /// Desktop entry (icon + label) scale, in percent. 100 keeps the default size.
    pub desktop_entry_scale_percent: u16,
    pub window_snap_preview_fill_opacity: u8,
    pub snap_preview_enabled: bool,
    /// `false` (XP style): move/resize shows an outline and applies the geometry on release.
    pub show_content_while_dragging: bool,
    pub focus_follows_mouse: bool,
    pub font_family: String,
    pub font_bold: bool,
    pub font_size_offset: i8,
    pub taskbar_color: String,
    pub taskbar_opacity: u8,
    pub taskbar_height: u16,
    pub taskbar_style: TaskbarStyle,
    pub taskbar_floating_margin: u16,
    pub taskbar_floating_radius: u16,
    pub taskbar_blur_percent: u8,
    pub taskbar_clock_format: ClockFormat,
    pub taskbar_show_seconds: bool,
    pub taskbar_show_date: bool,
    pub taskbar_show_weather: bool,
    pub taskbar_show_volume: bool,
    pub taskbar_show_network: bool,
    pub taskbar_show_battery: bool,
    /// Whole date/time cluster; `taskbar_show_date` and `taskbar_show_seconds` apply inside it.
    pub taskbar_show_clock: bool,
    pub taskbar_show_workspaces: bool,
    pub taskbar_battery_only_when_relevant: bool,
    pub weather_enabled: bool,
    /// Encoded weather location (`lat\tlon\tcountry\tname`); empty means unset.
    pub weather_location: String,
    /// `celsius` or `fahrenheit`.
    pub weather_units: String,
    /// Sets `weatherLocation` from the IP-geolocated position (privacy-first: off by default).
    pub auto_location_enabled: bool,
    /// Applies the IP-geolocated IANA time zone to the shell and lock clocks.
    pub auto_time_zone_enabled: bool,
    /// Last IANA time zone from automatic lookup (offline-start cache); empty means none.
    pub auto_time_zone: String,
    pub surface_color: String,
    pub surface_opacity: u8,
    pub surface_blur_percent: u8,
    pub start_button_text: String,
    pub custom_start_icon: String,
    pub sticky_notes_enabled: bool,
    pub organize_downloads: bool,
    pub start_menu_color: String,
    pub start_menu_opacity: u8,
    pub start_menu_blur_percent: u8,
    pub accent_opacity: u8,
    pub decoration: String,
    pub titlebar_button_style: String,
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
    /// Cursor pixel size; one of 24, 32, 48, 64.
    pub cursor_size: u16,
    pub cursor_shadow: bool,
    pub animations_enabled: bool,
    pub animation_speed: u16,
    pub start_animation: AnimationStyle,
    pub start_animation_enabled: bool,
    pub minimize_animation: AnimationStyle,
    pub maximize_animation: AnimationStyle,
    pub window_open_animation: AnimationStyle,
    pub window_open_animation_enabled: bool,
    pub window_close_animation: AnimationStyle,
    pub minimize_animation_enabled: bool,
    pub maximize_animation_enabled: bool,
    pub lock_background_color: String,
    pub lock_background_image: String,
    /// Saved custom lock image path; used only when `lock_background_mode` is `Custom`.
    pub lock_background_mode: BackgroundMode,
    pub lock_background_blur_percent: u16,
    pub lock_background_dim_percent: u16,
    /// Lock-screen profile picture path; empty selects the built-in ring icon.
    pub avatar_image: String,
    pub lock_clock_format: LockClockFormat,
    pub lock_show_date: bool,
    pub lock_show_username: bool,
    pub lock_timeout_minutes: u16,
    pub lock_on_suspend: bool,
    pub screensaver_mode: ScreensaverMode,
    pub screensaver_after_lock_minutes: u16,
    pub screensaver_num_ribbons: u16,
    pub screensaver_ribbon_width_tenths: u16,
    pub screensaver_span_monitors: bool,
    pub screensaver_display_off_minutes: u16,
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
            icon_theme: "PlagueSur".to_owned(),
            appearance: AppearanceMode::Dark,
            wallpaper: String::new(),
            wallpaper_mode: BackgroundMode::Default,
            wallpaper_color: "#232629".to_owned(),
            wallpaper_fit: WallpaperFit::Fill,
            share_wallpaper: false,
            desktop_selection_fill_opacity: 20,
            desktop_entry_scale_percent: 100,
            window_snap_preview_fill_opacity: 20,
            snap_preview_enabled: true,
            show_content_while_dragging: false,
            focus_follows_mouse: false,
            font_family: "IBM Plex Sans".to_owned(),
            font_bold: false,
            font_size_offset: 0,
            taskbar_color: "#191b1d".to_owned(),
            taskbar_opacity: 50,
            taskbar_height: 44,
            taskbar_style: TaskbarStyle::Floating,
            taskbar_floating_margin: 4,
            taskbar_floating_radius: 12,
            taskbar_blur_percent: 80,
            taskbar_clock_format: ClockFormat::TwentyFour,
            taskbar_show_seconds: false,
            taskbar_show_date: true,
            taskbar_show_weather: true,
            taskbar_show_volume: true,
            taskbar_show_network: true,
            taskbar_show_battery: true,
            taskbar_show_clock: true,
            taskbar_show_workspaces: true,
            taskbar_battery_only_when_relevant: false,
            weather_enabled: true,
            weather_location: String::new(),
            weather_units: "celsius".to_owned(),
            auto_location_enabled: false,
            auto_time_zone_enabled: false,
            auto_time_zone: String::new(),
            surface_color: "#191b1d".to_owned(),
            surface_opacity: 50,
            surface_blur_percent: 80,
            start_button_text: String::new(),
            custom_start_icon: String::new(),
            sticky_notes_enabled: true,
            organize_downloads: true,
            start_menu_color: "#191b1d".to_owned(),
            start_menu_opacity: 50,
            start_menu_blur_percent: 80,
            accent_opacity: 100,
            decoration: "rounded".to_owned(),
            titlebar_button_style: "circle".to_owned(),
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
            cursor_size: 24,
            cursor_shadow: true,
            animations_enabled: true,
            animation_speed: 100,
            start_animation: AnimationStyle::Slide,
            start_animation_enabled: true,
            minimize_animation: AnimationStyle::MagicLamp,
            maximize_animation: AnimationStyle::None,
            window_open_animation: AnimationStyle::None,
            window_open_animation_enabled: false,
            window_close_animation: AnimationStyle::Fade,
            minimize_animation_enabled: true,
            maximize_animation_enabled: false,
            lock_background_color: "#232629".to_owned(),
            lock_background_image: String::new(),
            lock_background_mode: BackgroundMode::Default,
            lock_background_blur_percent: 40,
            lock_background_dim_percent: 25,
            avatar_image: String::new(),
            lock_clock_format: LockClockFormat::TwentyFour,
            lock_show_date: true,
            lock_show_username: true,
            lock_timeout_minutes: 0,
            lock_on_suspend: true,
            screensaver_mode: ScreensaverMode::Ribbons,
            screensaver_after_lock_minutes: 1,
            screensaver_num_ribbons: 6,
            screensaver_ribbon_width_tenths: 110,
            screensaver_span_monitors: false,
            screensaver_display_off_minutes: 20,
            per_output_scale: BTreeMap::new(),
            panel_edges: BTreeMap::new(),
            hotkeys: BTreeMap::new(),
            unknown_keys: BTreeMap::new(),
        }
    }
}

impl ProductSettings {
    /// Effective Start menu style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn start_animation_style(&self) -> AnimationStyle {
        gated_animation(self.start_animation_enabled, self.start_animation)
    }

    /// Effective minimize style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn minimize_animation_style(&self) -> AnimationStyle {
        gated_animation(self.minimize_animation_enabled, self.minimize_animation)
    }

    /// Effective maximize style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn maximize_animation_style(&self) -> AnimationStyle {
        gated_animation(self.maximize_animation_enabled, self.maximize_animation)
    }

    /// Effective window-open style. A disabled toggle or the `none` style both mean no animation.
    #[must_use]
    pub fn window_open_animation_style(&self) -> AnimationStyle {
        gated_animation(
            self.window_open_animation_enabled,
            self.window_open_animation,
        )
    }

    /// Taskbar backdrop from the shared surface policy. See [`Backdrop`] for the blur-to-radius mapping.
    #[must_use]
    pub fn taskbar_backdrop(&self) -> Backdrop {
        backdrop(self.surface_opacity, self.surface_blur_percent)
    }

    /// Start menu backdrop. See [`Backdrop`] for the blur-to-radius mapping.
    #[must_use]
    pub fn start_menu_backdrop(&self) -> Backdrop {
        backdrop(self.start_menu_opacity, self.start_menu_blur_percent)
    }

    /// Typed screensaver policy. Ribbon width converts tenths of a pixel to `f32` pixels.
    #[must_use]
    pub fn screensaver(&self) -> ScreensaverSettings {
        ScreensaverSettings {
            mode: self.screensaver_mode,
            after_lock: Duration::from_secs(u64::from(self.screensaver_after_lock_minutes) * 60),
            num_ribbons: u8::try_from(self.screensaver_num_ribbons).unwrap_or(u8::MAX),
            ribbon_width_px: f32::from(self.screensaver_ribbon_width_tenths) / 10.0,
            span_monitors: self.screensaver_span_monitors,
            display_off_minutes: self.screensaver_display_off_minutes,
        }
    }

    pub fn validate(&self) -> FlameResult<()> {
        validate_color(&self.taskbar_color, false, "taskbarColor")?;
        validate_color(&self.surface_color, false, "surfaceColor")?;
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
        if !(50..=200).contains(&self.desktop_entry_scale_percent) {
            return invalid("DesktopEntryScalePercent must be 50..200");
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
        if self.taskbar_floating_margin > 32 {
            return invalid("taskbarFloatingMargin must be 0..32");
        }
        if self.taskbar_floating_radius > 24 {
            return invalid("taskbarFloatingRadius must be 0..24");
        }
        if self.taskbar_blur_percent > 100 {
            return invalid("taskbarBlurPercent must be 0..100");
        }
        if self.surface_opacity > 100 {
            return invalid("surfaceOpacity must be 0..100");
        }
        if self.surface_blur_percent > 100 {
            return invalid("surfaceBlurPercent must be 0..100");
        }
        if self.start_menu_blur_percent > 100 {
            return invalid("startMenuBlurPercent must be 0..100");
        }
        if !matches!(self.cursor_size, 24 | 32 | 48 | 64) {
            return invalid("cursorSize must be 24, 32, 48 or 64");
        }
        validate_color(&self.start_menu_color, false, "startMenuColor")?;
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
        if !matches!(self.decoration.as_str(), "rounded" | "squared") {
            return invalid("decoration must be rounded or squared");
        }
        if !matches!(self.titlebar_button_style.as_str(), "circle" | "square") {
            return invalid("titlebarButtonStyle must be circle or square");
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
        validate_color(&self.wallpaper_color, false, "wallpaperColor")?;
        validate_color(&self.lock_background_color, false, "lockBackgroundColor")?;
        validate_lock_image_path(&self.lock_background_image, "lockBackgroundImage")?;
        validate_lock_image_path(&self.avatar_image, "avatarImage")?;
        if !matches!(self.weather_units.as_str(), "celsius" | "fahrenheit") {
            return invalid("weatherUnits must be celsius or fahrenheit");
        }
        if !self.auto_time_zone.is_empty() && !is_valid_time_zone_name(&self.auto_time_zone) {
            return invalid("autoTimeZone must be an IANA time zone name");
        }
        if self.lock_background_blur_percent > 100 {
            return invalid("lockBackgroundBlurPercent must be 0..100");
        }
        if self.lock_background_dim_percent > 80 {
            return invalid("lockBackgroundDimPercent must be 0..80");
        }
        if self.lock_timeout_minutes > 240 {
            return invalid("lockTimeoutMinutes must be 0..240");
        }
        if self.screensaver_after_lock_minutes > 60 {
            return invalid("screensaverAfterLockMinutes must be 0..60");
        }
        if !(1..=12).contains(&self.screensaver_num_ribbons) {
            return invalid("screensaverNumRibbons must be 1..12");
        }
        if !(20..=280).contains(&self.screensaver_ribbon_width_tenths) {
            return invalid("screensaverRibbonWidthTenths must be 20..280");
        }
        if self.screensaver_display_off_minutes > 240 {
            return invalid("screensaverDisplayOffMinutes must be 0..240");
        }
        for output in self.panel_edges.keys() {
            if !is_valid_output_id(output) {
                return invalid("panel.edge output id is invalid");
            }
        }
        for (output, scale) in &self.per_output_scale {
            if !is_valid_output_id(output) {
                return invalid("perOutputScale output id is invalid");
            }
            if !crate::scale::is_supported_scale(*scale) {
                return invalid("perOutputScale supports 100..=200 in 5% steps");
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
        let mut has_surface_key = false;
        let mut has_legacy_taskbar_surface_key = false;
        let mut has_wallpaper_mode = false;
        let mut has_lock_background_mode = false;
        let mut has_button_style = false;
        // Legacy per-button styles: read only, each reduced to "is square".
        let mut legacy_square_flags = Vec::new();
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
            has_surface_key |= matches!(
                key,
                "surfaceColor" | "surfaceOpacity" | "surfaceBlurPercent"
            );
            has_legacy_taskbar_surface_key |= matches!(
                key,
                "taskbarColor" | "taskbarOpacity" | "taskbarBlurPercent"
            );
            has_wallpaper_mode |= key == "wallpaperMode";
            has_lock_background_mode |= key == "lockBackgroundMode";
            has_button_style |= key == "titlebarButtonStyle";
            if let Some(row) = stable_setting(key) {
                (row.parse)(&mut snapshot, value, key)?;
                continue;
            }
            match key {
                "revision" => snapshot.revision = parse(value, "revision")?,
                // Legacy titlebar button styles: read only, never kept as unknown keys.
                "titlebarMinimizeStyle" | "titlebarMaximizeStyle" | "titlebarCloseStyle" => {
                    legacy_square_flags.push(legacy_button_style_is_square(value)?);
                }
                // Legacy titlebar background colour: tolerated and ignored, never stored or written.
                "titlebarColor" => {}
                // Legacy taskbar surface keys: read only, promoted to surface* below.
                "taskbarColor" => snapshot.taskbar_color = Slot::from_text(value, key)?,
                "taskbarOpacity" => snapshot.taskbar_opacity = Slot::from_text(value, key)?,
                "taskbarBlurPercent" => {
                    snapshot.taskbar_blur_percent = Slot::from_text(value, key)?;
                }
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
        if has_legacy_taskbar_surface_key && !has_surface_key {
            snapshot.surface_color = snapshot.taskbar_color.clone();
            snapshot.surface_opacity = snapshot.taskbar_opacity;
            snapshot.surface_blur_percent = snapshot.taskbar_blur_percent;
        }
        // Files written before the mode keys existed derive the mode from the saved values.
        if !has_wallpaper_mode {
            snapshot.wallpaper_mode = legacy_wallpaper_mode(&snapshot);
        }
        if !has_lock_background_mode {
            snapshot.lock_background_mode = legacy_lock_background_mode(&snapshot);
        }
        // One shared shape: square only when every present legacy button agrees; otherwise circle.
        if !has_button_style && !legacy_square_flags.is_empty() {
            let all_square = legacy_square_flags.iter().all(|square| *square);
            snapshot.titlebar_button_style =
                if all_square { "square" } else { "circle" }.to_owned();
        }
        if snapshot.decoration == "ios" {
            "rounded".clone_into(&mut snapshot.decoration);
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
            if matches!(row.key, "wallpaperMode" | "lockBackgroundMode") {
                continue;
            }
            let value = (row.get)(snapshot);
            if row.base || value != (row.get)(&defaults) {
                lines.push(format!("{}={}", row.key, wire_text(&value, row.base)));
            }
        }
        // A mode is written only when the legacy inference from the saved values would pick a
        // different one, so defaults stay silent and Default with a saved path still round-trips.
        if snapshot.wallpaper_mode != legacy_wallpaper_mode(snapshot) {
            lines.push(format!(
                "wallpaperMode={}",
                snapshot.wallpaper_mode.as_str()
            ));
        }
        if snapshot.lock_background_mode != legacy_lock_background_mode(snapshot) {
            lines.push(format!(
                "lockBackgroundMode={}",
                snapshot.lock_background_mode.as_str()
            ));
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
        write_atomic_text(path, &Self::serialize(snapshot)).map_err(io_error)
    }
}

/// Typed bridge between one `ProductSettings` field and its file text, API value, and apply input.
trait Slot: Sized + Clone {
    fn to_value(&self) -> SettingValue;
    fn from_text(text: &str, key: &str) -> FlameResult<Self>;
    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self>;
}

/// Text-valued slots read `SettingValue::Text` through their own `from_text`.
fn from_text_value<T: Slot>(value: &SettingValue, key: &str) -> FlameResult<T> {
    match value {
        SettingValue::Text(text) => T::from_text(text, key),
        _ => invalid("setting requires text"),
    }
}

impl Slot for String {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.clone())
    }

    fn from_text(text: &str, _key: &str) -> FlameResult<Self> {
        Ok(text.to_owned())
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        from_text_value(value, key)
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

fn gated_animation(enabled: bool, style: AnimationStyle) -> AnimationStyle {
    if enabled { style } else { AnimationStyle::None }
}

fn backdrop(opacity_percent: u8, blur_percent: u8) -> Backdrop {
    let radius_px = if opacity_percent >= 100 {
        0.0
    } else {
        f32::from(blur_percent) / 100.0 * 24.0
    };
    Backdrop {
        opacity_percent,
        blur_percent,
        radius_px,
    }
}

impl Slot for AnimationStyle {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.as_str().to_owned())
    }

    fn from_text(text: &str, key: &str) -> FlameResult<Self> {
        Self::parse_for(key, text).map_or_else(|| invalid_owned(format!("bad {key}")), Ok)
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        from_text_value(value, key)
    }
}

macro_rules! text_enum_slot {
    ($($ty:ty),*) => {$(
        impl Slot for $ty {
            fn to_value(&self) -> SettingValue {
                SettingValue::Text(self.as_str().to_owned())
            }

            fn from_text(text: &str, key: &str) -> FlameResult<Self> {
                Self::parse(text).map_or_else(|| invalid_owned(format!("bad {key}")), Ok)
            }

            fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
                from_text_value(value, key)
            }
        }
    )*};
}

text_enum_slot!(
    LockClockFormat,
    TaskbarStyle,
    BackgroundMode,
    WallpaperFit,
    ScreensaverMode
);

impl Slot for AppearanceMode {
    fn to_value(&self) -> SettingValue {
        SettingValue::Text(self.as_str().to_owned())
    }

    fn from_text(text: &str, _key: &str) -> FlameResult<Self> {
        AppearanceMode::parse(text).ok_or_else(|| FlameError::invalid("bad appearance"))
    }

    fn from_value(value: &SettingValue, key: &str) -> FlameResult<Self> {
        from_text_value(value, key)
    }
}

/// Accepts an IANA zone name such as `Europe/Berlin` or `UTC`: ASCII letters, digits, `_`, `+`,
/// `-`, `/`, no leading `/` or `.`, no `..`, at most 64 bytes. Existence is checked separately.
#[must_use]
pub fn is_valid_time_zone_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('/')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'/'))
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
    (false, desktop, "wallpaperMode", wallpaper_mode),
    (false, desktop, "wallpaperColor", wallpaper_color),
    (false, desktop, "shareWallpaper", share_wallpaper),
    (true, desktop, "DesktopSelectionFillOpacity", desktop_selection_fill_opacity),
    (true, desktop, "DesktopEntryScalePercent", desktop_entry_scale_percent),
    (true, windows, "WindowSnapPreviewFillOpacity", window_snap_preview_fill_opacity),
    (true, fonts, "fontFamily", font_family),
    (true, fonts, "fontBold", font_bold),
    (true, fonts, "fontSizeOffset", font_size_offset),
    (true, taskbar, "taskbarHeight", taskbar_height),
    (false, taskbar, "taskbarStyle", taskbar_style),
    (false, taskbar, "taskbarFloatingMargin", taskbar_floating_margin),
    (false, taskbar, "taskbarFloatingRadius", taskbar_floating_radius),
    (false, taskbar, "surfaceColor", surface_color),
    (false, taskbar, "surfaceOpacity", surface_opacity),
    (false, taskbar, "surfaceBlurPercent", surface_blur_percent),
    (true, taskbar, "startButtonText", start_button_text),
    (true, taskbar, "customStartIcon", custom_start_icon),
    (true, desktop, "stickyNotesEnabled", sticky_notes_enabled),
    (true, desktop, "organizeDownloads", organize_downloads),
    (false, appearance, "iconTheme", icon_theme),
    (false, taskbar, "startMenuColor", start_menu_color),
    (false, taskbar, "startMenuOpacity", start_menu_opacity),
    (false, taskbar, "startMenuBlurPercent", start_menu_blur_percent),
    (false, taskbar, "taskbarClockFormat", taskbar_clock_format),
    (false, taskbar, "taskbarShowSeconds", taskbar_show_seconds),
    (false, taskbar, "taskbarShowDate", taskbar_show_date),
    (false, taskbar, "taskbarShowWeather", taskbar_show_weather),
    (false, taskbar, "taskbarShowVolume", taskbar_show_volume),
    (false, taskbar, "taskbarShowNetwork", taskbar_show_network),
    (false, taskbar, "taskbarShowBattery", taskbar_show_battery),
    (false, taskbar, "taskbarShowClock", taskbar_show_clock),
    (false, taskbar, "taskbarShowWorkspaces", taskbar_show_workspaces),
    (false, taskbar, "taskbarBatteryOnlyWhenRelevant", taskbar_battery_only_when_relevant),
    (false, weather, "weatherEnabled", weather_enabled),
    (false, weather, "weatherLocation", weather_location),
    (false, weather, "weatherUnits", weather_units),
    (false, weather, "autoLocationEnabled", auto_location_enabled),
    (false, time, "autoTimeZoneEnabled", auto_time_zone_enabled),
    (false, time, "autoTimeZone", auto_time_zone),
    (false, desktop, "wallpaperFit", wallpaper_fit),
    (false, windows, "snapPreviewEnabled", snap_preview_enabled),
    (false, windows, "showContentWhileDragging", show_content_while_dragging),
    (false, windows, "focusFollowsMouse", focus_follows_mouse),
    (false, appearance, "cursorSize", cursor_size),
    (false, appearance, "cursorShadow", cursor_shadow),
    (false, appearance, "accentOpacity", accent_opacity),
    (false, appearance, "decoration", decoration),
    (false, windows, "titlebarButtonStyle", titlebar_button_style),
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
    (false, windows, "minimizeAnimation", minimize_animation),
    (false, windows, "maximizeAnimation", maximize_animation),
    (false, windows, "windowOpenAnimation", window_open_animation),
    (false, windows, "windowOpenAnimationEnabled", window_open_animation_enabled),
    (false, windows, "windowCloseAnimation", window_close_animation),
    (false, windows, "minimizeAnimationEnabled", minimize_animation_enabled),
    (false, windows, "maximizeAnimationEnabled", maximize_animation_enabled),
    (false, lock, "lockBackgroundColor", lock_background_color),
    (false, lock, "lockBackgroundImage", lock_background_image),
    (false, lock, "lockBackgroundMode", lock_background_mode),
    (false, lock, "lockBackgroundBlurPercent", lock_background_blur_percent),
    (false, lock, "lockBackgroundDimPercent", lock_background_dim_percent),
    (false, appearance, "avatarImage", avatar_image),
    (false, lock, "lockClockFormat", lock_clock_format),
    (false, lock, "lockShowDate", lock_show_date),
    (false, lock, "lockShowUsername", lock_show_username),
    (false, lock, "lockTimeoutMinutes", lock_timeout_minutes),
    (false, lock, "lockOnSuspend", lock_on_suspend),
    (false, screensaver, "screensaverMode", screensaver_mode),
    (false, screensaver, "screensaverAfterLockMinutes", screensaver_after_lock_minutes),
    (false, screensaver, "screensaverNumRibbons", screensaver_num_ribbons),
    (false, screensaver, "screensaverRibbonWidthTenths", screensaver_ribbon_width_tenths),
    (false, screensaver, "screensaverSpanMonitors", screensaver_span_monitors),
    (false, screensaver, "screensaverDisplayOffMinutes", screensaver_display_off_minutes),
}

#[must_use]
pub(crate) fn stable_setting(key: &str) -> Option<&'static StableSetting> {
    let key = if key == "wallpaperShare" {
        "shareWallpaper"
    } else {
        key
    };
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

fn is_valid_output_id(output: &str) -> bool {
    !output.is_empty()
        && !output
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '=')
}

fn validate_color(value: &str, allow_empty: bool, name: &str) -> FlameResult<()> {
    if (allow_empty && value.is_empty()) || parse_hex_rgb(value).is_some() {
        return Ok(());
    }
    invalid_owned(format!(
        "{name} must be #RRGGBB{}",
        if allow_empty { " or empty" } else { "" }
    ))
}

/// Empty (built-in default) or an absolute path of at most 4096 bytes without control characters.
fn validate_lock_image_path(value: &str, name: &str) -> FlameResult<()> {
    if value.len() > 4096
        || !(value.is_empty() || value.starts_with('/'))
        || value.chars().any(char::is_control)
    {
        return invalid_owned(format!(
            "{name} must be empty or an absolute path of at most 4096 bytes"
        ));
    }
    Ok(())
}

/// Desktop mode implied by saved values alone: a saved path means custom, otherwise bundled.
/// Old per-button style values: `square` is square, `default|flat|circle` are circle.
fn legacy_button_style_is_square(value: &str) -> FlameResult<bool> {
    match value {
        "square" => Ok(true),
        "default" | "flat" | "circle" => Ok(false),
        _ => invalid("titlebar button style must be default/flat/circle/square"),
    }
}

fn legacy_wallpaper_mode(snapshot: &ProductSettings) -> BackgroundMode {
    if snapshot.wallpaper.is_empty() {
        BackgroundMode::Default
    } else {
        BackgroundMode::Custom
    }
}

/// Lock mode implied by saved values alone: a saved path means custom, a non-default colour
/// means solid, and otherwise the bundled image.
fn legacy_lock_background_mode(snapshot: &ProductSettings) -> BackgroundMode {
    if !snapshot.lock_background_image.is_empty() {
        BackgroundMode::Custom
    } else if snapshot.lock_background_color != ProductSettings::default().lock_background_color {
        BackgroundMode::Solid
    } else {
        BackgroundMode::Default
    }
}

fn is_stable_key(key: &str) -> bool {
    key == "revision"
        || stable_setting(key).is_some()
        || key.starts_with("scale.")
        || key.starts_with("panel.edge.")
        || key.starts_with("hotkey.")
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

    fn load(text: &str) -> FlameResult<ProductSettings> {
        ConfigStore::default().parse_snapshot(text)
    }

    /// Serializes `parsed`, asserts it reloads to the same settings, and returns the text.
    fn assert_round_trip(parsed: &ProductSettings) -> String {
        let text = ConfigStore::serialize(parsed);
        assert_eq!(load(&text).expect("round trip"), *parsed);
        text
    }

    /// Each line, set on an otherwise default file, must be rejected as `InvalidArgument`.
    fn assert_rejected<S: AsRef<str>>(lines: &[S]) {
        for line in lines {
            let line = line.as_ref();
            assert_eq!(
                load(&format!("revision=2\n{line}\n")).expect_err(line).code,
                ErrorCode::InvalidArgument,
                "{line} must be rejected"
            );
        }
    }

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
    fn unset_icon_theme_defaults_to_plaguesur_and_writes_nothing() {
        let parsed = load("revision=2\n").expect("valid snapshot");
        assert_eq!(parsed.icon_theme, "PlagueSur");
        assert!(!ConfigStore::serialize(&parsed).contains("iconTheme"));
    }

    #[test]
    fn legacy_explicit_mcmojave_icon_theme_is_kept_verbatim() {
        let parsed = load("revision=2\niconTheme=McMojave-circle\n").expect("legacy icon theme");
        assert_eq!(parsed.icon_theme, "McMojave-circle");
        assert!(ConfigStore::serialize(&parsed).contains("iconTheme=McMojave-circle\n"));
    }

    #[test]
    fn explicit_icon_theme_is_kept_and_written() {
        let parsed = load("revision=2\niconTheme=*:-HighContrast\n").expect("explicit icon theme");
        assert_eq!(parsed.icon_theme, "*:-HighContrast");
        let text = assert_round_trip(&parsed);
        assert!(text.contains("iconTheme=*:-HighContrast\n"));
    }

    #[test]
    fn appearance_snapshot_round_trip_defaults_to_dark() {
        let parsed = load("revision=2\n").expect("valid snapshot");
        assert_eq!(parsed.appearance, AppearanceMode::Dark);
        let text = ConfigStore::serialize(&ProductSettings::default());
        assert!(text.contains("appearance=dark\n"));
        let reparsed = load(&text).expect("round trip");
        assert_eq!(reparsed.appearance, AppearanceMode::Dark);
    }

    #[test]
    fn invalid_appearance_value_is_rejected() {
        assert_eq!(
            load("revision=2\nappearance=neon\n")
                .expect_err("bad appearance")
                .code,
            ErrorCode::InvalidArgument
        );
    }

    #[test]
    fn parser_preserves_unknown_compatible_keys() {
        let parsed = load("revision=2\nfutureFeature=value\n").expect("valid future key");
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
    fn five_percent_scales_persist_without_quantization() {
        for percent in (100..=200).step_by(5) {
            let mut settings = ProductSettings::default();
            settings
                .per_output_scale
                .insert("eDP-1".to_owned(), percent);
            settings.validate().expect("valid five-percent scale");
            let saved = ConfigStore::serialize(&settings);
            assert!(saved.contains(&format!("scale.eDP-1={percent}\n")));
            let parsed = load(&saved).expect("saved scale is readable");
            assert_eq!(parsed.per_output_scale.get("eDP-1"), Some(&percent));
        }
        for percent in [99, 101, 104, 106, 133, 199, 201] {
            let mut settings = ProductSettings::default();
            settings
                .per_output_scale
                .insert("eDP-1".to_owned(), percent);
            assert!(settings.validate().is_err(), "{percent} cannot persist");
        }
    }

    #[test]
    fn cursor_shadow_defaults_to_true_and_round_trips() {
        let defaults = ProductSettings::default();
        assert!(defaults.cursor_shadow);
        let mut disabled = defaults;
        disabled.cursor_shadow = false;
        let saved = ConfigStore::serialize(&disabled);
        assert!(saved.contains("cursorShadow=false\n"));
        assert!(!load(&saved).expect("reload cursorShadow").cursor_shadow);
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
    fn avatar_image_defaults_empty_and_round_trips() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.avatar_image, "");
        assert!(!ConfigStore::serialize(&defaults).contains("avatarImage"));
        let parsed = load("revision=2\navatarImage=/home/u/.local/share/flamewm/wallpapers/avatar-0123456789abcdef.png\n")
            .expect("absolute avatar path");
        assert_eq!(
            parsed.avatar_image,
            "/home/u/.local/share/flamewm/wallpapers/avatar-0123456789abcdef.png"
        );
        let text = assert_round_trip(&parsed);
        assert!(text.contains("avatarImage=/home/u/"));
    }

    #[test]
    fn invalid_avatar_image_paths_are_rejected() {
        let long_path = format!("/{}", "a".repeat(4096));
        assert_rejected(&[
            "avatarImage=face.png".to_owned(),
            "avatarImage=images/face.png".to_owned(),
            "avatarImage=/tmp/a\tb.png".to_owned(),
            format!("avatarImage={long_path}"),
        ]);
    }

    #[test]
    fn background_modes_migrate_from_legacy_values_when_keys_are_absent() {
        let legacy_wallpaper =
            load("revision=2\nwallpaper=/home/u/a.png\n").expect("legacy wallpaper");
        assert_eq!(legacy_wallpaper.wallpaper_mode, BackgroundMode::Custom);
        let empty_wallpaper = load("revision=2\n").expect("empty wallpaper");
        assert_eq!(empty_wallpaper.wallpaper_mode, BackgroundMode::Default);
        let custom_lock =
            load("revision=2\nlockBackgroundImage=/home/u/bg.png\n").expect("custom lock");
        assert_eq!(custom_lock.lock_background_mode, BackgroundMode::Custom);
        let solid_lock = load("revision=2\nlockBackgroundColor=#102030\n").expect("solid lock");
        assert_eq!(solid_lock.lock_background_mode, BackgroundMode::Solid);
        let default_lock = load("revision=2\n").expect("default lock");
        assert_eq!(default_lock.lock_background_mode, BackgroundMode::Default);
    }

    #[test]
    fn explicit_background_mode_wins_over_saved_path_and_keeps_it() {
        let parsed = load(
            "revision=2\nwallpaperMode=default\nwallpaper=/home/u/a.png\n\
                 lockBackgroundMode=solid\nlockBackgroundImage=/home/u/bg.png\n",
        )
        .expect("explicit modes");
        assert_eq!(parsed.wallpaper_mode, BackgroundMode::Default);
        assert_eq!(parsed.wallpaper, "/home/u/a.png");
        assert_eq!(parsed.lock_background_mode, BackgroundMode::Solid);
        assert_eq!(parsed.lock_background_image, "/home/u/bg.png");
    }

    #[test]
    fn background_modes_round_trip_and_clear_differs_from_default() {
        for mode in [
            BackgroundMode::Default,
            BackgroundMode::Solid,
            BackgroundMode::Custom,
        ] {
            let mut snapshot = ProductSettings::default();
            snapshot.revision = 9;
            snapshot.wallpaper = "/home/u/a.png".to_owned();
            snapshot.wallpaper_mode = mode;
            snapshot.lock_background_image = "/home/u/bg.png".to_owned();
            snapshot.lock_background_mode = mode;
            let text = ConfigStore::serialize(&snapshot);
            let parsed = load(&text).expect("round trip");
            assert_eq!(parsed.wallpaper_mode, mode);
            assert_eq!(parsed.lock_background_mode, mode);
            assert_eq!(parsed.wallpaper, "/home/u/a.png");
            assert_eq!(parsed.lock_background_image, "/home/u/bg.png");
        }
        let mut clear = ProductSettings::default();
        clear.lock_background_mode = BackgroundMode::Solid;
        let mut default = ProductSettings::default();
        default.lock_background_mode = BackgroundMode::Default;
        assert_ne!(
            ConfigStore::serialize(&clear),
            ConfigStore::serialize(&default),
            "Clear (solid) must not persist the same output as Default"
        );
    }

    #[test]
    fn invalid_background_mode_and_wallpaper_color_are_rejected() {
        for text in [
            "wallpaperMode=clear\n",
            "lockBackgroundMode=Custom\n",
            "wallpaperColor=red\n",
        ] {
            assert!(load(text).is_err(), "{text} must be rejected");
        }
    }

    #[test]
    fn lock_background_blur_and_dim_defaults_write_nothing() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.lock_background_blur_percent, 40);
        assert_eq!(defaults.lock_background_dim_percent, 25);
        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("lockBackgroundBlurPercent"));
        assert!(!text.contains("lockBackgroundDimPercent"));
    }

    #[test]
    fn lock_background_blur_and_dim_legacy_file_loads_defaults() {
        let parsed = load("revision=1\nlockBackgroundColor=#102030\n").expect("legacy lock file");
        assert_eq!(parsed.lock_background_blur_percent, 40);
        assert_eq!(parsed.lock_background_dim_percent, 25);
    }

    #[test]
    fn lock_background_blur_and_dim_round_trip_bounds() {
        let parsed =
            load("revision=2\nlockBackgroundBlurPercent=100\nlockBackgroundDimPercent=80\n")
                .expect("bounds are valid");
        assert_eq!(parsed.lock_background_blur_percent, 100);
        assert_eq!(parsed.lock_background_dim_percent, 80);
        let text = assert_round_trip(&parsed);
        assert!(text.contains("lockBackgroundBlurPercent=100\n"));
        assert!(text.contains("lockBackgroundDimPercent=80\n"));

        let zero = load("revision=2\nlockBackgroundBlurPercent=0\nlockBackgroundDimPercent=0\n")
            .expect("zero is valid");
        assert_eq!(zero.lock_background_blur_percent, 0);
        assert_eq!(zero.lock_background_dim_percent, 0);
    }

    #[test]
    fn invalid_lock_background_blur_and_dim_values_are_rejected() {
        assert_rejected(&[
            "lockBackgroundBlurPercent=101",
            "lockBackgroundBlurPercent=-1",
            "lockBackgroundBlurPercent=40.5",
            "lockBackgroundDimPercent=81",
            "lockBackgroundDimPercent=-1",
            "lockBackgroundDimPercent=25%",
        ]);
    }

    #[test]
    fn lock_screen_defaults_match_the_contract_and_write_nothing() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.lock_background_color, "#232629");
        assert_eq!(defaults.lock_background_image, "");
        assert_eq!(defaults.lock_clock_format, LockClockFormat::TwentyFour);
        assert!(defaults.lock_show_date);
        assert!(defaults.lock_show_username);
        assert_eq!(defaults.lock_timeout_minutes, 0);
        assert!(defaults.lock_on_suspend);
        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("lock"));
    }

    #[test]
    fn lock_screen_round_trips_non_default_values() {
        let parsed = load(
            "revision=2\nlockBackgroundColor=#102030\nlockBackgroundImage=/usr/share/bg.png\n\
                 lockClockFormat=12h\nlockShowDate=false\nlockShowUsername=0\n\
                 lockTimeoutMinutes=240\nlockOnSuspend=false\n",
        )
        .expect("valid lock settings");
        assert_eq!(parsed.lock_background_color, "#102030");
        assert_eq!(parsed.lock_background_image, "/usr/share/bg.png");
        assert_eq!(parsed.lock_clock_format, LockClockFormat::Twelve);
        assert!(!parsed.lock_show_date);
        assert!(!parsed.lock_show_username);
        assert_eq!(parsed.lock_timeout_minutes, 240);
        assert!(!parsed.lock_on_suspend);
        let text = assert_round_trip(&parsed);
        assert!(text.contains("lockClockFormat=12h\n"));
        assert!(text.contains("lockShowDate=false\n"));
    }

    #[test]
    fn invalid_lock_screen_values_are_rejected() {
        let long_path = format!("/{}", "a".repeat(4096));
        assert_rejected(&[
            "lockBackgroundColor=red".to_owned(),
            "lockBackgroundColor=#12345".to_owned(),
            "lockClockFormat=13h".to_owned(),
            "lockClockFormat=24H".to_owned(),
            "lockTimeoutMinutes=241".to_owned(),
            "lockTimeoutMinutes=-5".to_owned(),
            "lockTimeoutMinutes=5.5".to_owned(),
            "lockBackgroundImage=wallpaper.png".to_owned(),
            "lockBackgroundImage=images/bg.png".to_owned(),
            "lockBackgroundImage=/tmp/a\tb.png".to_owned(),
            format!("lockBackgroundImage={long_path}"),
            "lockShowDate=maybe".to_owned(),
        ]);
    }

    #[test]
    fn lock_timeout_zero_means_never_and_is_accepted() {
        let parsed = load("revision=2\nlockTimeoutMinutes=0\n").expect("zero timeout is never");
        assert_eq!(parsed.lock_timeout_minutes, 0);
    }

    #[test]
    fn taskbar_style_and_backdrop_defaults_write_nothing() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.taskbar_style, TaskbarStyle::Floating);
        assert_eq!(defaults.taskbar_floating_margin, 4);
        assert_eq!(defaults.taskbar_floating_radius, 12);
        assert_eq!(defaults.taskbar_blur_percent, 80);
        assert_eq!(defaults.start_menu_blur_percent, 80);
        let taskbar = defaults.taskbar_backdrop();
        assert_eq!(taskbar.opacity_percent, 50);
        assert_eq!(taskbar.blur_percent, 80);
        assert!((taskbar.radius_px - 19.2).abs() < 1e-4);
        let start = defaults.start_menu_backdrop();
        assert_eq!(start.opacity_percent, 50);
        assert!((start.radius_px - 19.2).abs() < 1e-4);
        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("taskbarStyle"));
        assert!(!text.contains("taskbarBlurPercent"));
        assert!(!text.contains("startMenuBlurPercent"));
        assert!(!text.contains("taskbarFloating"));
    }

    #[test]
    fn taskbar_style_and_blur_round_trip_and_opaque_has_no_radius() {
        let parsed = load(
            "revision=2\ntaskbarStyle=classic\ntaskbarFloatingMargin=32\n\
                 taskbarFloatingRadius=0\nsurfaceBlurPercent=100\nsurfaceOpacity=100\n\
                 startMenuBlurPercent=0\nstartMenuOpacity=50\n",
        )
        .expect("valid taskbar settings");
        assert_eq!(parsed.taskbar_style, TaskbarStyle::Classic);
        assert_eq!(parsed.taskbar_floating_margin, 32);
        assert_eq!(parsed.taskbar_floating_radius, 0);
        assert!((parsed.taskbar_backdrop().radius_px - 0.0).abs() < 1e-6);
        assert!((parsed.start_menu_backdrop().radius_px - 0.0).abs() < 1e-6);
        let text = assert_round_trip(&parsed);
        assert!(text.contains("taskbarStyle=classic\n"));
        assert!(text.contains("surfaceBlurPercent=100\n"));
    }

    #[test]
    fn invalid_taskbar_style_and_blur_values_are_rejected() {
        assert_rejected(&[
            "taskbarStyle=Floating",
            "taskbarStyle=bar",
            "taskbarFloatingMargin=33",
            "taskbarFloatingMargin=-1",
            "taskbarFloatingRadius=25",
            "taskbarBlurPercent=101",
            "taskbarBlurPercent=80.5",
            "startMenuBlurPercent=101",
        ]);
    }

    #[test]
    fn screensaver_defaults_match_contract_and_write_nothing() {
        let defaults = ProductSettings::default();
        assert_eq!(
            defaults.screensaver(),
            ScreensaverSettings {
                mode: ScreensaverMode::Ribbons,
                after_lock: Duration::from_secs(60),
                num_ribbons: 6,
                ribbon_width_px: 11.0,
                span_monitors: false,
                display_off_minutes: 20,
            }
        );
        // lockTimeoutMinutes stays 0 (manual lock) so upgrades never lock unexpectedly.
        assert_eq!(defaults.lock_timeout_minutes, 0);
        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("screensaver"));
    }

    #[test]
    fn legacy_file_without_screensaver_keys_loads_defaults() {
        let parsed = load("revision=1\nlockTimeoutMinutes=5\nlockOnSuspend=false\n")
            .expect("legacy lock file");
        assert_eq!(
            parsed.screensaver(),
            ProductSettings::default().screensaver()
        );
        assert_eq!(parsed.lock_timeout_minutes, 5);
    }

    #[test]
    fn screensaver_round_trips_non_default_values() {
        let parsed = load(
            "revision=2\nscreensaverMode=none\nscreensaverAfterLockMinutes=0\n\
                 screensaverNumRibbons=12\nscreensaverRibbonWidthTenths=280\n\
                 screensaverSpanMonitors=true\nscreensaverDisplayOffMinutes=240\n",
        )
        .expect("valid screensaver settings");
        assert_eq!(
            parsed.screensaver(),
            ScreensaverSettings {
                mode: ScreensaverMode::None,
                after_lock: Duration::ZERO,
                num_ribbons: 12,
                ribbon_width_px: 28.0,
                span_monitors: true,
                display_off_minutes: 240,
            }
        );
        let text = assert_round_trip(&parsed);
        assert!(text.contains("screensaverMode=none\n"));
        assert!(text.contains("screensaverSpanMonitors=true\n"));
    }

    #[test]
    fn invalid_screensaver_values_are_rejected() {
        assert_rejected(&[
            "screensaverMode=Ribbons",
            "screensaverMode=blank",
            "screensaverAfterLockMinutes=61",
            "screensaverAfterLockMinutes=-1",
            "screensaverNumRibbons=0",
            "screensaverNumRibbons=13",
            "screensaverRibbonWidthTenths=19",
            "screensaverRibbonWidthTenths=281",
            "screensaverRibbonWidthTenths=11.0",
            "screensaverDisplayOffMinutes=241",
            "screensaverSpanMonitors=maybe",
        ]);
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
    fn persisted_settings_are_exactly_the_serialized_bytes() {
        let dir = std::env::temp_dir().join(format!("flamewm-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        let path = dir.join("settings.conf");
        let mut settings = ProductSettings::default();
        settings.revision = 9;
        settings
            .unknown_keys
            .insert("extension.key".into(), "value".into());
        let expected = ConfigStore::serialize(&settings);

        ConfigStore::persist(&path, &settings).expect("persist settings");

        assert_eq!(std::fs::read(path).unwrap(), expected.as_bytes());
    }

    #[test]
    fn older_settings_without_new_fields_keep_defaults() {
        let settings = load("revision=4\naccent=#112233\n").expect("legacy settings");
        assert_eq!(settings.revision, 4);
        assert_eq!(settings.accent, "#112233");
        assert!(settings.cursor_shadow);
    }

    #[test]
    fn new_appearance_keys_default_and_stay_unwritten_when_unchanged() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.start_menu_color, "#191b1d");
        assert_eq!(defaults.start_menu_opacity, 50);
        assert_eq!(defaults.taskbar_opacity, 50);
        assert_eq!(defaults.accent_opacity, 100);
        assert_eq!(defaults.decoration, "rounded");
        assert_eq!(defaults.titlebar_button_style, "circle");
        assert!(defaults.titlebar_close_color.is_empty());
        assert!(defaults.cursor_theme.is_empty());
        assert_eq!(defaults.animation_speed, 100);
        assert_eq!(defaults.start_animation, AnimationStyle::Slide);
        assert_eq!(defaults.minimize_animation, AnimationStyle::MagicLamp);
        assert_eq!(defaults.maximize_animation, AnimationStyle::None);
        assert_eq!(defaults.window_open_animation, AnimationStyle::None);
        assert_eq!(defaults.window_close_animation, AnimationStyle::Fade);
        assert!(!defaults.maximize_animation_enabled);

        let text = ConfigStore::serialize(&defaults);
        assert!(!text.contains("startMenuColor"));
        assert!(!text.contains("titlebar"));
        assert!(!text.contains("animation"));
    }

    #[test]
    fn old_settings_file_parses_to_defaults_and_serializes_unchanged() {
        let old = "revision=3\naccent=#EF4048\ntaskbarColor=#191b1d\nfutureFeature=kept\n";
        let parsed = load(old).expect("old file");
        assert_eq!(parsed.animation_speed, 100);
        let text = assert_round_trip(&parsed);
        assert!(!text.contains("titlebar"));
        assert!(text.contains("futureFeature=kept\n"));
    }

    #[test]
    fn legacy_titlebar_color_is_tolerated_ignored_and_never_written() {
        // Old files carry titlebarColor; even an invalid value must not error or touch surface*.
        for value in ["#A0B0C0", "#12345"] {
            let legacy = load(&format!(
                "revision=3\ntitlebarColor={value}\nsurfaceColor=#445566\n"
            ))
            .expect("legacy titlebarColor tolerated");
            assert_eq!(legacy.surface_color, "#445566");
            assert_eq!(
                legacy.surface_opacity,
                ProductSettings::default().surface_opacity
            );
            let text = ConfigStore::serialize(&legacy);
            assert!(!text.contains("titlebarColor"), "{text}");
            assert!(
                !legacy.unknown_keys.contains_key("titlebarColor"),
                "legacy key is ignored, not kept as unknown"
            );
        }
        assert!(stable_setting("titlebarColor").is_none());
    }

    #[test]
    fn new_appearance_keys_round_trip_when_set() {
        let text = "revision=2\nstartMenuColor=#112233\nstartMenuOpacity=80\naccentOpacity=40\n\
            decoration=ios\n\
            titlebarMinimizeHoverColor=#FFFFFF\ncursorTheme=Adwaita\nanimationsEnabled=false\n\
            titlebarButtonStyle=square\n\
            animationSpeed=150\nstartAnimationEnabled=0\nfutureFeature=kept\n";
        let parsed = load(text).expect("valid new keys");
        assert_eq!(parsed.start_menu_color, "#112233");
        assert_eq!(parsed.start_menu_opacity, 80);
        assert_eq!(parsed.accent_opacity, 40);
        assert_eq!(parsed.decoration, "rounded");
        assert_eq!(parsed.titlebar_button_style, "square");
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
        assert_eq!(load(&serialized).expect("round trip"), parsed);
    }

    #[test]
    fn surface_defaults_and_legacy_taskbar_surface_promotes() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.surface_color, "#191b1d");
        assert_eq!(defaults.surface_opacity, 50);
        assert_eq!(defaults.surface_blur_percent, 80);
        assert!(!ConfigStore::serialize(&defaults).contains("surface"));

        let legacy =
            load("revision=2\ntaskbarColor=#112233\ntaskbarOpacity=70\ntaskbarBlurPercent=30\n")
                .expect("legacy taskbar surface");
        assert_eq!(legacy.surface_color, "#112233");
        assert_eq!(legacy.surface_opacity, 70);
        assert_eq!(legacy.surface_blur_percent, 30);
        assert_eq!(legacy.taskbar_color, "#112233");

        let explicit =
            load("revision=3\nsurfaceColor=#445566\nsurfaceOpacity=10\nsurfaceBlurPercent=5\n")
                .expect("explicit surface");
        assert_eq!(explicit.surface_color, "#445566");
        assert_eq!(explicit.surface_opacity, 10);
        assert_eq!(explicit.surface_blur_percent, 5);

        // Legacy taskbar* keys alongside surface* are tolerated on read and ignored for surface values.
        let tolerated = load(
            "revision=3\ntaskbarColor=#112233\ntaskbarOpacity=70\ntaskbarBlurPercent=30\n\
                 surfaceColor=#445566\nsurfaceOpacity=10\nsurfaceBlurPercent=5\n",
        )
        .expect("legacy taskbar keys tolerated");
        assert_eq!(tolerated.surface_color, "#445566");
        assert_eq!(tolerated.surface_opacity, 10);
        assert_eq!(tolerated.surface_blur_percent, 5);

        // Live taskbar backdrop follows surface*, never the legacy taskbar* fields.
        let split = load("revision=5\ntaskbarOpacity=100\ntaskbarBlurPercent=100\nsurfaceOpacity=10\nsurfaceBlurPercent=5\n")
            .expect("surface keys win over legacy taskbar keys");
        let backdrop = split.taskbar_backdrop();
        assert_eq!(backdrop.opacity_percent, 10);
        assert_eq!(backdrop.blur_percent, 5);

        // Legacy keys are read but never written; the surface pair carries the value forward.
        let migrated =
            load("revision=2\ntaskbarColor=#112233\ntaskbarOpacity=70\ntaskbarBlurPercent=30\n")
                .expect("legacy taskbar surface");
        let text = ConfigStore::serialize(&migrated);
        assert!(!text.contains("taskbarColor"));
        assert!(!text.contains("taskbarOpacity"));
        assert!(!text.contains("taskbarBlurPercent"));
        assert!(text.contains("surfaceColor=#112233\n"));
        assert!(text.contains("surfaceOpacity=70\n"));
        assert!(text.contains("surfaceBlurPercent=30\n"));
        assert!(stable_setting("taskbarOpacity").is_none());

        let round = load(&ConfigStore::serialize(&explicit)).expect("round trip");
        assert_eq!(round, explicit);
        assert!(load("revision=4\nsurfaceOpacity=101\n").is_err());
        assert!(load("revision=4\nsurfaceColor=red\n").is_err());
        assert!(load("revision=4\nsurfaceBlurPercent=101\n").is_err());
    }

    #[test]
    fn decoration_ios_parses_as_rounded_and_squared_is_accepted() {
        let ios = load("revision=2\ndecoration=ios\n").expect("legacy ios");
        assert_eq!(ios.decoration, "rounded");
        let squared = load("revision=2\ndecoration=squared\n").expect("squared");
        assert_eq!(squared.decoration, "squared");
        assert!(load("revision=2\ndecoration=neon\n").is_err());
    }

    #[test]
    fn floating_margin_defaults_to_four_and_stored_value_is_kept() {
        assert_eq!(
            load("revision=2\n").expect("unset").taskbar_floating_margin,
            4
        );
        let legacy = load("revision=2\ntaskbarFloatingMargin=8\n").expect("stored 8");
        assert_eq!(legacy.taskbar_floating_margin, 8);
    }

    #[test]
    fn legacy_titlebar_button_styles_map_to_one_shape_and_are_never_written() {
        let parse = |body: &str| {
            load(&format!("revision=2\n{body}\n")).expect("legacy titlebar styles parse")
        };
        let all_square = "titlebarMinimizeStyle=square\ntitlebarMaximizeStyle=square\n\
            titlebarCloseStyle=square";
        let parsed = parse(all_square);
        assert_eq!(parsed.titlebar_button_style, "square");
        assert_eq!(
            parse("titlebarMinimizeStyle=default\ntitlebarMaximizeStyle=flat\ntitlebarCloseStyle=circle")
                .titlebar_button_style,
            "circle"
        );
        assert_eq!(
            parse("titlebarCloseStyle=square\ntitlebarMinimizeStyle=circle").titlebar_button_style,
            "circle"
        );
        assert_eq!(
            parse("titlebarCloseStyle=square").titlebar_button_style,
            "square"
        );
        assert_eq!(
            parse("titlebarButtonStyle=circle\ntitlebarCloseStyle=square").titlebar_button_style,
            "circle"
        );
        let text = assert_round_trip(&parsed);
        assert!(text.contains("titlebarButtonStyle=square\n"));
        assert!(!text.contains("titlebarMinimizeStyle"));
        assert!(!text.contains("titlebarCloseStyle"));
        assert!(!text.contains("titlebarMaximizeStyle"));
    }

    #[test]
    fn invalid_new_appearance_values_are_rejected() {
        assert_rejected(&[
            "startMenuColor=red",
            "titlebarCloseColor=#GGGGGG",
            "startMenuOpacity=101",
            "accentOpacity=101",
            "decoration=neon",
            "titlebarMinimizeStyle=round",
            "titlebarButtonStyle=round",
            "cursorTheme=../evil",
            "cursorTheme=a\\b",
            "animationSpeed=24",
            "animationSpeed=201",
            "startAnimation=bounce",
            "animationsEnabled=maybe",
        ]);
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
        let reparsed = load(&text).expect("round trip");
        assert_eq!(reparsed, settings);
    }

    #[test]
    fn legacy_slide_up_migrates_to_slide_and_stays_unwritten_at_default() {
        let parsed = load("revision=2\nstartAnimation=slide-up\n").expect("legacy alias");
        assert_eq!(parsed.start_animation, AnimationStyle::Slide);
        assert_eq!(
            Slot::to_value(&parsed.start_animation),
            SettingValue::Text("slide".to_owned())
        );
        let serialized = ConfigStore::serialize(&parsed);
        assert!(
            !serialized.contains("startAnimation"),
            "default style stays unwritten"
        );
        assert!(!serialized.contains("slide-up"));
    }

    #[test]
    fn animation_style_values_are_validated_per_key() {
        for line in [
            "startAnimation=none",
            "startAnimation=slide",
            "startAnimation=fade",
            "startAnimation=magic-lamp",
            "minimizeAnimation=none",
            "minimizeAnimation=fade",
            "minimizeAnimation=magic-lamp",
            "maximizeAnimation=none",
            "maximizeAnimation=fade",
            "maximizeAnimation=magic-lamp", // legacy spelling is accepted as Scale
            "maximizeAnimation=scale",
            "windowOpenAnimation=none",
            "windowOpenAnimation=scale",
            "windowOpenAnimation=fade",
            "windowCloseAnimation=none",
            "windowCloseAnimation=fade",
        ] {
            load(&format!("revision=2\n{line}\n"))
                .unwrap_or_else(|error| panic!("{line} must be accepted: {error:?}"));
        }
        assert_rejected(&[
            "startAnimation=bounce",
            "startAnimation=scale",
            "startAnimation=slide-up-x",
            "minimizeAnimation=slide",
            "minimizeAnimation=slide-up",
            "minimizeAnimation=scale",
            "maximizeAnimation=slide",
            "maximizeAnimation=",
            "windowOpenAnimation=slide",
            "windowOpenAnimation=magic-lamp",
            "windowOpenAnimation=Scale",
            "windowCloseAnimation=scale",
            "windowCloseAnimation=magic-lamp",
            "windowCloseAnimation=Fade",
            "windowCloseAnimation=slide",
        ]);
    }

    #[test]
    fn animation_style_precedence_treats_disabled_or_none_as_none() {
        let mut settings = ProductSettings::default();
        assert_eq!(settings.start_animation_style(), AnimationStyle::Slide);
        assert_eq!(
            settings.minimize_animation_style(),
            AnimationStyle::MagicLamp
        );
        assert_eq!(settings.maximize_animation_style(), AnimationStyle::None);
        assert_eq!(settings.window_open_animation_style(), AnimationStyle::None);

        settings.window_open_animation = AnimationStyle::Scale;
        settings.window_open_animation_enabled = true;
        assert_eq!(
            settings.window_open_animation_style(),
            AnimationStyle::Scale
        );

        settings.start_animation_enabled = false;
        settings.minimize_animation_enabled = false;
        settings.maximize_animation_enabled = false;
        settings.window_open_animation_enabled = false;
        assert_eq!(settings.start_animation_style(), AnimationStyle::None);
        assert_eq!(settings.minimize_animation_style(), AnimationStyle::None);
        assert_eq!(settings.maximize_animation_style(), AnimationStyle::None);
        assert_eq!(settings.window_open_animation_style(), AnimationStyle::None);

        settings.start_animation_enabled = true;
        settings.start_animation = AnimationStyle::None;
        settings.minimize_animation_enabled = true;
        settings.minimize_animation = AnimationStyle::None;
        settings.maximize_animation_enabled = true;
        settings.maximize_animation = AnimationStyle::None;
        settings.window_open_animation_enabled = true;
        settings.window_open_animation = AnimationStyle::None;
        assert_eq!(settings.start_animation_style(), AnimationStyle::None);
        assert_eq!(settings.minimize_animation_style(), AnimationStyle::None);
        assert_eq!(settings.maximize_animation_style(), AnimationStyle::None);
        assert_eq!(settings.window_open_animation_style(), AnimationStyle::None);

        let parsed = load("revision=2\nminimizeAnimation=fade\nmaximizeAnimationEnabled=0\n")
            .expect("style and toggle");
        assert_eq!(parsed.minimize_animation_style(), AnimationStyle::Fade);
        assert_eq!(parsed.maximize_animation_style(), AnimationStyle::None);

        let saved = load("revision=2\nwindowOpenAnimation=scale\nwindowOpenAnimationEnabled=0\n")
            .expect("disabled style remains saved");
        assert_eq!(saved.window_open_animation, AnimationStyle::Scale);
        assert_eq!(saved.window_open_animation_style(), AnimationStyle::None);
        let serialized = ConfigStore::serialize(&saved);
        assert!(!serialized.contains("windowOpenAnimationEnabled="));
        let reloaded = load(&serialized).expect("disabled style round trip");
        assert_eq!(reloaded.window_open_animation, AnimationStyle::Scale);
        assert!(!reloaded.window_open_animation_enabled);
        assert_eq!(reloaded.window_open_animation_style(), AnimationStyle::None);
    }

    #[test]
    fn animation_style_keys_round_trip_when_set() {
        let text = "revision=2\nstartAnimation=fade\nminimizeAnimation=none\n\
            maximizeAnimation=fade\nwindowOpenAnimation=fade\nwindowOpenAnimationEnabled=0\n\
            windowCloseAnimation=none\n";
        let parsed = load(text).expect("valid animation styles");
        assert_eq!(parsed.window_close_animation, AnimationStyle::None);
        assert_eq!(parsed.start_animation, AnimationStyle::Fade);
        assert_eq!(parsed.minimize_animation, AnimationStyle::None);
        assert_eq!(parsed.maximize_animation, AnimationStyle::Fade);
        assert_eq!(parsed.window_open_animation, AnimationStyle::Fade);
        assert_eq!(parsed.window_open_animation_style(), AnimationStyle::None);
        let serialized = ConfigStore::serialize(&parsed);
        assert!(serialized.contains("startAnimation=fade\n"));
        assert!(serialized.contains("minimizeAnimation=none\n"));
        assert!(!serialized.contains("windowOpenAnimationEnabled="));
        assert!(serialized.contains("windowCloseAnimation=none\n"));
        assert_eq!(load(&serialized).expect("round trip"), parsed);

        let mut enabled = parsed;
        enabled.window_open_animation_enabled = true;
        assert_eq!(enabled.window_open_animation_style(), AnimationStyle::Fade);
        let serialized = ConfigStore::serialize(&enabled);
        assert!(serialized.contains("windowOpenAnimationEnabled=true\n"));
        assert_eq!(load(&serialized).expect("enabled round trip"), enabled);
    }

    #[test]
    fn new_customization_defaults_match_contract_and_write_nothing() {
        let defaults = ProductSettings::default();
        assert_eq!(defaults.wallpaper_fit, WallpaperFit::Fill);
        assert!(!defaults.share_wallpaper);
        assert_eq!(defaults.cursor_size, 24);
        assert_eq!(defaults.taskbar_clock_format, ClockFormat::TwentyFour);
        assert!(!defaults.taskbar_show_seconds);
        assert!(defaults.taskbar_show_date);
        assert!(defaults.snap_preview_enabled);
        assert!(!defaults.focus_follows_mouse);
        let text = ConfigStore::serialize(&defaults);
        for key in [
            "wallpaperFit",
            "shareWallpaper",
            "cursorSize",
            "taskbarClockFormat",
            "taskbarShowSeconds",
            "taskbarShowDate",
            "snapPreviewEnabled",
            "focusFollowsMouse",
        ] {
            assert!(!text.contains(key), "{key} must not be written at default");
        }
    }

    #[test]
    fn new_customization_keys_round_trip_when_set() {
        let parsed = load(
            "revision=2\nwallpaperFit=tile\ncursorSize=64\ntaskbarClockFormat=12h\n\
                 taskbarShowSeconds=true\ntaskbarShowDate=false\nsnapPreviewEnabled=0\n\
                 focusFollowsMouse=yes\n",
        )
        .expect("valid customization settings");
        assert_eq!(parsed.wallpaper_fit, WallpaperFit::Tile);
        assert_eq!(parsed.cursor_size, 64);
        assert_eq!(parsed.taskbar_clock_format, ClockFormat::Twelve);
        assert!(parsed.taskbar_show_seconds);
        assert!(!parsed.taskbar_show_date);
        assert!(!parsed.snap_preview_enabled);
        assert!(parsed.focus_follows_mouse);
        let text = assert_round_trip(&parsed);
        assert!(text.contains("wallpaperFit=tile\n"));
        assert!(text.contains("cursorSize=64\n"));
        assert!(text.contains("taskbarClockFormat=12h\n"));
        assert!(text.contains("taskbarShowSeconds=true\n"));
        assert!(text.contains("snapPreviewEnabled=false\n"));
    }

    #[test]
    fn shared_wallpaper_toggle_round_trips_and_reads_legacy_spelling() {
        let enabled = load("revision=2\nshareWallpaper=true\n").expect("canonical setting");
        assert!(enabled.share_wallpaper);
        let serialized = ConfigStore::serialize(&enabled);
        assert!(serialized.contains("shareWallpaper=true\n"));
        assert!(!serialized.contains("wallpaperShare="));
        assert!(load(&serialized).expect("canonical reload").share_wallpaper);

        let legacy = load("revision=2\nwallpaperShare=1\n").expect("legacy setting");
        assert!(legacy.share_wallpaper);
        assert!(ConfigStore::serialize(&legacy).contains("shareWallpaper=true\n"));
    }

    #[test]
    fn invalid_new_customization_values_are_rejected() {
        assert_rejected(&[
            "wallpaperFit=Fill",
            "wallpaperFit=zoom",
            "cursorSize=16",
            "cursorSize=20",
            "cursorSize=65",
            "cursorSize=24.0",
            "cursorSize=-24",
            "taskbarClockFormat=24H",
            "taskbarClockFormat=ampm",
            "taskbarShowSeconds=maybe",
            "taskbarShowDate=2",
            "snapPreviewEnabled=on",
            "focusFollowsMouse=sometimes",
        ]);
    }

    #[test]
    fn time_zone_names_accept_iana_shapes_and_reject_paths() {
        for ok in [
            "UTC",
            "Europe/Berlin",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+3",
        ] {
            assert!(super::is_valid_time_zone_name(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/localtime",
            "../etc/passwd",
            "Europe/../UTC",
            "a b",
            "x\ny",
        ] {
            assert!(!super::is_valid_time_zone_name(bad), "{bad:?}");
        }
        assert!(load("revision=2\nautoTimeZone=../../etc\n").is_err());
        assert!(load("revision=2\nautoTimeZone=Asia/Tokyo\n").is_ok());
    }

    #[test]
    fn weather_keys_default_on_without_location_and_write_nothing() {
        let defaults = ProductSettings::default();
        assert!(defaults.weather_enabled);
        assert!(defaults.weather_location.is_empty());
        assert_eq!(defaults.weather_units, "celsius");
        assert!(!defaults.auto_location_enabled);
        assert!(!defaults.auto_time_zone_enabled);
        let text = ConfigStore::serialize(&defaults);
        for key in [
            "weatherEnabled",
            "weatherLocation",
            "weatherUnits",
            "autoLocationEnabled",
            "autoTimeZoneEnabled",
        ] {
            assert!(!text.contains(key), "{key} must not be written at default");
        }
    }

    #[test]
    fn weather_keys_are_typed_stable_rows_and_round_trip() {
        let location = "51.5\t-0.12\tGB\tLondon";
        let parsed = load(&format!(
            "revision=2\nweatherEnabled=true\nweatherUnits=fahrenheit\nweatherLocation={location}\n"
        ))
        .expect("valid weather settings");
        assert!(parsed.weather_enabled);
        assert_eq!(parsed.weather_units, "fahrenheit");
        assert_eq!(parsed.weather_location, location);
        assert!(
            parsed.unknown_keys.is_empty(),
            "weather keys must not fall into unknown_keys"
        );
        let text = assert_round_trip(&parsed);
        // Default-on weather is not written; an explicit off must survive a round trip.
        assert!(!text.contains("weatherEnabled"));
        assert!(text.contains("weatherUnits=fahrenheit\n"));
        let off = load("revision=2\nweatherEnabled=false\n").expect("explicit off");
        assert!(!off.weather_enabled);
        assert!(ConfigStore::serialize(&off).contains("weatherEnabled=false\n"));

        let stable = |key: &str| super::stable_setting(key).expect("typed weather row");
        let defaults = ProductSettings::default();
        assert_eq!(
            (stable("weatherEnabled").get)(&defaults),
            flamewm_api::settings::SettingValue::Boolean(true)
        );
        let mut settings = parsed.clone();
        (stable("weatherLocation").set)(&mut settings, &defaults, None, "weatherLocation")
            .expect("clearing location");
        assert!(settings.weather_location.is_empty());
    }

    #[test]
    fn weather_units_reject_values_other_than_celsius_or_fahrenheit() {
        assert!(load("revision=2\nweatherUnits=kelvin\n").is_err());
        assert!(load("revision=2\nweatherUnits=celsius\n").is_ok());
    }

    #[test]
    fn taskbar_widget_defaults_match_contract_and_write_nothing() {
        let defaults = ProductSettings::default();
        assert!(defaults.taskbar_show_weather);
        assert!(defaults.taskbar_show_volume);
        assert!(defaults.taskbar_show_network);
        assert!(defaults.taskbar_show_battery);
        assert!(defaults.taskbar_show_clock);
        assert!(defaults.taskbar_show_workspaces);
        assert!(!defaults.taskbar_battery_only_when_relevant);
        let text = ConfigStore::serialize(&defaults);
        for key in [
            "taskbarShowWeather",
            "taskbarShowVolume",
            "taskbarShowNetwork",
            "taskbarShowBattery",
            "taskbarShowClock",
            "taskbarShowWorkspaces",
            "taskbarBatteryOnlyWhenRelevant",
        ] {
            assert!(!text.contains(key), "{key} must not be written at default");
        }
    }

    #[test]
    fn taskbar_widget_keys_round_trip_off_and_on_defaults() {
        let parsed = load(
            "revision=2\ntaskbarShowWeather=false\ntaskbarShowVolume=false\n\
                 taskbarShowNetwork=false\ntaskbarShowBattery=false\n\
                 taskbarShowClock=false\ntaskbarShowWorkspaces=false\n\
                 taskbarBatteryOnlyWhenRelevant=true\n",
        )
        .expect("valid taskbar widget settings");
        assert!(!parsed.taskbar_show_weather);
        assert!(!parsed.taskbar_show_volume);
        assert!(!parsed.taskbar_show_network);
        assert!(!parsed.taskbar_show_battery);
        assert!(!parsed.taskbar_show_clock);
        assert!(!parsed.taskbar_show_workspaces);
        assert!(parsed.taskbar_battery_only_when_relevant);
        let text = assert_round_trip(&parsed);
        assert!(text.contains("taskbarShowWeather=false\n"));
        assert!(text.contains("taskbarShowClock=false\n"));
        assert!(text.contains("taskbarBatteryOnlyWhenRelevant=true\n"));

        let explicit_on = load("revision=2\ntaskbarShowWeather=true\ntaskbarShowClock=true\n")
            .expect("explicit default-on values");
        assert!(explicit_on.taskbar_show_weather && explicit_on.taskbar_show_clock);
        let on_text = ConfigStore::serialize(&explicit_on);
        assert!(!on_text.contains("taskbarShowWeather"));
        assert!(!on_text.contains("taskbarShowClock"));
    }

    #[test]
    fn taskbar_widget_keys_reject_invalid_and_keep_unknown_keys() {
        assert_eq!(
            load("revision=2\ntaskbarShowNetwork=maybe\n")
                .expect_err("invalid bool")
                .code,
            ErrorCode::InvalidArgument
        );
        let parsed =
            load("revision=2\naccent=#EF4048\ntaskbarShowBattery=false\nfutureFeature=kept\n")
                .expect("mixed file");
        assert_eq!(parsed.accent, "#EF4048");
        assert!(!parsed.taskbar_show_battery);
        assert!(parsed.taskbar_show_clock);
        assert_eq!(
            parsed.unknown_keys.get("futureFeature").map(String::as_str),
            Some("kept")
        );
    }

    #[test]
    fn legacy_file_without_taskbar_widget_keys_loads_defaults() {
        let parsed =
            load("revision=1\naccent=#EF4048\ntaskbarHeight=44\n").expect("legacy taskbar file");
        assert!(parsed.taskbar_show_weather);
        assert!(parsed.taskbar_show_volume);
        assert!(parsed.taskbar_show_network);
        assert!(parsed.taskbar_show_battery);
        assert!(parsed.taskbar_show_clock);
        assert!(parsed.taskbar_show_workspaces);
        assert!(!parsed.taskbar_battery_only_when_relevant);
    }

    #[test]
    fn legacy_file_without_customization_keys_loads_defaults() {
        let parsed = load("revision=1\naccent=#EF4048\ntaskbarHeight=44\n").expect("legacy file");
        assert_eq!(parsed.wallpaper_fit, WallpaperFit::Fill);
        assert_eq!(parsed.cursor_size, 24);
        assert_eq!(parsed.taskbar_clock_format, ClockFormat::TwentyFour);
        assert!(parsed.snap_preview_enabled);
        assert!(!parsed.focus_follows_mouse);
    }

    #[test]
    fn focus_follows_mouse_writes_only_true() {
        let mut settings = ProductSettings::default();
        settings.focus_follows_mouse = true;
        let text = ConfigStore::serialize(&settings);
        assert!(text.contains("focusFollowsMouse=true\n"));
        assert!(load(&text).expect("round trip").focus_follows_mouse);
    }

    #[test]
    fn wallpaper_fit_parse_and_as_str_are_inverse() {
        for fit in [
            WallpaperFit::Fill,
            WallpaperFit::Fit,
            WallpaperFit::Stretch,
            WallpaperFit::Center,
            WallpaperFit::Tile,
        ] {
            assert_eq!(WallpaperFit::parse(fit.as_str()), Some(fit));
        }
        assert_eq!(WallpaperFit::parse("Tile"), None);
    }
}
