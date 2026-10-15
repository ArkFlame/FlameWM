//! One shared popup transaction owner for the shell.
//!
//! Reusable orchestration over EXISTING mechanisms only: retained
//! `SurfaceRuntime` anchors/measure plus `flamewm-shell-core::popup`
//! placement math. No new native surface backend, no new placement
//! engine. Missing semantic nodes are hard errors; there is no
//! root-measure fallback and no `(0, 0)` fallback.

use crate::popup_role::PopupRole;
use crate::quick_controls::QuickControlKind;
use flamewm_api::{PanelEdge, Rect, Size};
use flamewm_shell_core::popup::{fitted_start_placement, measured_popup_rect, PopupRefusal};
use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};
use flamewm_ui_x11::{SurfaceHandle, SurfaceRuntime};

/// Shared shell popup gap (px) between a popup and its panel anchor.
pub(crate) const POPUP_GAP: i32 = 8;

/// Reusable popup open descriptor. Raw HTML ids, never `'#...'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupSpec {
    pub role: PopupRole,
    pub node_id: &'static str,
    pub source_node_id: &'static str,
    pub alignment: PopoverAlign,
    pub helper_kind: Option<QuickControlKind>,
}

impl PopupSpec {
    #[must_use]
    pub const fn from_role(role: PopupRole, alignment: PopoverAlign) -> Self {
        Self {
            role,
            node_id: role.intrinsic_node_id(),
            source_node_id: role.source_node_id(),
            alignment,
            helper_kind: role.quick_control_kind(),
        }
    }
}

/// Narrow typed transaction errors. No stringly fallbacks at call sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupError {
    MissingSemanticNode(String),
    PendingLayout,
    NoSourceAnchor,
    OutOfWorkArea,
    InvalidSize(i32),
    GrabRefused,
    Surface(String),
}

impl std::fmt::Display for PopupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSemanticNode(id) => {
                write!(f, "missing semantic node '{id}'")
            }
            Self::PendingLayout => write!(f, "popup layout pending measurement"),
            Self::NoSourceAnchor => write!(f, "popup anchor pending resolution"),
            Self::OutOfWorkArea => write!(f, "popup placement outside work area"),
            Self::InvalidSize(value) => write!(f, "invalid popup size {value}"),
            Self::GrabRefused => write!(f, "popup pointer grab refused"),
            Self::Surface(message) => write!(f, "{message}"),
        }
    }
}

impl From<PopupRefusal> for PopupError {
    fn from(refusal: PopupRefusal) -> Self {
        match refusal {
            PopupRefusal::PendingLayout => Self::PendingLayout,
            PopupRefusal::PendingAnchor => Self::NoSourceAnchor,
            PopupRefusal::PointerGrabRefused => Self::GrabRefused,
        }
    }
}

/// Pure anchor gate: `None` or degenerate rects refuse, never `(0, 0)`.
#[must_use]
pub fn resolve_anchor(anchor: Option<Rect>) -> Result<Rect, PopupError> {
    let anchor = anchor.ok_or(PopupError::NoSourceAnchor)?;
    if anchor.width <= 0 || anchor.height <= 0 {
        return Err(PopupError::NoSourceAnchor);
    }
    Ok(anchor)
}

/// Pure intrinsic gate: `None` or degenerate sizes refuse measurement.
#[must_use]
pub fn measure_intrinsic(size: Option<Size>) -> Result<Size, PopupError> {
    let size = size.ok_or(PopupError::PendingLayout)?;
    if size.width <= 0 || size.height <= 0 {
        return Err(PopupError::PendingLayout);
    }
    Ok(size)
}

/// Pure fit: aligned placement clamped to `work_area` via the existing
/// `measured_popup_rect` engine. Refuses instead of falling back.
pub fn fit_popup(
    anchor: Rect,
    intrinsic: Size,
    edge: PopoverEdge,
    align: PopoverAlign,
    work_area: Rect,
    gap: i32,
) -> Result<Rect, PopupError> {
    let anchor = resolve_anchor(Some(anchor))?;
    let size = measure_intrinsic(Some(intrinsic))?;
    let rect = measured_popup_rect(Some(anchor), Some(size), edge, align, work_area, gap)?;
    if rect.width <= 0 || rect.height <= 0 {
        return Err(PopupError::PendingLayout);
    }
    if rect.x < work_area.x
        || rect.y < work_area.y
        || rect.right() > work_area.right()
        || rect.bottom() > work_area.bottom()
    {
        return Err(PopupError::OutOfWorkArea);
    }
    Ok(rect)
}

