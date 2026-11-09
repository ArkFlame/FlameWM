//! Dock model: pinned-app list mutation, pinned/running merge order, and
//! gradient-edge geometry for the taskbar surface. Pure logic; no X11 or CSS
//! output lives here.

/// Screen edge a dock or gradient surface sits on or fades from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }

    /// CSS `linear-gradient` direction that starts opaque at this screen edge
    /// and ends transparent toward the screen interior.
    #[must_use]
    pub fn gradient_direction(self) -> &'static str {
        match self {
            Edge::Top => "to bottom",
            Edge::Bottom => "to top",
            Edge::Left => "to right",
            Edge::Right => "to left",
        }
    }
}

/// Maximum pinned entries kept; bounds the persisted `dock.pinned` list.
pub const MAX_PINNED: usize = 32;

/// Adds `app_id` at the end unless already pinned (dedupe). Returns `false`
/// when nothing changed (duplicate, empty id, or list full).
pub fn pin_app(pinned: &mut Vec<String>, app_id: &str) -> bool {
    if app_id.is_empty() || pinned.iter().any(|id| id == app_id) || pinned.len() >= MAX_PINNED {
        return false;
    }
    pinned.push(app_id.to_owned());
    true
}

/// Removes `app_id` if present. Returns whether it was removed.
pub fn unpin_app(pinned: &mut Vec<String>, app_id: &str) -> bool {
    let before = pinned.len();
    pinned.retain(|id| id != app_id);
    pinned.len() != before
}

/// Moves the entry at `from` so it lands at index `to` (clamped to the end).
/// Returns `false` for an out-of-range source.
pub fn move_pinned(pinned: &mut Vec<String>, from: usize, to: usize) -> bool {
    if from >= pinned.len() {
        return false;
    }
    let item = pinned.remove(from);
    let to = to.min(pinned.len());
    pinned.insert(to, item);
    true
}

/// One dock slot. `running` drives the activity dot; `pinned` decides which
/// side of the separator it sits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockEntry {
    pub app_id: String,
    pub pinned: bool,
    pub running: bool,
}

/// Merged dock layout: pinned apps in stored order, then (when enabled)
/// running-but-unpinned apps in first-seen order. `separator_after` is the
/// index after which a separator is drawn, present only when both groups are
/// non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockLayout {
    pub entries: Vec<DockEntry>,
    pub separator_after: Option<usize>,
}

/// Builds the dock layout. Running apps that are also pinned are shown once,
/// in their pinned position. Duplicate running ids are ignored. Unpinned
/// running apps are omitted entirely when `show_running` is false.
#[must_use]
pub fn merge_dock(pinned: &[String], running: &[String], show_running: bool) -> DockLayout {
    let mut entries: Vec<DockEntry> = Vec::with_capacity(pinned.len() + running.len());
    for id in pinned {
        if id.is_empty() || entries.iter().any(|e| &e.app_id == id) {
            continue;
        }
        entries.push(DockEntry {
            app_id: id.clone(),
            pinned: true,
            running: running.iter().any(|r| r == id),
        });
    }
    let pinned_count = entries.len();
    if show_running {
        for id in running {
            if id.is_empty() || entries.iter().any(|e| &e.app_id == id) {
                continue;
            }
            entries.push(DockEntry {
                app_id: id.clone(),
                pinned: false,
                running: true,
            });
        }
    }
    let separator_after =
        (pinned_count > 0 && entries.len() > pinned_count).then(|| pinned_count - 1);
    DockLayout {
        entries,
        separator_after,
    }
}

/// Slot index under a pointer position along the dock strip. `along` is the
/// pointer offset from the strip start in device px, `slot_extent` one slot
/// (icon plus gap). Clamped to `0..count`; `count == 0` yields 0.
#[must_use]
pub fn slot_at(along: f32, slot_extent: f32, count: usize) -> usize {
    if count == 0 || slot_extent <= 0.0 || along.is_nan() || along < 0.0 {
        return 0;
    }
    ((along / slot_extent) as usize).min(count - 1)
}

/// Whether a pinned icon released at `cross` px from the dock's inner edge
/// should be unpinned (dragged off the bar). The bar is removed when the
/// pointer leaves it by more than `threshold` px on the cross axis.
#[must_use]
pub fn drag_off_removes(cross: f32, bar_extent: f32, threshold: f32) -> bool {
    cross < -threshold || cross > bar_extent + threshold
}

