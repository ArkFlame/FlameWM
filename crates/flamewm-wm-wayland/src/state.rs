//! Compositor state: protocol globals, the window model and the glue that keeps smithay
//! toplevels in step with the pure `model::WindowModel`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::{Child, Command};

use flamewm_platform::shortcuts::ShortcutRegistry;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::input::{Seat, SeatState};
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::ping::Ping;
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Client, DisplayHandle, protocol::wl_surface::WlSurface};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::wlr_layer::LayerSurfaceData;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState};
use smithay::wayland::shm::ShmState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::xwayland::{X11Surface, X11Wm};

use crate::model::{Mode, Rect, ResizeEdge, Shortcuts, WindowId, WindowModel};
use crate::xwayland::{Props, Unmanaged};

/// Per-client data required by `CompositorState`.
#[derive(Default)]
pub struct ClientState {
    pub compositor: CompositorClientState,
}

impl smithay::reexports::wayland_server::backend::ClientData for ClientState {}

/// Pointer-driven interactive operation in progress.
#[derive(Clone, Copy, Debug)]
pub enum Grab {
    Move {
        id: WindowId,
        pointer: (f64, f64),
        start: Rect,
    },
    Resize {
        id: WindowId,
        pointer: (f64, f64),
        start: Rect,
        edge: ResizeEdge,
    },
}

/// What the tty backend should do with one output on the next loop turn.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDecision {
    /// Nothing changed: no paint, and no timer that would paint later.
    Idle,
    /// Paint now.
    Draw,
    /// A frame is already in flight. The request is kept, and the vblank that clears `queued`
    /// plans this output again, so a request made while queued is deferred, never lost.
    Deferred,
}

/// Frame scheduling rules of the tty backend.
///
/// Free of GPU, loop and session state so the rules are testable on a machine without DRM:
/// * paused wins over everything — a paused session paints nothing at all;
/// * a queued frame defers the request to its vblank;
/// * otherwise a paint happens only for a dirty output or an outstanding client frame callback,
///   so an idle compositor issues zero paints and needs no periodic timer.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn plan_frame(queued: bool, dirty: bool, has_callback: bool, paused: bool) -> FrameDecision {
    if paused {
        return FrameDecision::Idle;
    }
    let wanted = dirty || has_callback;
    match (queued, wanted) {
        (true, true) => FrameDecision::Deferred,
        (true, false) | (false, false) => FrameDecision::Idle,
        (false, true) => FrameDecision::Draw,
    }
}

/// Whether a paint pass should arm the bounded retry timer.
///
/// Only a real sync or buffer error does. An empty frame or a clean output must never arm a timer:
/// that is what keeps an idle compositor at zero periodic paints.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn arm_retry(needs_retry: bool, dirty: bool, paused: bool) -> bool {
    needs_retry && !dirty && !paused
}

/// Whether a connector scan that created `outputs` outputs must fail startup.
///
/// Never for a partial scan: one connector that cannot be set up is skipped and the session runs on
/// the others, exactly like anvil. Only a scan that left zero outputs behind is fatal.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn scan_failed(outputs: usize) -> bool {
    outputs == 0
}

/// What one seat GPU can drive, as seen by the tty backend's startup probe.
///
/// Plain data on purpose: the choice between several cards is then testable without a GPU.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuProbe {
    /// This card has a connected internal panel (eDP, LVDS, DSI, DPI).
    pub internal: bool,
    /// This card has at least one connected connector, internal or external.
    pub connected: bool,
}

/// Picks the GPU to drive from the cards that opened, in probe order.
///
/// The first card with a connected internal panel wins, else the first with any connected
/// connector, else the first card that opened at all. That last case is deliberate: a machine whose
/// only card reports nothing connected still gets an attempt, and the connector scan is what
/// reports `stage=outputs status=fail detail=no_connector` — a clearer failure than "no GPU".
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn pick_gpu(probes: &[GpuProbe]) -> Option<usize> {
    probes
        .iter()
        .position(|probe| probe.internal)
        .or_else(|| probes.iter().position(|probe| probe.connected))
        .or((!probes.is_empty()).then_some(0))
}

/// Per-output frame scheduling state. The tty backend keeps one of these per connector; the
/// transitions live here so `plan_frame` is exercised without a GPU. `dirty` is deliberately
/// public: a request that arrives while `queued` survives as `dirty` until the vblank re-plans.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputFrames {
    /// A frame is queued and waiting for its vblank.
    pub queued: bool,
    /// This output has to be painted (scene changed, or its state was reset).
    pub dirty: bool,
    /// The last pass failed in a way a bounded retry can recover from.
    pub needs_retry: bool,
}

