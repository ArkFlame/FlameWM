use chrono::Local;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::PanelEdge;
use flamewm_image_core::scale::ScaleFilter;
use flamewm_image_core::{decode_bytes_at, ImageSourceFormat};
use flamewm_platform::chrome::{parse_hex_rgb, ACCENT_SOFT_ALPHA};
use flamewm_platform::settings::ClockFormat;
use flamewm_ui_core::context_menu::shape_radius;
use flamewm_ui_x11::surface_style::{
    project_surface_border, surface_background as shared_surface_background,
};
use flamewm_ui_x11::{RuntimeImage, SlideConfig, SlideEdge, UiColor, UiDocumentAccess};

use crate::projection::{project_clock_at, ClockDisplay, TaskbarItems};

/// Panel image node that carries the custom Start icon (`ui/shell/panel.html`).
const START_ICON_ID: &str = "task-start-icon";
/// Square raster for the 25px `.task-start-icon` box.
const START_ICON_SIZE: u32 = 25;
/// Largest custom Start icon file read, so a bad path cannot stall the panel.
const MAX_START_ICON_BYTES: u64 = 8 * 1024 * 1024;

/// Neutral grey until a settings snapshot supplies `accent`.
const DEFAULT_ACCENT: u32 = 0x007f_878e;
const DEFAULT_ANIMATION_SPEED: i64 = 100;
const DEFAULT_START_ANIMATION: &str = "slide";
const DEFAULT_SURFACE_OPACITY: u8 = 50;
const DEFAULT_SURFACE_BLUR_PERCENT: u8 = 80;
const DEFAULT_ACCENT_OPACITY: u8 = 100;
const DEFAULT_FONT_FAMILY: &str = "IBM Plex Sans";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Panel,
    Start,
    Launcher,
    Switcher,
    TaskMenu,
    Media,
    Osd,
}

/// Settings are presentation input only. Panel placement, edge, and size are
/// deliberately absent from this projection and remain service-owned.
pub fn project(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
    surface: SurfaceKind,
) -> Result<(), String> {
    let root = match surface {
        SurfaceKind::Panel => "taskbar",
        SurfaceKind::Start => "start-menu",
        SurfaceKind::Launcher => "launcher-popup",
        SurfaceKind::Switcher => "switcher-popup",
        SurfaceKind::TaskMenu => "task-menu",
        SurfaceKind::Media => "media-popup",
        SurfaceKind::Osd => "osd",
    };
    let accent = accent_color(snapshot);
    let font_family = text_setting(snapshot, "fontFamily", DEFAULT_FONT_FAMILY);
    document.font_family(font_family)?;
    document.ui_font_defaults(
        integer_setting(snapshot, "fontSizeOffset", 0) as f32,
        bool_setting(snapshot, "fontBold", false),
    )?;
    project_accent_tokens(document, snapshot)?;
    project_surface_border(document, root, snapshot)?;

    match surface {
        SurfaceKind::Panel => {
            let fill = surface_background(snapshot);
            crate::projection::set_taskbar_surface(fill);
            crate::projection::set_taskbar_border(flamewm_ui_x11::surface_style::surface_border(
                snapshot,
            ));
            // Separate mode paints the wallpaper (the captured backdrop) across the bar and one
            // capsule per element; the root must not tint the gaps between them.
            let separate = bool_setting(snapshot, "taskbarSeparate", true);
            document.background(
                "taskbar",
                if separate {
                    UiColor { a: 0, ..fill }
                } else {
                    fill
                },
            )?;
            // A transparent fill alone does not suppress the separately projected 1px
            // contour. Only the per-node capsules have outlines in separated mode.
            if separate {
                document.border("taskbar", UiColor { a: 0, ..fill })?;
            }
            crate::projection::set_task_accent(accent);
            // A settings change must also re-apply (or clear) the per-element capsules, so
            // disabling `taskbarSeparate` cannot leave the last projected fills behind.
            crate::projection::reapply_element_backgrounds(
                document,
                taskbar_items(Some(snapshot)),
            )?;
            project_clock_at(document, Local::now(), clock_display(snapshot))?;
            project_start_icon(document, custom_start_icon(snapshot))?;
        }
        SurfaceKind::Start => {
            // Start shares the surfaceColor/surfaceOpacity pair with the taskbar and popups.
            document.background(root, surface_background(snapshot))?;
            document.corner_radius(root, popup_corner_radius(snapshot))?;
            // Opaque interior token: the menu root already carries the surface alpha, so the
            // search field must not apply surfaceOpacity a second time.
            document.background(
                "start-search-box",
                UiColor {
                    r: 0x14,
                    g: 0x16,
                    b: 0x18,
                    a: 0xff,
                },
            )?;
            // No accent outline: the menu's rounded panes cover the border on three sides, so an
            // accent-coloured border only ever showed as a red line down the left edge.
        }
        SurfaceKind::TaskMenu => {
            // Task menu shares the desktop menu frame (ui/shared/menu.css 1px border); no accent outline.
        }
        SurfaceKind::Launcher | SurfaceKind::Switcher => {
            // Launcher and switcher share the Start surface pair so the popups match in every theme.
            document.background(root, surface_background(snapshot))?;
            document.corner_radius(root, popup_corner_radius(snapshot))?;
        }
        SurfaceKind::Media => {
            // Root contour is always the shared Surfaces border, not the accent.
        }
        SurfaceKind::Osd => {
            // Volume OSD card: the shared surface fill; its outline and pill radius stay in osd.html.
            document.background(root, surface_background(snapshot))?;
        }
    }
    Ok(())
}

