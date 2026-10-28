//! `XWayland` hosting and X11 window bookkeeping.
//!
//! smithay runs the X11 window manager side (`X11Wm`); this module decides how X11 windows enter
//! the compositor model. Normal windows join `WindowModel` and receive `FlameWM` frames.
//! Override-redirect windows (menus, tooltips), docks and the desktop are unmanaged: they are
//! drawn at their own geometry, never framed or moved by the compositor. Docks reserve their
//! strut from the work area. smithay 0.7 does not expose `_NET_WM_WINDOW_TYPE` or
//! `_NET_WM_STRUT*`, so they are read through a second X connection to the `XWayland` display.

use std::error::Error;
use std::os::fd::OwnedFd;

use smithay::delegate_xwayland_shell;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::x11rb::connection::Connection;
use smithay::reexports::x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt, CreateWindowAux, Window as X11Window, WindowClass,
};
use smithay::reexports::x11rb::rust_connection::RustConnection;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};

use crate::model::{Rect, ResizeEdge as Edge, WindowId};
use crate::state::{Grab, State};

/// Window role as far as `FlameWM` cares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dock,
    Desktop,
    Other,
}

/// Second X connection used for the properties smithay does not expose.
pub struct Props {
    conn: RustConnection,
    window_type: Atom,
    dock: Atom,
    desktop: Atom,
    strut: Atom,
    strut_partial: Atom,
    net_wm_state: Atom,
    fullscreen: Atom,
    net_wm_name: Atom,
    /// Unmapped InputOnly window retaining the _NET_WM_CM_S0 selection for XWayland.
    _compositor_owner: Option<X11Window>,
}

impl Props {
    pub fn connect(display: u32) -> Result<Self, Box<dyn Error>> {
        let (conn, _) = RustConnection::connect(Some(&format!(":{display}")))?;
        let atom = |name: &str| -> Result<Atom, Box<dyn Error>> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let props = Self {
            window_type: atom("_NET_WM_WINDOW_TYPE")?,
            dock: atom("_NET_WM_WINDOW_TYPE_DOCK")?,
            desktop: atom("_NET_WM_WINDOW_TYPE_DESKTOP")?,
            strut: atom("_NET_WM_STRUT")?,
            strut_partial: atom("_NET_WM_STRUT_PARTIAL")?,
            net_wm_state: atom("_NET_WM_STATE")?,
            fullscreen: atom("_NET_WM_STATE_FULLSCREEN")?,
            net_wm_name: atom("_NET_WM_NAME")?,
            _compositor_owner: None,
            conn,
        };
        Ok(props)
    }

    /// Smithay really composites the XWayland client buffers, including their alpha. Advertise
    /// that fact to X11 clients on this *private XWayland display* before launching Shell.
    /// Do not displace a pre-existing CM owner, and keep the owner window alive for our lifetime.
    fn claim_compositor(&mut self) -> Result<bool, Box<dyn Error>> {
        let selection = self
            .conn
            .intern_atom(false, b"_NET_WM_CM_S0")?
            .reply()?
            .atom;
        if self.conn.get_selection_owner(selection)?.reply()?.owner != 0 {
            return Ok(false);
        }
        let root = self.conn.setup().roots[0].root;
        let owner = self.conn.generate_id()?;
        self.conn
            .create_window(
                0,
                owner,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
        self.conn
            .set_selection_owner(owner, selection, 0_u32)?
            .check()?;
        let actual = self.conn.get_selection_owner(selection)?.reply()?.owner;
        if actual != owner {
            self.conn.destroy_window(owner)?.check()?;
            return Ok(false);
        }
        self._compositor_owner = Some(owner);
        Ok(true)
    }

    fn read32(&self, window: X11Window, property: Atom, ty: AtomEnum) -> Vec<u32> {
        self.conn
            .get_property(false, window, property, ty, 0, 64)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(std::iter::Iterator::collect))
            .unwrap_or_default()
    }