#[cfg_attr(not(feature = "drm"), allow(dead_code))]
impl OutputFrames {
    /// What to do right now.
    pub fn plan(&self, has_callback: bool, paused: bool) -> FrameDecision {
        plan_frame(self.queued, self.dirty, has_callback, paused)
    }

    /// A frame went to the kernel: nothing is stale any more.
    pub fn queue(&mut self) {
        self.queued = true;
        self.dirty = false;
        self.needs_retry = false;
    }

    /// The renderer drew nothing: no vblank follows, and only a pending state sync wants another
    /// pass. An idle output arms no timer.
    pub fn empty(&mut self, needs_sync: bool) {
        self.queued = false;
        self.dirty = false;
        self.needs_retry = needs_sync;
    }

    /// The frame failed; a bounded retry is owed.
    pub fn fail(&mut self) {
        self.needs_retry = true;
    }

    /// The vblank that presented the queued frame arrived.
    pub fn presented(&mut self) {
        self.queued = false;
    }

    /// Everything stopped (pause, device loss): no frame in flight, nothing owed.
    pub fn park(&mut self) {
        self.queued = false;
        self.dirty = false;
        self.needs_retry = false;
    }

    /// Coming back from a session pause: scanout was reset, so this output is stale and has to be
    /// painted in full.
    pub fn wake(&mut self) {
        self.queued = false;
        self.dirty = true;
        self.needs_retry = false;
    }
}

pub struct State {
    pub display: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub socket_name: String,
    pub compositor: CompositorState,
    pub xdg_shell: XdgShellState,
    pub layer_shell: WlrLayerShellState,
    pub shm: ShmState,
    pub seat_state: SeatState<State>,
    pub data_device: DataDeviceState,
    pub popups: PopupManager,
    pub seat: Seat<State>,
    pub output: Output,
    pub output_size: Size<i32, Logical>,
    pub damage: OutputDamageTracker,
    pub model: WindowModel,
    /// Smithay toplevels keyed by the model identifier.
    pub toplevels: Vec<(WindowId, ToplevelSurface)>,
    pub next_id: WindowId,
    pub focused: Option<WindowId>,
    pub shortcuts: Shortcuts,
    pub pointer: Point<f64, Logical>,
    pub grab: Option<Grab>,
    /// Bare Super press that has not been combined with another key yet.
    pub super_tap: bool,
    /// Keycodes whose press was consumed as a shortcut; their release is consumed too.
    pub swallowed_keys: Vec<u32>,
    pub backend: Option<WinitGraphicsBackend<GlowRenderer>>,
    pub dump: Option<PathBuf>,
    pub dumped: bool,
    pub exit_after_dump: bool,
    /// Coalescing repaint request for the tty backend: set by anything that changes what is on
    /// screen, consumed by exactly one paint. Single-threaded, so a `Cell` is enough.
    redraw_dirty: Cell<bool>,
    /// Wakes the tty backend for `redraw_dirty`; installed by `drm::init`, `None` when nested.
    /// The `RefCell` keeps `ping` callable from the `&self` request path.
    drm_waker: Option<RefCell<Ping>>,

    pub loop_handle: LoopHandle<'static, State>,
    pub xwayland_shell: XWaylandShellState,
    /// X11 window manager, present once XWayland reports ready.
    pub xwm: Option<X11Wm>,
    /// Managed X11 windows, keyed by the model identifier.
    pub x11: Vec<(WindowId, X11Surface)>,
    /// Override-redirect windows, docks and the desktop; the last entry is the topmost.
    pub unmanaged: Vec<Unmanaged>,
    /// DISPLAY number of the running XWayland server.
    pub display_number: Option<u32>,
    pub props: Option<Props>,
    /// Programs started once XWayland is ready (`FLAMEWM_WAYLAND_AUTOSTART`, colon separated).
    pub autostart: Vec<String>,
    /// Children started by the compositor; reaped by `reap_children` so they never linger as zombies.
    pub children: RefCell<Vec<Child>>,
    /// `com.arkflame.FlameWM1` service; installed after the loop exists (see `control::install`).
    pub control: Option<std::rc::Rc<crate::control::Service>>,
}

