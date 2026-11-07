//! Dock surface geometry and document projection. The layout is pure: slot,
//! dot, separator and label rectangles are computed in surface CSS px, then
//! written into the single `dock.html` template with absolute positions, so one
//! template serves every edge.

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_control_core::ControlRequest;
use flamewm_shell_core::dock::{merge_dock, DockLayout, DockMenuAction, Edge, MAX_PINNED};
use flamewm_ui_document::UiDocumentAccess;

/// Typed `dock.*` settings. Missing keys take the spec defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct DockSettings {
    pub enabled: bool,
    pub edge: Edge,
    pub icon_size: f32,
    pub pinned: Vec<String>,
    pub show_running: bool,
}

#[must_use]
pub fn dock_settings(snapshot: Option<&SettingsSnapshot>) -> DockSettings {
    let Some(snapshot) = snapshot else {
        return DockSettings {
            enabled: false,
            edge: Edge::Bottom,
            icon_size: 48.0,
            pinned: Vec::new(),
            show_running: true,
        };
    };
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => Some(value.as_str()),
        _ => None,
    };
    let enabled = matches!(snapshot.values.get("dock.enabled"), Some(SettingValue::Boolean(true)));
    let show_running = !matches!(snapshot.values.get("dock.showRunning"), Some(SettingValue::Boolean(false)));
    let edge = match text("dock.position") {
        Some("left") => Edge::Left,
        Some("right") => Edge::Right,
        Some("top") => Edge::Top,
        _ => Edge::Bottom,
    };
    let icon_size = match snapshot.values.get("dock.iconSize") {
        Some(SettingValue::Integer(px)) => (*px).clamp(24, 96) as f32,
        _ => 48.0,
    };
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
    merge_dock(&settings.pinned, &running_apps(windows), settings.show_running)
}

/// Click on an app slot: restore a minimised window, focus a running one, or
/// launch an app with no window. Uses the most recent focused window first.
#[must_use]
pub fn click_request(app_id: &str, windows: &[WindowSnapshot]) -> ControlRequest {
    let candidates = || windows.iter().filter(|w| !w.skip_taskbar && w.app_id.as_str() == app_id);
    let window = candidates().find(|w| w.focused).or_else(|| candidates().next());
    match window {
        Some(window) if window.state == WindowState::Minimized => {
            ControlRequest::RestoreWindow(window.reference.clone())
        }
        Some(window) => ControlRequest::ActivateWindow(window.reference.clone()),
        None => ControlRequest::LaunchApplication {
            app: flamewm_api::DesktopAppId::new(app_id),
            options: ApplicationLaunchOptions::default(),
        },
    }
}

/// Document slots available in `dock.html`; entries beyond this are not shown.
pub const MAX_SLOTS: usize = 24;
const PAD: f32 = 8.0;
const GAP: f32 = 6.0;
/// Room beside the bar for the hover label (horizontal docks only).
const LABEL_ROOM: f32 = 28.0;
const LABEL_W: f32 = 160.0;
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
    pub separator: Option<Rect>,
    pub edge: Edge,
}

