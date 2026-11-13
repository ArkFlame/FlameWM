//! Smithay protocol handlers and delegate registrations for `State`.

use crate::clipboard::write_off_loop;
use flamewm_wm_wayland::protocols::toplevel::ToplevelFacts;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::utils::{
    import_surface, on_commit_buffer_handler, with_renderer_surface_state,
};
use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_dmabuf;
use smithay::delegate_layer_shell;
use smithay::delegate_output;
use smithay::delegate_pointer_constraints;
use smithay::delegate_relative_pointer;
use smithay::delegate_seat;
use smithay::delegate_session_lock;
use smithay::delegate_shm;
use smithay::delegate_viewporter;
use smithay::delegate_xdg_decoration;
use smithay::delegate_xdg_shell;
use smithay::desktop::{
    LayerSurface as DesktopLayer, PopupKind, WindowSurfaceType, layer_map_for_output,
};
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::reexports::wayland_server::protocol::{
    wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
};
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorHandler, CompositorState, SurfaceAttributes, TraversalAction, with_states,
    with_surface_tree_upward,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::foreign_toplevel_list::{
    ForeignToplevelListHandler, ForeignToplevelListState,
};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::pointer_constraints::{
    PointerConstraintsHandler, PointerConstraintsState, with_pointer_constraint,
};
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    set_data_device_focus,
};
use smithay::wayland::selection::primary_selection::set_primary_focus;
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ShellClient, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use std::os::fd::OwnedFd;

use crate::model::{Mode, ResizeEdge};
use crate::state::{Grab, State};

/// Maps the xdg-shell edge onto the compositor's edge flags.
///
/// `ResizeEdge::None` means the client named no edge, which is not a resize, so no grab starts.
/// Each named edge covers the two flags that move it; corners set one of each axis.
fn model_edges(
    edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
) -> Option<ResizeEdge> {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as E;
    let (left, right, top, bottom) = match edges {
        E::None => return None,
        E::Top => (false, false, true, false),
        E::Bottom => (false, false, false, true),
        E::Left => (true, false, false, false),
        E::Right => (false, true, false, false),
        E::TopLeft => (true, false, true, false),
        E::TopRight => (false, true, true, false),
        E::BottomLeft => (true, false, false, true),
        E::BottomRight => (false, true, false, true),
        // The protocol enum is non-exhaustive: an edge this build does not know cannot be
        // resized meaningfully, so it behaves like `None` rather than guessing a direction.
        _ => return None,
    };
    Some(ResizeEdge {
        left,
        right,
        top,
        bottom,
    })
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        State::client_compositor_state(client)
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Read before `on_commit_buffer_handler`, which takes the buffer and damage out of the state.
        let visible = commit_changes_pixels(surface);
        let before = buffer_geometry(surface);
        on_commit_buffer_handler::<Self>(surface);
        // A repaint of a window the open switcher lists refreshes its preview (throttled there).
        if visible
            && let Some(id) = self.window_for_surface(surface)
            && let Some(control) = self.control.clone()
            && control.note_switcher_commit(id)
        {
            self.request_redraw();
        }
        if reimports_on_resize(before, buffer_geometry(surface)) {
            self.import_resized_buffer(surface);
        }
        self.popups.commit(surface);

        if let Some(PopupKind::Xdg(xdg)) = self.popups.find_popup(surface)
            && !xdg.is_initial_configure_sent()
        {
            let _ = xdg.send_configure();
        }

        // Layer surfaces: re-arrange exclusive zones and send the first configure once.
        let out = self.output_handle_of(surface);
        let layer = {
            let map = layer_map_for_output(&out);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
        };
        if let Some(layer) = layer {
            layer_map_for_output(&out).arrange();
            self.reflow_if_work_area_changed();
            if !State::layer_initial_configure_sent(surface) {
                layer.layer_surface().send_configure();
            }
        }
        // A commit that carries no new buffer, no damage and no frame request changes nothing on
        // screen. Repainting for it is the idle wake storm this gate removes.
        if visible {
            self.request_redraw();
        }
    }
}

impl State {
    /// Imports a just-resized buffer now, while the renderer is at hand. Nested sessions only:
    /// the DRM path has no renderer in the commit handler and keeps the render-time import.
    fn import_resized_buffer(&mut self, surface: &WlSurface) {
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let renderer = backend.renderer();
        if let Err(error) = with_states(surface, |states| import_surface(renderer, states)) {
            eprintln!("flamewm-wayland: eager import of resized buffer failed: {error}");
        }
    }
}

