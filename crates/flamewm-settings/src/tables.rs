//! Split from `lib.rs` (`tables`); see crate docs.

use super::*;

/// Color rows in the Appearance page: action target and the `#RRGGBB` setting.
pub(crate) const COLOR_TARGETS: [(&str, &str); 5] = [
    ("border", "surfaceBorderColor"),
    ("surface", "surfaceColor"),
    ("accent", "accent"),
    ("lockbackground", "lockBackgroundColor"),
    ("taskbarTint", "taskbarCustomTint"),
];

/// Minutes offered by the lock-screen idle timeout segments; 0 means never.
pub(crate) const LOCK_TIMEOUT_CHOICES: [i64; 5] = [0, 5, 10, 15, 30];

/// Cursor sizes offered by the Appearance segments; the wire value is the pixel size.
pub(crate) const CURSOR_SIZE_CHOICES: [i64; 4] = [24, 32, 48, 64];

/// Wallpaper placements cycled by the Desktop selector, in wire order.
pub(crate) const WALLPAPER_FITS: [&str; 5] = ["fill", "fit", "stretch", "center", "tile"];

pub(crate) fn wallpaper_fit_label(value: &str) -> String {
    let label = match value {
        "fill" => "Fill",
        "fit" => "Fit",
        "stretch" => "Stretch",
        "center" => "Center",
        "tile" => "Tile",
        other => other,
    };
    label.to_owned()
}

/// A stepper row: its HTML slug (`settings.<slug>.up|down`), the integer key it writes, and the
/// step size and inclusive bounds.
pub(crate) struct IntegerStepper {
    pub(crate) slug: &'static str,
    pub(crate) key: &'static str,
    pub(crate) step: i64,
    pub(crate) minimum: i64,
    pub(crate) maximum: i64,
}

pub(crate) const INTEGER_STEPPERS: [IntegerStepper; 9] = [
    IntegerStepper {
        slug: "window-margin",
        key: "windowMargin",
        step: 1,
        minimum: 0,
        maximum: 32,
    },
    IntegerStepper {
        slug: "taskbar-floating-margin",
        key: "taskbarFloatingMargin",
        step: 1,
        minimum: 0,
        maximum: 32,
    },
    IntegerStepper {
        slug: "taskbar-floating-radius",
        key: "taskbarFloatingRadius",
        step: 1,
        minimum: 0,
        maximum: 24,
    },
    IntegerStepper {
        slug: "screensaver-after-lock",
        key: "screensaverAfterLockMinutes",
        step: 1,
        minimum: 0,
        maximum: 60,
    },
    IntegerStepper {
        slug: "screensaver-ribbons",
        key: "screensaverNumRibbons",
        step: 1,
        minimum: 1,
        maximum: 12,
    },
    IntegerStepper {
        slug: "screensaver-ribbon-width",
        key: "screensaverRibbonWidthTenths",
        step: 5,
        minimum: 20,
        maximum: 280,
    },
    IntegerStepper {
        slug: "screensaver-display-off",
        key: "screensaverDisplayOffMinutes",
        step: 5,
        minimum: 0,
        maximum: 240,
    },
    IntegerStepper {
        slug: "lock-bg-blur",
        key: "lockBackgroundBlurPercent",
        step: 5,
        minimum: 0,
        maximum: 100,
    },
    IntegerStepper {
        slug: "lock-bg-dim",
        key: "lockBackgroundDimPercent",
        step: 5,
        minimum: 0,
        maximum: 80,
    },
];

/// Preset circles offered beside each surface color row; `preset.<index>` selects one.
pub(crate) const SURFACE_PRESETS: [&str; 6] = [
    "#191B1D", "#336699", "#3DAEE9", "#1ABC9C", "#EF4048", "#9B59B6",
];

