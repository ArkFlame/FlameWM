//! Dock surface geometry and document projection. The layout is pure: slot,
//! dot, separator and label rectangles are computed in surface CSS px, then
//! written into the single `dock.html` template with absolute positions, so one
//! template serves every edge.

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_control_core::ControlRequest;
use flamewm_shell_core::dock::{
    merge_dock, DockEntry, DockLayout, DockMenuAction, Edge, MAX_PINNED, TRASH_SLOT_ID,
};
use flamewm_ui_document::UiDocumentAccess;

/// Typed `dock.*` settings. Missing keys take the spec defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct DockSettings {
    pub enabled: bool,
    pub edge: Edge,
    pub icon_size: f32,
    pub pinned: Vec<String>,
    pub show_running: bool,
    /// `dock.showTrash`: the Trash slot after the dock apps.
    pub show_trash: bool,
    /// `dock.hoverScale`: the hovered icon grows by `HOVER_SCALE` (subtle mode).
    pub hover_scale: bool,
    /// `dock.autoHideCovered`: with auto-hide on, hide only while a window covers the dock.
    pub auto_hide_covered: bool,
    /// `dock.launchBounce`: the icon of a pending launch hops until its first window.
    pub launch_bounce: bool,
    /// `dock.autoHide`: the dock slides away when idle and reveals at its edge.
    pub auto_hide: bool,
    /// `dock.cornerStyle` is `squared`: the dock pill takes zero corner radius. Read-only here; the
    /// radius is chosen in `pill_corner_radius` and the bar's `corner_radius` document call.
    pub squared_corners: bool,
}

#[must_use]
pub fn dock_settings(snapshot: Option<&SettingsSnapshot>) -> DockSettings {
    let Some(snapshot) = snapshot else {
        return DockSettings {
            enabled: false,
            edge: Edge::Bottom,
            icon_size: 32.0,
            pinned: Vec::new(),
            show_running: true,
            show_trash: true,
            hover_scale: false,
            auto_hide_covered: false,
            launch_bounce: false,
            auto_hide: false,
            squared_corners: false,
        };
    };
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => Some(value.as_str()),
        _ => None,
    };
    let enabled = matches!(
        snapshot.values.get("dock.enabled"),
        Some(SettingValue::Boolean(true))
    );
    let show_running = !matches!(
        snapshot.values.get("dock.showRunning"),
        Some(SettingValue::Boolean(false))
    );
    let show_trash = !matches!(
        snapshot.values.get("dock.showTrash"),
        Some(SettingValue::Boolean(false))
    );
    let hover_scale = matches!(
        snapshot.values.get("dock.hoverScale"),
        Some(SettingValue::Boolean(true))
    );
    let auto_hide_covered = matches!(
        snapshot.values.get("dock.autoHideCovered"),
        Some(SettingValue::Boolean(true))
    );
    let launch_bounce = matches!(
        snapshot.values.get("dock.launchBounce"),
        Some(SettingValue::Boolean(true))
    );
    let edge = match text("dock.position") {
        Some("left") => Edge::Left,
        Some("right") => Edge::Right,
        Some("top") => Edge::Top,
        _ => Edge::Bottom,
    };
    let icon_size = match snapshot.values.get("dock.iconSize") {
        Some(SettingValue::Integer(px)) => (*px).clamp(24, 96) as f32,
        _ => 32.0,
    };
    // Missing `dock.autoHide` is the default (off); the settings file always writes the key.
    let auto_hide = matches!(
        snapshot.values.get("dock.autoHide"),
        Some(SettingValue::Boolean(true))
    );
    let squared_corners = text("dock.cornerStyle") == Some("squared");
    let mut pinned: Vec<String> = Vec::new();
    for id in text("dock.pinned").unwrap_or("").split(',').map(str::trim) {
        if !id.is_empty()
            && pinned.len() < MAX_PINNED
            && flamewm_platform::settings::is_valid_dock_app_id(id)
            && !pinned.iter().any(|known| known == id)
        {
            pinned.push(id.to_owned());
        }
    }
    DockSettings {
        enabled,
        edge,
        icon_size,
        pinned,
        show_running,
        show_trash,
        hover_scale,
        auto_hide_covered,
        launch_bounce,
        auto_hide,
        squared_corners,
    }
}

