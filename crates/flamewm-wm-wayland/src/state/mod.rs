//! Compositor state: protocol globals, the window model and the glue that keeps smithay
//! toplevels in step with the pure `model::WindowModel`.

mod focus;
mod frame_pacing;
#[cfg(feature = "drm")]
pub(crate) use frame_pacing::{
    FrameDecision, GpuProbe, OutputFrames, SYNC_WAIT_BUDGET, SceneBuild, SyncWait, pick_gpu,
    retry_owed, scan_failed, scene_build, sync_wait_decision,
};
mod outputs;
mod process;
mod redraw;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

/// Minimum spacing between nested-window paints (about one 60 Hz vblank).
const WINIT_FRAME_INTERVAL: Duration = Duration::from_millis(16);

use flamewm_platform::shortcuts::ShortcutRegistry;
use flamewm_window_core::dock_reserve::{self, ReserveEdge};
use flamewm_wm_wayland::protocols::toplevel::{ToplevelFacts, diff as diff_toplevel_facts};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::desktop::{PopupKind, PopupManager, layer_map_for_output};
use smithay::input::pointer::{CursorIcon, CursorImageStatus};
use smithay::input::{Seat, SeatState};
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::ping::Ping;
use smithay::reexports::calloop::{
    Interest, LoopHandle, LoopSignal, Mode as CalloopMode, PostAction,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::{Client, DisplayHandle, protocol::wl_surface::WlSurface};
use smithay::utils::{Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState, ImportNotifier};
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

pub struct State {
    pub display: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub socket_name: String,
    pub compositor: CompositorState,
    /// `ext-session-lock-v1` global state; `Some` after `init_session_lock`.
    pub session_lock: Option<smithay::wayland::session_lock::SessionLockManagerState>,
    /// The surfaces a locking client drew, one per output, while the session is locked.
    pub locked_outputs: Vec<crate::session_lock::LockedOutput>,
    /// Which outputs of the layout a live lock surface covers while the session is locked.
    pub lock_cover: crate::lock_cover::LockCover,
    /// True from a lock request until unlock. It outlives the locker: a locker that dies leaves
    /// the output blank and the keyboard unfocused, never the desktop.
    pub session_locked: bool,
    /// `zwlr_screencopy` copies waiting for the next render's readback.
    pub screencopy_pending: Vec<crate::screencopy::PendingCopy>,
    pub xdg_shell: XdgShellState,
    pub layer_shell: WlrLayerShellState,
    pub shm: ShmState,
    /// `zwp_linux_dmabuf_v1` state. The global is created by `dmabuf::init` once a renderer exists.
    pub dmabuf_state: DmabufState,
    /// The dmabuf global `dmabuf::init` created, kept so a renderer rebuild can refresh its default
    /// feedback. `None` when no global was offered.
    pub dmabuf_global: Option<DmabufGlobal>,
    /// Client dmabufs waiting for the native renderer, which lives in `drm::Drm` rather than here.
    /// Settled by `dmabuf::settle_pending` on the next paint.
    pub pending_dmabufs: Vec<(Dmabuf, ImportNotifier)>,
    pub seat_state: SeatState<State>,
    pub data_device: DataDeviceState,
    /// Primary selection, xdg-activation and idle-inhibit state (see `compat`).
    pub compat: crate::compat::CompatState,
    pub popups: PopupManager,
    pub seat: Seat<State>,
    pub output: Output,
    pub output_size: Size<i32, Logical>,
    /// Name of the primary [`Self::output`], cached at construction.
    ///
    /// `Output::name` clones a `String` out of the output's lock on every call, and the primary
    /// name is compared on every client commit (surface homing), every work-area query (pointer
    /// motion during a move grab) and every strut scan. An `Output`'s name never changes after
    /// creation, so the clone is cached once instead of being paid on each of those paths.
    primary_name: String,
    /// Last work area per output, used to place snapped/maximized/fullscreen windows.
    cached_work_areas: Vec<(String, Rect)>,
    /// Dock cut from the work area, from `dock.*` settings; `None` while the dock is hidden.
    pub(crate) dock_reserve: Option<(ReserveEdge, i32)>,
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
    /// Last position a backend reported. Equals `pointer` unless a pointer lock keeps the cursor
    /// still; relative motion is measured from here so locked deltas are not lost or doubled.
    pub raw_pointer: Point<f64, Logical>,
    /// Logical rects of the DRM outputs in the placed row; empty when only the primary output
    /// exists (nested or single output), so `on_motion` clamps to `output_rect` as before.
    pub pointer_layout: Vec<crate::model::Rect>,
    /// Connector names of the DRM outputs in the layout, kept in step with `pointer_layout`. A lock
    /// seeds its cover from these; empty means the primary output alone.
    pub layout_outputs: Vec<String>,
    /// Handles of the DRM outputs, kept in step with `layout_outputs`. Layer surfaces map on, and
    /// enter/leave is sent for, the handle of the output they sit on; empty means the primary alone.
    pub output_handles: Vec<Output>,
    /// Output each surface sits on (toplevels, popups, layer surfaces); absent means the primary output.
    pub surface_outputs:
        crate::output_layout::SurfaceOutputs<smithay::reexports::wayland_server::backend::ObjectId>,
    /// Scale of every DRM output by name, kept in step with `layout_outputs`; empty means primary only.
    pub output_scales: Vec<(String, f64)>,
    /// Debug-only extra outputs for the nested backend (`debug_outputs`); empty unless env-enabled.
    pub(crate) debug_outputs: crate::debug_outputs::VirtualOutputs,
    /// What clients asked the pointer to show (`wl_pointer.set_cursor`, `XWayland` included).
    pub client_cursor_status: CursorImageStatus,
    /// Resize cursor of the compositor handle under the pointer; it overrides the client's request.
    pub decor_cursor: Option<CursorIcon>,
    /// Pointer image in effect: the decoration cursor when set, else the client's request.
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
    /// `showContentWhileDragging`: live window move (true) or a landing outline only (false).
    pub show_content_while_dragging: bool,
    /// `snapPreviewEnabled`: draw the snap-zone ring while a move targets a zone (X11 default on).
    pub snap_preview_enabled: bool,
    /// `colourFilter`: the compositor colour filter (none by default). Synced from settings.
    pub colour_filter: crate::colour_filter::ColourFilter,
    /// Compiled colour-filter shader; `Some(Err(()))` after a failed compile, so it is not retried.
    pub colour_program: Option<Result<smithay::backend::renderer::gles::GlesTexProgram, ()>>,
    /// Magnifier factor (1.0..=8.0). Seeded from `zoomLevel`; zoom shortcuts change it at runtime.
    pub zoom_level: f64,
    /// `zoomFollowMode`: how the magnified viewport tracks the pointer. Synced from settings.
    pub zoom_follow: crate::zoom::FollowMode,
    /// Last magnified viewport origin in physical pixels, held by `Push`. Cleared on zoom changes.
    pub zoom_origin: Option<(f64, f64)>,
    /// Title release that moved nothing, awaiting a possible double click (see `TitleClick`).
    pub last_title_click: Option<crate::input::TitleClick>,
    /// `focusFollowsMouse`, read once at startup: pointer entry focuses the window under it.
    pub focus_follows_mouse: bool,
    /// Bare Super press that has not been combined with another key yet.
    pub super_tap: bool,
    /// Keycodes whose press was consumed as a shortcut; their release is consumed too.
    pub swallowed_keys: Vec<u32>,
    /// Open Alt+F7/F8 keyboard move or resize: the focused window and its grab state.
    pub kbd_grab: Option<(
        WindowId,
        flamewm_window_core::keyboard_move_resize::KeyboardGrab,
    )>,
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
        crate::handlers::init_pointer_protocols(&display);
        crate::presentation_scale::init(&display);
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
        let (show_content_while_dragging, snap_preview_enabled) =
            crate::input::saved_drag_feedback();
        let mut model = WindowModel::new(4);
        model.configure(titlebars, window_margin);
        let primary_name = output.name();

        Ok(Self {
            session_lock,
            locked_outputs,
            lock_cover: crate::lock_cover::LockCover::default(),
            session_locked: false,
            screencopy_pending: Vec::new(),
            display,
            loop_signal,
            socket_name,
            compositor,
            xdg_shell,
            layer_shell,
            shm,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            pending_dmabufs: Vec::new(),
            seat_state,
            data_device,
            compat,
            popups: PopupManager::default(),
            seat,
            output,
            output_size: Size::from((1280, 800)),
            primary_name,
            cached_work_areas: Vec::new(),
            dock_reserve: None,
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
            raw_pointer: Point::from((0.0, 0.0)),
            pointer_layout: Vec::new(),
            layout_outputs: Vec::new(),
            output_handles: Vec::new(),
            surface_outputs: Default::default(),
            output_scales: Vec::new(),
            debug_outputs: Default::default(),
            client_cursor_status: CursorImageStatus::default_named(),
            decor_cursor: None,
            cursor_status: CursorImageStatus::default_named(),
            cursors: crate::cursor::Cursors::from_env(),
            decorations: RefCell::default(),
            grab: None,
            pressed_hit: None,
            pending_configure: None,
            snap_preview: Default::default(),
            show_content_while_dragging,
            snap_preview_enabled,
            colour_filter: Default::default(),
            colour_program: None,
            zoom_level: flamewm_api::settings::ZOOM_LEVEL_MIN,
            zoom_follow: Default::default(),
            zoom_origin: None,
            last_title_click: None,
            focus_follows_mouse: crate::input::saved_focus_follows_mouse(),
            super_tap: false,
            swallowed_keys: Vec::new(),
            kbd_grab: None,
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
        // Layout is logical: the mode is physical, so divide by the output scale.
        let percent = (self.output_scale() * 100.0).round() as u16;
        let (w, h) = crate::output_layout::logical_size((width, height), percent);
        self.output_size = Size::from((w, h));
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

    /// Usable area: the output minus layer-shell exclusive zones (panels).
    /// Layer-shell zones and X11 docks (`_NET_WM_STRUT*`) both shrink the usable area.
    pub fn work_area(&self) -> Rect {
        // The zone stays empty until arrange() runs, which layer-shell events alone trigger; without
        // a layer surface every maximize would get a 0x0 work area.
        let mut map = layer_map_for_output(&self.output);
        map.arrange();
        let zone: Rectangle<i32, Logical> = map.non_exclusive_zone();
        let size = (self.output_size.w, self.output_size.h);
        let struts = self.struts_on(&self.primary_name, size);
        work_area_with_dock(
            Rect::new(zone.loc.x, zone.loc.y, zone.size.w, zone.size.h),
            &struts,
            self.dock_reserve,
        )
    }

    /// Sanitized unmanaged struts that reserve space on output `name`, in its local coordinates.
    fn struts_on(&self, name: &str, size: (i32, i32)) -> Vec<[i32; 4]> {
        let owned: Vec<(Rect, [i32; 4])> = self
            .unmanaged
            .iter()
            .filter_map(|entry| {
                let strut = entry.strut?;
                let geometry = entry.surface.geometry();
                let rect = Rect::new(
                    geometry.loc.x,
                    geometry.loc.y,
                    geometry.size.w,
                    geometry.size.h,
                );
                Some((rect, strut))
            })
            .collect();
        crate::output_layout::struts_on_output(
            &self.pointer_layout,
            &self.layout_outputs,
            &self.primary_name,
            &owned,
            name,
        )
        .into_iter()
        .filter_map(|strut| flamewm_window_core::strut::sanitize_strut(strut, None, size))
        .collect()
    }

    /// Output-local layout rect of `name`: its row in the placed layout, or the primary output.
    pub fn output_rect_on(&self, name: &str) -> Rect {
        self.layout_outputs
            .iter()
            .position(|n| n == name)
            .and_then(|index| self.pointer_layout.get(index).copied())
            .unwrap_or_else(|| self.output_rect())
    }

    /// Work area of output `name`: its layer zone and the unmanaged struts whose window sits on it,
    /// lifted to layout coordinates. The Dock stays on the primary output.
    pub fn work_area_on(&self, name: &str) -> Rect {
        if name == self.primary_name {
            return self.work_area();
        }
        let out = self.output_handle(name);
        let mut map = layer_map_for_output(&out);
        map.arrange();
        let zone = map.non_exclusive_zone();
        let zone = Rect::new(zone.loc.x, zone.loc.y, zone.size.w, zone.size.h);
        let output = self.output_rect_on(name);
        let struts = self.struts_on(name, (output.w, output.h));
        let zone = reserve_struts(zone, struts.into_iter());
        crate::output_layout::zone_in_layout(output, zone)
    }

    /// Work area of the output the window's rect sits on, the primary output when none does.
    pub fn work_area_of(&self, id: crate::model::WindowId) -> Rect {
        let Some(rect) = self.model.get(id).map(|w| w.rect) else {
            return self.work_area();
        };
        let name = crate::output_layout::output_name_for_rect(
            &self.pointer_layout,
            &self.layout_outputs,
            rect,
            &self.primary_name,
        );
        self.work_area_on(name)
    }

    /// Work area of the output under the pointer, where snapping and move-snap apply.
    ///
    /// Runs on every pointer motion of a move/resize grab, so it stays allocation-free: the
    /// output's name is borrowed from the layout instead of being cloned into a `String`.
    pub fn pointer_work_area(&self) -> Rect {
        match crate::output_layout::output_at(&self.pointer_layout, self.pointer.x, self.pointer.y)
            .and_then(|index| self.layout_outputs.get(index))
        {
            Some(name) => self.work_area_on(name),
            None => self.work_area(),
        }
    }

    /// Refits placements on each output whose work area changed, once per change. Outputs whose
    /// work area is untouched keep their placements, so a strut on one output never moves another.
    pub(crate) fn reflow_if_work_area_changed(&mut self) {
        let primary = self.primary_name.clone();
        let mut names = self.layout_outputs.clone();
        if !names.contains(&primary) {
            names.push(primary);
        }
        let current: Vec<(String, Rect)> = names
            .into_iter()
            .map(|name| {
                let work = self.work_area_on(&name);
                (name, work)
            })
            .collect();
        self.model.set_primary_output(&self.primary_name);
        for (name, work) in &current {
            self.model.record_work_area(name, *work);
        }
        let changed = changed_work_areas(&self.cached_work_areas, &current);
        self.cached_work_areas = current;
        // A shrunk or unplugged output can leave the pointer past its edge; keep it on screen.
        let output = self.output_rect();
        let (px, py) = crate::model::clamp_point_into(self.pointer.x, self.pointer.y, output);
        self.pointer = Point::from((px, py));
        let mut fitted = Vec::new();
        for (name, previous, work) in &changed {
            let rect = self.output_rect_on(name);
            let ids: Vec<WindowId> = self
                .model
                .managed()
                .iter()
                .filter(|window| self.rect_on_output(window.rect, name))
                .map(|window| window.id)
                .collect();
            // Floating windows only need pulling back when this output's usable area itself changed
            // (resize, unplug or panel); otherwise they keep the spot the user put them.
            let floating: Vec<WindowId> = self
                .model
                .managed()
                .iter()
                .filter(|window| window.mode == Mode::Normal && ids.contains(&window.id))
                .map(|window| window.id)
                .collect();
            fitted.extend(refit_model_placements(
                &mut self.model,
                rect,
                *previous,
                *work,
                |window| ids.contains(&window.id),
            ));
            fitted.extend(
                floating
                    .into_iter()
                    .filter(|id| self.model.clamp_floating_window_into(*id, rect)),
            );
        }
        for id in fitted {
            self.apply_geometry(id);
        }
        if !changed.is_empty() {
            self.request_redraw();
        }
    }

    /// Whether `rect` sits on output `name`, by the same rule `work_area_of` applies.
    fn rect_on_output(&self, rect: Rect, name: &str) -> bool {
        crate::output_layout::output_name_for_rect(
            &self.pointer_layout,
            &self.layout_outputs,
            rect,
            &self.primary_name,
        ) == name
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

    pub fn layer_initial_configure_sent(surface: &WlSurface) -> bool {
        with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .is_none_or(|data| data.lock().map_or(true, |d| d.initial_configure_sent))
        })
    }

    /// Compositor state for a client: our own clients carry `ClientState`, `XWayland` carries its own.
    pub fn client_compositor_state(client: &Client) -> &CompositorClientState {
        if let Some(data) = client.get_data::<ClientState>() {
            return &data.compositor;
        }
        if let Some(data) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &data.compositor_state;
        }
        missing_client_compositor_state()
    }
}

/// Fallback for a client that carries no compositor data. A misbehaving or half-initialised client
/// must not panic the compositor, so it shares one default state and the miss is logged once.
fn missing_client_compositor_state() -> &'static CompositorClientState {
    static FALLBACK: std::sync::OnceLock<CompositorClientState> = std::sync::OnceLock::new();
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!("flamewm-wayland: client without compositor data; using default state");
    }
    FALLBACK.get_or_init(CompositorClientState::default)
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