/// `ui/shell/flamewm.css` accent-derived tokens with the alpha each keeps in its `:root` default.
const ACCENT_TOKEN_ALPHAS: [(&str, u8); 9] = [
    ("--accent", 0xff),
    ("--accent-soft", ACCENT_SOFT_ALPHA),
    ("--accent-strong", 0xdd),
    ("--desktop-hover", 0x26),
    ("--desktop-hover-border", 0x4d),
    ("--desktop-selected", 0x47),
    ("--desktop-selected-border", 0xa6),
    ("--selection-fill", 0x33),
    ("--snap-fill", 0x33),
];

/// Projects every accent-derived token from the live `accent`/`accentOpacity` pair.
fn project_accent_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    for (name, alpha) in ACCENT_TOKEN_ALPHAS {
        document.color_variable(name, accent_tint(snapshot, alpha))?;
    }
    Ok(())
}

/// Shared surface background for the audio, Wi-Fi and calendar popups: the `surfaceColor` /
/// `surfaceOpacity` pair on the popup root. Layers inside stay transparent.
pub fn project_menu_background(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
    root: &str,
) -> Result<(), String> {
    document.background(root, surface_background(snapshot))?;
    project_surface_border(document, root, snapshot)?;
    document.corner_radius(root, popup_corner_radius(snapshot))?;
    // Popups own their accent too: the `--accent` token drives the battery bar, and the
    // Wi-Fi/audio track override reads the global task accent, which must match this snapshot.
    crate::projection::set_task_accent(accent_color(snapshot));
    project_accent_tokens(document, snapshot)
}

/// Popup and menu corner radius from the `decoration` setting: Rounded 12, Squared 0.
#[must_use]
pub fn popup_corner_radius(snapshot: &SettingsSnapshot) -> f32 {
    let rounded = text_setting(snapshot, "decoration", "rounded") != "squared";
    shape_radius(rounded) as f32
}

/// Start menu background colour: the shared surface pair, same as [`surface_background`].
#[must_use]
pub fn menu_background(snapshot: &SettingsSnapshot) -> UiColor {
    surface_background(snapshot)
}

/// Shared surface colour with the `surfaceOpacity` alpha (taskbar, popups, desktop menus).
/// The mapping is owned by `flamewm_ui_core::context_menu::surface_fill`.
#[must_use]
pub fn surface_background(snapshot: &SettingsSnapshot) -> UiColor {
    shared_surface_background(snapshot)
}

/// Configured Start icon path (`customStartIcon`). `None` keeps the default `FlameWM` glyph.
#[must_use]
pub fn custom_start_icon(snapshot: &SettingsSnapshot) -> Option<String> {
    let path = text_setting(snapshot, "customStartIcon", "");
    (!path.trim().is_empty()).then_some(path)
}

/// Applies the configured Start icon to the panel. `None`, or a path that fails
/// to load, keeps the packaged default `flamewm-start.svg` already in the template.
fn project_start_icon(
    document: &mut impl UiDocumentAccess,
    path: Option<String>,
) -> Result<(), String> {
    // Clearing (or a path that no longer loads) drops any previously applied custom raster.
    match path.as_deref().and_then(|path| load_start_icon(path).ok()) {
        Some(image) => document.image_rgba8(START_ICON_ID, image),
        None => document.image_reset(START_ICON_ID),
    }
}

