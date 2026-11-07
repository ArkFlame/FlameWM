//! Compositor state: protocol globals, the window model and the glue that keeps smithay
//! toplevels in step with the pure `model::WindowModel`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

/// Minimum spacing between nested-window paints (about one 60 Hz vblank).
const WINIT_FRAME_INTERVAL: Duration = Duration::from_millis(16);

use flamewm_platform::shortcuts::ShortcutRegistry;
use flamewm_wm_wayland::protocols::toplevel::{ToplevelFacts, diff as diff_toplevel_facts};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::input::pointer::CursorImageStatus;
use smithay::input::{Seat, SeatState};
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::ping::Ping;
use smithay::reexports::calloop::{
    Interest, LoopHandle, LoopSignal, Mode as CalloopMode, PostAction,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Client, DisplayHandle, protocol::wl_surface::WlSurface};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState};
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

use crate::model::{Hit, Managed, Mode, Rect, ResizeEdge, Shortcuts, WindowId, WindowModel};
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
        start_mode: Mode,
        restore: Rect,
        anchor_num: i32,
        anchor_den: i32,
        anchor_y: i32,
        titlebar: bool,
    },
    Resize {
        id: WindowId,
        pointer: (f64, f64),
        start: Rect,
        edge: ResizeEdge,
    },
}

impl Grab {
    pub fn move_window(
        id: WindowId,
        pointer: (f64, f64),
        managed: &Managed,
        titlebar: bool,
    ) -> Self {
        let frame = managed.frame();
        let anchor_num = (pointer.0.round() as i32 - frame.x).clamp(0, frame.w);
        let anchor_y = (pointer.1.round() as i32 - frame.y).clamp(0, crate::model::TITLEBAR_HEIGHT);
        Self::Move {
            id,
            pointer,
            start: managed.rect,
            start_mode: managed.mode,
            restore: managed.restore,
            anchor_num,
            anchor_den: frame.w.max(1),
            anchor_y,
            titlebar,
        }
    }
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
        (true | false, false) => FrameDecision::Idle,
        (false, true) => FrameDecision::Draw,
    }
}

/// How a paint obtains its scene elements (everything except the cursor).
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneBuild {
    /// Rebuild the window, decoration and desktop elements and recapture the backdrop.
    Full,
    /// Keep the cached elements; only the cursor elements change.
    CursorOnly,
}

/// Cursor-only paints need a cached scene and no scene change since it was built.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn scene_build(cached: bool, scene_dirty: bool) -> SceneBuild {
    if cached && !scene_dirty {
        SceneBuild::CursorOnly
    } else {
        SceneBuild::Full
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

/// Whether one paint pass owes the bounded retry timer.
///
/// A real sync or buffer failure arms it ([`arm_retry`]); so does an output whose GPU fence did not
/// signal this pass ([`OutputFrames::defer_sync`], recognised by `sync_waited_ms > 0`), because a
/// deferred output stays dirty and the bounded retry pass is the only thing that paints it again.
/// Never while paused, and the caller still caps the number of attempts.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn retry_owed(needs_retry: bool, dirty: bool, sync_waited_ms: u128, paused: bool) -> bool {
    if paused {
        return false;
    }
    sync_waited_ms > 0 || arm_retry(needs_retry, dirty, paused)
}

/// Longest a single paint may block on a GPU fence before the output is deferred instead.
///
/// Smithay returns `RenderFrameResult::needs_sync` when KMS cannot wait for the render itself (no
/// explicit fencing, or a fence that cannot be handed to KMS). The documented contract is that the
/// caller synchronizes before rendering/submitting again; a wait longer than this means the GPU is not
/// going to finish for us, so the compositor must stop blocking the loop.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub const SYNC_WAIT_BUDGET: Duration = Duration::from_millis(250);

/// [`SYNC_WAIT_BUDGET`] in milliseconds, for the pure decision function.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub const SYNC_WAIT_BUDGET_MS: u128 = SYNC_WAIT_BUDGET.as_millis();

/// Cumulative fence waiting after which an output is treated as wedged and left to the retry bound.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub const SYNC_WAIT_LIMIT_MS: u128 = 750;