    /// One-time title read when mapping the window. X11 title strings are not in Wayland
    /// buffer pixels; the compositor needs the WM_NAME to paint its native titlebar.
    pub fn title(&self, window: X11Window) -> Option<String> {
        for property in [self.net_wm_name, AtomEnum::WM_NAME.into()] {
            let Ok(cookie) = self
                .conn
                .get_property(false, window, property, AtomEnum::ANY, 0, 256)
            else {
                continue;
            };
            let Ok(reply) = cookie.reply() else {
                continue;
            };
            let value = String::from_utf8_lossy(&reply.value);
            let title = value.split('\0').next().unwrap_or("").trim();
            if !title.is_empty() {
                return Some(title.to_owned());
            }
        }
        None
    }

    /// `_NET_WM_STATE_FULLSCREEN` set before the map request, which smithay does not report.
    pub fn is_fullscreen(&self, window: X11Window) -> bool {
        self.read32(window, self.net_wm_state, AtomEnum::ATOM)
            .contains(&self.fullscreen)
    }

    pub fn kind(&self, window: X11Window) -> Kind {
        let types = self.read32(window, self.window_type, AtomEnum::ATOM);
        if types.contains(&self.dock) {
            Kind::Dock
        } else if types.contains(&self.desktop) {
            Kind::Desktop
        } else {
            Kind::Other
        }
    }

    /// Screen-edge reservation as `[left, right, top, bottom]`.
    pub fn strut(&self, window: X11Window) -> Option<[i32; 4]> {
        let partial = self.read32(window, self.strut_partial, AtomEnum::CARDINAL);
        let values = if partial.len() >= 4 {
            partial
        } else {
            self.read32(window, self.strut, AtomEnum::CARDINAL)
        };
        if values.len() < 4 || values[..4].iter().all(|value| *value == 0) {
            return None;
        }
        Some([
            values[0] as i32,
            values[1] as i32,
            values[2] as i32,
            values[3] as i32,
        ])
    }
}

/// X11 window the compositor draws without managing.
pub struct Unmanaged {
    pub surface: X11Surface,
    /// Drawn beneath managed windows (desktop background) instead of above them.
    pub below: bool,
    pub strut: Option<[i32; 4]>,
}

/// Spawns `XWayland` and returns the source to insert into the loop. `Ready` starts the WM.
pub fn spawn_xwayland(
    dh: &smithay::reexports::wayland_server::DisplayHandle,
    handle: &LoopHandle<'static, State>,
) -> Result<smithay::reexports::wayland_server::Client, Box<dyn Error>> {
    let (xwayland, client) = XWayland::spawn(
        dh,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        std::process::Stdio::null(),
        std::process::Stdio::inherit(),
        |_| (),
    )?;
    let wm_handle = handle.clone();
    let wm_client = client.clone();
    handle.insert_source(xwayland, move |event, (), state| match event {
        XWaylandEvent::Ready {
            x11_socket,
            display_number,
        } => match X11Wm::start_wm(wm_handle.clone(), x11_socket, wm_client.clone()) {
            Ok(wm) => state.xwayland_ready(wm, display_number),
            Err(error) => eprintln!("flamewm-wayland: X11 window manager failed to start: {error}"),
        },
        XWaylandEvent::Error => eprintln!("flamewm-wayland: XWayland failed to start"),
    })?;
    Ok(client)
}