/// Unique app ids of the windows listed in the task surfaces, in window order.
#[must_use]
pub fn running_apps(windows: &[WindowSnapshot]) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for window in windows.iter().filter(|window| !window.skip_taskbar) {
        let id = window.app_id.as_str();
        if !id.is_empty() && !apps.iter().any(|known| known == id) {
            apps.push(id.to_owned());
        }
    }
    apps
}

/// Dock layout for the current settings and windows.
#[must_use]
pub fn layout_for(settings: &DockSettings, windows: &[WindowSnapshot]) -> DockLayout {
    let mut layout = merge_dock(
        &settings.pinned,
        &running_apps(windows),
        settings.show_running,
    );
    if settings.show_trash {
        // Trash is the last slot, after its own separator (drawn in addition to any group separator).
        layout.trash_separator_after = layout.entries.len().checked_sub(1);
        layout.entries.push(DockEntry {
            app_id: TRASH_SLOT_ID.to_owned(),
            pinned: false,
            running: false,
        });
    }
    layout
}

/// `$XDG_DATA_HOME` or `~/.local/share`: the base of the freedesktop Trash folder.
#[must_use]
pub fn trash_data_home() -> std::path::PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_default()
}

/// The Trash `files/` folder the Trash slot opens.
#[must_use]
pub fn trash_files_dir() -> std::path::PathBuf {
    trash_data_home().join("Trash/files")
}

/// Click on an app slot: restore a minimised window, focus a running one, or
/// launch an app with no window. Uses the most recent focused window first.
#[must_use]
pub fn click_request(app_id: &str, windows: &[WindowSnapshot]) -> ControlRequest {
    let candidates = || {
        windows
            .iter()
            .filter(|w| !w.skip_taskbar && w.app_id.as_str() == app_id)
    };
    let window = candidates()
        .find(|w| w.focused)
        .or_else(|| candidates().next());
    match window {
        Some(window) if window.state == WindowState::Minimized => {
            ControlRequest::RestoreWindow(window.reference)
        }
        Some(window) => ControlRequest::ActivateWindow(window.reference),
        None => ControlRequest::LaunchApplication {
            app: flamewm_api::DesktopAppId::new(app_id),
            options: ApplicationLaunchOptions::default(),
        },
    }
}

/// Document slots available in `dock.html`; entries beyond this are not shown.
pub const MAX_SLOTS: usize = 24;
const PAD: f32 = 6.0;
const GAP: f32 = 3.0;
/// Gap between the bar and the screen edge it floats above, in CSS px.
pub const DOCK_MARGIN: f32 = 8.0;
/// Thickness of the reveal strip left on the edge while the dock is hidden, in CSS px.
pub const REVEAL_PX: f32 = 3.0;
/// Hover label size and its gap above (bottom) or below (top) the bar, in CSS px. The label is
/// its own popup surface, so the dock window is exactly the bar.
pub const LABEL_W: f32 = 160.0;
pub const LABEL_H: f32 = 22.0;
const LABEL_GAP: f32 = 4.0;
const DOT: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DockFrame {
    /// Surface size in CSS px.
    pub width: f32,
    pub height: f32,
    pub bar: Rect,
    /// One per visible entry, in surface coordinates.
    pub slots: Vec<Rect>,
    pub icons: Vec<Rect>,
    pub dots: Vec<Option<Rect>>,
    pub labels: Vec<Rect>,
    /// Group and trash separators, in slot order; at most two.
    pub separators: Vec<Rect>,
    pub edge: Edge,
}

