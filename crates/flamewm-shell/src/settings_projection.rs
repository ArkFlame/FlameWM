use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_ui_x11::{UiColor, UiDocumentAccess};

const DEFAULT_ACCENT: u32 = 0xef4048;
const DEFAULT_TASKBAR_COLOR: u32 = 0x191b1d;
const DEFAULT_TASKBAR_OPACITY: u8 = 99;
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
    let accent = color_setting(snapshot, "accent", DEFAULT_ACCENT);
    let font_family = text_setting(snapshot, "fontFamily", DEFAULT_FONT_FAMILY);
    let font_bold = bool_setting(snapshot, "fontBold", false);
    let font_size = font_size_px(snapshot);

    document.font_family(font_family)?;
    document.font_size(root, font_size)?;
    document.font_weight(root, if font_bold { 700 } else { 400 })?;

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
                    a: ((u16::from(opacity) * 255) / 100) as u8,
                },
            )?;
            // Stable presentation node; this never changes panel geometry.
            document.background("taskbar-line", accent)?;
        }
        SurfaceKind::Start | SurfaceKind::TaskMenu | SurfaceKind::Media => {
            document.border(root, accent)?;
        }
    }
    Ok(())
}

fn font_size_px(snapshot: &SettingsSnapshot) -> f32 {
    let font_size_offset = integer_setting(snapshot, "fontSizeOffset", 0) as f32;
    (DEFAULT_FONT_SIZE + font_size_offset).max(6.0)
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
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return rgb(default, 255);
    }
    let Ok(value) = u32::from_str_radix(digits, 16) else {
        return rgb(default, 255);
    };
    rgb(value, 255)
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
    fn color_setting_parses_hex_and_falls_back_to_default() {
        let red = color_setting(&snapshot_with("taskbarColor", "#FF0000"), "taskbarColor", 0);
        assert_eq!((red.r, red.g, red.b, red.a), (255, 0, 0, 255));
        let green = color_setting(&snapshot_with("accent", "00ff00"), "accent", 0);
        assert_eq!((green.r, green.g, green.b), (0, 255, 0));
        let bad = color_setting(&snapshot_with("accent", "#GG0000"), "accent", DEFAULT_ACCENT);
        assert_eq!(
            (bad.r, bad.g, bad.b),
            ((DEFAULT_ACCENT >> 16) as u8, (DEFAULT_ACCENT >> 8) as u8, DEFAULT_ACCENT as u8)
        );
        let missing = color_setting(&SettingsSnapshot::new(1), "taskbarColor", DEFAULT_TASKBAR_COLOR);
        assert_eq!(missing.r, 0x19);
    }
}