/// Logical buffer size and scale of a surface, or `None` before its first buffer.
fn buffer_geometry(surface: &WlSurface) -> Option<(i32, i32, i32)> {
    with_renderer_surface_state(surface, |state| {
        let size = state.buffer_size()?;
        Some((size.w, size.h, state.buffer_scale()))
    })
    .flatten()
}

/// True when a commit changed an existing buffer's geometry. Smithay's damage bag keeps damage
/// from commits since the last import, clipped to each commit's own buffer; a resize between
/// imports would leave wider damage that the old texture cannot take (`GL_INVALID_VALUE` in
/// glTexSubImage2D). Importing at the resize commit drops that stale damage.
pub(crate) fn reimports_on_resize(
    before: Option<(i32, i32, i32)>,
    after: Option<(i32, i32, i32)>,
) -> bool {
    matches!((before, after), (Some(b), Some(a)) if b != a)
}

/// True when this commit (or a subsurface in its tree) gives the output something to show: a new
/// or removed buffer, surface damage, or a frame callback the client waits on. Must run before
/// `on_commit_buffer_handler`, which drains the buffer and damage from the committed state.
fn commit_changes_pixels(surface: &WlSurface) -> bool {
    let mut visible = false;
    with_surface_tree_upward(
        surface,
        (),
        |_, _, ()| TraversalAction::DoChildren(()),
        |_, states, ()| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let attrs = guard.current();
            visible |= attrs.buffer.is_some()
                || !attrs.damage.is_empty()
                || !attrs.frame_callbacks.is_empty();
        },
        |_, _, ()| true,
    );
    visible
}

delegate_compositor!(State);

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

delegate_shm!(State);

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // Nested: the winit backend owns the renderer, so the import is answered now.
        if let Some(backend) = self.backend.as_mut() {
            crate::dmabuf::settle_import(backend.renderer(), &dmabuf, notifier);
            return;
        }
        // Native: the renderer lives in `drm::Drm`. The queue is settled on the next paint, and
        // this wake-up guarantees that paint happens even when no other client is drawing.
        self.pending_dmabufs.push((dmabuf, notifier));
        self.request_redraw();
    }
}