/// Lays out `running.len()` slots (one per dock entry, `running[i]` drives the
/// dot) on `edge`. `icon` is the icon edge in CSS px.
pub fn dock_frame(running: &[bool], separator_after: Option<usize>, icon: f32, edge: Edge) -> DockFrame {
    let count = running.len().min(MAX_SLOTS);
    let slot = icon + 2.0 * GAP;
    let vertical = edge.is_vertical();
    let main = count as f32 * slot + 2.0 * PAD;
    let cross = slot + 2.0 * PAD;
    let room = if vertical { 0.0 } else { LABEL_ROOM };
    // Bottom puts the label room above the bar; top puts it below.
    let bar_y = if edge == Edge::Bottom { room } else { 0.0 };
    let (width, height, bar) = if vertical {
        (cross, main, Rect { x: 0.0, y: 0.0, w: cross, h: main })
    } else {
        (main, cross + room, Rect { x: 0.0, y: bar_y, w: main, h: cross })
    };
    // Maps a bar-local (along, across) pair to surface coordinates for a rect of the given size.
    let map = |along: f32, across: f32, w: f32, h: f32| -> Rect {
        if vertical {
            Rect { x: bar.x + across, y: bar.y + along, w, h }
        } else {
            Rect { x: bar.x + along, y: bar.y + across, w, h }
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
        icons.push(Rect { x: s.x + GAP, y: s.y + GAP, w: icon, h: icon });
        dots.push(running.then(|| {
            if vertical {
                Rect { x: s.x + 1.0, y: s.y + (slot - DOT) / 2.0, w: DOT, h: DOT }
            } else {
                Rect { x: s.x + (slot - DOT) / 2.0, y: s.y + slot - DOT - 1.0, w: DOT, h: DOT }
            }
        }));
        labels.push(if vertical {
            Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }
        } else {
            let label_y = if edge == Edge::Bottom { 2.0 } else { bar.y + cross + 2.0 };
            Rect {
                x: s.x + slot / 2.0 - LABEL_W / 2.0,
                y: label_y,
                w: LABEL_W,
                h: LABEL_ROOM - 4.0,
            }
        });
    }
    let separator = separator_after
        .filter(|after| *after + 1 < count)
        .map(|after| {
            let at = PAD + (after + 1) as f32 * slot - 0.5;
            if vertical {
                Rect { x: bar.x + PAD + 4.0, y: bar.y + at, w: cross - 2.0 * (PAD + 4.0), h: 1.0 }
            } else {
                Rect { x: bar.x + at, y: bar.y + PAD + 4.0, w: 1.0, h: cross - 2.0 * (PAD + 4.0) }
            }
        });
    DockFrame {
        width,
        height,
        bar,
        slots,
        icons,
        dots,
        labels,
        separator,
        edge,
    }
}

/// Device-px placement of a dock surface on `output` (device px). `inset` is
/// the reserved strip on the dock's edge (for example the taskbar height when
/// both sit on the same edge). Centred along the edge.
pub fn place(frame: &DockFrame, output: (i32, i32, i32, i32), scale: f32, inset: i32) -> (i32, i32, u32, u32) {
    let w = (frame.width * scale).round() as i32;
    let h = (frame.height * scale).round() as i32;
    let (ox, oy, ow, oh) = output;
    let (x, y) = match frame.edge {
        Edge::Bottom => (ox + (ow - w) / 2, oy + oh - h - inset),
        Edge::Top => (ox + (ow - w) / 2, oy + inset),
        Edge::Left => (ox + inset, oy + (oh - h) / 2),
        Edge::Right => (ox + ow - w - inset, oy + (oh - h) / 2),
    };
    (x, y, w.max(1) as u32, h.max(1) as u32)
}

/// Shows the hover label of slot `hovered` (0-based) and hides the rest. The
/// renderer takes pseudo-states only on the rightmost selector, so hover is
/// driven from the shell's hover events rather than CSS.
pub fn project_hover(
    document: &mut impl UiDocumentAccess,
    hovered: Option<usize>,
) -> Result<(), String> {
    for i in 0..MAX_SLOTS {
        document.visible(&format!("dock-slot-{}-label", i + 1), hovered == Some(i))?;
    }
    Ok(())
}

fn rect_to_css(rect: Rect) -> (f32, f32, f32, f32) {
    (rect.x, rect.y, rect.w, rect.h)
}

