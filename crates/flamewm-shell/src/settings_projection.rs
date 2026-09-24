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
    let font_size_offset = integer_setting(snapshot, "fontSizeOffset", 0) as f32;

    document.font_family(font_family)?;
    document.font_size(root, (DEFAULT_FONT_SIZE + font_size_offset).max(6.0))?;
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
