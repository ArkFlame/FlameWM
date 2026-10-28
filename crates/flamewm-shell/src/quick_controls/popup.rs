//! J12 helper popup adapter: helper context -> shared popup owner.
//!
//! Role map Audio/Network/Calendar resolves through the canonical
//! [`crate::popup_role::PopupRole`] owner (raw ids `audio-popup`,
//! `wifi-popup`, `clock-popup`). Measurement and placement reuse the
//! shared [`crate::popup_controller`] transaction primitives
//! (`measure_node`, `fit_popup`, `close_popup`); commit/present/grab map
//! native failures into [`PopupError`]. No duplicate fitted/measure map
//! lives here or in `host.rs` after cutover.

use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::{PanelEdge, Rect, Size};
use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};
use flamewm_ui_x11::{SlideConfig, SurfaceHandle, SurfaceRuntime};

use crate::popup_controller::{self, PopupError, PopupSpec, POPUP_GAP};
use crate::popup_role::PopupRole;
use crate::quick_controls::protocol::{OpenRequest, QuickControlKind};

#[must_use]
pub fn role_for_kind(kind: QuickControlKind) -> PopupRole {
    match kind {
        QuickControlKind::Audio => PopupRole::Audio,
        QuickControlKind::Network => PopupRole::Network,
        QuickControlKind::Calendar => PopupRole::Calendar,
        QuickControlKind::Battery => PopupRole::Battery,
        QuickControlKind::Weather => PopupRole::Weather,
    }
}

#[must_use]
pub fn spec_for_kind(kind: QuickControlKind) -> PopupSpec {
    PopupSpec::from_role(role_for_kind(kind), PopoverAlign::OutputEnd)
}

#[must_use]
pub fn edge_for_panel(panel_edge: PanelEdge) -> PopoverEdge {
    match panel_edge {
        PanelEdge::Top => PopoverEdge::Below,
        _ => PopoverEdge::Above,
    }
}

fn surface_error(error: impl core::fmt::Debug) -> PopupError {
    PopupError::Surface(format!("{error:?}"))
}

/// Measure intrinsic size through the shared owner under the request
/// work-area constraint. Degenerate work areas refuse as pending layout.
pub fn measure_helper(
    runtime: &mut SurfaceRuntime,
    popup: SurfaceHandle,
    request: &OpenRequest,
) -> Result<Size, PopupError> {
    if request.work_area.width <= 0 || request.work_area.height <= 0 {
        return Err(PopupError::PendingLayout);
    }
    let spec = spec_for_kind(request.kind);
    popup_controller::measure_node(runtime, popup, spec.node_id, request.work_area)
}

/// Fit a measured intrinsic through the shared aligned engine. `work_area`
/// is the panel-aware work area of the owning output: the popup is pinned
/// to its right edge, fitted vertically inside it, and capped to its size
/// so it can never cover the panel strip.
pub fn fit_helper(request: &OpenRequest, intrinsic: Size) -> Result<Rect, PopupError> {
    let intrinsic = Size::new(
        intrinsic.width.min(request.work_area.width),
        intrinsic.height.min(request.work_area.height),
    );
    popup_controller::fit_popup(
        request.anchor,
        intrinsic,
        edge_for_panel(request.panel_edge),
        PopoverAlign::OutputEnd,
        request.work_area,
        POPUP_GAP,
    )
}

/// Prepare (`move_resize`) turn; refuses degenerate rects.
pub fn prepare_commit(
    runtime: &mut SurfaceRuntime,
    popup: SurfaceHandle,
    rect: Rect,
) -> Result<(), PopupError> {
    if rect.width <= 0 || rect.height <= 0 {
        return Err(PopupError::PendingLayout);
    }
    let width = u32::try_from(rect.width).map_err(|_| PopupError::InvalidSize(rect.width))?;
    let height = u32::try_from(rect.height).map_err(|_| PopupError::InvalidSize(rect.height))?;
    runtime
        .move_resize(popup, rect.x, rect.y, width, height)
        .map_err(surface_error)
}

/// Present (`show`) turn.
pub fn present(runtime: &mut SurfaceRuntime, popup: SurfaceHandle) -> Result<(), PopupError> {
    runtime.show(popup).map_err(surface_error)
}

