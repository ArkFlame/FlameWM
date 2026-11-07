//! `XWayland` hosting and X11 window bookkeeping.
//!
//! smithay runs the X11 window manager side (`X11Wm`); this module decides how X11 windows enter
//! the compositor model. Normal windows join `WindowModel` and receive `FlameWM` frames.
//! Override-redirect windows (menus, tooltips), docks and the desktop are unmanaged: they are
//! drawn at their own geometry, never framed or moved by the compositor. Docks reserve their
//! strut from the work area. smithay 0.7 does not expose `_NET_WM_WINDOW_TYPE` or
//! `_NET_WM_STRUT*`, so they are read through a second X connection to the `XWayland` display.

use std::collections::HashSet;
use std::error::Error;
use std::os::fd::{AsFd, OwnedFd};

use smithay::delegate_xwayland_shell;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode as CalloopMode, PostAction};
use smithay::reexports::x11rb::connection::Connection;
use smithay::reexports::x11rb::protocol::Event as X11Event;
use smithay::reexports::x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, CreateWindowAux, EventMask,
    InputFocus, Window as X11Window, WindowClass,
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
    /// Unmapped `InputOnly` window retaining the _`NET_WM_CM_S0` selection for `XWayland`.
    _compositor_owner: Option<X11Window>,
    /// Mapped Dock XIDs selected for property notifications on this connection.
    watched_docks: HashSet<X11Window>,
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
            watched_docks: HashSet::new(),
            conn,
        };
        Ok(props)
    }

    /// X-side keyboard focus: `XWayland` does not derive it from Wayland keyboard focus while we are
    /// the WM, so without this X clients receive no key events. Reuses the long-lived connection.
    pub fn set_input_focus(&self, window: X11Window) {
        let focus = self.conn.set_input_focus(
            InputFocus::POINTER_ROOT,
            window,
            smithay::reexports::x11rb::CURRENT_TIME,
        );
        if focus.is_ok() {
            let _ = self.conn.flush();
        }
    }

    /// Duplicate the properties connection fd for calloop readiness; the X11 connection remains
    /// the sole owner of event decoding and protocol requests.
    fn strut_event_source(&self) -> Result<Generic<OwnedFd>, std::io::Error> {
        let fd = self.conn.stream().as_fd().try_clone_to_owned()?;
        Ok(Generic::new(fd, Interest::READ, CalloopMode::Level))
    }

    /// Select notifications before the initial property read so an intervening update is either
    /// reflected by that read or remains queued for the event source.
    fn watch_dock(&mut self, window: X11Window) -> Result<(), Box<dyn Error>> {
        self.conn
            .change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        self.conn.flush()?;
        self.watched_docks.insert(window);
        Ok(())
    }

    /// Stop selecting a Dock after unmap/destroy. A destroyed XID may already be invalid, so
    /// selection cleanup is best-effort; queued notifications are fenced by `watched_docks`.
    fn unwatch_dock(&mut self, window: X11Window) {
        if !self.watched_docks.remove(&window) {
            return;
        }
        if let Ok(cookie) = self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
        ) {
            let _ = cookie.check();
        }
        let _ = self.conn.flush();
    }

    /// Drain queued X11 events and return only currently watched Dock XIDs whose strut atoms
    /// changed. Property values are fetched by the compositor callback, never on pointer motion.
    fn drain_strut_events(&mut self) -> Result<HashSet<X11Window>, Box<dyn Error>> {
        let mut affected = HashSet::new();
        while let Some(event) = self.conn.poll_for_event()? {
            if let X11Event::PropertyNotify(event) = event
                && self.watched_docks.contains(&event.window)
                && is_strut_property(event.atom, self.strut, self.strut_partial)
            {
                affected.insert(event.window);
            }
        }
        Ok(affected)
    }

    /// Smithay really composites the `XWayland` client buffers, including their alpha. Advertise
    /// that fact to X11 clients on this *private `XWayland` display* before launching Shell.
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
    /// buffer pixels; the compositor needs the `WM_NAME` to paint its native titlebar.
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
    /// Window role; only Dock properties reserve space, refreshed on property events and boundaries.
    pub kind: Kind,
    /// Drawn beneath managed windows (desktop background) instead of above them.
    pub below: bool,
    pub strut: Option<[i32; 4]>,
}

/// Replaces one cached Dock reservation and reports whether layout inputs changed.
fn replace_strut(cached: &mut Option<[i32; 4]>, current: Option<[i32; 4]>) -> bool {
    if *cached == current {
        return false;
    }
    *cached = current;
    true
}