impl State {
    pub fn new(
        display: DisplayHandle,
        loop_signal: LoopSignal,
        socket_name: String,
        loop_handle: LoopHandle<'static, State>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let compositor = CompositorState::new::<State>(&display);
        let xdg_shell = XdgShellState::new::<State>(&display);
        let _xdg_decoration = XdgDecorationState::new::<State>(&display);
        let layer_shell = WlrLayerShellState::new::<State>(&display);
        let shm = ShmState::new::<State>(&display, vec![]);
        let _outputs = OutputManagerState::new_with_xdg_output::<State>(&display);
        let mut seat_state = SeatState::new();
        let data_device = DataDeviceState::new::<State>(&display);
        let _viewporter = ViewporterState::new::<State>(&display);
        let xwayland_shell = XWaylandShellState::new::<State>(&display);

        let output = Output::new(
            "flamewm-winit".to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "FlameWM".to_owned(),
                model: "Nested".to_owned(),
            },
        );
        let _global = output.create_global::<State>(&display);

        let mut seat = seat_state.new_wl_seat(&display, "seat0");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();

        let shortcuts = Shortcuts::from_bindings(ShortcutRegistry::default().bindings());
        let damage = OutputDamageTracker::from_output(&output);

        Ok(Self {
            display,
            loop_signal,
            socket_name,
            compositor,
            xdg_shell,
            layer_shell,
            shm,
            seat_state,
            data_device,
            popups: PopupManager::default(),
            seat,
            output,
            output_size: Size::from((1280, 800)),
            damage,
            model: WindowModel::new(4),
            toplevels: Vec::new(),
            next_id: 1,
            focused: None,
            shortcuts,
            pointer: Point::from((0.0, 0.0)),
            grab: None,
            super_tap: false,
            swallowed_keys: Vec::new(),
            backend: None,
            dump: None,
            dumped: false,
            exit_after_dump: false,
            // The first frame is always painted.
            redraw_dirty: Cell::new(true),
            drm_waker: None,
            loop_handle,
            xwayland_shell,
            xwm: None,
            x11: Vec::new(),
            unmanaged: Vec::new(),
            display_number: None,
            props: None,
            autostart: std::env::var("FLAMEWM_WAYLAND_AUTOSTART")
                .map(|value| {
                    value
                        .split(':')
                        .filter(|p| !p.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            children: RefCell::new(Vec::new()),
            control: None,
        })
    }

    /// Collects exited children. Called from a periodic timer: calloop has no child source here,
    /// and an unreaped child stays a zombie until the compositor exits.
    pub fn reap_children(&self) {
        self.children
            .borrow_mut()
            .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_)) | Err(_)));
    }

    /// Output resized by the nested host window: update the mode and re-fit fullscreen windows.
    pub fn on_resized(&mut self, width: i32, height: i32) {
        let mode = smithay::output::Mode {
            size: (width, height).into(),
            refresh: 60_000,
        };
        self.output
            .change_current_state(Some(mode), None, None, None);
        self.output.set_preferred(mode);
        self.output_size = Size::from((width, height));
        let output = self.output_rect();
        let fullscreen: Vec<WindowId> = self
            .model
            .visible_top_first()
            .into_iter()
            .filter(|id| {
                self.model
                    .get(*id)
                    .is_some_and(|w| w.mode == Mode::Fullscreen)
            })
            .collect();
        for id in fullscreen {
            self.model.set_fullscreen(id, output, true);
            self.apply_geometry(id);
        }
        self.request_redraw();
    }

    pub fn alloc_id(&mut self) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn toplevel(&self, id: WindowId) -> Option<&ToplevelSurface> {
        self.toplevels
            .iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, surface)| surface)
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.toplevels
            .iter()
            .find(|(_, top)| top.wl_surface() == surface)
            .map(|(id, _)| *id)
            .or_else(|| {
                self.x11
                    .iter()
                    .find(|(_, window)| window.wl_surface().as_ref() == Some(surface))
                    .map(|(id, _)| *id)
            })
    }

    /// Surface drawn for a managed window, whichever protocol it arrived on.
    pub fn window_wl_surface(&self, id: WindowId) -> Option<WlSurface> {
        match self.toplevel(id) {
            Some(top) => Some(top.wl_surface().clone()),
            None => self.x11_window(id)?.wl_surface(),
        }
    }

    pub fn output_rect(&self) -> Rect {
        Rect::new(0, 0, self.output_size.w, self.output_size.h)
    }

    /// Usable area: the output minus layer-shell exclusive zones (panels).
    /// Layer-shell zones and X11 docks (`_NET_WM_STRUT*`) both shrink the usable area.
    pub fn work_area(&self) -> Rect {
        // The zone stays empty until arrange() runs, which layer-shell events alone trigger; without
        // a layer surface every maximize would get a 0x0 work area.
        let mut map = layer_map_for_output(&self.output);
        map.arrange();
        let zone: Rectangle<i32, Logical> = map.non_exclusive_zone();
        let struts = self.unmanaged.iter().filter_map(|entry| entry.strut);
        reserve_struts(
            Rect::new(zone.loc.x, zone.loc.y, zone.size.w, zone.size.h),
            struts,
        )
    }

    /// Pushes the model rectangle, state flags and server-side decoration mode to the client.
    pub fn apply_geometry(&mut self, id: WindowId) {
        let Some(managed) = self.model.get(id).cloned() else {
            return;
        };
        let activated = self.focused == Some(id);
        if let Some(window) = self.x11_window(id).cloned() {
            let _ = window.set_activated(activated);
            let _ = window.set_maximized(managed.mode == Mode::Maximized);
            let _ = window.set_fullscreen(managed.mode == Mode::Fullscreen);
            let _ = window.configure(Rectangle::<i32, Logical>::new(
                Point::from((managed.rect.x, managed.rect.y)),
                Size::from((managed.rect.w, managed.rect.h)),
            ));
            return;
        }
        let Some(surface) = self.toplevel(id).cloned() else {
            return;
        };
        surface.with_pending_state(|state| {
            state.size = Some(Size::from((managed.rect.w, managed.rect.h)));
            state.decoration_mode = Some(
                smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide,
            );
            set_state(&mut state.states, xdg_toplevel::State::Activated, activated);
            set_state(&mut state.states, xdg_toplevel::State::Maximized, managed.mode == Mode::Maximized);
            set_state(&mut state.states, xdg_toplevel::State::Fullscreen, managed.mode == Mode::Fullscreen);
        });
        surface.send_pending_configure();
    }

    /// Single focus authority: raises the window, activates it and moves keyboard focus.
    pub fn set_focus(&mut self, id: Option<WindowId>) {
        let previous = self.focused;
        self.focused = id;
        if let Some(id) = id {
            self.model.raise(id);
        }
        if let Some(prev) = previous.filter(|prev| Some(*prev) != id) {
            self.apply_geometry(prev);
        }
        if let Some(id) = id {
            self.apply_geometry(id);
        }
        let keyboard = self.seat.get_keyboard();
        let surface = id.and_then(|id| self.window_wl_surface(id));
        if let (Some(id), Some(xwm)) = (id, self.xwm.as_mut()) {
            if let Some((_, window)) = self.x11.iter().find(|(wid, _)| *wid == id) {
                let _ = xwm.raise_window(window);
            }
        }
        if let Some(keyboard) = keyboard {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
        self.request_redraw();
    }

    /// Focuses the topmost visible window, or clears focus when the workspace is empty.
    pub fn refocus(&mut self) {
        let next = self.model.visible_top_first().first().copied();
        self.set_focus(next);
    }

    /// Asks for a complete repaint of the tty backend: sets the coalescing dirty flag and pings the
    /// wake pipe, so the frame happens on the next loop turn instead of waiting for an unrelated
    /// event. Any number of calls before one paint cost a single frame.
    pub fn request_full_redraw(&self) {
        self.redraw_dirty.set(true);
        if let Some(waker) = self.drm_waker.as_ref() {
            // The ping coalesces in the kernel and a closed source only reports an error, which is
            // nothing the compositor can act on.
            let _ = waker.borrow_mut().ping();
        }
    }

    /// Installs the tty wake pipe. Called once by `drm::init`; the nested backend never sets it.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn set_drm_waker(&mut self, waker: Ping) {
        self.drm_waker = Some(RefCell::new(waker));
    }

    /// True while a repaint is outstanding. The tty backend uses it to know that a client frame
    /// callback wants a frame even before the scene is rebuilt.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn redraw_pending(&self) -> bool {
        self.redraw_dirty.get()
    }

    /// Consumes the pending request: the paint that takes it is the one that answers it.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn take_redraw(&self) -> bool {
        self.redraw_dirty.replace(false)
    }

    pub fn request_redraw(&self) {
        self.request_full_redraw();
        if let Some(backend) = self.backend.as_ref() {
            backend.window().request_redraw();
        }
    }

    /// Starts a program. Once XWayland runs, children get `DISPLAY` and no `WAYLAND_DISPLAY`,
    /// so toolkits use X11 and reach the compositor through XWayland.
    pub fn spawn(&self, program: &str, args: &[&str]) {
        let mut command = Command::new(program);
        command.args(args);
        match self.display_number {
            Some(display) => {
                command
                    .env("DISPLAY", format!(":{display}"))
                    .env_remove("WAYLAND_DISPLAY")
                    .env("GDK_BACKEND", "x11")
                    .env("QT_QPA_PLATFORM", "xcb")
                    .env("SDL_VIDEODRIVER", "x11")
                    .env("MOZ_ENABLE_WAYLAND", "0");
            }
            None => {
                command.env("WAYLAND_DISPLAY", &self.socket_name);
            }
        }
        match command.spawn() {
            Ok(child) => self.children.borrow_mut().push(child),
            Err(error) => eprintln!("flamewm-wayland: spawn {program} failed: {error}"),
        }
    }

    /// Emits a `com.arkflame.FlameWM1` signal on the session bus, the same signal the X11 WM sends.
    pub fn emit_control_signal(&self, member: &str) {
        let interface = format!("com.arkflame.FlameWM1.{member}");
        self.spawn(
            "dbus-send",
            &[
                "--session",
                "--type=signal",
                "/com/arkflame/FlameWM1",
                interface.as_str(),
            ],
        );
    }

    pub fn layer_initial_configure_sent(surface: &WlSurface) -> bool {
        with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .map(|data| {
                    data.lock()
                        .map(|d| d.initial_configure_sent)
                        .unwrap_or(true)
                })
                .unwrap_or(true)
        })
    }

    /// Compositor state for a client: our own clients carry `ClientState`, XWayland carries its own.
    pub fn client_compositor_state(client: &Client) -> &CompositorClientState {
        if let Some(data) = client.get_data::<ClientState>() {
            return &data.compositor;
        }
        &client
            .get_data::<smithay::xwayland::XWaylandClientData>()
            .expect("every client carries compositor data")
            .compositor_state
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("socket", &self.socket_name)
            .finish_non_exhaustive()
    }
}

