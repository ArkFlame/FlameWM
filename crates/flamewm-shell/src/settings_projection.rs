use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::PanelEdge;
use flamewm_platform::chrome::{parse_hex_rgb, ACCENT_SOFT_ALPHA};
use flamewm_ui_x11::{SlideConfig, SlideEdge, UiColor, UiDocumentAccess};

const DEFAULT_ACCENT: u32 = 0xef4048;
const DEFAULT_ANIMATION_SPEED: i64 = 100;
const DEFAULT_START_ANIMATION: &str = "slide-up";
const DEFAULT_TASKBAR_COLOR: u32 = 0x191b1d;
const DEFAULT_TASKBAR_OPACITY: u8 = 99;
const DEFAULT_START_MENU_COLOR: u32 = 0x191b1d;
const DEFAULT_START_MENU_OPACITY: u8 = 99;
const DEFAULT_ACCENT_OPACITY: u8 = 100;
const DEFAULT_FONT_FAMILY: &str = "IBM Plex Sans";
const DEFAULT_FONT_SIZE: f32 = 13.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceKind {
    Panel,
    Start,
    TaskMenu,
    Media,
}

/// Settings are presentation input only. Panel placement, edge, and size are
/// deliberately absent from this projection and remain service-owned.
pub(crate) fn project(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
    surface: SurfaceKind,
) -> Result<(), String> {
    let root = match surface {
        SurfaceKind::Panel => "taskbar",
        SurfaceKind::Start => "start-menu",
        SurfaceKind::TaskMenu => "task-menu",
        SurfaceKind::Media => "media-popup",
    };
    let accent = accent_color(snapshot);
    let font_family = text_setting(snapshot, "fontFamily", DEFAULT_FONT_FAMILY);
    let font_bold = bool_setting(snapshot, "fontBold", false);
    let font_size = font_size_px(snapshot);

    document.font_family(font_family)?;
    document.font_size(root, font_size)?;
    document.font_weight(root, if font_bold { 700 } else { 400 })?;
    // Accent tokens drive CSS var(--accent*) consumers in ui/shell/flamewm.css.
    document.color_variable("--accent", accent)?;
    document.color_variable("--accent-soft", accent_tint(snapshot, ACCENT_SOFT_ALPHA))?;
    document.color_variable("--accent-strong", accent_tint(snapshot, 0xdd))?;

    match surface {
        SurfaceKind::Panel => {
            let taskbar = color_setting(snapshot, "taskbarColor", DEFAULT_TASKBAR_COLOR);
            let opacity = percent_setting(snapshot, "taskbarOpacity", DEFAULT_TASKBAR_OPACITY);
            document.background(
                "taskbar",
                UiColor {
                    r: taskbar.r,
                    g: taskbar.g,
                    b: taskbar.b,
                    a: opacity_alpha(opacity),
                },
            )?;
            // Stable presentation node; this never changes panel geometry.
            document.background("taskbar-line", accent)?;
        }
        SurfaceKind::Start => {
            let color = color_setting(snapshot, "startMenuColor", DEFAULT_START_MENU_COLOR);
            let opacity =
                percent_setting(snapshot, "startMenuOpacity", DEFAULT_START_MENU_OPACITY);
            document.background(
                root,
                UiColor {
                    a: opacity_alpha(opacity),
                    ..color
                },
            )?;
            // No accent outline: the menu's rounded panes cover the border on three sides, so an
            // accent-coloured border only ever showed as a red line down the left edge.
        }
        SurfaceKind::TaskMenu | SurfaceKind::Media => {
            document.border(root, accent)?;
        }
    }
    Ok(())
}

/// Start slide for the panel edge hosting Start. `None` keeps the immediate show.
pub(crate) fn start_slide(snapshot: &SettingsSnapshot, edge: PanelEdge) -> Option<SlideConfig> {
    if !bool_setting(snapshot, "animationsEnabled", true)
        || !bool_setting(snapshot, "startAnimationEnabled", true)
        || text_setting(snapshot, "startAnimation", DEFAULT_START_ANIMATION) != "slide-up"
    {
        return None;
    }
    let speed = integer_setting(snapshot, "animationSpeed", DEFAULT_ANIMATION_SPEED).clamp(25, 200);
    let edge = match edge {
        PanelEdge::Bottom => SlideEdge::Bottom,
        PanelEdge::Top => SlideEdge::Top,
        PanelEdge::Left => SlideEdge::Left,
        PanelEdge::Right => SlideEdge::Right,
    };
    Some(SlideConfig {
        edge,
        speed: speed as u16,
    })
}

