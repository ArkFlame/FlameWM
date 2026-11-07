//! Start popup open split from `runtime.rs` (J11).
//!
//! Thin owner over the shared `popup_controller` transaction for the
//! in-process Start surface. No placement math and no measurement logic
//! here; anchor/measure/fit come from `popup_controller`.

use crate::popup_controller::{self, PopupError, PopupSpec, POPUP_GAP};
use crate::popup_role::PopupRole;
use flamewm_api::{PanelEdge, Rect};
use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};
use flamewm_ui_x11::{SurfaceHandle, SurfaceRuntime};

/// Shared spec for the Start popup. Fitted engine; alignment is unused
/// by the fit but kept explicit.
#[must_use]
pub fn spec() -> PopupSpec {
    PopupSpec::from_role(PopupRole::Start, PopoverAlign::Start)
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
        PopoverEdge::Above,
        panel_edge,
        POPUP_GAP,
    )
    .map_err(|error| error.to_string())
}

/// Grab-preserving refit for the already-open Start surface:
/// anchor -> measure -> fit only. The caller prepares/presents without
/// grabbing. Exposes typed `PopupError` so the caller leaves the mapped
/// rect untouched on refusal, and reports (A12-2) when the work area
/// capped the real intrinsic.
pub fn refit(
    runtime: &mut SurfaceRuntime,
    panel: SurfaceHandle,
    popup: SurfaceHandle,
    work_area: Rect,
    panel_edge: PanelEdge,
    gap: i32,
) -> Result<Rect, PopupError> {
    let anchor = popup_controller::resolve_source_rect(runtime, panel, spec().source_node_id)?;
    let intrinsic = popup_controller::measure_node(runtime, popup, spec().node_id, work_area)?;
    let placement = popup_controller::fit_start(anchor, intrinsic, panel_edge, work_area, gap)?;
    popup_controller::report_start_placement(placement);
    Ok(placement.rect)
}