/// Right-click menu entries for one dock slot, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockMenuAction {
    KeepInDock,
    RemoveFromDock,
    Open,
    CloseWindows,
}

/// Menu entries for a slot. Pinned slots offer Remove, unpinned offer Keep.
/// Open always shows; Close windows only when the app has a running window.
#[must_use]
pub fn menu_items(pinned: bool, running: bool) -> Vec<DockMenuAction> {
    let mut items = Vec::with_capacity(3);
    items.push(if pinned {
        DockMenuAction::RemoveFromDock
    } else {
        DockMenuAction::KeepInDock
    });
    items.push(DockMenuAction::Open);
    if running {
        items.push(DockMenuAction::CloseWindows);
    }
    items
}

/// Inline CSS `background-image` for a taskbar surface using the gradient
/// style. `surface` must be a `var(--token)` reference, never a raw colour, so
/// the fill keeps following the surface colour token. The gradient is opaque at
/// `edge` and fades to transparent toward the interior.
#[must_use]
pub fn gradient_background(edge: Edge, surface: &str) -> String {
    format!(
        "linear-gradient({}, {surface} 0%, transparent 100%)",
        edge.gradient_direction()
    )
}

/// Straight RGBA8 ramp for a gradient taskbar: `steps` samples from `alpha` (opaque) at the
/// fade origin down to 0 at the far end. `from_start` puts the origin at index 0 (top or left).
#[must_use]
pub fn gradient_ramp(rgb: [u8; 3], alpha: u8, steps: usize, from_start: bool) -> Vec<u8> {
    let steps = steps.max(2);
    let mut pixels = Vec::with_capacity(steps * 4);
    for index in 0..steps {
        let along = if from_start { index } else { steps - 1 - index };
        let t = along as f32 / (steps - 1) as f32;
        let a = (f32::from(alpha) * (1.0 - t)).round() as u8;
        pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
    }
    pixels
}

/// Live drag preview of a pinned slot: the origin follows the pointer on the strip axis only.
/// `base` is the slot's rest origin, `start` the press point, `pos` the current pointer.
#[must_use]
pub fn drag_preview_origin(
    base: (f32, f32),
    start: (f32, f32),
    pos: (f32, f32),
    vertical: bool,
) -> (f32, f32) {
    let (dx, dy) = (pos.0 - start.0, pos.1 - start.1);
    if vertical {
        (base.0, base.1 + dy)
    } else {
        (base.0 + dx, base.1)
    }
}

/// Factor of the subtle hover scale on a dock icon (`dock.hoverScale`).
pub const HOVER_SCALE: f32 = 1.15;

/// Icon rect `(x, y, w, h)` grown by `factor`, anchored on the bar side so the icon rises away
/// from the screen edge and keeps its centre on the cross axis.
#[must_use]
pub fn hover_scaled_rect(
    rect: (f32, f32, f32, f32),
    factor: f32,
    edge: Edge,
) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = rect;
    let (nw, nh) = (w * factor, h * factor);
    let (dx, dy) = (nw - w, nh - h);
    match edge {
        Edge::Bottom => (x - dx / 2.0, y - dy, nw, nh),
        Edge::Top => (x - dx / 2.0, y, nw, nh),
        Edge::Left => (x, y - dy / 2.0, nw, nh),
        Edge::Right => (x - dx, y - dy / 2.0, nw, nh),
    }
}

/// True when the window rect `(x, y, width, height)` overlaps the dock rect `(x, y, w, h)`.
/// Touching edges do not count as overlap.
#[must_use]
pub fn rect_overlaps(window: (i32, i32, i32, i32), dock: (i32, i32, u32, u32)) -> bool {
    let (wx, wy, ww, wh) = window;
    let (dx, dy, dw, dh) = dock;
    wx < dx + dw as i32 && dx < wx + ww && wy < dy + dh as i32 && dy < wy + wh
}

/// Reserved slot id of the Trash slot. Never a desktop app id, so it is never pinned or launched as one.
pub const TRASH_SLOT_ID: &str = "flamewm:trash";

