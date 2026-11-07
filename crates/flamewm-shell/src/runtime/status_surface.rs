//! Media status popup open split from `runtime.rs` (J11).
//!
//! Thin owner over the shared `popup_controller` transaction for the
//! in-process Media surface. No placement math and no measurement logic
//! here; anchor/measure/fit come from `popup_controller`.

use crate::popup_controller::{self, PopupError, PopupSpec, POPUP_GAP};
use crate::popup_role::PopupRole;
use flamewm_api::{PanelEdge, Rect};
use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};
use flamewm_ui_x11::{SurfaceHandle, SurfaceRuntime};

/// Shared spec for the Media popup: end-aligned above/below the panel.
#[must_use]
pub fn spec() -> PopupSpec {
    PopupSpec::from_role(PopupRole::Media, PopoverAlign::End)
}

/// Edge for a status popup from the panel edge.
#[must_use]
pub fn edge_for_panel_edge(edge: PanelEdge) -> PopoverEdge {
    match edge {
        PanelEdge::Top => PopoverEdge::Below,
        _ => PopoverEdge::Above,
    }
}

/// Full open transaction: anchor -> measure -> fit -> prepare ->
/// present -> grab. Maps `PopupError` to the legacy `String` contract
/// at this boundary.
pub fn open(
    runtime: &mut SurfaceRuntime,
    panel: SurfaceHandle,
    popup: SurfaceHandle,
    work_area: Rect,
    panel_edge: PanelEdge,
) -> Result<Rect, String> {
    popup_controller::open_popup(
        runtime,
        panel,
        popup,
        spec(),
        work_area,
        edge_for_panel_edge(panel_edge),
        panel_edge,
        POPUP_GAP,
    )
    .map_err(|error| error.to_string())
}

/// Grab-preserving refit for the open Media surface: anchor ->
/// measure -> fit only. Typed `PopupError` so the caller leaves the
/// mapped rect untouched on refusal.
pub fn refit(
    runtime: &mut SurfaceRuntime,
    panel: SurfaceHandle,
    popup: SurfaceHandle,
    work_area: Rect,
    panel_edge: PanelEdge,
) -> Result<Rect, PopupError> {
    let anchor = popup_controller::resolve_source_rect(runtime, panel, spec().source_node_id)?;
    let intrinsic = popup_controller::measure_node(runtime, popup, spec().node_id, work_area)?;
    popup_controller::fit_popup(
        anchor,
        intrinsic,
        edge_for_panel_edge(panel_edge),
        spec().alignment,
        work_area,
        POPUP_GAP,
    )
}