impl State {
    pub fn x11_window(&self, id: WindowId) -> Option<&X11Surface> {
        self.x11
            .iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, window)| window)
    }

    pub fn x11_id(&self, window: &X11Surface) -> Option<WindowId> {
        self.x11
            .iter()
            .find(|(_, managed)| managed.window_id() == window.window_id())
            .map(|(id, _)| *id)
    }

    fn kind_of(&self, window: &X11Surface) -> Kind {
        self.props
            .as_ref()
            .map_or(Kind::Other, |props| props.kind(window.window_id()))
    }

    /// `XWayland` is serving: keep the WM and the properties connection, then start the session clients.
    pub fn xwayland_ready(&mut self, wm: X11Wm, display: u32) {
        self.props = match Props::connect(display) {
            Ok(mut props) => {
                match props.claim_compositor() {
                    Ok(true) => {
                        eprintln!("flamewm-wayland: XWayland ARGB compositor selection active")
                    }
                    Ok(false) => {
                        eprintln!("flamewm-wayland: XWayland compositor selection already owned")
                    }
                    Err(error) => eprintln!(
                        "flamewm-wayland: XWayland compositor selection unavailable: {error}"
                    ),
                }
                Some(props)
            }
            Err(error) => {
                eprintln!(
                    "flamewm-wayland: X11 property connection failed: {error}; docks unavailable"
                );
                None
            }
        };
        self.xwm = Some(wm);
        self.display_number = Some(display);
        eprintln!("flamewm-wayland: XWayland ready on DISPLAY=:{display}");
        for program in self.autostart.clone() {
            self.spawn(&program, &[]);
        }
    }

    fn manage_x11(&mut self, window: X11Surface) {
        let id = self.alloc_id();
        let workspace = self.model.current_workspace();
        let cascade = self.model.cascade_rect(workspace);
        let geometry = window.geometry();
        let (w, h) = if geometry.size.w > 0 && geometry.size.h > 0 {
            (geometry.size.w, geometry.size.h)
        } else {
            (cascade.w, cascade.h)
        };
        // Fullscreen set before the map arrives as state, not as a later request.
        let wants_fullscreen = window.is_fullscreen()
            || self
                .props
                .as_ref()
                .is_some_and(|props| props.is_fullscreen(window.window_id()));
        self.model
            .add(id, Rect::new(cascade.x, cascade.y, w, h), workspace);
        let title = self
            .props
            .as_ref()
            .and_then(|props| props.title(window.window_id()))
            .unwrap_or_else(|| "FlameWM".to_owned());
        self.titles.insert(id, title);
        self.x11.push((id, window));
        if wants_fullscreen {
            let output = self.output_rect();
            self.model.set_fullscreen(id, output, true);
        }
        self.set_focus(Some(id));
    }

    fn map_unmanaged(&mut self, window: X11Surface, below: bool) {
        let strut = self
            .props
            .as_ref()
            .and_then(|props| props.strut(window.window_id()));
        let wid = window.window_id();
        self.unmanaged
            .retain(|entry| entry.surface.window_id() != wid);
        self.unmanaged.push(Unmanaged {
            surface: window,
            below,
            strut,
        });
        // XWayland routes pointer buttons by X stacking, not by the compositor's pointer focus. A
        // desktop mapped after a dock sits above it in X and takes the dock's clicks, so docks are
        // re-raised here to keep X stacking in step with the compositor's model.
        if let Some(xwm) = self.xwm.as_mut() {
            for entry in self.unmanaged.iter().filter(|entry| !entry.below) {
                let _ = xwm.raise_window(&entry.surface);
            }
        }
        self.request_redraw();
    }

    /// Removes an X11 window from every list it may sit in and refocuses if it was focused.
    fn forget_x11(&mut self, window: &X11Surface) {
        let wid = window.window_id();
        self.unmanaged
            .retain(|entry| entry.surface.window_id() != wid);
        let Some(index) = self
            .x11
            .iter()
            .position(|(_, managed)| managed.window_id() == wid)
        else {
            self.request_redraw();
            return;
        };
        let (id, _) = self.x11.remove(index);
        self.model.remove(id);
        self.titles.remove(&id);
        if self.grab.is_some_and(
            |grab| matches!(grab, Grab::Move { id: g, .. } | Grab::Resize { id: g, .. } if g == id),
        ) {
            self.grab = None;
        }
        if self.focused == Some(id) {
            self.focused = None;
            self.refocus();
        }
        self.request_redraw();
    }

    /// Asks the X11 client to close; the client decides whether to honour it.
    pub fn close_window(&self, id: WindowId) {
        if let Some(top) = self.toplevel(id) {
            top.send_close();
        } else if let Some(window) = self.x11_window(id) {
            let _ = window.close();
        }
    }
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell
    }

    fn surface_associated(
        &mut self,
        _xwm: XwmId,
        _wl_surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        window: X11Surface,
    ) {
        // The wl_surface can arrive after the map request; re-apply focus so keyboard input lands.
        if let Some(id) = self.x11_id(&window) {
            if self.focused == Some(id) {
                self.set_focus(Some(id));
            }
        }
        // Pointer focus is only resolved on motion, and an unmanaged window (taskbar, dock) has no
        // wl_surface until this callback, so a pointer resting over it never got focus until moved.
        self.on_motion(self.pointer, 0);
    }
}