/// What the tty backend does about the GPU fence of a frame whose render returned `needs_sync`.
///
/// There is no third option on purpose: *never* render again without either having waited for the
/// fence or having deferred this output. Rendering over an unfinished buffer is what produces the
/// repeated buffer/IO errors and finally a wedged pipeline.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncWait {
    /// Nothing outstanding or the fence signaled: the frame may be queued and the next render may run.
    Wait,
    /// The fence did not signal inside the budget. Do not queue over it: leave the output pending and
    /// let a bounded retry re-plan it instead of blocking the compositor thread.
    Defer,
    /// The fence is past the cumulative limit: the pipeline is wedged. Mark the output and move on to
    /// the other outputs; the bounded retry counter is what finally stops the attempts.
    Fail,
}

/// Decides what to do with a fence that has already been waited on for `waited_ms` (cumulative for one
/// output, so repeated deferrals reach [`SyncWait::Fail`] instead of waiting forever).
///
/// Pure on purpose: the whole sync contract is then testable on a machine without a GPU.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn sync_wait_decision(needs_sync: bool, waited_ms: u128) -> SyncWait {
    if !needs_sync {
        // Nothing to synchronize with: this is the only case where queueing straight away is correct.
        return SyncWait::Wait;
    }
    if waited_ms < SYNC_WAIT_BUDGET_MS {
        SyncWait::Wait
    } else if waited_ms < SYNC_WAIT_LIMIT_MS {
        SyncWait::Defer
    } else {
        SyncWait::Fail
    }
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
    /// Cumulative milliseconds this output has spent blocking on a GPU fence since it last queued a
    /// frame. It is what turns "deferred" into "wedged" after repeated passes, and it is why the wait
    /// never has to be unbounded.
    pub sync_waited_ms: u128,
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
        self.sync_waited_ms = 0;
    }

    /// The renderer drew nothing: no vblank follows, and nothing is in flight to synchronize with, so
    /// no retry may be armed (`empty(false)`). An idle output arms no timer.
    pub fn empty(&mut self, needs_sync: bool) {
        self.queued = false;
        self.dirty = false;
        self.needs_retry = needs_sync;
        self.sync_waited_ms = 0;
    }

    /// The frame failed; a bounded retry is owed.
    pub fn fail(&mut self) {
        self.needs_retry = true;
    }

    /// The GPU fence of this pass did not signal: record the time already spent waiting, keep the
    /// output pending (so a bounded retry re-plans it) and ask for that retry.
    ///
    /// The frame is *not* queued: submitting a buffer the GPU is still writing is what corrupts the
    /// scanout and produces the repeated buffer/IO errors.
    pub fn defer_sync(&mut self, waited_ms: u128) {
        self.sync_waited_ms = self.sync_waited_ms.saturating_add(waited_ms);
        self.queued = false;
        self.dirty = true;
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
        self.sync_waited_ms = 0;
    }

    /// Coming back from a session pause: scanout was reset, so this output is stale and has to be
    /// painted in full.
    pub fn wake(&mut self) {
        self.queued = false;
        self.dirty = true;
        self.needs_retry = false;
        self.sync_waited_ms = 0;
    }
}