/// Pointer-grab turn.
pub fn grab(runtime: &mut SurfaceRuntime, popup: SurfaceHandle) -> Result<(), PopupError> {
    runtime
        .grab_pointer(popup)
        .map_err(|_| PopupError::GrabRefused)
}

/// Slide for a helper popup: the Start slide preference and speed on the panel edge the
/// popup anchors to. Magic Lamp has no target here, so it presents immediately.
#[must_use]
pub fn popup_slide(settings: &SettingsSnapshot, edge: PanelEdge) -> Option<SlideConfig> {
    crate::settings_projection::start_slide(settings, edge, None)
}

/// Arms the popup's slide for its next `show`; `None` keeps the immediate present.
pub fn arm_slide(
    runtime: &mut SurfaceRuntime,
    popup: SurfaceHandle,
    slide: Option<SlideConfig>,
) -> Result<(), PopupError> {
    runtime.set_slide(popup, slide).map_err(surface_error)
}

/// Shared close transaction over the existing single-turn close owner. `slide` is the
/// slide armed at open; `Some` slides out, `None` unmaps at once.
pub fn close_helper(
    runtime: &mut SurfaceRuntime,
    popup: SurfaceHandle,
    slide: Option<SlideConfig>,
) -> Result<(), PopupError> {
    popup_controller::close_popup(runtime, popup, slide.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::settings::SettingValue;
    use flamewm_shell_core::popup::work_area_for_panel;
    use flamewm_ui_x11::SlideEdge;

    fn request(kind: QuickControlKind, anchor: Rect, output: Rect, edge: PanelEdge) -> OpenRequest {
        OpenRequest {
            kind,
            anchor,
            work_area: output,
            panel_edge: edge,
            dock: None,
            scope: None,
        }
    }

    #[test]
    fn quick_popups_right_edge_equals_owning_output_for_every_kind() {
        // Anchor sits far left of the output right edge; the popup must not follow it.
        let output = Rect::new(0, 0, 1920, 1080);
        let anchor = Rect::new(300, 1036, 30, 44);
        for kind in [
            QuickControlKind::Audio,
            QuickControlKind::Network,
            QuickControlKind::Calendar,
            QuickControlKind::Battery,
            QuickControlKind::Weather,
        ] {
            let req = request(kind, anchor, output, PanelEdge::Bottom);
            let rect = fit_helper(&req, Size::new(310, 220)).expect("fit");
            assert_eq!(rect.right(), output.right(), "{kind:?}");
            assert_eq!(rect.width, 310);
            assert!(rect.bottom() <= anchor.y, "{kind:?} {rect:?}");
        }
    }

    #[test]
    fn negative_origin_output_right_edge_is_its_own_right() {
        // Secondary output left of primary: right edge is -1920 + 1920 = 0.
        let output = Rect::new(-1920, 0, 1920, 1080);
        let anchor = Rect::new(-400, 1036, 30, 44);
        let req = request(QuickControlKind::Network, anchor, output, PanelEdge::Bottom);
        let rect = fit_helper(&req, Size::new(310, 220)).expect("fit");
        assert_eq!(rect.right(), 0);
        assert!(rect.x >= output.x);
    }

    #[test]
    fn scaled_device_output_right_edge_is_exact() {
        // 125% scale: device-pixel output 2400 wide; no rounding drift at the edge.
        let output = Rect::new(0, 0, 2400, 1500);
        let anchor = Rect::new(2000, 1456, 40, 44);
        let req = request(
            QuickControlKind::Calendar,
            anchor,
            output,
            PanelEdge::Bottom,
        );
        let rect = fit_helper(&req, Size::new(388, 275)).expect("fit");
        assert_eq!(rect.right(), 2400);
    }

    #[test]
    fn width_capped_to_output_and_pinned_to_its_left_edge() {
        let output = Rect::new(-800, 0, 800, 600);
        let anchor = Rect::new(-100, 556, 30, 44);
        let req = request(QuickControlKind::Audio, anchor, output, PanelEdge::Bottom);
        let rect = fit_helper(&req, Size::new(1000, 200)).expect("fit");
        assert_eq!(rect.width, 800);
        assert_eq!(rect.x, output.x);
        assert_eq!(rect.right(), output.right());
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
    }

    // Real caller shape: `OpenRequest.work_area` is the panel-aware work area
    // of the owning output, never the raw output rect. Tall popup (taller than
    // any work area) forces the fallback clamp; it must never cover the panel.
    fn assert_clear_of_panel(edge: PanelEdge, output: Rect, panel: Rect, anchor: Rect) {
        let work = work_area_for_panel(output, panel, edge);
        let req = request(QuickControlKind::Calendar, anchor, work, edge);
        let rect = fit_helper(&req, Size::new(310, 1040)).expect("fit");
        assert!(
            !overlaps(rect, panel),
            "{edge:?} popup {rect:?} overlaps panel {panel:?}"
        );
        assert!(
            rect.x >= work.x && rect.right() <= work.right(),
            "{edge:?} {rect:?} outside work {work:?}"
        );
        assert!(
            rect.y >= work.y && rect.bottom() <= work.bottom(),
            "{edge:?} {rect:?} outside work {work:?}"
        );
    }

    #[test]
    fn bottom_panel_popup_never_overlaps_strip_44_or_72() {
        let output = Rect::new(0, 0, 1920, 1080);
        for (height, panel) in [
            (44, Rect::new(0, 1036, 1920, 44)),
            (72, Rect::new(0, 1008, 1920, 72)),
        ] {
            let anchor = Rect::new(1800, panel.y, 30, height);
            assert_clear_of_panel(PanelEdge::Bottom, output, panel, anchor);
            let work = work_area_for_panel(output, panel, PanelEdge::Bottom);
            let req = request(QuickControlKind::Calendar, anchor, work, PanelEdge::Bottom);
            let rect = fit_helper(&req, Size::new(310, 1040)).expect("fit");
            assert_eq!(rect.right(), output.right(), "panel {height}");
        }
    }

    #[test]
    fn top_panel_popup_never_overlaps_strip() {
        let output = Rect::new(0, 0, 1920, 1080);
        for height in [44, 72] {
            let panel = Rect::new(0, 0, 1920, height);
            let anchor = Rect::new(1800, 0, 30, height);
            assert_clear_of_panel(PanelEdge::Top, output, panel, anchor);
            let work = work_area_for_panel(output, panel, PanelEdge::Top);
            let req = request(QuickControlKind::Audio, anchor, work, PanelEdge::Top);
            let rect = fit_helper(&req, Size::new(310, 1040)).expect("fit");
            assert_eq!(rect.right(), output.right(), "top panel {height}");
        }
    }

    #[test]
    fn left_panel_popup_never_overlaps_strip_and_pins_output_right() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(0, 0, 44, 1080);
        let anchor = Rect::new(0, 500, 44, 30);
        assert_clear_of_panel(PanelEdge::Left, output, panel, anchor);
        let work = work_area_for_panel(output, panel, PanelEdge::Left);
        let req = request(QuickControlKind::Network, anchor, work, PanelEdge::Left);
        let rect = fit_helper(&req, Size::new(310, 220)).expect("fit");
        assert_eq!(rect.right(), output.right());
    }

    // RULING: a full-height right panel occupies the output's right strip, so
    // a popup with right == output.right would sit on the panel. The panel
    // wins: the popup is pinned to the panel-aware work-area right instead.
    #[test]
    fn right_panel_popup_never_overlaps_strip() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(1876, 0, 44, 1080);
        let anchor = Rect::new(1876, 500, 44, 30);
        assert_clear_of_panel(PanelEdge::Right, output, panel, anchor);
        let work = work_area_for_panel(output, panel, PanelEdge::Right);
        let req = request(QuickControlKind::Audio, anchor, work, PanelEdge::Right);
        let rect = fit_helper(&req, Size::new(310, 220)).expect("fit");
        assert_eq!(rect.right(), work.right());
    }

    #[test]
    fn popup_slide_follows_the_start_preference_and_panel_edge() {
        let mut on = SettingsSnapshot::new(1);
        on.values.insert(
            "startAnimation".to_owned(),
            SettingValue::Text("slide".to_owned()),
        );
        assert_eq!(
            popup_slide(&on, PanelEdge::Top),
            Some(SlideConfig::slide(SlideEdge::Top, 100))
        );
        let mut off = on.clone();
        off.values
            .insert("animationsEnabled".to_owned(), SettingValue::Boolean(false));
        assert_eq!(
            popup_slide(&off, PanelEdge::Top),
            None,
            "disabled animation stays immediate"
        );
    }
}
