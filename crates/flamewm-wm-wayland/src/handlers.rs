//! Smithay protocol handlers and delegate registrations for `State`.

use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_layer_shell;
use smithay::delegate_output;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_viewporter;
use smithay::delegate_xdg_decoration;
use smithay::delegate_xdg_shell;
use smithay::desktop::{
    LayerSurface as DesktopLayer, PopupKind, WindowSurfaceType, layer_map_for_output,
};
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1;
use smithay::reexports::wayland_server::protocol::{
    wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
};
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::Serial;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{CompositorHandler, CompositorState};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    set_data_device_focus,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::wlr_layer::{
    Layer, LayerSurface as WlrLayerSurface, LayerSurfaceConfigure, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ShellClient, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use std::os::fd::OwnedFd;

use crate::model::Mode;
use crate::state::{Grab, State};

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
        on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);

        if let Some(popup) = self.popups.find_popup(surface) {
            if let PopupKind::Xdg(xdg) = popup {
                if !xdg.is_initial_configure_sent() {
                    let _ = xdg.send_configure();
                }
            }
        }

        // Layer surfaces: re-arrange exclusive zones and send the first configure once.
        let layer = {
            let map = layer_map_for_output(&self.output);
            map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .cloned()
        };
        if let Some(layer) = layer {
            layer_map_for_output(&self.output).arrange();
            if !State::layer_initial_configure_sent(surface) {
                layer.layer_surface().send_configure();
            }
        }
        self.request_redraw();
    }
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
        self.toplevels.push((id, surface));
        self.set_focus(Some(id));
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
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
        if let Some(managed) = self.model.get(id).cloned() {
            self.grab = Some(Grab::Move {
                id,
                pointer: (self.pointer.x, self.pointer.y),
                start: managed.rect,
            });
        }
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: WlSeat,
        _serial: Serial,
        _edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        // Client-driven resize is not wired yet; the compositor's bottom-right grip is the path.
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
        self.toplevels.retain(|(wid, _)| *wid != id);
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
    /// Maximize or restore by explicit request, idempotent in both directions.
    pub fn set_maximized(&mut self, surface: &WlSurface, on: bool) {
        let Some(id) = self.window_for_surface(surface) else {
            return;
        };
        self.set_maximized_id(id, on);
    }

    pub fn set_maximized_id(&mut self, id: crate::model::WindowId, on: bool) {
        let Some(current) = self.model.get(id).map(|w| w.mode) else {
            return;
        };
        let want_max = on;
        let is_max = current == Mode::Maximized;
        if want_max != is_max && current != Mode::Fullscreen {
            let work = self.work_area();
            self.model.toggle_maximize(id, work);
            self.apply_geometry(id);
        }
    }
}

delegate_xdg_shell!(State);

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(zxdg_toplevel_decoration_v1::Mode::ServerSide)
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
        let _ = output;
        let desktop = DesktopLayer::new(surface, namespace);
        let mut map = layer_map_for_output(&self.output);
        let _ = map.map_layer(&desktop);
        map.arrange();
    }

    fn ack_configure(&mut self, _surface: WlSurface, _configure: LayerSurfaceConfigure) {}

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let mut map = layer_map_for_output(&self.output);
        let target = map
            .layers()
            .find(|layer| layer.layer_surface().wl_surface() == surface.wl_surface())
            .cloned();
        if let Some(target) = target {
            map.unmap_layer(&target);
        }
        map.arrange();
        drop(map);
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
        set_data_device_focus(&self.display, seat, client);
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {
        // The nested backend shows the host cursor; client cursor images are not forwarded yet.
    }
}

delegate_seat!(State);

impl SelectionHandler for State {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
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
        // An X11 client pastes from a Wayland owner: stream the data into the X11 side.
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(error) = xwm.send_selection(ty, mime_type, fd, self.loop_handle.clone()) {
                eprintln!("flamewm-wayland: clipboard transfer to X11 failed: {error}");
            }
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

delegate_viewporter!(State);