/// Whether a property event can affect a Dock's reserved work area.
fn is_strut_property(atom: Atom, strut: Atom, strut_partial: Atom) -> bool {
    atom == strut || atom == strut_partial
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

    /// Gives X input focus to a managed X11 client.
    ///
    /// Keyboard focus on the `wl_surface` alone does not reach the X client: `XWayland` leaves X
    /// input focus where it is when a WM owns the window, so keys go to `PointerRoot`. The WM must
    /// `SetInputFocus` itself, which any client connection may do. A second connection keeps this
    /// independent of the XWM connection smithay owns.
    pub fn set_x11_input_focus(&self, window: &X11Surface) {
        if let Some(props) = self.props.as_ref() {
            props.set_input_focus(window.window_id());
        }
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
                        eprintln!("flamewm-wayland: XWayland ARGB compositor selection active");
                    }
                    Ok(false) => {
                        eprintln!("flamewm-wayland: XWayland compositor selection already owned");
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
        if let Some(props) = self.props.as_ref() {
            match props.strut_event_source() {
                Ok(source) => {
                    if let Err(error) = self.loop_handle.insert_source(source, |_, _, state| {
                        let affected = match state.props.as_mut().map(Props::drain_strut_events) {
                            Some(Ok(affected)) => affected,
                            Some(Err(error)) => {
                                eprintln!(
                                    "flamewm-wayland: X11 Dock property watcher stopped: {error}"
                                );
                                return Ok(PostAction::Remove);
                            }
                            None => return Ok(PostAction::Remove),
                        };
                        state.refresh_dock_strut_ids(affected);
                        Ok(PostAction::Continue)
                    }) {
                        eprintln!(
                            "flamewm-wayland: X11 Dock property watcher unavailable: {error}"
                        );
                    }
                }
                Err(error) => {
                    eprintln!("flamewm-wayland: X11 Dock property watcher unavailable: {error}");
                }
            }
        }
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

    fn map_unmanaged(&mut self, window: X11Surface, kind: Kind) {
        let wid = window.window_id();
        if kind == Kind::Dock
            && let Some(props) = self.props.as_mut()
            && let Err(error) = props.watch_dock(wid)
        {
            eprintln!("flamewm-wayland: cannot watch Dock properties for 0x{wid:x}: {error}");
        }
        let strut = (kind == Kind::Dock)
            .then(|| self.props.as_ref().and_then(|props| props.strut(wid)))
            .flatten();
        self.unmanaged
            .retain(|entry| entry.surface.window_id() != wid);
        self.unmanaged.push(Unmanaged {
            surface: window,
            kind,
            below: kind == Kind::Desktop,
            strut,
        });
        self.reflow_if_work_area_changed();
        // XWayland routes buttons by X stacking rather than Wayland's pointer focus. Reapply the
        // compositor's complete relative X11 order after each map, including desktop surfaces.
        self.sync_xwayland_stacking();
        self.request_redraw();
    }

    /// Refresh current Dock reservations at a layout boundary, then reflow every placed window.
    /// The cached value stays untouched during pointer motion, so property reads are never on the
    /// drag hot path.
    pub(crate) fn refresh_dock_struts(&mut self) {
        let docks: Vec<_> = self
            .unmanaged
            .iter()
            .filter(|entry| entry.kind == Kind::Dock)
            .map(|entry| entry.surface.window_id())
            .collect();
        self.refresh_dock_strut_ids(docks);
    }

    fn refresh_dock_strut(&mut self, window: &X11Surface) {
        let xid = window.window_id();
        if self
            .unmanaged
            .iter()
            .any(|entry| entry.kind == Kind::Dock && entry.surface.window_id() == xid)
        {
            self.refresh_dock_strut_ids([xid]);
        }
    }

    fn refresh_dock_strut_ids(&mut self, docks: impl IntoIterator<Item = X11Window>) {
        let Some(props) = self.props.as_ref() else {
            return;
        };
        let updates: Vec<_> = docks
            .into_iter()
            .map(|xid| (xid, props.strut(xid)))
            .collect();
        let mut changed = false;
        for (xid, strut) in updates {
            if let Some(entry) = self
                .unmanaged
                .iter_mut()
                .find(|entry| entry.kind == Kind::Dock && entry.surface.window_id() == xid)
            {
                changed |= replace_strut(&mut entry.strut, strut);
            }
        }
        if changed {
            self.reflow_if_work_area_changed();
            self.request_redraw();
        }
    }

    /// Keep `XWayland`'s stack in the same relative order as the compositor scene: desktop, managed
    /// windows bottom-to-top, then override-redirect docks/menus bottom-to-top. Smithay synchronizes
    /// its internal stack in `raise_window`; failures are reported with the affected XID so input
    /// routing cannot silently drift from the scene order.
    pub(crate) fn sync_xwayland_stacking(&mut self) {
        let Some(xwm) = self.xwm.as_mut() else {
            return;
        };
        let visible_top_first = self.model.visible_top_first();
        let order = x11_stacking_order(
            self.x11
                .iter()
                .filter(|(id, _)| !visible_top_first.contains(id))
                .map(|(_, window)| window.window_id()),
            self.unmanaged
                .iter()
                .filter(|entry| entry.below)
                .map(|entry| entry.surface.window_id()),
            visible_top_first.iter().rev().copied().filter_map(|id| {
                self.x11
                    .iter()
                    .find(|(window_id, _)| *window_id == id)
                    .map(|(_, window)| window.window_id())
            }),
            self.unmanaged
                .iter()
                .filter(|entry| !entry.below)
                .map(|entry| entry.surface.window_id()),
        );
        for xid in order {
            let surface = self
                .unmanaged
                .iter()
                .map(|entry| &entry.surface)
                .chain(self.x11.iter().map(|(_, window)| window))
                .find(|surface| surface.window_id() == xid);
            let Some(surface) = surface else {
                continue;
            };
            if let Err(error) = xwm.raise_window(surface) {
                eprintln!("flamewm-wayland: failed to restack XWayland window 0x{xid:x}: {error}");
            }
        }
    }

    /// Removes an X11 window from every list it may sit in and refocuses if it was focused.
    fn forget_x11(&mut self, window: &X11Surface) {
        let wid = window.window_id();
        let was_dock = self
            .unmanaged
            .iter()
            .any(|entry| entry.kind == Kind::Dock && entry.surface.window_id() == wid);
        if was_dock && let Some(props) = self.props.as_mut() {
            props.unwatch_dock(wid);
        }
        self.unmanaged
            .retain(|entry| entry.surface.window_id() != wid);
        self.reflow_if_work_area_changed();
        let Some(index) = self
            .x11
            .iter()
            .position(|(_, managed)| managed.window_id() == wid)
        else {
            self.sync_xwayland_stacking();
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
        self.sync_xwayland_stacking();
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

/// X11 bottom-to-top stacking order matching the scene's front-to-back order. Managed windows
/// not currently rendered stay below the desktop/unmanaged lower stack, so hidden workspaces do
/// not remain accidental X input targets.
fn x11_stacking_order(
    managed_hidden: impl IntoIterator<Item = X11Window>,
    unmanaged_below: impl IntoIterator<Item = X11Window>,
    managed_visible_bottom_to_top: impl IntoIterator<Item = X11Window>,
    unmanaged_above: impl IntoIterator<Item = X11Window>,
) -> Vec<X11Window> {
    managed_hidden
        .into_iter()
        .chain(unmanaged_below)
        .chain(managed_visible_bottom_to_top)
        .chain(unmanaged_above)
        .collect()
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
        let focused_managed = self
            .x11_id(&window)
            .is_some_and(|id| self.focused == Some(id));
        if focused_managed {
            self.set_focus(self.focused);
        }
        // Association may complete after mapping and changes which X surfaces are available for
        // input and restacking, even when this is not the keyboard-focused window.
        self.sync_xwayland_stacking();
        // Resolve pointer focus even if the pointer is still over a surface just associated.
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
        let kind = self.kind_of(&window);
        match kind {
            Kind::Dock | Kind::Desktop => self.map_unmanaged(window, kind),
            Kind::Other => self.manage_x11(window),
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let kind = self.kind_of(&window);
        self.map_unmanaged(window, kind);
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
        window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<X11Window>,
    ) {
        // smithay tracks geometry on the surface; rendering reads it on each frame. Dock struts
        // are private X properties, so their owner boundary refreshes the cached work area here.
        self.refresh_dock_strut(&window);
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let Some(id) = self.x11_id(&window) else {
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

    fn allow_selection_access(&mut self, _xwm: XwmId, selection: SelectionTarget) -> bool {
        x11_may_read_selection(selection)
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
        // The duplicated calloop fd observes peer closure and removes its source; dropping Props
        // here also prevents a late queued property event from mutating state after XWayland ends.
        self.props = None;
    }
}

/// X11 clients may read only the clipboard: it is the one selection bridged to Wayland (`send_selection`
/// and `new_selection` above). Smithay denies every X11 read unless this returns true, so answering
/// `false` for `CLIPBOARD` leaves X11 apps unable to paste Wayland clipboard content.
const fn x11_may_read_selection(selection: SelectionTarget) -> bool {
    matches!(selection, SelectionTarget::Clipboard)
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

#[cfg(test)]
mod selection_tests {
    use super::x11_may_read_selection;
    use smithay::wayland::selection::SelectionTarget;

    #[test]
    fn x11_reads_are_allowed_only_for_the_bridged_clipboard() {
        assert!(x11_may_read_selection(SelectionTarget::Clipboard));
        assert!(!x11_may_read_selection(SelectionTarget::Primary));
    }
}

#[cfg(test)]
mod stacking_tests {
    use super::{is_strut_property, replace_strut, x11_stacking_order};
    use crate::model::{Mode, Rect, WindowModel};
    use crate::state::refit_model_placements;
    use flamewm_window_core::SnapTarget;

    #[test]
    fn adding_and_removing_dock_strut_reflows_snapped_geometry() {
        let output = Rect::new(0, 0, 1350, 641);
        let floating = Rect::new(80, 80, 600, 400);
        let mut model = WindowModel::new(1);
        model.configure(false, 0);
        model.add(1, floating, 0);
        model.snap_to(1, SnapTarget::BottomRightQuarter, output, floating);
        assert_eq!(
            model.get(1).unwrap().rect.y + model.get(1).unwrap().rect.h,
            641
        );

        let mut cached = None;
        assert!(replace_strut(&mut cached, Some([0, 0, 0, 48])));
        let work = crate::state::reserve_struts(output, cached.into_iter());
        assert_eq!(
            refit_model_placements(&mut model, output, output, work),
            [1]
        );
        let snapped_area = work;
        let unchanged = model.get(1).unwrap().clone();
        assert!(refit_model_placements(&mut model, output, snapped_area, snapped_area).is_empty());
        assert_eq!(model.get(1), Some(&unchanged));
        let snapped = model.get(1).unwrap();
        assert_eq!(snapped.mode, Mode::Snapped(SnapTarget::BottomRightQuarter));
        assert_eq!(snapped.rect.y + snapped.rect.h, 593);
        assert_eq!(snapped.restore, floating);

        assert!(replace_strut(&mut cached, None));
        let work = crate::state::reserve_struts(output, cached.into_iter());
        assert_eq!(work, output);
        assert_eq!(
            refit_model_placements(&mut model, output, snapped_area, work),
            [1]
        );
        let snapped = model.get(1).unwrap();
        assert_eq!(snapped.mode, Mode::Snapped(SnapTarget::BottomRightQuarter));
        assert_eq!(snapped.rect.y + snapped.rect.h, output.y + output.h);
        assert_eq!(snapped.restore, floating);
    }

    #[test]
    fn layer_bottom_zone_change_refits_placed_geometry() {
        let output = Rect::new(0, 0, 1350, 641);
        let start = Rect::new(80, 80, 600, 400);
        let layer_work = Rect::new(0, 0, output.w, output.h - 48);
        let mut model = WindowModel::new(1);
        model.configure(false, 0);
        model.add(1, start, 0);
        model.snap_to(1, SnapTarget::BottomRightQuarter, output, start);

        assert_eq!(
            refit_model_placements(&mut model, output, output, layer_work),
            [1]
        );
        let placed = model.get(1).unwrap();
        assert_eq!(placed.mode, Mode::Snapped(SnapTarget::BottomRightQuarter));
        assert_eq!(placed.rect.y + placed.rect.h, layer_work.y + layer_work.h);
        assert_eq!(placed.restore, start);
    }

    #[test]
    fn only_strut_property_atoms_trigger_dock_refresh() {
        let strut = 10;
        let strut_partial = 11;
        assert!(is_strut_property(strut, strut, strut_partial));
        assert!(is_strut_property(strut_partial, strut, strut_partial));
        assert!(!is_strut_property(12, strut, strut_partial));
    }

    #[test]
    fn cached_dock_strut_replacement_reports_only_real_changes() {
        let initial = Some([0, 0, 0, 48]);
        let mut cached = initial;
        assert!(!replace_strut(&mut cached, initial));
        assert_eq!(cached, initial);

        let updated = Some([0, 0, 0, 52]);
        assert!(replace_strut(&mut cached, updated));
        assert_eq!(cached, updated);
        assert!(replace_strut(&mut cached, None));
        assert_eq!(cached, None);
    }

    #[test]
    fn docks_are_restacked_above_desktops_and_managed_x11_windows() {
        let order = x11_stacking_order([5], [10], [20, 21], [30]);
        assert_eq!(order, [5, 10, 20, 21, 30]);
        assert_eq!(
            order.last(),
            Some(&30),
            "the foreground panel must be the top X11 input target"
        );
    }

    #[test]
    fn clearing_visible_stack_and_unmapping_windows_rebuilds_remaining_order() {
        // A cleared/empty visible model stack leaves managed windows hidden below the desktop;
        // it must not leave a formerly visible X11 window above the desktop.
        assert_eq!(x11_stacking_order([5, 6], [10], [], [30]), [5, 6, 10, 30]);

        // An unmapped managed or unmanaged surface is absent on the next complete restack.
        let after_unmap = x11_stacking_order([5], [10], [20], []);
        assert_eq!(after_unmap, [5, 10, 20]);
        assert!(!after_unmap.contains(&30));
    }
}