delegate_xwayland_shell!(State);

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("XWayland handlers fire only while the X11 window manager runs")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_mapped(true);
        match self.kind_of(&window) {
            Kind::Dock => self.map_unmanaged(window, false),
            Kind::Desktop => self.map_unmanaged(window, true),
            Kind::Other => self.manage_x11(window),
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let below = self.kind_of(&window) == Kind::Desktop;
        self.map_unmanaged(window, below);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.forget_x11(&window);
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.forget_x11(&window);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Managed windows keep the model geometry; unmanaged ones get what they asked for.
        let geometry = if let Some(managed) = self.x11_id(&window).and_then(|id| self.model.get(id))
        {
            Rectangle::<i32, Logical>::new(
                Point::from((managed.rect.x, managed.rect.y)),
                Size::from((managed.rect.w, managed.rect.h)),
            )
        } else {
            let current = window.geometry();
            Rectangle::<i32, Logical>::new(
                Point::from((x.unwrap_or(current.loc.x), y.unwrap_or(current.loc.y))),
                Size::from((
                    w.map_or(current.size.w, |value| value as i32),
                    h.map_or(current.size.h, |value| value as i32),
                )),
            )
        };
        let _ = window.configure(geometry);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<X11Window>,
    ) {
        // smithay tracks geometry on the surface; rendering reads it on each frame.
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let Some(id) = self.x11_id(&window) else {
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

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, edge: ResizeEdge) {
        let Some(id) = self.x11_id(&window) else {
            return;
        };
        if let Some(managed) = self.model.get(id).cloned() {
            self.grab = Some(Grab::Resize {
                id,
                pointer: (self.pointer.x, self.pointer.y),
                start: managed.rect,
                edge: xwm_edge(edge),
            });
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.x11_id(&window) {
            self.set_maximized_id(id, true);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.x11_id(&window) {
            self.set_maximized_id(id, false);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.x11_id(&window) {
            let output = self.output_rect();
            self.model.set_fullscreen(id, output, true);
            self.apply_geometry(id);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(id) = self.x11_id(&window) {
            let output = self.output_rect();
            self.model.set_fullscreen(id, output, false);
            self.apply_geometry(id);
        }
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        if matches!(selection, SelectionTarget::Clipboard) {
            if let Err(error) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                eprintln!("flamewm-wayland: clipboard request from X11 failed: {error}");
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        if matches!(selection, SelectionTarget::Clipboard) {
            set_data_device_selection(&self.display, &self.seat, mime_types, ());
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        if matches!(selection, SelectionTarget::Clipboard) {
            clear_data_device_selection(&self.display, &self.seat);
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        self.xwm = None;
    }
}

/// Edge flags for an XWM `_NET_WM_MOVERESIZE` resize request.
fn xwm_edge(edge: ResizeEdge) -> Edge {
    let (left, right, top, bottom) = match edge {
        ResizeEdge::Top => (false, false, true, false),
        ResizeEdge::Bottom => (false, false, false, true),
        ResizeEdge::Left => (true, false, false, false),
        ResizeEdge::Right => (false, true, false, false),
        ResizeEdge::TopLeft => (true, false, true, false),
        ResizeEdge::TopRight => (false, true, true, false),
        ResizeEdge::BottomLeft => (true, false, false, true),
        ResizeEdge::BottomRight => (false, true, false, true),
    };
    Edge {
        left,
        right,
        top,
        bottom,
    }
}
