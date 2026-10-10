//! J12 helper popup adapter: helper context -> shared popup owner.
//!
//! Role map Audio/Network/Calendar resolves through the canonical
//! [`crate::popup_role::PopupRole`] owner (raw ids `audio-popup`,
//! `wifi-popup`, `clock-popup`). Measurement and placement reuse the
//! shared [`crate::popup_controller`] transaction primitives
//! (`measure_node`, `fit_popup`, `close_popup`); commit/present/grab map
//! native failures into [`PopupError`]. No duplicate fitted/measure map
//! lives here or in `host.rs` after cutover.

use flamewm_api::{PanelEdge, Rect, Size};
use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};
use flamewm_ui_x11::{SurfaceHandle, SurfaceRuntime};

use crate::popup_controller::{self, PopupError, PopupSpec};
use crate::popup_role::PopupRole;
use crate::quick_controls::protocol::{OpenRequest, QuickControlKind};

const POPUP_GAP: i32 = 8;

#[must_use]
pub fn role_for_kind(kind: QuickControlKind) -> PopupRole {
    match kind {
        QuickControlKind::Audio => PopupRole::Audio,
        QuickControlKind::Network => PopupRole::Network,
        QuickControlKind::Calendar => PopupRole::Calendar,
    }
}

#[must_use]
pub fn spec_for_kind(kind: QuickControlKind) -> PopupSpec {
    PopupSpec::from_role(role_for_kind(kind), PopoverAlign::End)
}

#[must_use]
pub fn edge_for_panel(panel_edge: PanelEdge) -> PopoverEdge {
    match panel_edge {
        PanelEdge::Top => PopoverEdge::Below,
        _ => PopoverEdge::Above,
    }
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

/// Fit a measured intrinsic through the shared aligned engine.
pub fn fit_helper(request: &OpenRequest, intrinsic: Size) -> Result<Rect, PopupError> {
    popup_controller::fit_popup(
        request.anchor,
        intrinsic,
        edge_for_panel(request.panel_edge),
        PopoverAlign::End,
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
        .map_err(|error| PopupError::Surface(format!("{error:?}")))
}

/// Present (`show`) turn.
pub fn present(runtime: &mut SurfaceRuntime, popup: SurfaceHandle) -> Result<(), PopupError> {
    runtime
        .show(popup)
        .map_err(|error| PopupError::Surface(format!("{error:?}")))
}

/// Pointer-grab turn.
pub fn grab(runtime: &mut SurfaceRuntime, popup: SurfaceHandle) -> Result<(), PopupError> {
    runtime
        .grab_pointer(popup)
        .map_err(|_| PopupError::GrabRefused)
}

/// Shared close transaction over the existing single-turn close owner.
pub fn close_helper(runtime: &mut SurfaceRuntime, popup: SurfaceHandle) -> Result<(), PopupError> {
    popup_controller::close_popup(runtime, popup)
}
