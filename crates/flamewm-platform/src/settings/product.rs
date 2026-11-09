//! `ProductSettings`: the typed settings snapshot and its defaults.

use super::*;

/// System accent used everywhere while `accentCustom` is off.
pub const DEFAULT_ACCENT: &str = flamewm_api::settings::DEFAULT_ACCENT;

/// Settings key of the Start Favorites list (`;`-joined desktop ids). Same string as the
/// `stable_settings!` row for `ProductSettings::start_favorites`.
pub const START_FAVORITES_KEY: &str = "startFavorites";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSettings {
    pub revision: u64,
    pub accent: String,
    /// When false the system accent (`DEFAULT_ACCENT`) applies everywhere; `accent` is ignored.
    pub accent_custom: bool,
    /// Progressive-disclosure switch for the `surface` group (`surfaceCustom`); off resolves its members to defaults.
    pub surface_custom: bool,
    /// Progressive-disclosure switch for the `glass` group (`glassCustom`); off resolves its members to defaults.
    pub glass_custom: bool,
    /// Progressive-disclosure switch for the `cursor` group (`cursorCustom`); off resolves its members to defaults.
    pub cursor_custom: bool,
    /// Progressive-disclosure switch for the `minimizeMaximize` group (`minimizeMaximizeCustom`); off resolves its members to defaults.
    pub minimize_maximize_custom: bool,
    /// Progressive-disclosure switch for the `lockBackground` group (`lockBackgroundCustom`); off resolves its members to defaults.
    pub lock_background_custom: bool,
    /// Progressive-disclosure switch for the `lockClock` group (`lockClockCustom`); off resolves its members to defaults.
    pub lock_clock_custom: bool,
    /// Progressive-disclosure switch for the `screensaver` group (`screensaverCustom`); off resolves its members to defaults.
    pub screensaver_custom: bool,
    /// Progressive-disclosure switch for the `windows` group (`windowsCustom`); off resolves its members to defaults.
    pub windows_custom: bool,
    /// Progressive-disclosure switch for the `panel` group (`panelCustom`); off resolves its members to defaults.
    pub panel_custom: bool,
    /// Progressive-disclosure switch for the `items` group (`itemsCustom`); off resolves its members to defaults.
    pub items_custom: bool,
    /// Progressive-disclosure switch for the `fonts` group (`fontsCustom`); off resolves its members to defaults.
    pub fonts_custom: bool,
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
    pub window_margin: u16,
    /// `false` (XP style): move/resize shows an outline and applies the geometry on release.
    pub show_content_while_dragging: bool,
    /// `titlebars`: draw the Flame titlebar; when off, windows keep only resize edges.
    pub titlebars: bool,
    pub focus_follows_mouse: bool,
    pub font_family: String,
    pub font_bold: bool,
    /// `textShadow`: the shared soft shadow behind UI text; off paints all text without it.
    pub text_shadow: bool,
    pub font_size_offset: i8,
    pub taskbar_color: String,
    pub taskbar_opacity: u8,
    pub taskbar_height: u16,
    pub taskbar_style: TaskbarStyle,
    pub taskbar_enabled: bool,
    pub taskbar_fill: TaskbarFill,
    pub taskbar_gradient_edge: TaskbarGradientEdge,
    pub dock_enabled: bool,
    pub dock_position: DockPosition,
    pub dock_icon_size: u16,
    pub dock_pinned: Vec<String>,
    pub dock_show_running: bool,
    /// Trash slot after the dock apps (`dock.showTrash`); empty/full icon from the Trash folder.
    pub dock_show_trash: bool,
    /// `dock.hoverScale`: subtle hover scale of the dock icon (default off).
    pub dock_hover_scale: bool,
    /// `dock.autoHideCovered`: auto-hide only while a window covers the dock (default off).
    pub dock_auto_hide_covered: bool,
    /// `dock.launchBounce`: the dock icon hops while its launch is pending (default off).
    pub dock_launch_bounce: bool,
    /// `dock.autoHide`: slide the dock away when idle; reveal on pointer push to its edge.
    pub dock_auto_hide: bool,
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
    pub taskbar_show_desktop_button: bool,
    pub taskbar_battery_only_when_relevant: bool,
    /// `taskbarActivities`: show the running-window area; off also forbids minimizing.
    pub taskbar_activities: bool,
    /// `taskbarSeparate`: one rounded background per element instead of a unified bar.
    pub taskbar_separate: bool,
    pub weather_enabled: bool,
    /// `weatherTint`: tint the taskbar weather glyph by condition (default on).
    pub weather_tint: bool,
    /// `workspaceOsd`: show a brief workspace number/name card on switch (default off).
    pub workspace_osd: bool,
    /// Encoded weather location (`lat\tlon\tcountry\tname`); empty means unset.
    pub weather_location: String,
    /// `celsius` or `fahrenheit`.
    pub weather_units: String,
    /// Sets `weatherLocation` from the IP-geolocated position.
    pub auto_location_enabled: bool,
    /// Applies the IP-geolocated IANA time zone to the shell and lock clocks.
    pub auto_time_zone_enabled: bool,
    /// Last IANA time zone from automatic lookup (offline-start cache); empty means none.
    pub auto_time_zone: String,
    pub surface_color: String,
    pub surface_opacity: u8,
    pub surface_blur_percent: u8,
    /// `blueLightFilter` percent (0..=100): warmth of the blue-light filter. `0` is neutral and the
    /// filter is off; the shared maths lives in `flamewm_api::blue_light`.
    pub blue_light_filter: u8,
    /// `nightShift`: enables the blue-light filter, all day or on the schedule below.
    pub night_shift: bool,
    /// `nightShiftSchedule`: when true the filter follows `nightShiftStart`..`nightShiftEnd`.
    pub night_shift_schedule: bool,
    /// `nightShiftStart`/`nightShiftEnd`: minutes after local midnight (0..=1439). A start greater
    /// than the end wraps over midnight, so 21:00..07:00 is a valid night window.
    pub night_shift_start: u16,
    pub night_shift_end: u16,
    /// `nightShiftMode`: `manual` (default) uses the start/end minutes; `sunset` follows the sun.
    pub night_shift_mode: NightShiftMode,
    /// `highContrast`: stronger borders and text contrast for the shared surfaces. Default off.
    pub high_contrast: bool,
    /// `reducedMotion`: the shell skips decorative motion such as launch feedback. Default off.
    pub reduced_motion: bool,
    /// `textScalePercent` (100..=200): UI text scale. `100` is the default size.
    pub text_scale_percent: u16,
    /// `stickyKeys`: modifier keys latch for one keystroke instead of being held. Default off.
    pub sticky_keys: bool,
    /// `slowKeys`: keys must be held briefly before they register. Default off.
    pub slow_keys: bool,
    /// `shakeToLocate`: a fast left-right pointer shake enlarges the cursor for a moment. Default off.
    pub shake_to_locate: bool,
    pub surface_outline_color: String,
    pub surface_outline_opacity: u8,
    /// `surfaceOutlineWidth` in px (1..=4); default 1.
    pub surface_outline_width: u16,
    /// `customWindowOutline`: off (default) = window frames inherit the surface outline; on = the
    /// `windowOutline*` settings below apply.
    pub custom_window_outline: bool,
    /// `windowOutlineColor`: window frame outline colour, separate from the taskbar surface contour.
    pub window_outline_color: String,
    /// `windowOutlineOpacity` percent (0..=100); the default is half opacity.
    pub window_outline_opacity: u8,
    /// `windowOutlineWidth` in px (1..=4); the default is the working one-pixel band.
    pub window_outline_width: u16,
    /// `outlines`: when false, no surface contour is drawn and the outline settings are hidden.
    pub outlines: bool,
    /// `surfaceSaturationPercent` (0..=200): backdrop saturation, 100 is identity.
    pub surface_saturation_percent: u8,
    /// `reduceTransparency`: all materials opaque with no blur. Default off: surfaces are translucent black at 75%.
    pub reduce_transparency: bool,
    /// `glassOpacityPercent` (4..=100): `GlassRegular` tint alpha, default 75.
    pub glass_opacity_percent: u8,
    /// `glassBlurPx` (0..=54): `GlassRegular` backdrop blur in logical px.
    pub glass_blur_px: u8,
    /// `taskbarMaterialMode`: `inherit` (default) | `blur-only` | `custom-tint`.
    pub taskbar_material_mode: String,
    /// `taskbarOpacityPercent` (0..=75): taskbar tint alpha in Blur Only and Custom Tint.
    pub taskbar_opacity_percent: u8,
    /// `taskbarCustomTint` (`#RRGGBB`): used only in Custom Tint mode.
    pub taskbar_custom_tint: String,
    pub custom_start_icon: String,
    pub sticky_notes_enabled: bool,
    pub organize_downloads: bool,
    pub accent_opacity: u8,
    pub decoration: String,
    pub render_acceleration: String,
    pub render_backend: String,
    pub titlebar_button_style: String,
    pub cursor_theme: String,
    /// Cursor pixel size; one of 24, 32, 48, 64.
    pub cursor_size: u16,
    /// `pointerSpeed`: libinput acceleration speed in percent of its normalized [-1, 1] range.
    /// `0` is the device default, negative is slower and positive faster.
    pub pointer_speed: i8,
    /// `pointerRawInput`: use libinput's flat ("none") acceleration profile instead of the adaptive
    /// one. This changes the pointer curve only; it does not bypass libinput acceleration.
    pub pointer_raw_input: bool,
    /// `keyboardLayout`: xkb layout name the Wayland seat compiles its keymap from; one of
    /// [`flamewm_api::settings::KEYBOARD_LAYOUTS`]. The X11 session's own X server layout wins there.
    pub keyboard_layout: String,
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
    /// Terminal for `FlameWM` launches: a bare command name or absolute path. Empty selects `FlameWM` Terminal.
    pub default_terminal: String,
    /// `startFavorites`: desktop application ids pinned to the launcher Favorites category, joined by `;`.
    pub start_favorites: String,
    /// `assistant.enabled`: allows the assistant and grabs its chords (Super+C, Ctrl+Alt+C).
    pub assistant_enabled: bool,
    /// `assistant.taskbarIcon`: shows the assistant button in the taskbar; on by default.
    pub assistant_taskbar_icon: bool,
    /// `assistant.speechToText`: speech input; turning it on starts the background voice setup. Off by default.
    pub assistant_speech_to_text: bool,
    /// `assistant.textToSpeech`: speech output; turning it on starts the background voice setup. Off by default.
    pub assistant_text_to_speech: bool,
    /// `assistant.apiKey`: ARK AI key; empty uses the built-in default key.
    pub assistant_api_key: String,
    pub lock_clock_format: LockClockFormat,
    pub lock_show_date: bool,
    pub lock_show_username: bool,
    pub lock_timeout_minutes: u16,
    pub screensaver_mode: ScreensaverMode,
    pub screensaver_after_lock_minutes: u16,
    pub screensaver_num_ribbons: u16,
    pub screensaver_ribbon_width_tenths: u16,
    pub screensaver_span_monitors: bool,
    pub screensaver_display_off_minutes: u16,
    pub per_output_scale: BTreeMap<String, u16>,
    pub panel_edges: BTreeMap<String, PanelEdge>,
    pub hot_corner_top_left: HotCornerAction,
    pub hot_corner_top_right: HotCornerAction,
    pub hot_corner_bottom_left: HotCornerAction,
    pub hot_corner_bottom_right: HotCornerAction,
    pub edge_snap_enabled: bool,
    /// Edge-snap distance in px; file values are clamped to 4..=64 on load, API values must be in range.
    pub edge_snap_threshold_px: u16,
    pub hotkeys: BTreeMap<String, String>,
    pub unknown_keys: BTreeMap<String, String>,
}