/// Colour picker palette: hue columns by shade rows, then one row of greys.
pub(crate) const PICKER_HUES: usize = 12;
pub(crate) const PICKER_SHADES: usize = 6;
pub(crate) const PICKER_GREYS: usize = 6;
/// Typed key text from the native key source, see `keyboard_text`.
pub(crate) const KEYBOARD_INPUT_ACTION: &str = "keyboard.input";
/// Longest buffer the picker accepts: `#` plus six hex digits.
pub(crate) const PICKER_BUFFER_MAX: usize = 7;

/// State of the one shared colour picker: the setting it writes and its hex buffer.
pub(crate) struct ColorPicker {
    pub(crate) key: &'static str,
    pub(crate) buffer: String,
    pub(crate) focused: bool,
    pub(crate) error: Option<String>,
}

/// Animation style rows in the Animations card: stable key and the values its segments write.
/// Each segment's node id is `animation_style_node(key, value)`.
pub(crate) const ANIMATION_STYLE_ROWS: [(&str, &[&str]); 5] = [
    ("startAnimation", &["none", "slide", "fade", "magic-lamp"]),
    ("minimizeAnimation", &["none", "fade", "magic-lamp"]),
    ("maximizeAnimation", &["none", "fade", "scale"]),
    ("windowOpenAnimation", &["none", "scale", "fade"]),
    ("windowCloseAnimation", &["none", "fade"]),
];

/// Node id of one animation style segment, e.g. `startAnimation` + `magic-lamp`.
pub(crate) fn animation_style_node(key: &str, value: &str) -> String {
    let row = match key {
        "startAnimation" => "start-style",
        "minimizeAnimation" => "minimize-style",
        "maximizeAnimation" => "maximize-style",
        "windowCloseAnimation" => "window-close-style",
        _ => "window-open-style",
    };
    format!("{row}-{value}")
}

/// Highlights each `(value, node id)` option that equals `current`; a missing value clears them all.
pub(crate) fn highlight_options(
    document: &mut impl UiDocumentAccess,
    current: Option<&str>,
    options: &[(&str, &str)],
    accent: UiColor,
) -> Result<(), String> {
    for (value, id) in options {
        highlight_segment(document, id, current == Some(*value), accent)?;
    }
    Ok(())
}

/// Paints one segment with the accent when selected, and clears its highlight otherwise.
pub(crate) fn highlight_segment(
    document: &mut impl UiDocumentAccess,
    id: &str,
    selected: bool,
    accent: UiColor,
) -> Result<(), String> {
    if selected {
        document.background(id, accent)
    } else {
        document.background_clear(id)
    }
}

/// Parses `<target>.preset.<0..5>`, the rest after `settings.color.`.
pub(crate) fn parse_color_preset(rest: &str) -> Result<SettingsAction, String> {
    let (target, rest) = rest
        .split_once(".preset.")
        .ok_or_else(|| format!("invalid color action '{rest}'"))?;
    let key = color_target_key(target)?;
    let index = rest
        .parse::<usize>()
        .ok()
        .and_then(|index| SURFACE_PRESETS.get(index))
        .ok_or_else(|| format!("unknown color preset '{rest}'"))?;
    Ok(SettingsAction::ColorSwatch { key, value: index })
}

/// Parses the picker actions, the rest after `settings.picker.`.
pub(crate) fn parse_color_picker(rest: &str) -> Result<SettingsAction, String> {
    match rest {
        "close" => return Ok(SettingsAction::ColorPickerClose),
        "apply" => return Ok(SettingsAction::ColorPickerApply),
        "hex.focus" => return Ok(SettingsAction::ColorPickerFocus),
        _ => {}
    }
    if let Some(target) = rest.strip_prefix("open.") {
        return Ok(SettingsAction::ColorPickerOpen(color_target_key(target)?));
    }
    let cell = rest
        .strip_prefix("pick.")
        .ok_or_else(|| format!("unknown picker action '{rest}'"))?;
    if let Some(step) = cell.strip_prefix("grey.") {
        let step = step
            .parse::<usize>()
            .ok()
            .filter(|step| *step < PICKER_GREYS)
            .ok_or_else(|| format!("unknown picker grey '{step}'"))?;
        return Ok(SettingsAction::ColorPickerPick(picker_grey(step)));
    }
    let (hue, shade) = cell
        .split_once('.')
        .and_then(|(hue, shade)| Some((hue.parse::<usize>().ok()?, shade.parse::<usize>().ok()?)))
        .filter(|(hue, shade)| *hue < PICKER_HUES && *shade < PICKER_SHADES)
        .ok_or_else(|| format!("unknown picker cell '{cell}'"))?;
    Ok(SettingsAction::ColorPickerPick(picker_grid_color(
        hue, shade,
    )))
}