pub struct State {
    pub display: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub socket_name: String,
    pub compositor: CompositorState,
    /// `ext-session-lock-v1` global state; `Some` after `init_session_lock`.
    pub session_lock: Option<smithay::wayland::session_lock::SessionLockManagerState>,
    /// The surfaces a locking client drew, one per output, while the session is locked.
    pub locked_outputs: Vec<crate::session_lock::LockedOutput>,
    pub xdg_shell: XdgShellState,
    pub layer_shell: WlrLayerShellState,
    pub shm: ShmState,
    pub seat_state: SeatState<State>,
    pub data_device: DataDeviceState,
    /// Primary selection, xdg-activation and idle-inhibit state (see `compat`).
    pub compat: crate::compat::CompatState,
    pub popups: PopupManager,
    pub seat: Seat<State>,
    pub output: Output,
    pub output_size: Size<i32, Logical>,
    /// Last work area used to place snapped/maximized/fullscreen windows.
    cached_work_area: Rect,
    pub damage: OutputDamageTracker,
    pub model: WindowModel,
    /// Resolved blue-light intensity (0..=100). The compositor has no output gamma ramp through
    /// Smithay, so it paints the shared amber veil at this strength; see `flamewm_api::blue_light`.
    pub blue_light_intensity: u8,
    /// Smithay toplevels keyed by the model identifier.
    pub toplevels: Vec<(WindowId, ToplevelSurface)>,
    /// `ext-foreign-toplevel-list-v1` global; one handle per window, with the facts last sent.
    pub foreign_toplevels: ForeignToplevelListState,
    pub foreign_toplevel_handles: HashMap<WindowId, (ForeignToplevelHandle, ToplevelFacts)>,
    pub next_id: WindowId,
    pub focused: Option<WindowId>,
    pub shortcuts: Shortcuts,
    pub pointer: Point<f64, Logical>,
    /// What clients asked the pointer to show (`wl_pointer.set_cursor`, `XWayland` included).
    pub cursor_status: CursorImageStatus,
    /// Session xcursor theme and the cursors decoded from it.
    pub cursors: crate::cursor::Cursors,
    /// Server-side titlebar fills, stable across frames (see `render::Decorations`).
    pub decorations: RefCell<crate::render::Decorations>,
    pub grab: Option<Grab>,
    /// Titlebar control held down by the primary button: shown pressed while the pointer stays on it.
    pub pressed_hit: Option<Hit>,
    /// Window whose resize configure a grab deferred; sent once at the next paint.
    pub pending_configure: Option<WindowId>,
    /// Rounded snap-target preview, shown only while a move grab targets a snap; hidden on release.
    pub snap_preview: crate::snap_preview::SnapPreview,
    /// Title release that moved nothing, awaiting a possible double click (see `TitleClick`).
    pub last_title_click: Option<crate::input::TitleClick>,
    /// `focusFollowsMouse`, read once at startup: pointer entry focuses the window under it.
    pub focus_follows_mouse: bool,
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
    /// Set by anything that can change the scene below the cursor (windows, pixels, decorations,
    /// backdrop). A paint that finds it clear reuses the last scene and only swaps the cursor.
    scene_dirty: Cell<bool>,
    /// Last nested-window paint, and whether a paced redraw timer is already armed for it.
    winit_frame_at: Cell<Option<std::time::Instant>>,
    winit_frame_armed: Cell<bool>,
    /// Set by any change that can alter the shell's mirror (windows, focus, settings): the control
    /// service rebuilds and publishes the mirror only then, so pointer motion never pays for it.
    mirror_dirty: Cell<bool>,
    /// Why a running session has to end as a startup failure, set by the tty backend's first-frame
    /// deadline (`startup::FirstFrameGate`) and read by `main` after the loop, so the process exits
    /// non-zero.
    startup_failure: Cell<Option<&'static str>>,
    /// Wakes the tty backend for `redraw_dirty`; installed by `drm::init`, `None` when nested.
    /// The `RefCell` keeps `ping` callable from the `&self` request path.
    drm_waker: Option<RefCell<Ping>>,

    pub loop_handle: LoopHandle<'static, State>,
    pub xwayland_shell: XWaylandShellState,
    /// X11 window manager, present once `XWayland` reports ready.
    pub xwm: Option<X11Wm>,
    /// Managed X11 windows, keyed by the model identifier.
    pub x11: Vec<(WindowId, X11Surface)>,
    /// Titles cached on map; never roundtrip to `XWayland` in the renderer hot path.
    pub titles: HashMap<WindowId, String>,
    /// Override-redirect windows, docks and the desktop; the last entry is the topmost.
    pub unmanaged: Vec<Unmanaged>,
    /// DISPLAY number of the running `XWayland` server.
    pub display_number: Option<u32>,
    pub props: Option<Props>,
    /// Programs started once `XWayland` is ready (`FLAMEWM_WAYLAND_AUTOSTART`, colon separated).
    pub autostart: Vec<String>,
    /// Children started by the compositor; reaped by `reap_children` when one exits (its pidfd wakes
    /// the loop), so they never linger as zombies.
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
        let session_lock = None;
        let locked_outputs = Vec::new();
        let xdg_shell = XdgShellState::new::<State>(&display);
        let _xdg_decoration = XdgDecorationState::new::<State>(&display);
        let layer_shell = WlrLayerShellState::new::<State>(&display);
        let shm = ShmState::new::<State>(&display, vec![]);
        let _outputs = OutputManagerState::new_with_xdg_output::<State>(&display);
        let mut seat_state = SeatState::new();
        let data_device = DataDeviceState::new::<State>(&display);
        let compat = crate::compat::CompatState::new(&display);
        let _viewporter = ViewporterState::new::<State>(&display);
        let foreign_toplevels = ForeignToplevelListState::new::<State>(&display);
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
        seat.add_keyboard(crate::input::saved_xkb_config(), 200, 25)?;
        seat.add_pointer();