/// Lays out `running.len()` slots (one per dock entry, `running[i]` drives the
/// dot) on `edge`. `icon` is the icon edge in CSS px.
#[must_use]
pub fn dock_frame(
    running: &[bool],
    separator_after: Option<usize>,
    trash_separator_after: Option<usize>,
    icon: f32,
    edge: Edge,
) -> DockFrame {
    let count = running.len().min(MAX_SLOTS);
    let slot = icon + 2.0 * GAP;
    let vertical = edge.is_vertical();
    let main = count as f32 * slot + 2.0 * PAD;
    let cross = slot + 2.0 * PAD;
    let (width, height, bar) = if vertical {
        (
            cross,
            main,
            Rect {
                x: 0.0,
                y: 0.0,
                w: cross,
                h: main,
            },
        )
    } else {
        (
            main,
            cross,
            Rect {
                x: 0.0,
                y: 0.0,
                w: main,
                h: cross,
            },
        )
    };
    // Maps a bar-local (along, across) pair to surface coordinates for a rect of the given size.
    let map = |along: f32, across: f32, w: f32, h: f32| -> Rect {
        if vertical {
            Rect {
                x: bar.x + across,
                y: bar.y + along,
                w,
                h,
            }
        } else {
            Rect {
                x: bar.x + along,
                y: bar.y + across,
                w,
                h,
            }
        }
    };
    let mut slots = Vec::with_capacity(count);
    let mut icons = Vec::with_capacity(count);
    let mut dots = Vec::with_capacity(count);
    let mut labels = Vec::with_capacity(count);
    for (i, running) in running.iter().take(count).enumerate() {
        let along = PAD + i as f32 * slot;
        let s = map(along, PAD, slot, slot);
        slots.push(s);
        icons.push(Rect {
            x: s.x + GAP,
            y: s.y + GAP,
            w: icon,
            h: icon,
        });
        dots.push(running.then(|| {
            if vertical {
                Rect {
                    x: s.x + 1.0,
                    y: s.y + (slot - DOT) / 2.0,
                    w: DOT,
                    h: DOT,
                }
            } else {
                Rect {
                    x: s.x + (slot - DOT) / 2.0,
                    y: s.y + slot - DOT - 1.0,
                    w: DOT,
                    h: DOT,
                }
            }
        }));
        labels.push(if vertical {
            Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            }
        } else {
            // Window-space rect: above the bar on bottom, below it on top (outside the window).
            let label_y = if edge == Edge::Bottom {
                bar.y - LABEL_GAP - LABEL_H
            } else {
                bar.y + bar.h + LABEL_GAP
            };
            Rect {
                x: s.x + slot / 2.0 - LABEL_W / 2.0,
                y: label_y,
                w: LABEL_W,
                h: LABEL_H,
            }
        });
    }
    let separators = [separator_after, trash_separator_after]
        .into_iter()
        .flatten()
        .filter(|after| *after + 1 < count)
        .map(|after| {
            let at = PAD + (after + 1) as f32 * slot - 0.5;
            if vertical {
                Rect {
                    x: bar.x + PAD + 4.0,
                    y: bar.y + at,
                    w: cross - 2.0 * (PAD + 4.0),
                    h: 1.0,
                }
            } else {
                Rect {
                    x: bar.x + at,
                    y: bar.y + PAD + 4.0,
                    w: 1.0,
                    h: cross - 2.0 * (PAD + 4.0),
                }
            }
        })
        .collect();
    DockFrame {
        width,
        height,
        bar,
        slots,
        icons,
        dots,
        labels,
        separators,
        edge,
    }
}

/// Device-px placement of a dock surface on `output` (device px). `inset` is
/// the reserved strip on the dock's edge (for example the taskbar height when
/// both sit on the same edge). Centred along the edge, floating `DOCK_MARGIN`
/// away from the screen edge.
#[must_use]
pub fn place(
    frame: &DockFrame,
    output: (i32, i32, i32, i32),
    scale: f32,
    inset: i32,
) -> (i32, i32, u32, u32) {
    let w = (frame.width * scale).round() as i32;
    let h = (frame.height * scale).round() as i32;
    let margin = device_px(DOCK_MARGIN, scale);
    let (ox, oy, ow, oh) = output;
    let (x, y) = match frame.edge {
        Edge::Bottom => (ox + (ow - w) / 2, oy + oh - h - inset - margin),
        Edge::Top => (ox + (ow - w) / 2, oy + inset + margin),
        Edge::Left => (ox + inset + margin, oy + (oh - h) / 2),
        Edge::Right => (ox + ow - w - inset - margin, oy + (oh - h) / 2),
    };
    (x, y, w.max(1) as u32, h.max(1) as u32)
}