/// Shrinks `area` by the largest reservation on each edge.
pub fn reserve_struts(area: Rect, struts: impl Iterator<Item = [i32; 4]>) -> Rect {
    let (mut left, mut right, mut top, mut bottom) = (0, 0, 0, 0);
    for [l, r, t, b] in struts {
        left = left.max(l);
        right = right.max(r);
        top = top.max(t);
        bottom = bottom.max(b);
    }
    let x0 = area.x + left;
    let y0 = area.y + top;
    let x1 = (area.x + area.w - right).max(x0);
    let y1 = (area.y + area.h - bottom).max(y0);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn set_state(
    states: &mut smithay::wayland::shell::xdg::ToplevelStateSet,
    state: xdg_toplevel::State,
    on: bool,
) {
    if on {
        states.set(state);
    } else {
        states.unset(state);
    }
}

/// Serial helper shared by handlers and input.
pub fn next_serial() -> Serial {
    SERIAL_COUNTER.next_serial()
}

#[cfg(test)]
mod tests {
    use super::{
        FrameDecision, GpuProbe, OutputFrames, Rect, arm_retry, pick_gpu, plan_frame,
        reserve_struts, scan_failed,
    };

    /// One connector that cannot be set up is skipped; only zero outputs is fatal.
    #[test]
    fn zero_outputs_is_fatal_and_one_of_two_is_not() {
        assert!(scan_failed(0));
        assert!(!scan_failed(1));
        assert!(!scan_failed(2));
    }

    /// The GPU pick: internal panel first, then any connected connector, then the first card.
    #[test]
    fn gpu_pick_prefers_internal_then_connected_then_first() {
        let nothing = GpuProbe::default();
        let external = GpuProbe {
            internal: false,
            connected: true,
        };
        let panel = GpuProbe {
            internal: true,
            connected: true,
        };
        assert_eq!(pick_gpu(&[]), None);
        // Nothing connected anywhere: the first card is still tried, so the failure is reported by
        // the connector scan instead of as "no GPU".
        assert_eq!(pick_gpu(&[nothing]), Some(0));
        assert_eq!(pick_gpu(&[nothing, nothing]), Some(0));
        assert_eq!(pick_gpu(&[nothing, external]), Some(1));
        assert_eq!(pick_gpu(&[external, nothing]), Some(0));
        // A hybrid laptop whose boot GPU has nothing connected falls through to the panel card…
        assert_eq!(pick_gpu(&[nothing, external, panel]), Some(2));
        // …and a connected panel beats an earlier card that only has external connectors.
        assert_eq!(pick_gpu(&[external, panel]), Some(1));
        assert_eq!(pick_gpu(&[panel, external]), Some(0));
    }

    #[test]
    fn struts_shrink_each_edge_by_the_largest_reservation() {
        let area = Rect::new(0, 0, 1000, 800);
        let work = reserve_struts(
            area,
            [[0, 0, 0, 30], [0, 0, 0, 50], [20, 0, 0, 0]].into_iter(),
        );
        assert_eq!(work, Rect::new(20, 0, 980, 750));
    }

    #[test]
    fn no_struts_keeps_the_area() {
        let area = Rect::new(10, 20, 300, 200);
        assert_eq!(reserve_struts(area, std::iter::empty()), area);
    }

    /// The first frame after init is painted: startup marks every output dirty.
    #[test]
    fn initial_frame_paints() {
        let frames = OutputFrames {
            dirty: true,
            ..OutputFrames::default()
        };
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
        // Even with no output state at all, the dirty flag alone is enough.
        assert_eq!(plan_frame(false, true, false, false), FrameDecision::Draw);
    }

    /// An idle compositor paints nothing; one change paints exactly one frame.
    #[test]
    fn dirty_after_idle_renders_once() {
        let mut frames = OutputFrames::default();
        assert_eq!(frames.plan(false, false), FrameDecision::Idle);
        assert_eq!(frames.plan(false, false), FrameDecision::Idle);
        frames.dirty = true;
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
        // The paint consumes the request, so the next plan is idle again.
        frames.queue();
        frames.presented();
        assert_eq!(frames.plan(false, false), FrameDecision::Idle);
    }

    /// A request that arrives while a frame is in flight survives to its vblank.
    #[test]
    fn request_while_queued_is_deferred_not_lost() {
        let mut frames = OutputFrames::default();
        frames.queue();
        frames.dirty = true;
        assert_eq!(frames.plan(false, false), FrameDecision::Deferred);
        assert!(
            frames.dirty,
            "a deferred request must not be cleared by planning"
        );
        // The vblank clears `queued`; the surviving request paints on the very next plan.
        frames.presented();
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    }

    /// Paused and idle both mean zero paints, whatever else is pending.
    #[test]
    fn idle_and_paused_never_paint() {
        let idle = OutputFrames::default();
        assert_eq!(idle.plan(false, false), FrameDecision::Idle);
        let paused = OutputFrames {
            dirty: true,
            queued: false,
            needs_retry: true,
        };
        assert_eq!(paused.plan(true, true), FrameDecision::Idle);
        assert_eq!(paused.plan(false, true), FrameDecision::Idle);
        assert_eq!(plan_frame(true, true, true, true), FrameDecision::Idle);
        // A callback alone is enough to want a frame when the session is ours.
        assert_eq!(plan_frame(false, false, true, false), FrameDecision::Draw);
    }

    /// A resume resets scanout, so every output has to be repainted in full.
    #[test]
    fn resume_forces_a_full_redraw() {
        let mut frames = OutputFrames {
            queued: true,
            dirty: false,
            needs_retry: true,
        };
        frames.park();
        assert_eq!(frames, OutputFrames::default());
        assert_eq!(frames.plan(false, true), FrameDecision::Idle);
        frames.wake();
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    }

    /// An empty frame leaves the output idle: no vblank follows and no timer may be armed.
    #[test]
    fn empty_render_without_dirty_arms_no_timer() {
        let mut frames = OutputFrames {
            dirty: true,
            ..OutputFrames::default()
        };
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
        frames.empty(false);
        assert!(!frames.needs_retry);
        assert!(!arm_retry(frames.needs_retry, frames.dirty, false));
        // Only a real sync or buffer error earns the bounded retry, and never while paused.
        assert!(arm_retry(true, false, false));
        assert!(!arm_retry(true, false, true));
        assert!(!arm_retry(true, true, false));
    }
}