/// Writes one frame into `dock.html`. `names` are app names in slot order.
pub fn project_dock(
    document: &mut impl UiDocumentAccess,
    frame: &DockFrame,
    names: &[String],
) -> Result<(), String> {
    for i in 0..MAX_SLOTS {
        let n = i + 1;
        let slot_id = format!("dock-slot-{n}");
        let icon_id = format!("dock-slot-{n}-icon");
        let dot_id = format!("dock-slot-{n}-dot");
        let label_id = format!("dock-slot-{n}-label");
        let Some(slot) = frame.slots.get(i).copied() else {
            for id in [&slot_id, &icon_id, &dot_id, &label_id] {
                document.visible(id, false)?;
            }
            continue;
        };
        let (x, y, w, h) = rect_to_css(slot);
        document.visible(&slot_id, true)?;
        document.position(&slot_id, x, y)?;
        document.size(&slot_id, w, h)?;
        // Icon, dot and label are children of the slot button, so their
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
        let label = frame.labels[i];
        if label.w > 0.0 {
            // Labels are direct children of the dock root, so surface coordinates apply.
            let (lx, ly, lw, lh) = rect_to_css(label);
            document.position(&label_id, lx, ly)?;
            document.size(&label_id, lw, lh)?;
        }
        let name = names.get(i).map_or("", String::as_str);
        document.text_ref(&label_id, if name.is_empty() { "\u{200b}" } else { name })?;
        document.visible(&label_id, false)?;
    }
    let (bx, by, bw, bh) = rect_to_css(frame.bar);
    document.position("dock-bar", bx, by)?;
    document.size("dock-bar", bw, bh)?;
    document.corner_radius("dock-bar", frame.bar.w.min(frame.bar.h) / 2.0)?;
    match frame.separator {
        Some(sep) => {
            let (x, y, w, h) = rect_to_css(sep);
            document.visible("dock-separator", true)?;
            document.position("dock-separator", x, y)?;
            document.size("dock-separator", w, h)?;
        }
        None => document.visible("dock-separator", false)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_bar_is_centred_with_label_room_above() {
        let frame = dock_frame(&[true, false], None, 48.0, Edge::Bottom);
        // slot = 60, main = 2*60 + 16 = 136, cross = 76.
        assert_eq!(frame.width, 136.0);
        assert_eq!(frame.height, 76.0 + LABEL_ROOM);
        assert_eq!(frame.bar.y, LABEL_ROOM);
        assert_eq!(frame.slots[1].x, PAD + 60.0);
        assert!(frame.dots[0].is_some() && frame.dots[1].is_none());
    }

    #[test]
    fn vertical_bar_runs_down_the_edge_without_labels() {
        let frame = dock_frame(&[false, false, false], None, 48.0, Edge::Left);
        assert_eq!(frame.width, 76.0);
        assert_eq!(frame.height, 3.0 * 60.0 + 16.0);
        assert_eq!(frame.slots[2].y, PAD + 120.0);
        assert!(frame.labels.iter().all(|l| l.w == 0.0));
    }

    #[test]
    fn separator_sits_on_group_boundary_only() {
        let split = dock_frame(&[true, true, false], Some(1), 48.0, Edge::Bottom);
        let sep = split.separator.expect("separator");
        assert!((sep.x - (PAD + 2.0 * 60.0 - 0.5)).abs() < 1e-3);
        assert!(dock_frame(&[true], None, 48.0, Edge::Bottom).separator.is_none());
        assert!(dock_frame(&[true], Some(0), 48.0, Edge::Bottom).separator.is_none());
    }

    #[test]
    fn slot_count_is_capped() {
        let running = vec![false; MAX_SLOTS + 5];
        assert_eq!(dock_frame(&running, None, 48.0, Edge::Bottom).slots.len(), MAX_SLOTS);
    }

    #[test]
    fn placement_respects_edge_and_inset() {
        let frame = dock_frame(&[true], None, 48.0, Edge::Bottom);
        let (x, y, w, h) = place(&frame, (0, 0, 1000, 800), 1.0, 44);
        assert_eq!((w, h), (76, 76 + LABEL_ROOM as u32));
        assert_eq!(x, (1000 - 76) / 2);
        assert_eq!(y, 800 - h as i32 - 44);
        let vertical = dock_frame(&[true], None, 48.0, Edge::Right);
        let (vx, _, _, _) = place(&vertical, (0, 0, 1000, 800), 1.0, 0);
        assert_eq!(vx, 1000 - 76);
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
}