/// Decodes a custom Start icon to a `START_ICON_SIZE` square RGBA raster through
/// the shared image core, which enforces the format and pixel limits.
fn load_start_icon(path: &str) -> Result<RuntimeImage, String> {
    let path = std::path::Path::new(path);
    let format = ImageSourceFormat::from_extension(path)
        .ok_or_else(|| format!("unsupported start icon format: {}", path.display()))?;
    let len = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if len > MAX_START_ICON_BYTES {
        return Err(format!("start icon exceeds {MAX_START_ICON_BYTES} bytes"));
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let image = decode_bytes_at(
        &bytes,
        format,
        START_ICON_SIZE,
        START_ICON_SIZE,
        ScaleFilter::Area,
    )?;
    Ok(RuntimeImage {
        source: path.display().to_string(),
        width: image.width,
        height: image.height,
        pixels: image.pixels,
    })
}

/// Start presentation for the panel edge hosting Start. `start_button` is the Start button's root
/// device rect `(x, y, width, height)`, which Magic Lamp folds into. `None` keeps the immediate
/// show: animations disabled, `none`, an unknown style, or Magic Lamp before the button has layout.
#[must_use]
pub fn start_slide(
    snapshot: &SettingsSnapshot,
    edge: PanelEdge,
    start_button: Option<(i32, i32, u32, u32)>,
) -> Option<SlideConfig> {
    if !bool_setting(snapshot, "animationsEnabled", true)
        || !bool_setting(snapshot, "startAnimationEnabled", true)
    {
        return None;
    }
    let speed =
        integer_setting(snapshot, "animationSpeed", DEFAULT_ANIMATION_SPEED).clamp(25, 200) as u16;
    let edge = match edge {
        PanelEdge::Bottom => SlideEdge::Bottom,
        PanelEdge::Top => SlideEdge::Top,
        PanelEdge::Left => SlideEdge::Left,
        PanelEdge::Right => SlideEdge::Right,
    };
    match text_setting(snapshot, "startAnimation", DEFAULT_START_ANIMATION).as_str() {
        "slide" | "slide-up" => Some(SlideConfig::slide(edge, speed)),
        "fade" => Some(SlideConfig::fade(edge, speed)),
        "magic-lamp" => start_button.map(|target| SlideConfig::lamp(edge, speed, target)),
        _ => None,
    }
}

/// Accent carries `accentOpacity` as alpha for every accent consumer.
/// Clock presentation keys; unknown or missing values fall back to the product defaults.
#[must_use]
pub fn clock_display(snapshot: &SettingsSnapshot) -> ClockDisplay {
    let defaults = ClockDisplay::default();
    let format = ClockFormat::parse(&text_setting(snapshot, "taskbarClockFormat", "24h"))
        .unwrap_or(defaults.format);
    ClockDisplay {
        format,
        show_seconds: bool_setting(snapshot, "taskbarShowSeconds", defaults.show_seconds),
        show_date: bool_setting(snapshot, "taskbarShowDate", defaults.show_date),
    }
}

/// Taskbar item visibility keys; a missing snapshot or key means its default.
#[must_use]
pub fn taskbar_items(snapshot: Option<&SettingsSnapshot>) -> TaskbarItems {
    let defaults = TaskbarItems::default();
    let Some(snapshot) = snapshot else {
        return defaults;
    };
    TaskbarItems {
        weather: bool_setting(snapshot, "taskbarShowWeather", defaults.weather),
        volume: bool_setting(snapshot, "taskbarShowVolume", defaults.volume),
        network: bool_setting(snapshot, "taskbarShowNetwork", defaults.network),
        battery: bool_setting(snapshot, "taskbarShowBattery", defaults.battery),
        battery_only_when_relevant: bool_setting(
            snapshot,
            "taskbarBatteryOnlyWhenRelevant",
            defaults.battery_only_when_relevant,
        ),
        clock: bool_setting(snapshot, "taskbarShowClock", defaults.clock),
        workspaces: bool_setting(snapshot, "taskbarShowWorkspaces", defaults.workspaces),
        activities: bool_setting(snapshot, "taskbarActivities", defaults.activities),
        separate: bool_setting(snapshot, "taskbarSeparate", defaults.separate),
        blur_percent: percent_setting(snapshot, "surfaceBlurPercent", defaults.blur_percent),
    }
}

fn accent_color(snapshot: &SettingsSnapshot) -> UiColor {
    let accent = color_setting(snapshot, "accent", DEFAULT_ACCENT);
    let opacity = percent_setting(snapshot, "accentOpacity", DEFAULT_ACCENT_OPACITY);
    UiColor {
        a: opacity_alpha(opacity),
        ..accent
    }
}

/// Derived accent token: keeps the stylesheet's base alpha, scaled by `accentOpacity`.
fn accent_tint(snapshot: &SettingsSnapshot, base_alpha: u8) -> UiColor {
    let accent = accent_color(snapshot);
    UiColor {
        a: ((u16::from(base_alpha) * u16::from(accent.a)) / 255) as u8,
        ..accent
    }
}

fn opacity_alpha(percent: u8) -> u8 {
    ((u16::from(percent) * 255) / 100) as u8
}

/// Flame-owned icon theme (`iconTheme`); unset resolves to the product default.
#[must_use]
pub fn icon_theme_name(snapshot: Option<&SettingsSnapshot>) -> String {
    let default = flamewm_platform::settings::ProductSettings::default().icon_theme;
    snapshot.map_or_else(
        || default.clone(),
        |snapshot| text_setting(snapshot, "iconTheme", &default),
    )
}

fn text_setting(snapshot: &SettingsSnapshot, key: &str, default: &str) -> String {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) if !value.trim().is_empty() => value.clone(),
        _ => default.to_owned(),
    }
}

fn bool_setting(snapshot: &SettingsSnapshot, key: &str, default: bool) -> bool {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    }
}

fn integer_setting(snapshot: &SettingsSnapshot, key: &str, default: i64) -> i64 {
    match snapshot.values.get(key) {
        Some(SettingValue::Integer(value)) => *value,
        _ => default,
    }
}

/// Surface `(opacity, blur)` percents for the taskbar pseudo-transparency backdrop.
#[must_use]
pub fn taskbar_backdrop_percents(snapshot: &SettingsSnapshot) -> (u8, u8) {
    (
        percent_setting(snapshot, "surfaceOpacity", DEFAULT_SURFACE_OPACITY),
        percent_setting(snapshot, "surfaceBlurPercent", DEFAULT_SURFACE_BLUR_PERCENT),
    )
}

/// Start popup `(opacity, blur)` percents for its pseudo-transparency backdrop: the shared
/// `surfaceOpacity` / `surfaceBlurPercent` pair.
#[must_use]
pub fn start_backdrop_percents(snapshot: &SettingsSnapshot) -> (u8, u8) {
    taskbar_backdrop_percents(snapshot)
}

fn percent_setting(snapshot: &SettingsSnapshot, key: &str, default: u8) -> u8 {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => *value,
        _ => default,
    }
}

fn color_setting(snapshot: &SettingsSnapshot, key: &str, default: u32) -> UiColor {
    let Some(SettingValue::Text(value)) = snapshot.values.get(key) else {
        return rgb(default, 255);
    };
    // Stored values carry `#`; the bare six-digit form is accepted too.
    let parsed = if value.starts_with('#') {
        parse_hex_rgb(value)
    } else {
        parse_hex_rgb(&format!("#{value}"))
    };
    match parsed {
        Some((r, g, b)) => UiColor { r, g, b, a: 255 },
        None => rgb(default, 255),
    }
}