#[cfg(test)]
mod dock_work_area_tests {
    use super::*;
    use flamewm_window_core::dock_reserve::{ReserveEdge, dock_extent};

    const OUTPUT: Rect = Rect::new(0, 0, 1280, 800);

    /// The `work_area()` pipeline with the unmanaged struts (already sanitized) and the Dock.
    fn work_area(zone: Rect, struts: &[[i32; 4]], dock: Option<(ReserveEdge, i32)>) -> Rect {
        work_area_with_dock(zone, struts, dock)
    }

    #[test]
    fn dock_strut_on_its_edge_is_not_reserved_twice() {
        // Bottom Dock at 58 px publishes its strut as [l, r, t, b] = [0, 0, 0, 58].
        let dock = Some((ReserveEdge::Bottom, dock_extent(32)));
        assert_eq!(
            work_area(OUTPUT, &[[0, 0, 0, dock_extent(32)]], dock),
            Rect::new(0, 0, 1280, 800 - dock_extent(32))
        );
    }

    #[test]
    fn dock_strut_with_a_taskbar_reserves_the_edge_once() {
        // Taskbar 44 px and Dock strut 58 px on the same edge: max per edge gives 58, not 102.
        let struts = [[0, 0, 0, 44], [0, 0, 0, dock_extent(32)]];
        let dock = Some((ReserveEdge::Bottom, dock_extent(32)));
        assert_eq!(
            work_area(OUTPUT, &struts, dock),
            Rect::new(0, 0, 1280, 800 - dock_extent(32))
        );
    }