fn device_px(css: f32, scale: f32) -> i32 {
    (css * scale).round() as i32
}

/// Device-px distance from `placed` to the point where it sits wholly past its screen edge on
/// `output`. The taskbar inset is part of the distance, so a hidden dock clears the taskbar too.
#[must_use]
pub fn slide_extent(placed: (i32, i32, u32, u32), edge: Edge, output: (i32, i32, i32, i32)) -> i32 {
    let (x, y, w, h) = placed;
    let (ox, oy, ow, oh) = output;
    match edge {
        Edge::Bottom => oy + oh - y,
        Edge::Top => y + h as i32 - oy,
        Edge::Left => x + w as i32 - ox,
        Edge::Right => ox + ow - x,
    }
}

/// `placed` moved `offset` device px toward its screen edge. Offset 0 is shown; the full
/// `slide_extent` is hidden.
#[must_use]
pub fn slide_rect(placed: (i32, i32, u32, u32), edge: Edge, offset: i32) -> (i32, i32, u32, u32) {
    let (x, y, w, h) = placed;
    match edge {
        Edge::Bottom => (x, y + offset, w, h),
        Edge::Top => (x, y - offset, w, h),
        Edge::Left => (x - offset, y, w, h),
        Edge::Right => (x + offset, y, w, h),
    }
}

/// Thin strip on `edge` of `output` spanning the dock's shown extent. It is the only
/// pointer target while the dock is unmapped.
#[must_use]
pub fn reveal_rect(
    placed: (i32, i32, u32, u32),
    output: (i32, i32, i32, i32),
    edge: Edge,
    scale: f32,
) -> (i32, i32, u32, u32) {
    let (x, y, w, h) = placed;
    let (ox, oy, ow, oh) = output;
    let px = device_px(REVEAL_PX, scale).max(1);
    match edge {
        Edge::Bottom => (x, oy + oh - px, w, px as u32),
        Edge::Top => (x, oy, w, px as u32),
        Edge::Left => (ox, y, px as u32, h),
        Edge::Right => (ox + ow - px, y, px as u32, h),
    }
}

fn rect_to_css(rect: Rect) -> (f32, f32, f32, f32) {
    (rect.x, rect.y, rect.w, rect.h)
}

/// Inner highlight of the pill: a 1 px line just inside the bar's top edge (its left edge when the
/// bar is vertical), inset by the rounded ends so it follows the curve.
fn highlight_css(bar: Rect) -> (f32, f32, f32, f32) {
    let radius = bar.w.min(bar.h) / 2.0;
    if bar.w >= bar.h {
        (
            bar.x + radius,
            bar.y + 1.0,
            (bar.w - 2.0 * radius).max(0.0),
            1.0,
        )
    } else {
        (
            bar.x + 1.0,
            bar.y + radius,
            1.0,
            (bar.h - 2.0 * radius).max(0.0),
        )
    }
}

/// Writes one frame into `dock.html`. Hover labels are separate popups (`sync_dock_label`).
pub fn project_dock(document: &mut impl UiDocumentAccess, frame: &DockFrame) -> Result<(), String> {
    for i in 0..MAX_SLOTS {
        let n = i + 1;
        let slot_id = format!("dock-slot-{n}");
        let icon_id = format!("dock-slot-{n}-icon");
        let dot_id = format!("dock-slot-{n}-dot");
        let Some(slot) = frame.slots.get(i).copied() else {
            for id in [&slot_id, &icon_id, &dot_id] {
                document.visible(id, false)?;
            }
            continue;
        };
        let (x, y, w, h) = rect_to_css(slot);
        document.visible(&slot_id, true)?;
        document.position(&slot_id, x, y)?;
        document.size(&slot_id, w, h)?;
        // Icon and dot are children of the slot button, so their
        // positions are slot-local.
        let local = |r: Rect| (r.x - slot.x, r.y - slot.y, r.w, r.h);
        let (ix, iy, iw, ih) = local(frame.icons[i]);
        document.visible(&icon_id, true)?;
        document.position(&icon_id, ix, iy)?;
        document.size(&icon_id, iw, ih)?;
        match frame.dots[i] {
            Some(dot) => {
                let (dx, dy, dw, dh) = local(dot);
                document.visible(&dot_id, true)?;
                document.position(&dot_id, dx, dy)?;
                document.size(&dot_id, dw, dh)?;
            }
            None => document.visible(&dot_id, false)?,
        }
    }
    let (bx, by, bw, bh) = rect_to_css(frame.bar);
    document.position("dock-bar", bx, by)?;
    document.size("dock-bar", bw, bh)?;
    let radius = frame.bar.w.min(frame.bar.h) / 2.0;
    document.corner_radius("dock-bar", radius)?;
    let (hx, hy, hw, hh) = highlight_css(frame.bar);
    document.position("dock-highlight", hx, hy)?;
    document.size("dock-highlight", hw, hh)?;
    for (id, sep) in [
        ("dock-separator", frame.separators.first()),
        ("dock-separator-2", frame.separators.get(1)),
    ] {
        match sep {
            Some(sep) => {
                let (x, y, w, h) = rect_to_css(*sep);
                document.visible(id, true)?;
                document.position(id, x, y)?;
                document.size(id, w, h)?;
            }
            None => document.visible(id, false)?,
        }
    }
    Ok(())
}