fn rgb(value: u32, alpha: u8) -> UiColor {
    UiColor {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
        a: alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_ui_x11::test_support::RecordingUiDoc;
    use std::sync::Mutex;

    /// Serializes tests that write the process-wide task accent and then read it back.
    static TASK_ACCENT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_task_accent() -> std::sync::MutexGuard<'static, ()> {
        TASK_ACCENT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn all_shell_surfaces_share_live_contour_settings() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "surfaceBorderColor".to_owned(),
            SettingValue::Text("#306090".to_owned()),
        );
        snapshot
            .values
            .insert("surfaceBorderOpacity".to_owned(), SettingValue::Percent(40));
        let contour = UiColor {
            r: 0x30,
            g: 0x60,
            b: 0x90,
            a: 102,
        };
        let mut doc = RecordingUiDoc::default();
        for (kind, root) in [
            (SurfaceKind::Panel, "taskbar"),
            (SurfaceKind::Start, "start-menu"),
            (SurfaceKind::Launcher, "launcher-popup"),
            (SurfaceKind::Switcher, "switcher-popup"),
            (SurfaceKind::TaskMenu, "task-menu"),
            (SurfaceKind::Media, "media-popup"),
            (SurfaceKind::Osd, "osd"),
        ] {
            project(&mut doc, &snapshot, kind).expect("shared surface projection");
            if kind == SurfaceKind::Panel {
                // Separate mode owns only node borders, not the transparent bar root.
                assert_eq!(doc.borders[root].a, 0, "{root}");
            } else {
                assert_eq!(doc.borders[root], contour, "{root}");
            }
        }
        for popup in [
            "audio-popup",
            "wifi-popup",
            "calendar-popup",
            "weather-popup",
        ] {
            project_menu_background(&mut doc, &snapshot, popup).expect("popup projection");
            assert_eq!(doc.borders[popup], contour, "{popup}");
        }
        snapshot
            .values
            .insert("surfaceBorderOpacity".to_owned(), SettingValue::Percent(0));
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("contour disabled");
        assert_eq!(doc.borders["start-menu"].a, 0);
    }

    #[test]
    fn osd_projection_applies_surface_fill_and_accent_token() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(40));
        snapshot.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#00ff00".to_owned()),
        );
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &snapshot, SurfaceKind::Osd).expect("osd projection");
        assert_eq!(doc.backgrounds["osd"], surface_background(&snapshot));
        assert_eq!(doc.backgrounds["osd"].a, 102);
        assert_eq!(doc.variables["--accent"].g, 0xff);
        assert!(!doc.backgrounds.contains_key("start-menu"));
    }

    #[test]
    fn start_projection_applies_background_and_accent_tokens() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "surfaceColor".to_owned(),
            SettingValue::Text("#102030".to_owned()),
        );
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(50));
        snapshot.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#00ff00".to_owned()),
        );
        snapshot
            .values
            .insert("accentOpacity".to_owned(), SettingValue::Percent(50));
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
        assert_eq!(
            doc.backgrounds["start-menu"],
            UiColor {
                r: 0x10,
                g: 0x20,
                b: 0x30,
                a: 127
            }
        );
        assert_eq!(
            doc.variables["--accent"],
            UiColor {
                r: 0,
                g: 255,
                b: 0,
                a: 127
            }
        );
        assert_eq!(doc.variables["--accent-soft"].a, 27);
        assert_eq!(doc.variables["--accent-strong"].a, 110);
    }

    #[test]
    fn start_projection_search_field_does_not_reapply_surface_opacity() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(0));
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
        assert_eq!(doc.backgrounds["start-menu"].a, 0);
        assert_eq!(doc.backgrounds["start-search-box"].a, 255);
    }

    #[test]
    fn launcher_projection_shares_start_surface_pair() {
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &SettingsSnapshot::new(1), SurfaceKind::Launcher).expect("launcher");
        project(&mut doc, &SettingsSnapshot::new(1), SurfaceKind::Start).expect("start");
        assert_eq!(
            doc.backgrounds["launcher-popup"],
            doc.backgrounds["start-menu"]
        );
        assert_eq!(doc.radii["launcher-popup"], doc.radii["start-menu"]);
    }

    #[test]
    fn default_projection_keeps_accent_tokens_at_stylesheet_values() {
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &SettingsSnapshot::new(1), SurfaceKind::Start).expect("start projection");
        let accent = UiColor {
            r: 0x7f,
            g: 0x87,
            b: 0x8e,
            a: 255,
        };
        assert_eq!(doc.variables["--accent"], accent);
        assert_eq!(
            doc.variables["--accent-soft"],
            UiColor { a: 0x38, ..accent }
        );
        assert_eq!(
            doc.variables["--accent-strong"],
            UiColor { a: 0xdd, ..accent }
        );
        assert_eq!(
            doc.backgrounds["start-menu"],
            UiColor {
                r: 0x19,
                g: 0x1b,
                b: 0x1d,
                a: 127
            }
        );
    }

    #[test]
    fn surface_background_uses_surface_keys_and_ignores_legacy_taskbar_keys() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "surfaceColor".to_owned(),
            SettingValue::Text("#102030".to_owned()),
        );
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(100));
        snapshot.values.insert(
            "taskbarColor".to_owned(),
            SettingValue::Text("#ff0000".to_owned()),
        );
        snapshot
            .values
            .insert("taskbarOpacity".to_owned(), SettingValue::Percent(0));
        assert_eq!(
            surface_background(&snapshot),
            UiColor {
                r: 0x10,
                g: 0x20,
                b: 0x30,
                a: 255
            }
        );
        assert_eq!(
            surface_background(&SettingsSnapshot::new(1)),
            UiColor {
                r: 0x19,
                g: 0x1b,
                b: 0x1d,
                a: 127
            }
        );
    }

    #[test]
    fn popup_and_start_corner_radius_follow_decoration_setting() {
        let _accent = lock_task_accent();
        let mut doc = RecordingUiDoc::default();
        project_menu_background(
            &mut doc,
            &snapshot_with("decoration", "rounded"),
            "audio-popup",
        )
        .expect("rounded popup");
        assert_eq!(doc.radii["audio-popup"], 12.0);
        project_menu_background(
            &mut doc,
            &snapshot_with("decoration", "squared"),
            "audio-popup",
        )
        .expect("squared popup");
        assert_eq!(doc.radii["audio-popup"], 0.0);
        project(
            &mut doc,
            &snapshot_with("decoration", "squared"),
            SurfaceKind::Start,
        )
        .expect("start");
        assert_eq!(doc.radii["start-menu"], 0.0);
    }

    #[test]
    fn start_and_popups_share_surface_pair_and_ignore_legacy_startmenu_keys() {
        let _accent = lock_task_accent();
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "surfaceColor".to_owned(),
            SettingValue::Text("#445566".to_owned()),
        );
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(10));
        snapshot.values.insert(
            "startMenuColor".to_owned(),
            SettingValue::Text("#102030".to_owned()),
        );
        snapshot
            .values
            .insert("startMenuOpacity".to_owned(), SettingValue::Percent(90));
        let mut doc = RecordingUiDoc::default();
        project_menu_background(&mut doc, &snapshot, "audio-popup").expect("popup");
        assert_eq!(
            doc.backgrounds["audio-popup"],
            UiColor {
                r: 0x44,
                g: 0x55,
                b: 0x66,
                a: 25
            }
        );
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("start");
        assert_eq!(
            doc.backgrounds["start-menu"],
            doc.backgrounds["audio-popup"]
        );
    }

    #[test]
    fn taskbar_projection_and_backdrop_follow_surface_keys() {
        let _accent = lock_task_accent();
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "surfaceColor".to_owned(),
            SettingValue::Text("#445566".to_owned()),
        );
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(20));
        snapshot
            .values
            .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(15));
        let mut doc = RecordingUiDoc::default();
        project(&mut doc, &snapshot, SurfaceKind::Panel).expect("panel");
        // `taskbarSeparate` defaults on: the root paints no tint so the wallpaper shows between
        // the per-element capsules.
        assert_eq!(
            doc.borders["taskbar"].a, 0,
            "separated bar root has no contour"
        );
        assert_eq!(
            doc.borders["workspace-pager"].a, 63,
            "pager matches other capsules"
        );
        assert_eq!(
            doc.backgrounds["workspace-pager"].a, 51,
            "pager uses shared surface fill"
        );
        assert_eq!(
            doc.backgrounds["taskbar"],
            UiColor {
                r: 0x44,
                g: 0x55,
                b: 0x66,
                a: 0
            }
        );
        snapshot
            .values
            .insert("taskbarSeparate".to_owned(), SettingValue::Boolean(false));
        let mut unified = RecordingUiDoc::default();
        project(&mut unified, &snapshot, SurfaceKind::Panel).expect("unified panel");
        assert_eq!(
            unified.backgrounds["taskbar"],
            UiColor {
                r: 0x44,
                g: 0x55,
                b: 0x66,
                a: 51
            }
        );
        assert!(
            unified.borders["taskbar"].a > 0,
            "unified bar owns its border"
        );
        assert_eq!(
            unified.borders["workspace-pager"].a, 0,
            "unified bar clears pager contour"
        );
        assert_eq!(taskbar_backdrop_percents(&snapshot), (20, 15));
    }

    #[test]
    fn popup_menu_background_projects_live_accent_to_popup_and_task_accent() {
        let _accent = lock_task_accent();
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#2080e0".to_owned()),
        );
        let mut doc = RecordingUiDoc::default();
        project_menu_background(&mut doc, &snapshot, "wifi-popup").expect("popup background");
        let accent = UiColor {
            r: 0x20,
            g: 0x80,
            b: 0xe0,
            a: 0xff,
        };
        assert_eq!(doc.variables["--accent"], accent);
        assert_eq!(crate::projection::task_accent(), accent);
    }

    fn write_ppm_fixture(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("flamewm-start-icon-{}-{name}", std::process::id()));
        let mut bytes = b"P6\n2 2\n255\n".to_vec();
        bytes.extend_from_slice(&[200u8; 12]);
        std::fs::write(&path, bytes).expect("write ppm fixture");
        path
    }

    #[test]
    fn configured_start_icon_projects_a_square_raster_into_the_panel_node() {
        let path = write_ppm_fixture("ok.ppm");
        let mut document = RecordingUiDoc::default();
        project_start_icon(&mut document, Some(path.display().to_string())).expect("project");
        let _ = std::fs::remove_file(&path);
        // A 2x2 source must be scaled to the 25px box, not kept at natural size.
        assert_eq!(document.images.get(START_ICON_ID), Some(&(25, 25)));
    }

    #[test]
    fn clearing_custom_start_icon_reverts_live_to_packaged_default() {
        let path = write_ppm_fixture("clear.ppm");
        let mut document = RecordingUiDoc::default();
        project_start_icon(&mut document, Some(path.display().to_string())).expect("apply");
        let _ = std::fs::remove_file(&path);
        assert_eq!(document.images.get(START_ICON_ID), Some(&(25, 25)));
        project_start_icon(&mut document, None).expect("clear");
        assert!(!document.images.contains_key(START_ICON_ID));
    }

    #[test]
    fn missing_none_or_unsupported_start_icon_keeps_the_packaged_default() {
        let mut document = RecordingUiDoc::default();
        project_start_icon(&mut document, None).expect("none");
        project_start_icon(&mut document, Some("/nonexistent/start.png".to_owned()))
            .expect("missing");
        let path = write_ppm_fixture("unsupported.txt");
        let renamed = path.with_extension("txt");
        std::fs::rename(&path, &renamed).expect("rename fixture");
        project_start_icon(&mut document, Some(renamed.display().to_string())).expect("txt");
        let _ = std::fs::remove_file(&renamed);
        assert!(document.images.is_empty());
    }

    #[test]
    fn custom_start_icon_is_none_for_empty_and_blank_values() {
        assert_eq!(custom_start_icon(&SettingsSnapshot::new(1)), None);
        assert_eq!(
            custom_start_icon(&snapshot_with("customStartIcon", "")),
            None
        );
        assert_eq!(
            custom_start_icon(&snapshot_with("customStartIcon", "   ")),
            None
        );
        assert_eq!(
            custom_start_icon(&snapshot_with(
                "customStartIcon",
                "/home/u/.local/share/flamewm/start.png"
            )),
            Some("/home/u/.local/share/flamewm/start.png".to_owned())
        );
    }

    #[test]
    fn start_backdrop_percents_read_flat_keys_with_defaults() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(
            start_backdrop_percents(&snapshot),
            (DEFAULT_SURFACE_OPACITY, DEFAULT_SURFACE_BLUR_PERCENT)
        );
        snapshot
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(50));
        snapshot
            .values
            .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(30));
        assert_eq!(start_backdrop_percents(&snapshot), (50, 30));
        snapshot
            .values
            .insert("startMenuOpacity".to_owned(), SettingValue::Percent(0));
        snapshot
            .values
            .insert("startMenuBlurPercent".to_owned(), SettingValue::Percent(0));
        assert_eq!(
            start_backdrop_percents(&snapshot),
            (50, 30),
            "legacy startMenu* keys are inert"
        );
    }

    fn snapshot_with(key: &str, value: &str) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert(key.to_owned(), SettingValue::Text(value.to_owned()));
        snapshot
    }

    #[test]
    fn font_mapping_applies_saved_family_and_size_offset() {
        let mut saved = SettingsSnapshot::new(2);
        saved.values.insert(
            "fontFamily".to_owned(),
            SettingValue::Text("DejaVu Sans".to_owned()),
        );
        saved
            .values
            .insert("fontSizeOffset".to_owned(), SettingValue::Integer(3));
        assert_eq!(
            text_setting(&saved, "fontFamily", DEFAULT_FONT_FAMILY),
            "DejaVu Sans"
        );
        assert_eq!(
            (
                integer_setting(&saved, "fontSizeOffset", 0) as f32,
                bool_setting(&saved, "fontBold", false)
            ),
            (3.0, false)
        );

        let defaults = SettingsSnapshot::new(1);
        assert_eq!(
            text_setting(&defaults, "fontFamily", DEFAULT_FONT_FAMILY),
            DEFAULT_FONT_FAMILY
        );
        assert_eq!(integer_setting(&defaults, "fontSizeOffset", 0), 0);

        let mut tiny = SettingsSnapshot::new(3);
        tiny.values
            .insert("fontSizeOffset".to_owned(), SettingValue::Integer(-40));
        assert_eq!(integer_setting(&tiny, "fontSizeOffset", 0), -40);
    }

    #[test]
    fn opacity_percent_maps_to_alpha_bounds() {
        assert_eq!(opacity_alpha(0), 0);
        assert_eq!(opacity_alpha(100), 255);
        assert_eq!(opacity_alpha(DEFAULT_SURFACE_OPACITY), 127);
    }

    #[test]
    fn accent_change_reprojects_every_accent_derived_token() {
        const TOKEN_ALPHAS: [(&str, u8); 9] = [
            ("--accent", 0xff),
            ("--accent-soft", 0x38),
            ("--accent-strong", 0xdd),
            ("--desktop-hover", 0x26),
            ("--desktop-hover-border", 0x4d),
            ("--desktop-selected", 0x47),
            ("--desktop-selected-border", 0xa6),
            ("--selection-fill", 0x33),
            ("--snap-fill", 0x33),
        ];
        for (hex, rgb) in [
            ("#3daee9", 0x003d_aee9_u32),
            ("#1abc9c", 0x001a_bc9c),
            ("#9b59b6", 0x009b_59b6),
        ] {
            let mut snapshot = SettingsSnapshot::new(1);
            snapshot
                .values
                .insert("accent".to_owned(), SettingValue::Text(hex.to_owned()));
            let mut doc = RecordingUiDoc::default();
            project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
            let base = UiColor {
                r: (rgb >> 16) as u8,
                g: (rgb >> 8) as u8,
                b: rgb as u8,
                a: 255,
            };
            for (name, alpha) in TOKEN_ALPHAS {
                assert_eq!(
                    doc.variables[name],
                    UiColor { a: alpha, ..base },
                    "{name} under {hex}"
                );
            }
        }
    }

    fn css_rules(css: &str) -> Vec<(&str, &str)> {
        css.split('}')
            .filter_map(|chunk| chunk.split_once('{'))
            .map(|(selector, body)| (selector.trim(), body))
            .collect()
    }

    fn border_radius_px(body: &str) -> Option<f32> {
        let rest = body.split("border-radius:").nth(1)?;
        rest.split(';')
            .next()?
            .trim()
            .trim_end_matches("px")
            .parse()
            .ok()
    }

    #[test]
    fn every_compiled_shell_surface_declares_accent_tokens() {
        let artifacts: [(&str, &[u8]); 10] = [
            (
                "launcher",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-launcher.rwr")),
            ),
            (
                "panel",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr")),
            ),
            (
                "start",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-start.rwr")),
            ),
            (
                "task-menu",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-task-menu.rwr")),
            ),
            (
                "media",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-media.rwr")),
            ),
            (
                "audio",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr")),
            ),
            (
                "network",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr")),
            ),
            (
                "calendar",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-calendar.rwr")),
            ),
            (
                "osd",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-osd.rwr")),
            ),
            (
                "battery",
                include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr")),
            ),
        ];
        for (surface, bytes) in artifacts {
            let mut document = flamewm_ui_x11::decode_document(bytes)
                .unwrap_or_else(|error| panic!("{surface} artifact must decode: {error}"));
            project_accent_tokens(&mut document, &SettingsSnapshot::new(1))
                .unwrap_or_else(|error| panic!("{surface} must declare accent tokens: {error}"));
        }
    }

    #[test]
    fn shell_css_has_no_hardcoded_default_red() {
        let css = include_str!("../../../ui/shell/flamewm.css");
        for line in css.lines().filter(|line| line.contains("ef4048")) {
            assert!(
                line.contains(".swatch-red"),
                "hardcoded accent outside swatch: {line}"
            );
        }
        assert!(
            !css.contains("e81123"),
            "close hover still hard-codes a red"
        );
    }

    #[test]
    fn shell_rust_projections_have_no_hardcoded_red_fill() {
        let sources = [
            include_str!("projection.rs"),
            include_str!("async_projection.rs"),
            include_str!("projection/network.rs"),
            include_str!("projection/audio.rs"),
        ];
        for source in sources {
            assert!(
                !source.contains("r: 239"),
                "hardcoded flame red fill in shell projection"
            );
        }
    }

    #[test]
    fn workspace_pager_rules_stay_square() {
        let css = include_str!("../../../ui/shell/flamewm.css");
        let rules = css_rules(css);
        for (selector, body) in &rules {
            if selector.contains("workspace-button") {
                if let Some(radius) = border_radius_px(body) {
                    assert!(radius <= 1.0, "pager radius {radius}px in `{selector}`");
                }
            }
        }
        for required in [".workspace-button:hover", ".workspace-button.active"] {
            let body = rules
                .iter()
                .find(|(selector, _)| *selector == required)
                .map(|(_, body)| *body)
                .expect(required);
            assert_eq!(border_radius_px(body), Some(1.0), "{required} radius");
        }
    }

    #[test]
    fn accent_carries_accent_opacity_as_alpha() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("accentOpacity".to_owned(), SettingValue::Percent(0));
        let transparent = accent_color(&snapshot);
        assert_eq!(
            (transparent.r, transparent.g, transparent.b, transparent.a),
            (0x7f, 0x87, 0x8e, 0)
        );

        snapshot
            .values
            .insert("accentOpacity".to_owned(), SettingValue::Percent(100));
        assert_eq!(accent_color(&snapshot).a, 255);
        assert_eq!(accent_color(&SettingsSnapshot::new(1)).a, 255);
    }

    #[test]
    fn color_setting_parses_hex_and_falls_back_to_default() {
        let red = color_setting(&snapshot_with("surfaceColor", "#FF0000"), "surfaceColor", 0);
        assert_eq!((red.r, red.g, red.b, red.a), (255, 0, 0, 255));
        let green = color_setting(&snapshot_with("accent", "00ff00"), "accent", 0);
        assert_eq!((green.r, green.g, green.b), (0, 255, 0));
        let signed = color_setting(
            &snapshot_with("accent", "#+12345"),
            "accent",
            DEFAULT_ACCENT,
        );
        assert_eq!(signed, rgb(DEFAULT_ACCENT, 255));
        let bad = color_setting(
            &snapshot_with("accent", "#GG0000"),
            "accent",
            DEFAULT_ACCENT,
        );
        assert_eq!(bad, rgb(DEFAULT_ACCENT, 255));
        let missing = color_setting(&SettingsSnapshot::new(1), "surfaceColor", 0x0019_1b1d);
        assert_eq!(missing.r, 0x19);
    }

    fn slide_snapshot(pairs: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in pairs {
            snapshot.values.insert((*key).to_owned(), value.clone());
        }
        snapshot
    }

    const START_BUTTON: (i32, i32, u32, u32) = (20, 1050, 48, 36);

    #[test]
    fn start_slide_maps_each_panel_edge() {
        let snapshot = SettingsSnapshot::new(1);
        let edge_of = |edge| start_slide(&snapshot, edge, None).map(|slide| slide.edge);
        assert_eq!(edge_of(PanelEdge::Bottom), Some(SlideEdge::Bottom));
        assert_eq!(edge_of(PanelEdge::Top), Some(SlideEdge::Top));
        assert_eq!(edge_of(PanelEdge::Left), Some(SlideEdge::Left));
        assert_eq!(edge_of(PanelEdge::Right), Some(SlideEdge::Right));
    }

    #[test]
    fn start_slide_missing_keys_default_enabled_at_base_speed() {
        let slide = start_slide(&SettingsSnapshot::new(1), PanelEdge::Bottom, None).expect("slide");
        assert_eq!(slide, SlideConfig::slide(SlideEdge::Bottom, 100));
    }

    #[test]
    fn start_slide_disabled_by_each_flag_for_every_style() {
        let off = SettingValue::Boolean(false);
        for style in ["slide", "fade", "magic-lamp"] {
            let style = SettingValue::Text(style.to_owned());
            let cases = [
                slide_snapshot(&[
                    ("animationsEnabled", off.clone()),
                    ("startAnimation", style.clone()),
                ]),
                slide_snapshot(&[
                    ("startAnimationEnabled", off.clone()),
                    ("startAnimation", style.clone()),
                ]),
            ];
            for snapshot in &cases {
                assert_eq!(
                    start_slide(snapshot, PanelEdge::Bottom, Some(START_BUTTON)),
                    None
                );
            }
        }
    }

    #[test]
    fn start_slide_none_and_unknown_styles_are_immediate() {
        for style in ["none", "bounce"] {
            let snapshot =
                slide_snapshot(&[("startAnimation", SettingValue::Text(style.to_owned()))]);
            assert_eq!(
                start_slide(&snapshot, PanelEdge::Bottom, Some(START_BUTTON)),
                None,
                "{style}"
            );
        }
    }

    #[test]
    fn start_slide_slide_and_legacy_slide_up_keep_the_slide() {
        for style in ["slide", "slide-up"] {
            let snapshot =
                slide_snapshot(&[("startAnimation", SettingValue::Text(style.to_owned()))]);
            let slide = start_slide(&snapshot, PanelEdge::Top, None).expect(style);
            assert_eq!(slide, SlideConfig::slide(SlideEdge::Top, 100), "{style}");
        }
    }

    #[test]
    fn start_slide_fade_has_no_target_and_keeps_the_speed() {
        let snapshot = slide_snapshot(&[
            ("startAnimation", SettingValue::Text("fade".to_owned())),
            ("animationSpeed", SettingValue::Integer(50)),
        ]);
        assert_eq!(
            start_slide(&snapshot, PanelEdge::Left, None),
            Some(SlideConfig::fade(SlideEdge::Left, 50))
        );
    }

    #[test]
    fn start_slide_magic_lamp_folds_into_the_start_button() {
        let snapshot = slide_snapshot(&[(
            "startAnimation",
            SettingValue::Text("magic-lamp".to_owned()),
        )]);
        assert_eq!(
            start_slide(&snapshot, PanelEdge::Bottom, Some(START_BUTTON)),
            Some(SlideConfig::lamp(SlideEdge::Bottom, 100, START_BUTTON))
        );
        // Without button layout the lamp cannot fold anywhere, so the show stays immediate.
        assert_eq!(start_slide(&snapshot, PanelEdge::Bottom, None), None);
    }

    #[test]
    fn start_slide_clamps_speed_to_supported_range() {
        let speed = |value| {
            let snapshot = slide_snapshot(&[("animationSpeed", SettingValue::Integer(value))]);
            start_slide(&snapshot, PanelEdge::Top, None)
                .expect("slide")
                .speed
        };
        assert_eq!(speed(10), 25);
        assert_eq!(speed(25), 25);
        assert_eq!(speed(150), 150);
        assert_eq!(speed(999), 200);
    }

    #[test]
    fn icon_theme_name_prefers_snapshot_then_product_default() {
        let set = snapshot_with("iconTheme", "ThemeX");
        assert_eq!(icon_theme_name(Some(&set)), "ThemeX");
        assert_eq!(icon_theme_name(None), "PlagueSur");
        assert_eq!(
            icon_theme_name(Some(&SettingsSnapshot::new(1))),
            "PlagueSur"
        );
    }
}