delegate_dmabuf!(State);

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell
    }

    fn new_client(&mut self, _client: ShellClient) {}

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let id = self.alloc_id();
        let workspace = self.model.current_workspace();
        let rect = self.model.cascade_rect(workspace);
        self.model.add(id, rect, workspace);
        let name = self.output_name_under_pointer();
        self.set_surface_output(surface.wl_surface(), name);
        self.toplevels.push((id, surface));
        self.foreign_toplevel_open(id, ToplevelFacts::default());
        self.set_focus(Some(id));
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.foreign_toplevel_refresh(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.foreign_toplevel_refresh(&surface);
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        self.forget_surface_output(surface.wl_surface());
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        if let Some(parent) = surface.get_parent_surface() {
            let name = self.output_name_of(&parent);
            self.set_surface_output(surface.wl_surface(), name);
        }
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        let _ = surface.send_configure();
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, _serial: Serial) {
        let Some(id) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        self.refresh_dock_struts();
        if let Some(managed) = self.model.get(id).cloned() {
            self.grab = Some(Grab::move_window(
                id,
                (self.pointer.x, self.pointer.y),
                &managed,
                false,
            ));
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: WlSeat,
        _serial: Serial,
        edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        // A client that draws its own decorations asks to be resized from an edge. Drive the same
        // `Grab::Resize` the compositor's grip uses, so the client sees ordinary configure changes
        // and the drag is owned by one code path rather than a second implementation.
        let Some(id) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        let Some(edge) = model_edges(edges) else {
            // `None`: no edge bits. Nothing to resize from.
            return;
        };
        if let Some(managed) = self.model.get(id).cloned() {
            self.set_focus(Some(id));
            self.grab = Some(Grab::Resize {
                id,
                pointer: (self.pointer.x, self.pointer.y),
                start: managed.rect,
                edge,
            });
        }
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {}

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.set_maximized(surface.wl_surface(), true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.set_maximized(surface.wl_surface(), false);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some(id) = self.window_for_surface(surface.wl_surface()) {
            let output = self.output_rect();
            self.model.set_fullscreen(id, output, true);
            self.apply_geometry(id);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_for_surface(surface.wl_surface()) {
            let output = self.output_rect();
            self.model.set_fullscreen(id, output, false);
            self.apply_geometry(id);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.window_for_surface(surface.wl_surface()) {
            self.model.minimize(id);
            if self.focused == Some(id) {
                self.refocus();
            }
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        surface.send_repositioned(token);
        let _ = surface.send_configure();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(id) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        self.forget_surface_output(surface.wl_surface());
        self.toplevels.retain(|(wid, _)| *wid != id);
        self.foreign_toplevel_close(id);
        self.model.remove(id);
        if self.grab.is_some_and(
            |grab| matches!(grab, Grab::Move { id: g, .. } | Grab::Resize { id: g, .. } if g == id),
        ) {
            self.grab = None;
        }
        if self.focused == Some(id) {
            self.focused = None;
            self.refocus();
        }
    }
}

impl State {
    fn foreign_toplevel_refresh(&mut self, surface: &ToplevelSurface) {
        if let Some(id) = self.window_for_surface(surface.wl_surface()) {
            self.foreign_toplevel_sync(id, toplevel_facts(surface));
        }
    }

    /// Maximize or restore by explicit request, idempotent in both directions.
    pub fn set_maximized(&mut self, surface: &WlSurface, on: bool) {
        let Some(id) = self.window_for_surface(surface) else {
            return;
        };
        self.set_maximized_id(id, on);
    }

    pub fn set_maximized_id(&mut self, id: crate::model::WindowId, on: bool) {
        self.refresh_dock_struts();
        let Some(current) = self.model.get(id).map(|w| w.mode) else {
            return;
        };
        let want_max = on;
        let is_max = current == Mode::Maximized;
        if want_max != is_max && current != Mode::Fullscreen {
            let work = self.work_area_of(id);
            self.model.toggle_maximize(id, work);
            self.apply_geometry(id);
        }
    }
}

delegate_xdg_shell!(State);

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        _mode: zxdg_toplevel_decoration_v1::Mode,
    ) {
        // Server-side decorations are the only mode FlameWM draws.
        self.new_decoration(toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.new_decoration(toplevel);
    }
}

delegate_xdg_decoration!(State);

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let requested = output
            .as_ref()
            .and_then(smithay::output::Output::from_resource)
            .map(|out| out.name());
        let live: Vec<String> = self
            .output_handles
            .iter()
            .map(smithay::output::Output::name)
            .chain([self.output.name()])
            .collect();
        let pointer = self.output_name_under_pointer();
        let name = crate::output_layout::choose_layer_output(requested.as_deref(), &live, &pointer)
            .to_owned();
        let out = self.output_handle(&name);
        self.set_surface_output(surface.wl_surface(), name);
        let desktop = DesktopLayer::new(surface, namespace);
        let mut map = layer_map_for_output(&out);
        let _ = map.map_layer(&desktop);
        map.arrange();
        drop(map);
        self.reflow_if_work_area_changed();
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let out = self.output_handle_of(surface.wl_surface());
        self.forget_surface_output(surface.wl_surface());
        let mut map = layer_map_for_output(&out);
        let target = map
            .layers()
            .find(|layer| layer.layer_surface().wl_surface() == surface.wl_surface())
            .cloned();
        if let Some(target) = target {
            map.unmap_layer(&target);
        }
        map.arrange();
        drop(map);
        self.reflow_if_work_area_changed();
        self.request_redraw();
    }
}

delegate_layer_shell!(State);

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|surface| self.display.get_client(surface.id()).ok());
        set_data_device_focus(&self.display, seat, client.clone());
        set_primary_focus(&self.display, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.client_cursor_status = image;
        self.refresh_cursor_status();
    }
}

delegate_seat!(State);

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        // A constraint created on the focused surface takes effect at once; its region is checked
        // on the next motion, when the compositor syncs constraints with the pointer position.
        if pointer.current_focus().as_ref() != Some(surface) {
            return;
        }
        with_pointer_constraint(surface, pointer, |c| {
            if let Some(c) = c.filter(|c| !c.is_active()) {
                c.activate();
            }
        });
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        // Warping the cursor to the hint on unlock is not implemented; the cursor stays put.
    }
}