impl Default for ProductSettings {
    fn default() -> Self {
        Self {
            revision: 1,
            accent: DEFAULT_ACCENT.to_owned(),
            accent_custom: false,
            surface_custom: false,
            glass_custom: false,
            cursor_custom: false,
            minimize_maximize_custom: false,
            lock_background_custom: false,
            lock_clock_custom: false,
            screensaver_custom: false,
            windows_custom: false,
            panel_custom: false,
            items_custom: false,
            fonts_custom: false,
            icon_theme: icon_theme::DEFAULT_ICON_THEME.to_owned(),
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
            window_margin: 2,
            show_content_while_dragging: false,
            titlebars: true,
            focus_follows_mouse: false,
            font_family: "IBM Plex Sans".to_owned(),
            font_bold: false,
            text_shadow: true,
            font_size_offset: 0,
            taskbar_color: "#191b1d".to_owned(),
            taskbar_opacity: 50,
            taskbar_height: 44,
            taskbar_style: TaskbarStyle::Classic,
            taskbar_enabled: true,
            taskbar_fill: TaskbarFill::Solid,
            taskbar_gradient_edge: TaskbarGradientEdge::Auto,
            dock_enabled: false,
            dock_position: DockPosition::Bottom,
            dock_icon_size: 44,
            dock_pinned: Vec::new(),
            dock_show_running: true,
            dock_show_trash: true,
            dock_hover_scale: false,
            dock_auto_hide_covered: false,
            dock_launch_bounce: false,
            dock_auto_hide: true,
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
            taskbar_show_desktop_button: true,
            taskbar_battery_only_when_relevant: false,
            taskbar_activities: true,
            taskbar_separate: false,
            weather_enabled: true,
            weather_tint: true,
            workspace_osd: false,
            weather_location: String::new(),
            weather_units: "celsius".to_owned(),
            auto_location_enabled: true,
            auto_time_zone_enabled: true,
            auto_time_zone: String::new(),
            surface_color: flamewm_api::settings_groups::DEFAULT_SURFACE_COLOR.to_owned(),
            surface_opacity: flamewm_api::settings_groups::DEFAULT_SURFACE_OPACITY,
            surface_blur_percent: 70,
            blue_light_filter: flamewm_api::blue_light::DEFAULT_INTENSITY,
            night_shift: false,
            night_shift_schedule: false,
            night_shift_start: 21 * 60,
            night_shift_end: 7 * 60,
            night_shift_mode: NightShiftMode::Manual,
            high_contrast: false,
            reduced_motion: false,
            text_scale_percent: 100,
            sticky_keys: false,
            slow_keys: false,
            shake_to_locate: false,
            surface_outline_color: "#999999".to_owned(),
            surface_outline_opacity: 50,
            surface_outline_width: 1,
            custom_window_outline: false,
            window_outline_color: "#999999".to_owned(),
            window_outline_opacity: 50,
            window_outline_width: 1,
            outlines: true,
            surface_saturation_percent: 165,
            reduce_transparency: false,
            glass_opacity_percent: flamewm_api::settings_groups::DEFAULT_GLASS_OPACITY,
            glass_blur_px: 28,
            taskbar_material_mode: "inherit".to_owned(),
            taskbar_opacity_percent: 8,
            taskbar_custom_tint: "#5879EE".to_owned(),
            custom_start_icon: String::new(),
            sticky_notes_enabled: true,
            organize_downloads: false,
            accent_opacity: 100,
            decoration: "rounded".to_owned(),
            render_acceleration: "cpu".to_owned(),
            render_backend: "cpu".to_owned(),
            titlebar_button_style: "circle".to_owned(),
            cursor_theme: String::new(),
            cursor_size: 24,
            pointer_speed: 0,
            pointer_raw_input: false,
            keyboard_layout: flamewm_api::settings::DEFAULT_KEYBOARD_LAYOUT.to_owned(),
            animations_enabled: true,
            animation_speed: 100,
            start_animation: AnimationStyle::None,
            start_animation_enabled: true,
            minimize_animation: AnimationStyle::None,
            maximize_animation: AnimationStyle::None,
            window_open_animation: AnimationStyle::None,
            window_open_animation_enabled: false,
            window_close_animation: AnimationStyle::None,
            minimize_animation_enabled: true,
            maximize_animation_enabled: false,
            lock_background_color: "#232629".to_owned(),
            lock_background_image: String::new(),
            lock_background_mode: BackgroundMode::Default,
            lock_background_blur_percent: 40,
            lock_background_dim_percent: 25,
            avatar_image: String::new(),
            default_terminal: String::new(),
            start_favorites: String::new(),
            assistant_enabled: true,
            assistant_taskbar_icon: true,
            assistant_speech_to_text: false,
            assistant_text_to_speech: false,
            assistant_api_key: String::new(),
            lock_clock_format: LockClockFormat::TwentyFour,
            lock_show_date: true,
            lock_show_username: true,
            lock_timeout_minutes: 0,
            screensaver_mode: ScreensaverMode::Ribbons,
            screensaver_after_lock_minutes: 1,
            screensaver_num_ribbons: 6,
            screensaver_ribbon_width_tenths: 110,
            screensaver_span_monitors: false,
            screensaver_display_off_minutes: 20,
            per_output_scale: BTreeMap::new(),
            panel_edges: BTreeMap::new(),
            hot_corner_top_left: HotCornerAction::None,
            hot_corner_top_right: HotCornerAction::None,
            hot_corner_bottom_left: HotCornerAction::None,
            hot_corner_bottom_right: HotCornerAction::None,
            edge_snap_enabled: true,
            edge_snap_threshold_px: 16,
            hotkeys: BTreeMap::new(),
            unknown_keys: BTreeMap::new(),
        }
    }
}
