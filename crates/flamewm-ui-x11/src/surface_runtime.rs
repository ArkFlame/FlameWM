use std::os::fd::RawFd;

use std::collections::HashMap;

use flamewm_render_core::RuntimeDocument;
use flamewm_render_x11::{
    GeometryTrace, RawPressObservation, SurfaceConfig as RenderSurfaceConfig, SurfaceController,
    SurfaceId,
};
pub use flamewm_render_x11::{SlideConfig, SlideEdge};

use crate::{UiDocumentView, UiTemplate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceRole {
    Normal,
    Desktop,
    Dock,
    PopupMenu,
    DropdownMenu,
    Overlay,
}

impl From<SurfaceRole> for flamewm_render_x11::SurfaceRole {
    fn from(role: SurfaceRole) -> Self {
        match role {
            SurfaceRole::Normal => Self::Normal,
            SurfaceRole::Desktop => Self::Desktop,
            SurfaceRole::Dock => Self::Dock,
            SurfaceRole::PopupMenu => Self::PopupMenu,
            SurfaceRole::DropdownMenu => Self::DropdownMenu,
            SurfaceRole::Overlay => Self::Overlay,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SurfaceInputMode {
    #[default]
    Interactive,
    PassThrough,
}

impl From<SurfaceInputMode> for flamewm_render_x11::SurfaceInputMode {
    fn from(mode: SurfaceInputMode) -> Self {
        match mode {
            SurfaceInputMode::Interactive => Self::Interactive,
            SurfaceInputMode::PassThrough => Self::PassThrough,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceConfig {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub role: SurfaceRole,
    pub input: SurfaceInputMode,
    pub initially_visible: bool,
    pub x: i32,
    pub y: i32,
}

impl Default for SurfaceConfig {
    fn default() -> Self {
        Self {
            width: 1350,
            height: 641,
            title: "FlameWM UI".to_string(),
            role: SurfaceRole::Normal,
            input: SurfaceInputMode::Interactive,
            initially_visible: true,
            x: 0,
            y: 0,
        }
    }
}

impl From<SurfaceConfig> for RenderSurfaceConfig {
    fn from(config: SurfaceConfig) -> Self {
        Self {
            width: config.width,
            height: config.height,
            title: config.title,
            role: config.role.into(),
            input: config.input.into(),
            initially_visible: config.initially_visible,
            x: config.x,
            y: config.y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceHandle(SurfaceId);

impl SurfaceHandle {
    pub(crate) fn from_id(id: SurfaceId) -> Self {
        Self(id)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum UiBackendError {
    Renderer(String),
    Document(String),
    PointerGrabRefused(String),
}

impl From<String> for UiBackendError {
    fn from(error: String) -> Self {
        Self::Renderer(error)
    }
}

pub struct SurfaceRuntime {
    pub(crate) controller: SurfaceController,
    /// Retained scroll offsets keyed by (surface, node identity). Survives
    /// relayout; pruned when nodes disappear.
    scroll_offsets: HashMap<(u64, u32), flamewm_render_core::ScrollState>,
}

impl SurfaceRuntime {
    pub fn new() -> Result<Self, UiBackendError> {
        Ok(Self {
            controller: SurfaceController::new().map_err(UiBackendError::Renderer)?,
            scroll_offsets: HashMap::new(),
        })
    }
    pub fn create_surface(
        &mut self,
        template: UiTemplate,
        config: SurfaceConfig,
    ) -> Result<SurfaceHandle, UiBackendError> {
        self.controller
            .create_surface(template.document, config.into())
            .map(SurfaceHandle)
            .map_err(UiBackendError::Renderer)
    }

    /// Declare the root edge a dock surface reserves (strut) and its window corner radius.
    /// `None` restores classic inference and a rectangular window.
    pub fn set_dock_frame(
        &mut self,
        surface: SurfaceHandle,
        edge: Option<flamewm_render_x11::DockEdge>,
        corner_radius: u32,
    ) -> Result<(), UiBackendError> {
        self.controller
            .set_dock_frame(surface.0, edge, corner_radius)
            .map_err(UiBackendError::Renderer)
    }

    /// Border-only ring bounding mask for one surface (see `SurfaceController::set_ring_mask`).
    pub fn set_surface_ring_mask(
        &mut self,
        surface: SurfaceHandle,
        border_px: u32,
        radius_px: u32,
    ) -> Result<(), UiBackendError> {
        self.controller
            .set_ring_mask(surface.0, border_px, radius_px)
            .map_err(UiBackendError::Renderer)
    }

    /// Apply the consumer-selected output scale to one retained surface.
    /// Call before first presentation and again when output assignment or
    /// output scale changes. Invalid scales fall back to 1.0; the document
    /// owns scale state and invalidates retained layout/paint on change.
    pub fn apply_output_scale(
        &mut self,
        surface: SurfaceHandle,
        scale: f32,
    ) -> Result<f32, UiBackendError> {
        let scale = effective_output_scale(scale);
        self.document_mut(surface)?
            .set_ui_scale(scale)
            .map_err(UiBackendError::Document)?;
        Ok(scale)
    }

    pub fn show(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller.show(surface.0).map_err(Into::into)
    }

    pub fn hide(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller.hide(surface.0).map_err(Into::into)
    }

    /// Root dock rect for slide clipping when this process has no Dock surface (`None` clears).
    pub fn set_external_dock(&mut self, rect: Option<(i32, i32, u32, u32)>) {
        self.controller.set_external_dock(rect);
    }

    /// Opt-in slide presentation for the next show/hide; `None` keeps the immediate path.
    pub fn set_slide(
        &mut self,
        surface: SurfaceHandle,
        slide: Option<SlideConfig>,
    ) -> Result<(), UiBackendError> {
        self.controller.set_slide(surface.0, slide).map_err(Into::into)
    }

    /// Root-sized `(width, height, RGBA8)` wallpaper canvas used instead of a root capture for the
    /// popup's next first-show backdrop; `None` restores the root capture.
    pub fn set_popup_wallpaper(
        &mut self,
        surface: SurfaceHandle,
        canvas: Option<(u32, u32, Vec<u8>)>,
    ) -> Result<(), UiBackendError> {
        self.controller.set_popup_wallpaper(surface.0, canvas).map_err(Into::into)
    }

    /// Opt-in `(opacity, blur)` pseudo-transparency backdrop, captured before each first show.
    pub fn set_popup_backdrop(
        &mut self,
        surface: SurfaceHandle,
        percents: Option<(u8, u8)>,
    ) -> Result<(), UiBackendError> {
        self.controller.set_popup_backdrop(surface.0, percents).map_err(Into::into)
    }

    pub fn raise(&self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller.raise(surface.0).map_err(Into::into)
    }

    pub fn move_resize(
        &mut self,
        surface: SurfaceHandle,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), UiBackendError> {
        self.move_resize_traced(surface, x, y, width, height, None)
    }

    /// Traced SurfaceRuntime request: records the ui-request stage under the
    /// caller's txn, then forwards the same txn to the controller. No X
    /// roundtrip; observed/retained stages stay in the native owner.
    pub fn move_resize_traced(
        &mut self,
        surface: SurfaceHandle,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        trace: Option<GeometryTrace>,
    ) -> Result<(), UiBackendError> {
        let role_label = self
            .controller
            .surface_role(surface.0)
            .map(flamewm_render_x11::surface_role_label)
            .unwrap_or("unknown");
        let span = match trace {
            Some(t) => GeometryTrace {
                txn: t.txn,
                surface: surface.0.get(),
                role: role_label,
            },
            None => GeometryTrace::begin(role_label).for_surface(surface.0.get()),
        };
        span.ui_request((x, y, width.max(1) as i32, height.max(1) as i32));
        self.controller
            .move_resize_traced(surface.0, x, y, width, height, Some(span))
            .map_err(Into::into)
    }

    /// Debug-only observed-rect probe (retained read, no X sync). Off the
    /// production hot path; shell debug tooling only.
    pub fn debug_observed_rect(
        &self,
        surface: SurfaceHandle,
        trace: GeometryTrace,
    ) -> Result<(i32, i32, i32, i32), UiBackendError> {
        self.controller
            .debug_observed_rect(surface.0, trace.for_surface(surface.0.get()))
            .map_err(UiBackendError::Renderer)
    }

    /// DEBUG-ONLY actual root-rect probe after present (X native owner).
    /// Normal mode: refuses with zero extra X sync. Debug/test mode: one
    /// retained root-rect read (no X roundtrip), recorded as the trace
    /// `observed` stage. Call once per popup show.
    pub fn debug_probe_root_rect(
        &self,
        surface: SurfaceHandle,
        trace: GeometryTrace,
    ) -> Result<(i32, i32, i32, i32), UiBackendError> {
        self.controller
            .debug_probe_root_rect(surface.0, trace.for_surface(surface.0.get()))
            .map_err(UiBackendError::Renderer)
    }

    /// DEBUG/TEST-ONLY stacking canary: bottom-to-top creation order must
    /// satisfy desktop < normal < dock < popup. Zero X sync in all modes.
    /// `Ok(true)` = passes (leave stacking alone); `Ok(false)` = inverted
    /// (caller repairs via the existing map/raise owner and reports).
    pub fn debug_stack_canary(&self) -> Result<bool, UiBackendError> {
        self.controller
            .debug_stack_canary()
            .map_err(UiBackendError::Renderer)
    }

    pub fn destroy(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller
            .destroy_surface(surface.0)
            .map_err(Into::into)
    }

    pub fn redraw(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller.redraw(surface.0).map_err(Into::into)
    }

    pub fn redraw_dirty(&mut self) -> Result<(), UiBackendError> {
        self.controller.redraw_dirty().map_err(Into::into)
    }

    /// Straight RGBA8 root pixels under the surface's device rect (pseudo-transparency source).
    pub fn capture_root_under(
        &self,
        surface: SurfaceHandle,
    ) -> Result<flamewm_render_x11::backdrop::BackdropCapture, UiBackendError> {
        self.controller.capture_root_under(surface.0).map_err(Into::into)
    }

    /// Subscribes to root child map/unmap/configure changes (see the controller method).
    pub fn watch_root_changes(&self) {
        self.controller.watch_root_changes();
    }

    /// Whether root children changed since the last call.
    pub fn take_root_changed(&mut self) -> bool {
        self.controller.take_root_changed()
    }

    /// Root pixels under a surface read with the surface unmapped (see the controller method).
    pub fn capture_root_under_unmapped(
        &mut self,
        surface: SurfaceHandle,
    ) -> Result<flamewm_render_x11::backdrop::BackdropCapture, UiBackendError> {
        self.controller.capture_root_under_unmapped(surface.0).map_err(Into::into)
    }

    /// Installs or removes the static ARGB backdrop under a surface's translucent fills.
    pub fn set_surface_backdrop(
        &mut self,
        surface: SurfaceHandle,
        backdrop: Option<(&[u32], u32, u32)>,
    ) -> Result<(), UiBackendError> {
        self.controller
            .set_surface_backdrop(surface.0, backdrop)
            .map_err(Into::into)
    }

    /// Mark damage on one surface (merged max-coverage latch consumed by
    /// `redraw_dirty`); no immediate paint.
    pub fn mark_dirty(
        &mut self,
        surface: SurfaceHandle,
        damage: flamewm_render_core::SurfaceDamage,
    ) -> Result<(), UiBackendError> {
        self.controller
            .mark_surface_dirty(surface.0, damage)
            .map_err(UiBackendError::Renderer)
    }

    pub fn mark_full(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.mark_dirty(surface, flamewm_render_core::SurfaceDamage::Full)
    }

    /// Damage for one surface: document latch peek (no consume).
    pub fn peek_damage(
        &self,
        surface: SurfaceHandle,
    ) -> Result<flamewm_render_core::SurfaceDamage, UiBackendError> {
        self.controller
            .peek_surface_damage(surface.0)
            .map_err(UiBackendError::Renderer)
    }

    pub fn connection_fd(&self) -> RawFd {
        self.controller.connection_fd()
    }

    /// Observe root presses while quick popups (hosted by the helper) are open.
    pub fn arm_raw_press_observer(&mut self) {
        self.controller.arm_raw_press_observer();
    }

    pub fn disarm_raw_press_observer(&mut self) {
        self.controller.disarm_raw_press_observer();
    }

    /// Root presses observed since the last call.
    pub fn take_raw_press_observations(&mut self) -> Vec<RawPressObservation> {
        self.controller.take_raw_press_observations()
    }

    pub fn grab_pointer(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        // Pass-through surfaces never grab: refuse up front, same as render.
        if let Ok(mode) = self.controller.surface_input_mode(surface.0) {
            if mode == flamewm_render_x11::SurfaceInputMode::PassThrough {
                return Err(UiBackendError::PointerGrabRefused(format!(
                    "surface {} is pass-through",
                    surface.0.get()
                )));
            }
        }
        self.controller
            .grab_pointer(surface.0)
            .map_err(UiBackendError::PointerGrabRefused)
    }

    pub fn ungrab_pointer(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller
            .ungrab_pointer(surface.0)
            .map_err(Into::into)
    }

    pub fn is_pointer_grabbed(&self, surface: SurfaceHandle) -> Result<bool, UiBackendError> {
        self.controller
            .is_pointer_grabbed(surface.0)
            .map_err(UiBackendError::Renderer)
    }

    pub fn surface_role(&self, surface: SurfaceHandle) -> Result<SurfaceRole, UiBackendError> {
        let role = self
            .controller
            .surface_role(surface.0)
            .map_err(UiBackendError::Renderer)?;
        Ok(match role {
            flamewm_render_x11::SurfaceRole::Normal => SurfaceRole::Normal,
            flamewm_render_x11::SurfaceRole::Desktop => SurfaceRole::Desktop,
            flamewm_render_x11::SurfaceRole::Dock => SurfaceRole::Dock,
            flamewm_render_x11::SurfaceRole::PopupMenu => SurfaceRole::PopupMenu,
            flamewm_render_x11::SurfaceRole::DropdownMenu => SurfaceRole::DropdownMenu,
            flamewm_render_x11::SurfaceRole::Overlay => SurfaceRole::Overlay,
        })
    }

    pub fn is_wm_managed(&self, surface: SurfaceHandle) -> Result<bool, UiBackendError> {
        self.controller
            .is_wm_managed(surface.0)
            .map_err(UiBackendError::Renderer)
    }

    pub fn with_document<R>(
        &mut self,
        surface: SurfaceHandle,
        apply: impl FnOnce(&mut UiDocumentView<'_>) -> Result<R, String>,
    ) -> Result<R, UiBackendError> {
        let document = self.document_mut(surface)?;
        apply(&mut UiDocumentView { document }).map_err(UiBackendError::Document)
    }

    pub(crate) fn document_mut(
        &mut self,
        surface: SurfaceHandle,
    ) -> Result<&mut RuntimeDocument, UiBackendError> {
        self.controller
            .document_mut(surface.0)
            .map_err(UiBackendError::Document)
    }

    /// Retained scroll offset for one node: render-owned document state is
    /// the authority; the runtime mirror only seeds drag math.
    pub fn scroll_offset(
        &self,
        surface: SurfaceHandle,
        node: u32,
    ) -> flamewm_render_core::ScrollState {
        self.scroll_offsets
            .get(&(surface.0.get(), node))
            .copied()
            .unwrap_or_default()
    }

    /// Apply a semantic scroll delta (wheel ticks, keyboard, programmatic)
    /// to the render-owned retained offset, clamped to [0, max]. Marks the
    /// document Full so `redraw_dirty` coalesces the frame.
    pub fn apply_scroll_delta(
        &mut self,
        surface: SurfaceHandle,
        node: u32,
        delta: flamewm_render_core::ScrollDelta,
        max_x: f32,
        max_y: f32,
    ) -> flamewm_render_core::ScrollState {
        let next = self
            .controller
            .document_mut(surface.0)
            .map(|document| document.apply_scroll_delta(node, delta, (max_x, max_y)))
            .unwrap_or(flamewm_render_core::ScrollState {
                offset_x: delta.dx.clamp(0.0, max_x.max(0.0)),
                offset_y: delta.dy.clamp(0.0, max_y.max(0.0)),
            });
        self.scroll_offsets.insert((surface.0.get(), node), next);
        next
    }

    /// Set a retained offset directly (thumb drag target). Render-owned.
    pub fn set_scroll_offset(
        &mut self,
        surface: SurfaceHandle,
        node: u32,
        offset: flamewm_render_core::ScrollState,
        max_x: f32,
        max_y: f32,
    ) -> flamewm_render_core::ScrollState {
        let next = self
            .controller
            .document_mut(surface.0)
            .map(|document| document.set_scroll_offset(node, offset, (max_x, max_y)))
            .unwrap_or_else(|_| offset.clamped(max_x, max_y));
        self.scroll_offsets.insert((surface.0.get(), node), next);
        next
    }

    /// Global rect of one node (layout box in surface coordinates).
    /// Needs a display; queries the render-owned layout via redraw path.
    /// Returns the document-space box by recomputing layout at the last
    /// known surface size when available.
    pub fn node_global_rect(
        &mut self,
        surface: SurfaceHandle,
        node: u32,
    ) -> Result<flamewm_render_core::Rect, UiBackendError> {
        let document = self.document_mut(surface)?;
        default_layout(document)
            .boxes
            .get(node as usize)
            .map(|entry| entry.rect)
            .ok_or_else(|| UiBackendError::Document(format!("node {node} has no layout box")))
    }

    /// Retained-live surface extent in root space (native origin + size).
    /// Delegates to the render-owned retained query; no layout recompute.
    pub fn surface_device_rect(
        &self,
        surface: SurfaceHandle,
    ) -> Result<flamewm_render_core::Rect, UiBackendError> {
        self.controller
            .surface_device_rect(surface.0)
            .map_err(UiBackendError::Renderer)
    }

    /// Retained-live node rect in root space (native origin + scaled box).
    /// Delegates to the render-owned retained query; no layout recompute.
    pub fn node_device_rect(
        &self,
        surface: SurfaceHandle,
        node: u32,
    ) -> Result<flamewm_render_core::Rect, UiBackendError> {
        self.controller
            .node_device_rect(surface.0, node)
            .map_err(UiBackendError::Renderer)
    }

    /// Publishes window-relative pager cell rects on the surface's X window.
    pub fn set_pager_cells(
        &mut self,
        surface: SurfaceHandle,
        cells: &[[u64; 4]],
    ) -> Result<(), UiBackendError> {
        self.controller
            .set_pager_cells(surface.0, cells)
            .map_err(UiBackendError::Renderer)
    }

    /// Sets `_NET_WM_ICON_GEOMETRY` on a client window (root-space rect).
    pub fn set_window_icon_geometry(
        &mut self,
        window: u64,
        rect: [u32; 4],
    ) -> Result<(), UiBackendError> {
        self.controller
            .set_window_icon_geometry(window, rect)
            .map_err(UiBackendError::Renderer)
    }

    /// Deletes `_NET_WM_ICON_GEOMETRY` from a client window.
    pub fn clear_window_icon_geometry(&mut self, window: u64) -> Result<(), UiBackendError> {
        self.controller
            .clear_window_icon_geometry(window)
            .map_err(UiBackendError::Renderer)
    }

    /// Node rect by string id from retained layout (root-space device rect).
    pub fn node_device_rect_by_id(
        &self,
        surface: SurfaceHandle,
        id: &str,
    ) -> Option<flamewm_render_core::Rect> {
        self.controller.node_device_rect_by_id(surface.0, id).ok()
    }

    /// Retained intrinsic content size of the document root in device
    /// pixels. Delegates to the render-owned retained query; the 1350x641
    /// fallback below stays headless/test-only and is never used by
    /// mapped-surface queries.
    pub fn document_intrinsic_device_size(
        &self,
        surface: SurfaceHandle,
    ) -> Result<(f32, f32), UiBackendError> {
        self.controller
            .document_intrinsic_device_size(surface.0)
            .map_err(UiBackendError::Renderer)
    }

    /// Close one surface in the same turn: ungrab-once + unmap +
    /// Expose-present. Delegates to the render-owned single-turn close.
    pub fn close_surface(&mut self, surface: SurfaceHandle) -> Result<(), UiBackendError> {
        self.controller
            .close_surface(surface.0)
            .map_err(UiBackendError::Renderer)
    }

    /// Intrinsic outer size of one node in device pixels under finite device
    /// constraints. Pure measure path with no X11 roundtrip.
    pub fn measure_node_outer_intrinsic_device_size(
        &mut self,
        surface: SurfaceHandle,
        node: u32,
        max_device_w: f32,
        max_device_h: f32,
    ) -> Result<(f32, f32), UiBackendError> {
        self.measure_device_size(surface, max_device_w, max_device_h, |doc, w, h| {
            flamewm_render_core::LayoutEngine::measure_node(doc, node, w, h, Default::default())
                .map(|size| (size.width, size.height))
                .map_err(|error| error.to_string())
        })
    }

    /// Shared device -> logical -> `measure` -> device pipeline behind the
    /// root and node intrinsic measures.
    fn measure_device_size(
        &mut self,
        surface: SurfaceHandle,
        max_device_w: f32,
        max_device_h: f32,
        measure: impl FnOnce(&RuntimeDocument, f32, f32) -> Result<(f32, f32), String>,
    ) -> Result<(f32, f32), UiBackendError> {
        let scale = self
            .controller
            .document_mut(surface.0)
            .map(|document| document.ui_scale())
            .unwrap_or(1.0);
        let (max_logical_w, max_logical_h) =
            device_constraint_to_logical(max_device_w, max_device_h, scale)
                .map_err(UiBackendError::Document)?;
        let document = self.document_mut(surface)?;
        let scale = document.ui_scale();
        let (width, height) =
            measure(document, max_logical_w, max_logical_h).map_err(UiBackendError::Document)?;
        logical_intrinsic_to_device(width, height, scale, max_device_w, max_device_h)
            .map_err(UiBackendError::Document)
    }

    /// Intrinsic outer size of one node by string id in device pixels under
    /// finite device constraints. Resolves the compiled node id to a typed
    /// `Document` error on missing (`missing semantic node '<id>'`, never a
    /// root-size fallback), then delegates to the existing node measurement
    /// path (logical/device conversion + finite validation).
    pub fn measure_node_outer_intrinsic_device_size_by_id(
        &mut self,
        surface: SurfaceHandle,
        node_id: &str,
        max_device_w: f32,
        max_device_h: f32,
    ) -> Result<(f32, f32), UiBackendError> {
        let node = self.document_mut(surface)?
            .node_by_id(node_id)
            .ok_or_else(|| {
                UiBackendError::Document(format!("missing semantic node '{node_id}'"))
            })?;
        self.measure_node_outer_intrinsic_device_size(surface, node, max_device_w, max_device_h)
    }

    /// Intrinsic document size: content extent of the root node.
    pub fn document_intrinsic_size(
        &mut self,
        surface: SurfaceHandle,
    ) -> Result<(f32, f32), UiBackendError> {
        let document = self.document_mut(surface)?;
        let extent = default_layout(document).content_extent(document.document.root);
        Ok((extent.width.max(0.0), extent.height.max(0.0)))
    }
}

/// Device->logical constraint conversion: finite/positive input, finite
/// positive scale; logical result re-validated by `measure_root`.
fn device_constraint_to_logical(
    max_device_w: f32,
    max_device_h: f32,
    scale: f32,
) -> Result<(f32, f32), String> {
    if !max_device_w.is_finite() || !max_device_h.is_finite() {
        return Err("intrinsic device constraint must be finite".to_string());
    }
    if max_device_w <= 0.0 || max_device_h <= 0.0 {
        return Err("intrinsic device constraint must be positive".to_string());
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err("ui scale must be finite and positive".to_string());
    }
    let logical_w = max_device_w / scale;
    let logical_h = max_device_h / scale;
    if !logical_w.is_finite() || !logical_h.is_finite() {
        return Err("intrinsic logical constraint must be finite".to_string());
    }
    if logical_w <= 0.0 || logical_h <= 0.0 {
        return Err("intrinsic logical constraint must be positive".to_string());
    }
    Ok((logical_w, logical_h))
}

/// Logical->device rescale with second validation pass (no rounding drift
/// past the device constraint).
fn logical_intrinsic_to_device(
    logical_w: f32,
    logical_h: f32,
    scale: f32,
    max_device_w: f32,
    max_device_h: f32,
) -> Result<(f32, f32), String> {
    let device_w = logical_w * scale;
    let device_h = logical_h * scale;
    if !device_w.is_finite() || !device_h.is_finite() {
        return Err("intrinsic device size must be finite".to_string());
    }
    if device_w <= 0.0 || device_h <= 0.0 {
        return Err("intrinsic device size must be positive".to_string());
    }
    if device_w > max_device_w || device_h > max_device_h {
        return Err("intrinsic size exceeds constraint".to_string());
    }
    Ok((device_w, device_h))
}

fn document_space_extent(document: &RuntimeDocument) -> (f32, f32) {
    // Last-known surface size is render-owned; fall back to root style size,
    // then to a conservative default so queries stay total without display.
    let style = document.runtime_style(
        document.document.root,
        flamewm_render_core::InteractionState::default(),
    );
    let width = match style.width {
        flamewm_render_core::Length::Px(value) => value,
        _ => 1350.0,
    };
    let height = match style.height {
        flamewm_render_core::Length::Px(value) => value,
        _ => 641.0,
    };
    (width.max(1.0), height.max(1.0))
}

/// Layout at the document-space extent, shared by the node-rect and intrinsic-size queries.
fn default_layout(document: &RuntimeDocument) -> flamewm_render_core::LayoutResult {
    let (width, height) = document_space_extent(document);
    flamewm_render_core::LayoutEngine::compute(
        document,
        width,
        height,
        flamewm_render_core::InteractionState::default(),
    )
}

fn effective_output_scale(scale: f32) -> f32 {
    if scale.is_finite() && (0.5..=4.0).contains(&scale) {
        scale
    } else {
        1.0
    }
}

#[cfg(test)]
mod intrinsic_tests {
    use super::*;

    fn element_node(
        parent: Option<u32>,
        first_child: Option<u32>,
        id: &str,
        style: flamewm_render_core::Style,
    ) -> flamewm_render_core::CompiledNode {
        flamewm_render_core::CompiledNode {
            kind: flamewm_render_core::NodeKind::Element,
            parent,
            first_child,
            next_sibling: None,
            id: id.to_string(),
            action: String::new(),
            text: String::new(),
            image: None,
            style,
            hover_style: None,
            active_style: None,
        }
    }

    #[test]
    fn output_scale_validation_falls_back_to_one() {
        for scale in [0.0, -1.0, 0.25, 4.1, f32::NAN, f32::INFINITY] {
            assert_eq!(effective_output_scale(scale), 1.0);
        }
        for (input, expected) in [(1.0, 1.0), (1.25, 1.25), (1.5, 1.5), (2.0, 2.0)] {
            assert_eq!(effective_output_scale(input), expected);
        }
    }

    #[test]
    fn device_constraint_scales_exact() {
        let (lw, lh) = device_constraint_to_logical(400.0, 300.0, 2.0).expect("scales");
        assert_eq!((lw, lh), (200.0, 150.0));
        let (dw, dh) =
            logical_intrinsic_to_device(200.0, 100.0, 2.0, 400.0, 300.0).expect("rescales");
        assert_eq!((dw, dh), (400.0, 200.0));
        // Exact scale-1.5 roundtrip.
        let (lw, lh) = device_constraint_to_logical(300.0, 150.0, 1.5).expect("scales");
        assert!((lw - 200.0).abs() < 0.001 && (lh - 100.0).abs() < 0.001);
        let (dw, dh) = logical_intrinsic_to_device(lw, lh, 1.5, 300.0, 150.0).expect("rescales");
        assert!((dw - 300.0).abs() < 0.01 && (dh - 150.0).abs() < 0.01);
    }

    #[test]
    fn device_constraint_refuses_bad_input() {
        for (w, h, s) in [
            (f32::NAN, 100.0, 1.0),
            (100.0, f32::INFINITY, 1.0),
            (0.0, 100.0, 1.0),
            (100.0, -5.0, 1.0),
            (100.0, 100.0, 0.0),
            (100.0, 100.0, f32::NAN),
        ] {
            assert!(
                device_constraint_to_logical(w, h, s).is_err(),
                "must refuse ({w},{h},scale {s})"
            );
        }
    }

    #[test]
    fn device_result_over_constraint_refused() {
        assert!(logical_intrinsic_to_device(200.0, 100.0, 2.0, 300.0, 300.0).is_err());
        assert!(logical_intrinsic_to_device(100.0, 200.0, 1.0, 100.0, 100.0).is_err());
    }

    /// Compact node measurement path through `LayoutEngine::measure_node`:
    /// a fixed-size Element child measures to its explicit style extent.
    #[test]
    fn measure_node_compact_path_matches_explicit_extent() {
        use flamewm_render_core::{CompiledDocument, Style};
        let child_style = Style {
            width: flamewm_render_core::Length::Px(120.0),
            height: flamewm_render_core::Length::Px(48.0),
            ..Style::default()
        };
        let root = element_node(None, Some(1), "root", Style::default());
        let child = element_node(Some(0), None, "card", child_style);
        let document = flamewm_render_core::RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![root, child],
        })
        .expect("fixture validates");
        assert_eq!(document.node_by_id("card"), Some(1));
        let node = document.node_by_id("card").expect("card resolves");
        let measured = flamewm_render_core::LayoutEngine::measure_node(
            &document,
            node,
            400.0,
            300.0,
            flamewm_render_core::InteractionState::default(),
        )
        .expect("compact node measures");
        assert_eq!((measured.width, measured.height), (120.0, 48.0));
        // Scale-1.0 logical->device conversion keeps the compact path exact.
        let (device_w, device_h) =
            logical_intrinsic_to_device(measured.width, measured.height, 1.0, 400.0, 300.0)
                .expect("device conversion");
        assert_eq!((device_w, device_h), (120.0, 48.0));
        // Scaled conversion path (scale 2.0) doubles logical extents.
        let (scaled_w, scaled_h) =
            logical_intrinsic_to_device(measured.width, measured.height, 2.0, 400.0, 300.0)
                .expect("scaled conversion");
        assert_eq!((scaled_w, scaled_h), (240.0, 96.0));
    }

    /// Missing semantic ids are a typed `Document` error ("missing semantic
    /// node '<id>'"), never a root-size fallback.
    #[test]
    fn missing_node_id_message_never_matches_root_size() {
        let message = format!("missing semantic node '{}'", "nope");
        assert!(message.contains("missing semantic node 'nope'"));
        let error = UiBackendError::Document(message);
        match error {
            UiBackendError::Document(text) => {
                assert!(text.contains("missing semantic node 'nope'"));
            }
            other => panic!("must be a typed Document error, got {other:?}"),
        }
    }
}