/// True when the freedesktop Trash `files/` folder under `data_home` holds at least one entry.
/// Read only when the dock is revealed or rebuilt, never on a timer.
#[must_use]
pub fn trash_has_items(data_home: &std::path::Path) -> bool {
    std::fs::read_dir(data_home.join("Trash/files"))
        .is_ok_and(|mut entries| entries.next().is_some())
}

/// Freedesktop icon for the Trash slot: `user-trash-full` when it has items, else `user-trash`.
#[must_use]
pub fn trash_icon_name(full: bool) -> &'static str {
    if full {
        "user-trash-full"
    } else {
        "user-trash"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn pin_dedupes_and_rejects_empty() {
        let mut p = s(&["a"]);
        assert!(!pin_app(&mut p, "a"));
        assert!(!pin_app(&mut p, ""));
        assert!(pin_app(&mut p, "b"));
        assert_eq!(p, s(&["a", "b"]));
    }

    #[test]
    fn pin_respects_cap() {
        let mut p: Vec<String> = (0..MAX_PINNED).map(|i| format!("app{i}")).collect();
        assert!(!pin_app(&mut p, "extra"));
        assert_eq!(p.len(), MAX_PINNED);
    }

    #[test]
    fn unpin_removes_only_match() {
        let mut p = s(&["a", "b", "c"]);
        assert!(unpin_app(&mut p, "b"));
        assert!(!unpin_app(&mut p, "b"));
        assert_eq!(p, s(&["a", "c"]));
    }

    #[test]
    fn move_reorders_and_clamps() {
        let mut p = s(&["a", "b", "c"]);
        assert!(move_pinned(&mut p, 0, 2));
        assert_eq!(p, s(&["b", "c", "a"]));
        assert!(move_pinned(&mut p, 2, 99));
        assert_eq!(p, s(&["b", "c", "a"]));
        assert!(move_pinned(&mut p, 2, 0));
        assert_eq!(p, s(&["a", "b", "c"]));
        assert!(!move_pinned(&mut p, 3, 0));
    }

    #[test]
    fn merge_orders_pinned_then_running_with_separator() {
        let layout = merge_dock(&s(&["term", "web"]), &s(&["mail", "web"]), true);
        let ids: Vec<&str> = layout.entries.iter().map(|e| e.app_id.as_str()).collect();
        assert_eq!(ids, ["term", "web", "mail"]);
        assert!(layout.entries[1].running && layout.entries[1].pinned);
        assert!(!layout.entries[0].running);
        assert!(!layout.entries[2].pinned);
        assert_eq!(layout.separator_after, Some(1));
    }

    #[test]
    fn merge_hides_unpinned_running_when_disabled() {
        let layout = merge_dock(&s(&["term"]), &s(&["mail"]), false);
        assert_eq!(layout.entries.len(), 1);
        assert_eq!(layout.separator_after, None);
    }

    #[test]
    fn merge_no_separator_without_both_groups() {
        assert_eq!(merge_dock(&[], &s(&["mail"]), true).separator_after, None);
        assert_eq!(merge_dock(&s(&["term"]), &[], true).separator_after, None);
    }

    #[test]
    fn slot_at_maps_pointer_to_clamped_index() {
        assert_eq!(slot_at(0.0, 60.0, 3), 0);
        assert_eq!(slot_at(59.9, 60.0, 3), 0);
        assert_eq!(slot_at(60.0, 60.0, 3), 1);
        assert_eq!(slot_at(500.0, 60.0, 3), 2);
        assert_eq!(slot_at(-5.0, 60.0, 3), 0);
        assert_eq!(slot_at(10.0, 60.0, 0), 0);
        assert_eq!(slot_at(f32::NAN, 60.0, 3), 0);
    }

    #[test]
    fn drag_off_only_past_threshold() {
        // Bar spans 0..48 on the cross axis; threshold 24.
        assert!(!drag_off_removes(20.0, 48.0, 24.0));
        assert!(!drag_off_removes(-20.0, 48.0, 24.0));
        assert!(drag_off_removes(-30.0, 48.0, 24.0));
        assert!(drag_off_removes(80.0, 48.0, 24.0));
    }

    #[test]
    fn menu_items_follow_pin_and_running_state() {
        use DockMenuAction::*;
        assert_eq!(menu_items(true, true), [RemoveFromDock, Open, CloseWindows]);
        assert_eq!(menu_items(true, false), [RemoveFromDock, Open]);
        assert_eq!(menu_items(false, true), [KeepInDock, Open, CloseWindows]);
    }

    #[test]
    fn gradient_fades_from_each_edge() {
        assert_eq!(
            gradient_background(Edge::Top, "var(--surface)"),
            "linear-gradient(to bottom, var(--surface) 0%, transparent 100%)"
        );
        assert_eq!(
            gradient_background(Edge::Bottom, "var(--surface)"),
            "linear-gradient(to top, var(--surface) 0%, transparent 100%)"
        );
        assert!(gradient_background(Edge::Left, "x").starts_with("linear-gradient(to right"));
        assert!(gradient_background(Edge::Right, "x").starts_with("linear-gradient(to left"));
        assert!(Edge::Left.is_vertical() && !Edge::Top.is_vertical());
    }

    #[test]
    fn gradient_ramp_is_opaque_at_origin_and_clear_at_far_end() {
        let start = gradient_ramp([1, 2, 3], 200, 4, true);
        assert_eq!(&start[..4], &[1, 2, 3, 200]);
        assert_eq!(&start[12..], &[1, 2, 3, 0]);
        let end = gradient_ramp([1, 2, 3], 200, 4, false);
        assert_eq!(&end[..4], &[1, 2, 3, 0]);
        assert_eq!(&end[12..], &[1, 2, 3, 200]);
        assert_eq!(
            gradient_ramp([0, 0, 0], 255, 1, true).len(),
            8,
            "steps clamp to two samples"
        );
    }

    #[test]
    fn trash_reports_items_only_when_files_folder_is_non_empty() {
        let root = std::env::temp_dir().join(format!("flamewm-trash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(!trash_has_items(&root), "missing folder is empty");
        std::fs::create_dir_all(root.join("Trash/files")).expect("files dir");
        assert!(!trash_has_items(&root), "empty folder is empty");
        std::fs::write(root.join("Trash/files/note.txt"), b"x").expect("item");
        assert!(trash_has_items(&root), "one file is full");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trash_icon_swaps_between_empty_and_full() {
        assert_eq!(trash_icon_name(false), "user-trash");
        assert_eq!(trash_icon_name(true), "user-trash-full");
    }

    #[test]
    fn drag_preview_moves_only_along_the_strip_axis() {
        assert_eq!(
            drag_preview_origin((10.0, 4.0), (50.0, 20.0), (65.0, 90.0), false),
            (25.0, 4.0)
        );
        assert_eq!(
            drag_preview_origin((10.0, 4.0), (50.0, 20.0), (65.0, 90.0), true),
            (10.0, 74.0)
        );
    }

    #[test]
    fn hover_scale_grows_away_from_the_edge_and_keeps_the_centre() {
        // 40x40 icon at (10,10) on a bottom bar: grows upward, centred horizontally.
        let (x, y, w, h) = hover_scaled_rect((10.0, 10.0, 40.0, 40.0), HOVER_SCALE, Edge::Bottom);
        assert!((w - 46.0).abs() < 1e-3 && (h - 46.0).abs() < 1e-3);
        assert!((x - 7.0).abs() < 1e-3, "centre x kept: {x}");
        assert!((y - 4.0).abs() < 1e-3, "bottom anchored: {y}");
        // Top bar grows downward (top anchored).
        let (_, ty, _, _) = hover_scaled_rect((10.0, 10.0, 40.0, 40.0), HOVER_SCALE, Edge::Top);
        assert!((ty - 10.0).abs() < 1e-3);
        // Factor 1 is the rest rect.
        assert_eq!(
            hover_scaled_rect((1.0, 2.0, 3.0, 4.0), 1.0, Edge::Left),
            (1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn rect_overlap_ignores_touching_edges() {
        let dock = (0, 100, 50, 20);
        assert!(rect_overlaps((10, 90, 30, 20), dock), "partial overlap");
        assert!(
            !rect_overlaps((0, 0, 50, 100), dock),
            "window ends on the dock top edge"
        );
        assert!(
            !rect_overlaps((50, 100, 10, 10), dock),
            "window starts on the dock right edge"
        );
        assert!(
            rect_overlaps((-10, 100, 100, 5), dock),
            "wide window across the dock"
        );
    }
}