    #[test]
    fn dock_stacks_above_a_bottom_taskbar() {
        let taskbar = [[0, 0, 0, 44]];
        let work = work_area(
            OUTPUT,
            &taskbar,
            Some((ReserveEdge::Bottom, dock_extent(48))),
        );
        assert_eq!(work, Rect::new(0, 0, 1280, 800 - 44 - dock_extent(48)));
    }

    #[test]
    fn dock_alone_cuts_only_its_edge() {
        let work = work_area(OUTPUT, &[], Some((ReserveEdge::Left, dock_extent(48))));
        assert_eq!(
            work,
            Rect::new(dock_extent(48), 0, 1280 - dock_extent(48), 800)
        );
    }

    #[test]
    fn hidden_dock_and_no_taskbar_leave_the_output_whole() {
        assert_eq!(work_area(OUTPUT, &[], None), OUTPUT);
    }

    /// `KWin`'s `Workspace::rearrange` keeps one client area per screen and reports only the
    /// screens whose area changed. A panel on output B must reflow B alone, not output A.
    #[test]
    fn strut_on_the_secondary_output_reflows_only_that_output() {
        let primary = Rect::new(0, 0, 1280, 800);
        let secondary = Rect::new(1280, 0, 1920, 1080);
        let secondary_with_panel = Rect::new(1280, 0, 1920, 1036);
        let previous = [("A".to_owned(), primary), ("B".to_owned(), secondary)];
        let current = [
            ("A".to_owned(), primary),
            ("B".to_owned(), secondary_with_panel),
        ];
        assert_eq!(
            changed_work_areas(&previous, &current),
            vec![("B".to_owned(), secondary, secondary_with_panel)]
        );
    }
}