delegate_pointer_constraints!(State);
delegate_relative_pointer!(State);

/// Registers `zwp_pointer_constraints_v1` and `zwp_relative_pointer_manager_v1`. Both managers only
/// own their globals, so they are not kept.
pub(crate) fn init_pointer_protocols(display: &DisplayHandle) {
    let _constraints = PointerConstraintsState::new::<State>(display);
    let _relative = RelativePointerManagerState::new::<State>(display);
}

impl SelectionHandler for State {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        seat: Seat<Self>,
    ) {
        // An explicit clear drops the kept text; a new owner's text is read and kept (see `clipboard`).
        match &source {
            Some(source) => self.read_client_selection(ty, &source.mime_types(), seat),
            None => self.compat.clipboard.slot_mut(ty).clear(),
        }
        // Wayland clipboard owner changed: advertise its mime types to X11 clients.
        if let Some(xwm) = self.xwm.as_mut() {
            let mime_types = source.map(|source| source.mime_types());
            if let Err(error) = xwm.new_selection(ty, mime_types) {
                eprintln!("flamewm-wayland: clipboard bridge to X11 failed: {error}");
            }
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        // Kept text (a Wayland owner that exited, or one re-published by the compositor) is served here.
        if let Some(bytes) = self.compat.clipboard.slot(ty).text(&mime_type) {
            write_off_loop(bytes.to_vec(), fd);
            return;
        }
        // An X11 client pastes from a Wayland owner: stream the data into the X11 side.
        if let Some(xwm) = self.xwm.as_mut()
            && let Err(error) = xwm.send_selection(ty, mime_type, fd, self.loop_handle.clone())
        {
            eprintln!("flamewm-wayland: clipboard transfer to X11 failed: {error}");
        }
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);

impl OutputHandler for State {}

delegate_output!(State);
delegate_session_lock!(State);

delegate_viewporter!(State);

smithay::delegate_foreign_toplevel_list!(State);

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.foreign_toplevels
    }
}

/// The title and app id a toplevel surface currently holds, read from its xdg role data.
fn toplevel_facts(surface: &ToplevelSurface) -> ToplevelFacts {
    with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|data| {
                // A poisoned role lock still holds valid title/app_id data; never panic the compositor on it.
                let data = data
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ToplevelFacts {
                    title: data.title.clone().unwrap_or_default(),
                    app_id: data.app_id.clone().unwrap_or_default(),
                }
            })
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn eager_import_only_when_an_existing_buffer_changes_geometry() {
        use super::reimports_on_resize;
        // First buffer: no earlier import to go stale.
        assert!(!reimports_on_resize(None, Some((10, 31, 1))));
        // Same geometry: the normal render-time import is correct.
        assert!(!reimports_on_resize(Some((32, 31, 1)), Some((32, 31, 1))));
        // Shrink or grow: earlier damage is wider or narrower than this buffer.
        assert!(reimports_on_resize(Some((32, 31, 1)), Some((10, 31, 1))));
        assert!(reimports_on_resize(Some((10, 31, 1)), Some((32, 31, 1))));
        // Scale change alone also changes the texture mapping.
        assert!(reimports_on_resize(Some((32, 31, 1)), Some((32, 31, 2))));
        // Buffer removed: nothing to import.
        assert!(!reimports_on_resize(Some((32, 31, 1)), None));
    }

    use super::model_edges;
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge as E;

    /// Every edge the protocol can name reaches the matching model flag, including corners.
    #[test]
    fn protocol_edges_map_to_model_edges() {
        let left = model_edges(E::Left).expect("left resizes");
        assert!(left.left && !left.right && !left.top && !left.bottom);

        let bottom_right = model_edges(E::BottomRight).expect("corner resizes");
        assert!(bottom_right.right && bottom_right.bottom);
        assert!(!bottom_right.left && !bottom_right.top);

        let top_left = model_edges(E::TopLeft).expect("corner resizes");
        assert!(top_left.left && top_left.top);
        assert!(!top_left.right && !top_left.bottom);
    }

    /// A request naming no edge is not a resize, so no grab is started.
    #[test]
    fn a_none_edge_is_not_a_resize() {
        assert!(model_edges(E::None).is_none());
    }
}