        let shortcuts = Shortcuts::from_bindings(ShortcutRegistry::default().bindings());
        let damage = OutputDamageTracker::from_output(&output);
        // `titlebars` and `windowMargin` are saved settings; without this the model draws a
        // titlebar and ignores the margin no matter what Settings holds.
        let (titlebars, window_margin) = crate::input::saved_model_settings();
        let mut model = WindowModel::new(4);
        model.configure(titlebars, window_margin);

        Ok(Self {
            session_lock,
            locked_outputs,
            display,
            loop_signal,
            socket_name,
            compositor,
            xdg_shell,
            layer_shell,
            shm,
            seat_state,
            data_device,
            compat,
            popups: PopupManager::default(),
            seat,
            output,
            output_size: Size::from((1280, 800)),
            cached_work_area: Rect::new(0, 0, 1280, 800),
            damage,
            model,
            blue_light_intensity: 0,
            toplevels: Vec::new(),
            next_id: 1,
            focused: None,
            foreign_toplevels,
            foreign_toplevel_handles: HashMap::new(),
            shortcuts,
            pointer: Point::from((0.0, 0.0)),
            cursor_status: CursorImageStatus::default_named(),
            cursors: crate::cursor::Cursors::from_env(),
            decorations: RefCell::default(),
            grab: None,
            pressed_hit: None,
            pending_configure: None,
            snap_preview: Default::default(),
            last_title_click: None,
            focus_follows_mouse: crate::input::saved_focus_follows_mouse(),
            super_tap: false,
            swallowed_keys: Vec::new(),
            backend: None,
            dump: None,
            dumped: false,
            exit_after_dump: false,
            // The first frame is always painted.
            redraw_dirty: Cell::new(true),
            scene_dirty: Cell::new(true),
            winit_frame_at: Cell::new(None),
            winit_frame_armed: Cell::new(false),
            mirror_dirty: Cell::new(true),
            startup_failure: Cell::new(None),
            drm_waker: None,
            loop_handle,
            xwayland_shell,
            xwm: None,
            x11: Vec::new(),
            titles: HashMap::new(),
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
        // Includes fullscreen and maximized windows on inactive workspaces, and keeps the
        // fullscreen restore rect in step with the output and current layer/dock reservations.
        self.reflow_if_work_area_changed();
        self.request_redraw();
    }

    pub fn window_title(&self, id: WindowId) -> &str {
        self.titles.get(&id).map_or("FlameWM", String::as_str)
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

    /// Announces a new window to `ext-foreign-toplevel-list` clients.
    pub fn foreign_toplevel_open(&mut self, id: WindowId, facts: ToplevelFacts) {
        let handle = self
            .foreign_toplevels
            .new_toplevel::<State>(facts.title.as_str(), facts.app_id.as_str());
        handle.send_done();
        self.foreign_toplevel_handles.insert(id, (handle, facts));
    }

    /// Sends only the fields that changed since the last announcement, then one `done`.
    pub fn foreign_toplevel_sync(&mut self, id: WindowId, facts: ToplevelFacts) {
        let Some((handle, sent)) = self.foreign_toplevel_handles.get_mut(&id) else {
            return;
        };
        let delta = diff_toplevel_facts(sent, &facts);
        if delta.is_empty() {
            return;
        }
        if delta.title {
            handle.send_title(&facts.title);
        }
        if delta.app_id {
            handle.send_app_id(&facts.app_id);
        }
        handle.send_done();
        *sent = facts;
    }

    /// Tells `ext-foreign-toplevel-list` clients that a window is gone.
    pub fn foreign_toplevel_close(&mut self, id: WindowId) {
        if let Some((handle, _)) = self.foreign_toplevel_handles.remove(&id) {
            self.foreign_toplevels.remove_toplevel(&handle);
        }
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

    /// Refit output-sized placements once, and only once, for each effective work-area change.
    pub(crate) fn reflow_if_work_area_changed(&mut self) {
        let work = self.work_area();
        let previous = std::mem::replace(&mut self.cached_work_area, work);
        let output = self.output_rect();
        // A shrunk or unplugged output can leave the pointer past its edge; keep it on screen.
        let (px, py) = crate::model::clamp_point_into(self.pointer.x, self.pointer.y, output);
        self.pointer = Point::from((px, py));
        let mut fitted = refit_model_placements(&mut self.model, output, previous, work);
        // Floating windows only need pulling back when the usable area itself changed (an output
        // resize, unplug or panel change); otherwise they keep the spot the user put them.
        if previous != work {
            fitted.extend(self.model.clamp_floating_into(output));
        }
        for id in fitted {
            self.apply_geometry(id);
        }
        if previous != work {
            self.request_redraw();
        }
    }

    /// Sends the configure a resize grab deferred. Called once per paint, so a burst of pointer
    /// motion between frames costs one configure.
    pub fn flush_pending_configure(&mut self) {
        if let Some(id) = self.pending_configure.take() {
            self.apply_geometry(id);
        }
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
    ///
    /// While a session lock is active no other client may take focus: the lock surface is the
    /// only thing allowed to receive input, or the lock could be bypassed by keystrokes.
    pub fn set_focus(&mut self, id: Option<WindowId>) {
        if self.pin_focus_to_lock() {
            return;
        }
        let previous = self.focused;
        let stack_before = self.model.visible_top_first();
        self.focused = id;
        if let Some(id) = id {
            self.model.raise(id);
        }
        let stack_changed = stack_before != self.model.visible_top_first();
        if let Some(prev) = previous.filter(|prev| Some(*prev) != id) {
            self.apply_geometry(prev);
        }
        if let Some(id) = id {
            self.apply_geometry(id);
        }
        let keyboard = self.seat.get_keyboard();
        let surface = id.and_then(|id| self.window_wl_surface(id));
        if focus_needs_restack(previous, id, stack_changed) {
            self.sync_xwayland_stacking();
        }
        if let Some(keyboard) = keyboard {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
        if let Some(window) = id.and_then(|id| self.x11_window(id)).cloned() {
            self.set_x11_input_focus(&window);
        }
        self.request_redraw();
    }

    /// Keeps the keyboard on the lock surface while locked.
    ///
    /// Returns `true` when a lock is active, meaning the caller must not move focus. A lock
    /// surface can appear or be replaced at any time, so the focus is re-asserted rather than
    /// assumed still correct.
    #[must_use]
    pub fn pin_focus_to_lock(&mut self) -> bool {
        let Some(surface) = self.lock_surface().map(|lock| lock.wl_surface().clone()) else {
            return false;
        };
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        }
        true
    }

    /// Focuses the topmost visible window, or clears focus when the workspace is empty.
    pub fn refocus(&mut self) {
        let next = self.model.visible_top_first().first().copied();
        self.set_focus(next);
    }

    /// `focusFollowsMouse`: focuses the window under the pointer without raising it.
    ///
    /// Entry must not restack, or sweeping the pointer across a stack would shuffle it. X11's
    /// `focus_hover` calls `focus_with_stacking(client, false)` for exactly this reason, and this
    /// is the same move: keyboard focus only.
    pub fn focus_hover(&mut self, id: WindowId) {
        if self.pin_focus_to_lock() {
            return;
        }
        if self.focused == Some(id) {
            return;
        }
        self.focused = Some(id);
        let keyboard = self.seat.get_keyboard();
        let surface = self.window_wl_surface(id);
        if let Some(keyboard) = keyboard {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
        if let Some(window) = self.x11_window(id).cloned() {
            self.set_x11_input_focus(&window);
        }
        self.request_redraw();
    }

    /// Asks for a complete repaint of the tty backend: sets the coalescing dirty flag and pings the
    /// wake pipe, so the frame happens on the next loop turn instead of waiting for an unrelated
    /// event. Any number of calls before one paint cost a single frame.
    pub fn request_full_redraw(&self) {
        self.mirror_dirty.set(true);
        self.request_paint();
    }

    /// Repaint only: the frame has to show new pixels (pointer motion moves the cursor), but nothing
    /// the shell mirror reports has changed.
    pub fn request_paint(&self) {
        self.scene_dirty.set(true);
        self.request_cursor_paint();
    }

    /// Repaint for pointer motion only: the scene is unchanged, so the paint may reuse it and move the
    /// cursor element. The caller guarantees nothing else it draws changed (hover included).
    pub fn request_cursor_paint(&self) {
        self.redraw_dirty.set(true);
        self.request_winit_frame();
        if let Some(waker) = self.drm_waker.as_ref() {
            // The ping coalesces in the kernel and a closed source only reports an error, which is
            // nothing the compositor can act on.
            let () = waker.borrow_mut().ping();
        }
    }

    /// True when the host window draws the pointer itself: a named theme cursor (or none). Client
    /// cursor surfaces stay software-drawn. Pointer motion then needs no frame at all.
    pub fn host_draws_pointer_status(status: &CursorImageStatus) -> bool {
        matches!(
            status,
            CursorImageStatus::Named(_) | CursorImageStatus::Hidden
        )
    }

    /// Nested winit only: mirrors the pointer image onto the host window. A surface or hidden
    /// cursor hides the host pointer; the software cursor then draws the surface, if any.
    pub fn sync_host_cursor(&self) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        let window = backend.window();
        match &self.cursor_status {
            CursorImageStatus::Named(icon) => {
                window.set_cursor_visible(true);
                window.set_cursor(*icon);
            }
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => {
                window.set_cursor_visible(false);
            }
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

    /// Consumes the scene-change flag: true when the next paint must rebuild every element.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn take_scene_dirty(&self) -> bool {
        self.scene_dirty.replace(false)
    }

    /// Consumes the mirror-change flag set by `request_full_redraw`.
    pub fn take_mirror_dirty(&self) -> bool {
        self.mirror_dirty.replace(false)
    }

    /// Records that the loop has to end as a startup failure (the tty backend's first-frame
    /// deadline calls this before stopping the loop). `main` reads it afterwards and exits non-zero,
    /// so the backend is torn down and the console comes back instead of a dead black VT.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn report_startup_failure(&self, reason: &'static str) {
        self.startup_failure.set(Some(reason));
    }

    /// The startup failure recorded during the run, if any.
    pub fn startup_failure(&self) -> Option<&'static str> {
        self.startup_failure.get()
    }

    /// True while the session still owes a desktop: autostart clients are configured but `XWayland`
    /// never became ready, so nothing can ever map a window.
    ///
    /// The first frame is presented before `XWayland` is even spawned, and it only ever contains the
    /// clear colour plus the cursor, so a completed flip is no evidence that a desktop will appear.
    /// This is the condition the post-present content deadline watches.
    pub fn desktop_pending(&self) -> bool {
        self.xwm.is_none() && !self.autostart.is_empty()
    }

    pub fn request_redraw(&self) {
        self.request_full_redraw();
    }

    /// Asks the nested window for a frame, at most one per [`WINIT_FRAME_INTERVAL`]. Pointer
    /// motion arrives at input rate and every frame is a full software repaint, so a burst of
    /// requests inside one interval collapses into a single timer-driven redraw.
    fn request_winit_frame(&self) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        let now = std::time::Instant::now();
        let Some(last) = self.winit_frame_at.get() else {
            return backend.window().request_redraw();
        };
        let elapsed = now.duration_since(last);
        if elapsed >= WINIT_FRAME_INTERVAL {
            return backend.window().request_redraw();
        }
        if self.winit_frame_armed.replace(true) {
            return;
        }
        let armed = self.loop_handle.insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(
                WINIT_FRAME_INTERVAL - elapsed,
            ),
            |_, (), state| {
                state.winit_frame_armed.set(false);
                if let Some(backend) = state.backend.as_ref() {
                    backend.window().request_redraw();
                }
                smithay::reexports::calloop::timer::TimeoutAction::Drop
            },
        );
        if armed.is_err() {
            self.winit_frame_armed.set(false);
            backend.window().request_redraw();
        }
    }

    /// Records that the nested window has just painted, starting the next pacing interval.
    pub fn note_winit_frame(&self) {
        self.winit_frame_at.set(Some(std::time::Instant::now()));
    }

    /// Starts a program. Once `XWayland` runs, children get `DISPLAY` and no `WAYLAND_DISPLAY`,
    /// so toolkits use X11 and reach the compositor through `XWayland`.
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
            Ok(child) => {
                self.watch_exit(&child);
                self.children.borrow_mut().push(child);
            }
            Err(error) => eprintln!("flamewm-wayland: spawn {program} failed: {error}"),
        }
    }

    /// Wakes the loop once when `child` exits so it is reaped then; nothing polls for exits.
    fn watch_exit(&self, child: &Child) {
        let pid = rustix::process::Pid::from_child(child);
        let watched = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
            .map_err(|error| error.to_string())
            .and_then(|pidfd| {
                self.loop_handle
                    .insert_source(
                        Generic::new(pidfd, Interest::READ, CalloopMode::Level),
                        |_, _, state| {
                            state.reap_children();
                            Ok(PostAction::Remove)
                        },
                    )
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = watched {
            eprintln!("flamewm-wayland: child {pid:?} exit is not watched: {error}");
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
                .is_none_or(|data| {
                    data.lock()
                        .map(|d| d.initial_configure_sent)
                        .unwrap_or(true)
                })
        })
    }

    /// Compositor state for a client: our own clients carry `ClientState`, `XWayland` carries its own.
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

/// Refit every output-sized placement only when the effective work area changes.
pub(crate) fn refit_model_placements(
    model: &mut WindowModel,
    output: Rect,
    previous_work: Rect,
    work: Rect,
) -> Vec<WindowId> {
    if previous_work == work {
        return Vec::new();
    }
    let fitted: Vec<_> = model
        .managed()
        .iter()
        .filter(|window| window.mode != Mode::Normal)
        .map(|window| window.id)
        .collect();
    for id in &fitted {
        model.refit_output(*id, output, work);
    }
    fitted
}

fn focus_needs_restack(
    previous: Option<WindowId>,
    next: Option<WindowId>,
    visible_stack_changed: bool,
) -> bool {
    previous != next || visible_stack_changed
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

/// Child handles must not be abandoned when the nested compositor window closes: `XWayland`
/// clients can otherwise survive and keep old DISPLAY/socket resources alive across tests.
impl Drop for State {
    fn drop(&mut self) {
        for mut child in self.children.get_mut().drain(..) {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FrameDecision, GpuProbe, OutputFrames, Rect, SYNC_WAIT_BUDGET_MS, SYNC_WAIT_LIMIT_MS,
        SceneBuild, SyncWait, arm_retry, focus_needs_restack, pick_gpu, plan_frame, reserve_struts,
        retry_owed, scan_failed, scene_build, sync_wait_decision,
    };

    /// Named and hidden pointers are drawn by the host window (no software frame per motion);
    /// client cursor surfaces stay software-drawn.
    #[test]
    fn host_draws_named_and_hidden_pointers_only() {
        use super::State;
        use smithay::input::pointer::{CursorIcon, CursorImageStatus};
        assert!(State::host_draws_pointer_status(
            &CursorImageStatus::default_named()
        ));
        assert!(State::host_draws_pointer_status(&CursorImageStatus::Named(
            CursorIcon::Pointer
        )));
        assert!(State::host_draws_pointer_status(&CursorImageStatus::Hidden));
    }

    /// Pointer motion over an unchanged scene reuses the cached elements; any scene change, or no
    /// cache yet, rebuilds them.
    #[test]
    fn cursor_only_paint_reuses_scene_only_when_clean_and_cached() {
        assert_eq!(scene_build(true, false), SceneBuild::CursorOnly);
        assert_eq!(scene_build(true, true), SceneBuild::Full);
        assert_eq!(scene_build(false, false), SceneBuild::Full);
        assert_eq!(scene_build(false, true), SceneBuild::Full);
    }

    /// Clearing keyboard focus is a restack transition even when the model order is unchanged.
    #[test]
    fn clearing_focus_requests_xwayland_restack() {
        assert!(focus_needs_restack(Some(7), None, false));
        assert!(focus_needs_restack(None, None, true));
        assert!(!focus_needs_restack(None, None, false));
    }

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
            sync_waited_ms: 0,
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
            sync_waited_ms: 0,
        };
        frames.park();
        assert_eq!(frames, OutputFrames::default());
        assert_eq!(frames.plan(false, true), FrameDecision::Idle);
        frames.wake();
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    }

    /// The sync contract, as a pure decision: only "no fence" and "inside the budget" may render, and
    /// repeated deferrals end in `Fail` instead of an unbounded wait.
    #[test]
    fn sync_wait_decision_waits_within_the_budget_then_defers_then_fails() {
        // No fence to synchronize with: queue straight away.
        assert_eq!(sync_wait_decision(false, 0), SyncWait::Wait);
        assert_eq!(
            sync_wait_decision(false, SYNC_WAIT_LIMIT_MS * 10),
            SyncWait::Wait
        );
        // Inside the budget a bounded wait is still the right answer (it normally signals in <1 ms).
        assert_eq!(sync_wait_decision(true, 0), SyncWait::Wait);
        assert_eq!(
            sync_wait_decision(true, SYNC_WAIT_BUDGET_MS - 1),
            SyncWait::Wait
        );
        // The budget is spent: stop blocking and defer this output, never render over the fence.
        assert_eq!(
            sync_wait_decision(true, SYNC_WAIT_BUDGET_MS),
            SyncWait::Defer
        );
        assert_eq!(
            sync_wait_decision(true, SYNC_WAIT_LIMIT_MS - 1),
            SyncWait::Defer
        );
        // Past the cumulative limit the pipeline is wedged: fail the output, keep the loop alive.
        assert_eq!(sync_wait_decision(true, SYNC_WAIT_LIMIT_MS), SyncWait::Fail);
        assert_eq!(sync_wait_decision(true, u128::MAX), SyncWait::Fail);
        // The thresholds are ordered, or the three outcomes would be unreachable in this order.
        const { assert!(SYNC_WAIT_BUDGET_MS < SYNC_WAIT_LIMIT_MS) };
    }

    /// A deferred fence keeps the output pending and asks for the bounded retry, and the waiting
    /// accumulates so it can never loop forever; the next queued frame clears the accumulation.
    #[test]
    fn deferred_fence_keeps_the_output_pending_and_accumulates() {
        let mut frames = OutputFrames {
            dirty: true,
            ..OutputFrames::default()
        };
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
        frames.defer_sync(SYNC_WAIT_BUDGET_MS);
        // Not queued (the GPU is still writing that buffer), still wanting a frame, retry owed.
        assert!(!frames.queued);
        assert!(frames.dirty);
        assert!(frames.needs_retry);
        // `arm_retry` alone would not arm it: the output is dirty, so the pass owes the retry itself.
        assert!(!arm_retry(frames.needs_retry, frames.dirty, false));
        assert!(retry_owed(
            frames.needs_retry,
            frames.dirty,
            frames.sync_waited_ms,
            false
        ));
        assert!(!retry_owed(
            frames.needs_retry,
            frames.dirty,
            frames.sync_waited_ms,
            true
        ));
        assert_eq!(frames.sync_waited_ms, SYNC_WAIT_BUDGET_MS);
        assert_eq!(frames.plan(false, false), FrameDecision::Draw);
        assert_eq!(
            sync_wait_decision(true, frames.sync_waited_ms),
            SyncWait::Defer
        );
        // Waiting accumulates across passes until the decision becomes Fail.
        frames.defer_sync(SYNC_WAIT_BUDGET_MS);
        frames.defer_sync(SYNC_WAIT_BUDGET_MS);
        assert!(frames.sync_waited_ms >= SYNC_WAIT_LIMIT_MS);
        assert_eq!(
            sync_wait_decision(true, frames.sync_waited_ms),
            SyncWait::Fail
        );
        // A frame that did queue clears the accumulation for the next cycle.
        frames.queue();
        assert_eq!(frames.sync_waited_ms, 0);
        assert_eq!(
            sync_wait_decision(true, frames.sync_waited_ms),
            SyncWait::Wait
        );
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