/// Window corner radius (device px) of the stadium pill: half the bar thickness.
#[must_use]
pub fn pill_corner_radius(bar_css: f32, scale: f32) -> u32 {
    (bar_css * scale / 2.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_defaults_are_32_px_without_auto_hide_and_rounded() {
        let defaults = dock_settings(None);
        assert_eq!(defaults.icon_size, 32.0);
        assert!(!defaults.auto_hide);
        assert!(!defaults.squared_corners);
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "dock.cornerStyle".into(),
            SettingValue::Text("squared".into()),
        );
        let squared = dock_settings(Some(&snapshot));
        assert!(squared.squared_corners);
        assert!(!squared.auto_hide, "a missing dock.autoHide is off");
        assert_eq!(squared.icon_size, 32.0);
    }

    #[test]
    fn trash_slot_is_last_after_a_separator_and_follows_the_setting() {
        let mut settings = DockSettings {
            enabled: true,
            edge: Edge::Bottom,
            icon_size: 44.0,
            pinned: vec!["term".to_owned()],
            show_running: false,
            show_trash: true,
            hover_scale: false,
            auto_hide_covered: false,
            launch_bounce: false,
            auto_hide: false,
            squared_corners: false,
        };
        let layout = layout_for(&settings, &[]);
        let ids: Vec<&str> = layout.entries.iter().map(|e| e.app_id.as_str()).collect();
        assert_eq!(ids, ["term", TRASH_SLOT_ID]);
        assert_eq!(layout.separator_after, None);
        assert_eq!(layout.trash_separator_after, Some(0));
        assert!(!layout.entries[1].pinned && !layout.entries[1].running);
        settings.show_trash = false;
        assert_eq!(layout_for(&settings, &[]).entries.len(), 1);
    }

    #[test]
    fn window_is_exactly_the_bar_and_labels_sit_outside_it() {
        let frame = dock_frame(&[true, false], None, None, 44.0, Edge::Bottom);
        // slot = 44 + 2*3 = 50, main = 2*50 + 2*6 = 112, cross = 50 + 12 = 62.
        assert_eq!(frame.width, 112.0);
        assert_eq!(frame.height, 62.0);
        assert_eq!(frame.bar.y, 0.0);
        // Bottom label sits above the bar, outside the window, and clears the bar by the gap.
        let label = frame.labels[0];
        assert_eq!(label.y + label.h + LABEL_GAP, frame.bar.y);
        assert!(label.y < 0.0);
        let top = dock_frame(&[true], None, None, 44.0, Edge::Top);
        assert_eq!(top.labels[0].y, top.bar.y + top.bar.h + LABEL_GAP);
        assert_eq!(frame.slots[1].x, PAD + 50.0);
        assert!(frame.dots[0].is_some() && frame.dots[1].is_none());
    }

    #[test]
    fn vertical_bar_runs_down_the_edge_without_labels() {
        let frame = dock_frame(&[false, false, false], None, None, 44.0, Edge::Left);
        assert_eq!(frame.width, 62.0);
        assert_eq!(frame.height, 3.0 * 50.0 + 12.0);
        assert_eq!(frame.slots[2].y, PAD + 100.0);
        assert!(frame.labels.iter().all(|l| l.w == 0.0));
    }

    #[test]
    fn dock_css_takes_no_hardcoded_colour() {
        const CSS: &str = include_str!("../../../ui/shell/flamewm.css");
        for selector in [".dock-bar {", ".dock-highlight {"] {
            let start = CSS
                .find(selector)
                .unwrap_or_else(|| panic!("{selector} rule"));
            let rule = &CSS[start..start + CSS[start..].find('}').expect("rule end")];
            assert!(
                !rule.contains('#'),
                "{selector} must not hardcode colour: {rule}"
            );
            assert!(
                !rule.contains("var(--dock-"),
                "{selector} must use shared tokens: {rule}"
            );
        }
    }

    #[test]
    fn highlight_follows_the_pill_top_edge_inside_the_rim() {
        let horizontal = Rect {
            x: 100.0,
            y: 700.0,
            w: 300.0,
            h: 60.0,
        };
        // Top line, 1 px inside the rim, inset by the 30 px rounded ends.
        assert_eq!(highlight_css(horizontal), (130.0, 701.0, 240.0, 1.0));
        let vertical = Rect {
            x: 8.0,
            y: 100.0,
            w: 60.0,
            h: 300.0,
        };
        assert_eq!(highlight_css(vertical), (9.0, 130.0, 1.0, 240.0));
    }

    #[test]
    fn separator_sits_on_group_boundary_only() {
        let split = dock_frame(&[true, true, false], Some(1), None, 44.0, Edge::Bottom);
        let sep = split.separators.first().copied().expect("separator");
        assert!((sep.x - (PAD + 2.0 * 50.0 - 0.5)).abs() < 1e-3);
        assert!(dock_frame(&[true], None, None, 44.0, Edge::Bottom)
            .separators
            .is_empty());
        assert!(dock_frame(&[true], Some(0), None, 44.0, Edge::Bottom)
            .separators
            .is_empty());
    }

    #[test]
    fn group_and_trash_separators_are_both_drawn() {
        // Same steps as `layout_for`: merge the pinned and running groups, then append Trash.
        let mut layout =
            flamewm_shell_core::dock::merge_dock(&["term".to_owned()], &["mail".to_owned()], true);
        layout.trash_separator_after = layout.entries.len().checked_sub(1);
        layout.entries.push(DockEntry {
            app_id: TRASH_SLOT_ID.to_owned(),
            pinned: false,
            running: false,
        });
        let ids: Vec<&str> = layout.entries.iter().map(|e| e.app_id.as_str()).collect();
        assert_eq!(ids, ["term", "mail", TRASH_SLOT_ID]);
        assert_eq!(layout.separator_after, Some(0));
        assert_eq!(layout.trash_separator_after, Some(1));
        let running: Vec<bool> = layout.entries.iter().map(|e| e.running).collect();
        let frame = dock_frame(
            &running,
            layout.separator_after,
            layout.trash_separator_after,
            44.0,
            Edge::Bottom,
        );
        assert_eq!(frame.separators.len(), 2);
        // Group boundary after slot 0, trash boundary after slot 1.
        assert!((frame.separators[0].x - (PAD + 50.0 - 0.5)).abs() < 1e-3);
        assert!((frame.separators[1].x - (PAD + 2.0 * 50.0 - 0.5)).abs() < 1e-3);
    }

    #[test]
    fn slot_count_is_capped() {
        let running = vec![false; MAX_SLOTS + 5];
        assert_eq!(
            dock_frame(&running, None, None, 44.0, Edge::Bottom)
                .slots
                .len(),
            MAX_SLOTS
        );
    }

    #[test]
    fn placement_floats_margin_from_its_edge() {
        let frame = dock_frame(&[true], None, None, 44.0, Edge::Bottom);
        let (x, y, w, h) = place(&frame, (0, 0, 1000, 800), 1.0, 44);
        assert_eq!((w, h), (62, 62));
        assert_eq!(x, (1000 - 62) / 2);
        assert_eq!(y, 800 - h as i32 - 44 - DOCK_MARGIN as i32);
        let vertical = dock_frame(&[true], None, None, 44.0, Edge::Right);
        let (vx, _, _, _) = place(&vertical, (0, 0, 1000, 800), 1.0, 0);
        assert_eq!(vx, 1000 - 62 - DOCK_MARGIN as i32);
        let top = dock_frame(&[true], None, None, 44.0, Edge::Top);
        assert_eq!(place(&top, (0, 0, 1000, 800), 1.0, 0).1, DOCK_MARGIN as i32);
        let left = dock_frame(&[true], None, None, 44.0, Edge::Left);
        assert_eq!(place(&left, (0, 0, 1000, 800), 2.0, 0).0, 16);
    }

    #[test]
    fn slide_moves_the_dock_fully_past_its_edge() {
        let output = (0, 0, 1000, 800);
        let placed = (100, 700, 500, 90);
        let extent = slide_extent(placed, Edge::Bottom, output);
        assert_eq!(extent, 100);
        // Fully hidden: the top edge sits exactly on the screen bottom (800).
        assert_eq!(slide_rect(placed, Edge::Bottom, extent).1, 800);
        assert_eq!(slide_rect(placed, Edge::Bottom, 0), placed);
        // Above a taskbar inset the hidden point is still the screen edge, not the bar edge.
        let inset = (100, 656, 500, 90);
        assert_eq!(slide_extent(inset, Edge::Bottom, output), 144);
        assert_eq!(slide_rect(inset, Edge::Bottom, 144).1, 800);
        let right = (900, 300, 62, 200);
        let right_extent = slide_extent(right, Edge::Right, output);
        assert_eq!(right_extent, 100);
        assert_eq!(slide_rect(right, Edge::Right, right_extent).0, 1000);
        let top = (100, 8, 500, 90);
        assert_eq!(
            slide_rect(top, Edge::Top, slide_extent(top, Edge::Top, output)).1,
            -90
        );
        assert_eq!(slide_rect(right, Edge::Left, 10).0, 890);
    }

    #[test]
    fn reveal_strip_hugs_the_edge_across_the_shown_extent() {
        let output = (0, 0, 1000, 800);
        assert_eq!(
            reveal_rect((100, 700, 500, 90), output, Edge::Bottom, 1.0),
            (100, 797, 500, 3)
        );
        assert_eq!(
            reveal_rect((0, 300, 62, 200), output, Edge::Left, 2.0),
            (0, 300, 6, 200)
        );
        assert_eq!(
            reveal_rect((938, 300, 62, 200), output, Edge::Right, 1.0),
            (997, 300, 3, 200)
        );
        assert_eq!(
            reveal_rect((100, 8, 500, 90), output, Edge::Top, 1.0),
            (100, 0, 500, 3)
        );
    }
}

/// Visible label for one right-click menu item.
#[must_use]
pub fn menu_label(action: DockMenuAction) -> &'static str {
    match action {
        DockMenuAction::KeepInDock => "Keep in dock",
        DockMenuAction::RemoveFromDock => "Remove from dock",
        DockMenuAction::Open => "Open",
        DockMenuAction::CloseWindows => "Close windows",
    }
}

#[cfg(test)]
mod menu_label_tests {
    use super::*;

    #[test]
    fn menu_labels_are_distinct_and_non_empty() {
        let labels = [
            menu_label(DockMenuAction::KeepInDock),
            menu_label(DockMenuAction::RemoveFromDock),
            menu_label(DockMenuAction::Open),
            menu_label(DockMenuAction::CloseWindows),
        ];
        for (i, label) in labels.iter().enumerate() {
            assert!(!label.is_empty());
            assert!(!labels[..i].contains(label), "duplicate label {label}");
        }
    }

    #[test]
    fn pill_corner_radius_is_half_bar_in_device_px() {
        assert_eq!(pill_corner_radius(62.0, 1.0), 31);
        assert_eq!(pill_corner_radius(62.0, 2.0), 62);
        assert_eq!(pill_corner_radius(0.0, 1.0), 0);
    }
}