pub(crate) fn color_target_key(target: &str) -> Result<&'static str, String> {
    COLOR_TARGETS
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, key)| *key)
        .ok_or_else(|| format!("unknown color target '{target}'"))
}

/// Parses `#RRGGBB` (the leading `#` is optional) and returns it uppercase with `#`.
pub(crate) fn parse_hex_color(text: &str) -> Result<String, String> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(format!("#{}", digits.to_ascii_uppercase()))
    } else {
        Err(format!("'{text}' is not a #RRGGBB colour"))
    }
}

/// Palette cell at `hue` (`0..PICKER_HUES`) and `shade` (`0..PICKER_SHADES)`: HSL with
/// fixed saturation and six lightness steps, lightest first.
pub(crate) fn picker_grid_color(hue: usize, shade: usize) -> String {
    const LIGHTNESS: [f64; PICKER_SHADES] = [0.88, 0.72, 0.56, 0.42, 0.28, 0.14];
    let lightness = LIGHTNESS[shade];
    let spread = (1.0 - (2.0 * lightness - 1.0).abs()) * 0.75;
    let sector = hue as f64 / 2.0;
    let x = spread * (1.0 - ((sector % 2.0) - 1.0).abs());
    let (r, g, b) = match sector.floor() as usize {
        0 => (spread, x, 0.0),
        1 => (x, spread, 0.0),
        2 => (0.0, spread, x),
        3 => (0.0, x, spread),
        4 => (x, 0.0, spread),
        _ => (spread, 0.0, x),
    };
    let offset = lightness - spread / 2.0;
    let channel = |value: f64| ((value + offset) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", channel(r), channel(g), channel(b))
}

/// Neutral palette cell `step` (`0..PICKER_GREYS`), black to white.
pub(crate) fn picker_grey(step: usize) -> String {
    let value = (step * 255 / (PICKER_GREYS - 1)) as u8;
    format!("#{value:02X}{value:02X}{value:02X}")
}

/// Next value in `values`, wrapping at both ends. An unknown current value
/// starts from the first entry.
pub(crate) fn cycle_value(values: &[i64], current: i64, direction: i8) -> i64 {
    let len = values.len();
    if len == 0 {
        return current;
    }
    let index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    let next = if direction < 0 {
        (index + len - 1) % len
    } else {
        (index + 1) % len
    };
    values[next]
}

pub(crate) fn cycle_text<'a>(values: &'a [&'a str], current: &str) -> &'a str {
    let index = values
        .iter()
        .position(|value| value.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    values[(index + 1) % values.len()]
}

pub(crate) fn next_edge(edge: PanelEdge) -> PanelEdge {
    match edge {
        PanelEdge::Bottom => PanelEdge::Top,
        PanelEdge::Top => PanelEdge::Left,
        PanelEdge::Left => PanelEdge::Right,
        PanelEdge::Right => PanelEdge::Bottom,
    }
}

pub(crate) fn hotkey_row(slot: &str) -> Option<&'static HotkeyRow> {
    HOTKEY_ROWS.iter().find(|row| row.slot == slot)
}

/// Display label for a shortcut action owner, or the raw owner if it has no row.
pub(crate) fn shortcut_label(owner: &str) -> String {
    HOTKEY_ROWS
        .iter()
        .find(|row| row.action == owner)
        .map_or_else(|| owner.to_owned(), |row| row.label.to_owned())
}