/// Accent carries `accentOpacity` as alpha for every accent consumer.
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

fn font_size_px(snapshot: &SettingsSnapshot) -> f32 {
    let font_size_offset = integer_setting(snapshot, "fontSizeOffset", 0) as f32;
    (DEFAULT_FONT_SIZE + font_size_offset).max(6.0)
}

/// Flame-owned icon theme (`iconTheme`); unset resolves to the product default.
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

fn percent_setting(snapshot: &SettingsSnapshot, key: &str, default: u8) -> u8 {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => *value,
        _ => default,
    }
}

fn color_setting(snapshot: &SettingsSnapshot, key: &str, default: u32) -> UiColor {
    let value = match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value,
        _ => return rgb(default, 255),
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
    use flamewm_ui_core::style::UiLayer;
    use flamewm_ui_x11::{Overflow, RuntimeImage};
    use std::collections::HashMap;

    #[derive(Default)]
    struct RecordingDocument {
        backgrounds: HashMap<String, UiColor>,
        variables: HashMap<String, UiColor>,
    }

    impl UiDocumentAccess for RecordingDocument {
        fn text(&mut self, _: &str, _: String) -> Result<(), String> {
            Ok(())
        }
        fn image_rgb8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn visible(&mut self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn size(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.backgrounds.insert(id.to_owned(), color);
            Ok(())
        }
        fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn background_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn border_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            Ok(())
        }
        fn font_family(&mut self, _: String) -> Result<(), String> {
            Ok(())
        }
        fn overflow(&mut self, _: &str, _: Overflow, _: Overflow) -> Result<(), String> {
            Ok(())
        }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> {
            Ok(())
        }
        fn color_variable(&mut self, name: &str, color: UiColor) -> Result<(), String> {
            self.variables.insert(name.to_owned(), color);
            Ok(())
        }
    }

    #[test]
    fn start_projection_applies_background_and_accent_tokens() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert("startMenuColor".to_owned(), SettingValue::Text("#102030".to_owned()));
        snapshot.values.insert("startMenuOpacity".to_owned(), SettingValue::Percent(50));
        snapshot.values.insert("accent".to_owned(), SettingValue::Text("#00ff00".to_owned()));
        snapshot.values.insert("accentOpacity".to_owned(), SettingValue::Percent(50));
        let mut doc = RecordingDocument::default();
        project(&mut doc, &snapshot, SurfaceKind::Start).expect("start projection");
        assert_eq!(
            doc.backgrounds["start-menu"],
            UiColor { r: 0x10, g: 0x20, b: 0x30, a: 127 }
        );
        assert_eq!(doc.variables["--accent"], UiColor { r: 0, g: 255, b: 0, a: 127 });
        assert_eq!(doc.variables["--accent-soft"].a, 27);
        assert_eq!(doc.variables["--accent-strong"].a, 110);
    }

    #[test]
    fn default_projection_keeps_accent_tokens_at_stylesheet_values() {
        let mut doc = RecordingDocument::default();
        project(&mut doc, &SettingsSnapshot::new(1), SurfaceKind::Start).expect("start projection");
        let accent = UiColor { r: 0xef, g: 0x40, b: 0x48, a: 255 };
        assert_eq!(doc.variables["--accent"], accent);
        assert_eq!(doc.variables["--accent-soft"], UiColor { a: 0x38, ..accent });
        assert_eq!(doc.variables["--accent-strong"], UiColor { a: 0xdd, ..accent });
        assert_eq!(
            doc.backgrounds["start-menu"],
            UiColor { r: 0x19, g: 0x1b, b: 0x1d, a: 252 }
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
        saved
            .values
            .insert("fontFamily".to_owned(), SettingValue::Text("DejaVu Sans".to_owned()));
        saved
            .values
            .insert("fontSizeOffset".to_owned(), SettingValue::Integer(3));
        assert_eq!(text_setting(&saved, "fontFamily", DEFAULT_FONT_FAMILY), "DejaVu Sans");
        assert_eq!(font_size_px(&saved), 16.0);

        let defaults = SettingsSnapshot::new(1);
        assert_eq!(text_setting(&defaults, "fontFamily", DEFAULT_FONT_FAMILY), DEFAULT_FONT_FAMILY);
        assert_eq!(font_size_px(&defaults), DEFAULT_FONT_SIZE);

        let mut tiny = SettingsSnapshot::new(3);
        tiny.values
            .insert("fontSizeOffset".to_owned(), SettingValue::Integer(-40));
        assert_eq!(font_size_px(&tiny), 6.0);
    }

    #[test]
    fn opacity_percent_maps_to_alpha_bounds() {
        assert_eq!(opacity_alpha(0), 0);
        assert_eq!(opacity_alpha(100), 255);
        assert_eq!(opacity_alpha(DEFAULT_TASKBAR_OPACITY), 252);
    }

    #[test]
    fn accent_carries_accent_opacity_as_alpha() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("accentOpacity".to_owned(), SettingValue::Percent(0));
        let transparent = accent_color(&snapshot);
        assert_eq!((transparent.r, transparent.g, transparent.b, transparent.a), (0xef, 0x40, 0x48, 0));

        snapshot
            .values
            .insert("accentOpacity".to_owned(), SettingValue::Percent(100));
        assert_eq!(accent_color(&snapshot).a, 255);
        assert_eq!(accent_color(&SettingsSnapshot::new(1)).a, 255);
    }

    #[test]
    fn start_menu_color_and_opacity_fall_back_when_invalid() {
        let color = color_setting(
            &snapshot_with("startMenuColor", "not-a-color"),
            "startMenuColor",
            DEFAULT_START_MENU_COLOR,
        );
        assert_eq!((color.r, color.g, color.b), (0x19, 0x1b, 0x1d));
        assert_eq!(opacity_alpha(DEFAULT_START_MENU_OPACITY), 252);
    }

    #[test]
    fn color_setting_parses_hex_and_falls_back_to_default() {
        let red = color_setting(&snapshot_with("taskbarColor", "#FF0000"), "taskbarColor", 0);
        assert_eq!((red.r, red.g, red.b, red.a), (255, 0, 0, 255));
        let green = color_setting(&snapshot_with("accent", "00ff00"), "accent", 0);
        assert_eq!((green.r, green.g, green.b), (0, 255, 0));
        let signed = color_setting(&snapshot_with("accent", "#+12345"), "accent", DEFAULT_ACCENT);
        assert_eq!(signed, rgb(DEFAULT_ACCENT, 255));
        let bad = color_setting(&snapshot_with("accent", "#GG0000"), "accent", DEFAULT_ACCENT);
        assert_eq!(
            (bad.r, bad.g, bad.b),
            ((DEFAULT_ACCENT >> 16) as u8, (DEFAULT_ACCENT >> 8) as u8, DEFAULT_ACCENT as u8)
        );
        let missing = color_setting(&SettingsSnapshot::new(1), "taskbarColor", DEFAULT_TASKBAR_COLOR);
        assert_eq!(missing.r, 0x19);
    }

    fn slide_snapshot(pairs: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in pairs {
            snapshot.values.insert((*key).to_owned(), value.clone());
        }
        snapshot
    }

    #[test]
    fn start_slide_maps_each_panel_edge() {
        let snapshot = SettingsSnapshot::new(1);
        let edge_of = |edge| start_slide(&snapshot, edge).map(|slide| slide.edge);
        assert_eq!(edge_of(PanelEdge::Bottom), Some(SlideEdge::Bottom));
        assert_eq!(edge_of(PanelEdge::Top), Some(SlideEdge::Top));
        assert_eq!(edge_of(PanelEdge::Left), Some(SlideEdge::Left));
        assert_eq!(edge_of(PanelEdge::Right), Some(SlideEdge::Right));
    }

    #[test]
    fn start_slide_missing_keys_default_enabled_at_base_speed() {
        let slide = start_slide(&SettingsSnapshot::new(1), PanelEdge::Bottom).expect("slide");
        assert_eq!(slide.speed, 100);
    }

    #[test]
    fn start_slide_disabled_by_each_flag_or_other_animation() {
        let off = SettingValue::Boolean(false);
        let cases = [
            slide_snapshot(&[("animationsEnabled", off.clone())]),
            slide_snapshot(&[("startAnimationEnabled", off)]),
            slide_snapshot(&[("startAnimation", SettingValue::Text("fade".to_owned()))]),
        ];
        for snapshot in &cases {
            assert_eq!(start_slide(snapshot, PanelEdge::Bottom), None);
        }
    }

    #[test]
    fn start_slide_clamps_speed_to_supported_range() {
        let speed = |value| {
            let snapshot = slide_snapshot(&[("animationSpeed", SettingValue::Integer(value))]);
            start_slide(&snapshot, PanelEdge::Top).expect("slide").speed
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
        assert_eq!(icon_theme_name(None), "McMojave-circle");
        assert_eq!(icon_theme_name(Some(&SettingsSnapshot::new(1))), "McMojave-circle");
    }
}