/// Work area after the layer zone and the unmanaged struts, then the Dock's reserve. The Dock's
/// own strut is already in `struts` when it touches its edge, so the same rule X11 applies skips
/// that edge instead of reserving it twice.
fn work_area_with_dock(zone: Rect, struts: &[[i32; 4]], dock: Option<(ReserveEdge, i32)>) -> Rect {
    let work = reserve_struts(zone, struts.iter().copied());
    let folded = struts.iter().fold([0_i64; 4], |mut folded, strut| {
        for (slot, value) in folded.iter_mut().zip(strut) {
            *slot = (*slot).max(i64::from(*value));
        }
        folded
    });
    cut_dock(
        work,
        dock_reserve::dock_reserve_uncovered_by_struts(folded, dock),
    )
}

/// Cuts the Dock's reserve from a work area; `None` (dock hidden) returns it unchanged.
fn cut_dock(work: Rect, dock: Option<(ReserveEdge, i32)>) -> Rect {
    let cut = dock_reserve::cut_dock(flamewm_api::Rect::new(work.x, work.y, work.w, work.h), dock);
    Rect::new(cut.x, cut.y, cut.width, cut.height)
}

/// Outputs whose work area differs from `previous`, as (name, previous, current). An output with no
/// previous entry, such as a fresh hotplug, counts as changed from an empty rect.
fn changed_work_areas(
    previous: &[(String, Rect)],
    current: &[(String, Rect)],
) -> Vec<(String, Rect, Rect)> {
    current
        .iter()
        .filter_map(|(name, work)| {
            let old = previous.iter().find(|(p, _)| p == name).map(|(_, r)| *r);
            (old != Some(*work))
                .then(|| (name.clone(), old.unwrap_or(Rect::new(0, 0, 0, 0)), *work))
        })
        .collect()
}

/// Refit every output-sized placement that `on_output` accepts, only when the work area changes.
pub(crate) fn refit_model_placements(
    model: &mut WindowModel,
    output: Rect,
    previous_work: Rect,
    work: Rect,
    on_output: impl Fn(&crate::model::Managed) -> bool,
) -> Vec<WindowId> {
    if previous_work == work {
        return Vec::new();
    }
    let fitted: Vec<_> = model
        .managed()
        .iter()
        .filter(|window| window.mode != Mode::Normal && on_output(window))
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
mod missing_client_state_tests {
    use super::missing_client_compositor_state;

    #[test]
    fn missing_client_data_falls_back_to_one_shared_default() {
        let first = std::ptr::from_ref(missing_client_compositor_state());
        let second = std::ptr::from_ref(missing_client_compositor_state());
        assert_eq!(first, second);
    }
}