/// Pure Start fit via the existing `fitted_start_placement` engine.
pub fn fit_start(
    anchor: Rect,
    intrinsic: Size,
    panel_edge: PanelEdge,
    work_area: Rect,
    gap: i32,
) -> Result<Rect, PopupError> {
    let anchor = resolve_anchor(Some(anchor))?;
    let size = measure_intrinsic(Some(intrinsic))?;
    Ok(fitted_start_placement(anchor, size, panel_edge, work_area, gap)?.rect)
}

/// Retained-device anchor for a raw panel source node id. Hard
/// `NoSourceAnchor` when no retained layout exists.
#[must_use]
pub fn resolve_source_rect(
    runtime: &SurfaceRuntime,
    panel: SurfaceHandle,
    source_id: &str,
) -> Result<Rect, PopupError> {
    runtime
        .node_device_rect_by_id(panel, source_id)
        .map(|rect| {
            Rect::new(
                rect.x as i32,
                rect.y as i32,
                rect.width.max(1.0) as i32,
                rect.height.max(1.0) as i32,
            )
        })
        .filter(|rect| rect.width > 0 && rect.height > 0)
        .ok_or(PopupError::NoSourceAnchor)
}

/// Existing `SurfaceRuntime` node measure under a work-area device
/// constraint. Missing semantic nodes map to `MissingSemanticNode`
/// (hard error, no root fallback); other failures are `PendingLayout`.
pub fn measure_node(
    runtime: &mut SurfaceRuntime,
    popup: SurfaceHandle,
    node_id: &str,
    work_area: Rect,
) -> Result<Size, PopupError> {
    let max_w = work_area.width.max(1) as f32;
    let max_h = work_area.height.max(1) as f32;
    let measured = runtime
        .measure_node_outer_intrinsic_device_size_by_id(popup, node_id, max_w, max_h)
        .map_err(|error| {
            let message = format!("{error:?}");
            if message.contains("missing semantic node") {
                PopupError::MissingSemanticNode(node_id.to_owned())
            } else {
                PopupError::PendingLayout
            }
        })?;
    if !measured.0.is_finite() || !measured.1.is_finite() {
        return Err(PopupError::PendingLayout);
    }
    let size = Size::new(measured.0.max(1.0) as i32, measured.1.max(1.0) as i32);
    measure_intrinsic(Some(size))
}

/// Shared open transaction: resolve anchor -> measure node -> fit ->
/// prepare (`move_resize`) -> present (`show`) -> grab. Start roles use
/// the fitted engine; other roles use the aligned engine.
pub fn open_popup(
    runtime: &mut SurfaceRuntime,
    panel: SurfaceHandle,
    popup: SurfaceHandle,
    spec: PopupSpec,
    work_area: Rect,
    edge: PopoverEdge,
    panel_edge: PanelEdge,
    gap: i32,
) -> Result<Rect, PopupError> {
    let anchor = resolve_source_rect(runtime, panel, spec.source_node_id)?;
    let intrinsic = measure_node(runtime, popup, spec.node_id, work_area)?;
    let rect = if spec.role == PopupRole::Start {
        fit_start(anchor, intrinsic, panel_edge, work_area, gap)?
    } else {
        fit_popup(anchor, intrinsic, edge, spec.alignment, work_area, gap)?
    };
    let width = u32::try_from(rect.width).map_err(|_| PopupError::InvalidSize(rect.width))?;
    let height = u32::try_from(rect.height).map_err(|_| PopupError::InvalidSize(rect.height))?;
    runtime
        .move_resize(popup, rect.x, rect.y, width, height)
        .map_err(|error| PopupError::Surface(format!("{error:?}")))?;
    runtime
        .show(popup)
        .map_err(|error| PopupError::Surface(format!("{error:?}")))?;
    runtime
        .grab_pointer(popup)
        .map_err(|_| PopupError::GrabRefused)?;
    Ok(rect)
}

/// Shared dismiss predicate: a configured slide closes through `hide` (slide-out),
/// otherwise the popup is unmapped at once by the single-turn close owner.
#[must_use]
pub const fn dismiss_animates(slide_configured: bool) -> bool {
    slide_configured
}

/// Shared close transaction. Slide-out through `hide` when `slide_configured`,
/// else the immediate single-turn close owner (`close_surface`).
pub fn close_popup(
    runtime: &mut SurfaceRuntime,
    popup: SurfaceHandle,
    slide_configured: bool,
) -> Result<(), PopupError> {
    let result = if dismiss_animates(slide_configured) {
        runtime.hide(popup)
    } else {
        runtime.close_surface(popup)
    };
    result.map_err(|error| PopupError::Surface(format!("{error:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismiss_routes_through_hide_only_when_a_slide_is_configured() {
        assert!(
            dismiss_animates(true),
            "configured slide must slide out via hide"
        );
        assert!(
            !dismiss_animates(false),
            "animation off must keep the immediate unmap"
        );
    }
}