#[cfg(test)]
mod clock_settings_tests {
    use super::clock_display;
    use crate::projection::ClockDisplay;
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use flamewm_platform::settings::ClockFormat;

    #[test]
    fn missing_keys_resolve_to_product_clock_defaults() {
        assert_eq!(
            clock_display(&SettingsSnapshot::new(1)),
            ClockDisplay::default()
        );
    }

    #[test]
    fn clock_keys_map_to_display_and_unknown_format_falls_back_to_24h() {
        let mut snapshot = SettingsSnapshot::new(2);
        snapshot.values.insert(
            "taskbarClockFormat".to_owned(),
            SettingValue::Text("12h".to_owned()),
        );
        snapshot
            .values
            .insert("taskbarShowSeconds".to_owned(), SettingValue::Boolean(true));
        snapshot
            .values
            .insert("taskbarShowDate".to_owned(), SettingValue::Boolean(false));
        let display = clock_display(&snapshot);
        assert_eq!(display.format, ClockFormat::Twelve);
        assert!(display.show_seconds);
        assert!(!display.show_date);

        snapshot.values.insert(
            "taskbarClockFormat".to_owned(),
            SettingValue::Text("bogus".to_owned()),
        );
        assert_eq!(clock_display(&snapshot).format, ClockFormat::TwentyFour);
    }
}
