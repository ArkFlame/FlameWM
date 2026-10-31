//! Split from `lib.rs` (`labels`); see crate docs.

use super::*;

#[must_use]
pub fn panel_edge_label(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Bottom => "Bottom",
        PanelEdge::Top => "Top",
        PanelEdge::Left => "Left",
        PanelEdge::Right => "Right",
    }
}

/// Label for one display-mode slot, e.g. `1920 x 1080 @ 60.00 Hz · current`.
#[must_use]
pub fn display_mode_label(mode: &flamewm_api::display::DisplayMode, current: bool) -> String {
    let tag = if current {
        " · current"
    } else if mode.preferred {
        " · preferred"
    } else {
        ""
    };
    format!(
        "{} x {} @ {:.2} Hz{tag}",
        mode.resolution.width,
        mode.resolution.height,
        f64::from(mode.refresh_millihz) / 1000.0
    )
}

#[must_use]
pub fn font_size_label(offset: i64) -> String {
    match offset {
        0 => "Default".to_owned(),
        value => format!("{value:+}"),
    }
}

/// Profile picture label: the managed file name, or `Default` for the built-in ring icon.
pub(crate) fn avatar_label(path: &str) -> String {
    if path.is_empty() {
        "Default".to_owned()
    } else {
        file_name_label(path)
    }
}

/// Start icon label: the managed file name, or `Built-in icon` for the default logo.
pub(crate) fn start_icon_label(path: &str) -> String {
    if path.is_empty() {
        "Built-in icon".to_owned()
    } else {
        file_name_label(path)
    }
}

pub(crate) fn file_name_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_owned()
}

/// Shows the first 8 characters of a saved key followed by dots; an unset key reads as the built-in key.
pub(crate) fn masked_api_key(key: &str) -> String {
    if key.is_empty() {
        return "Built-in key".to_owned();
    }
    let head: String = key.chars().take(8).collect();
    format!("{head}\u{2022}\u{2022}\u{2022}\u{2022}")
}
