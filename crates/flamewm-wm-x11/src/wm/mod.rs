use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::animation::{self, Animator, Begin, CaptureCache, Effect, OpenGate, Settings, Tick};
use crate::frame::geometry::keep_on_outputs;
use flamewm_render_x11::{
    CapturedImage, PanelEdge, TransitionDirection, TransitionStyle, WarpOverlay, WarpRect,
};

/// Frame cadence while a transition is in flight (60 Hz).
const TRANSITION_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// A live transition window; `mirrored` plays the warp from target back to source (restore).
struct TransitionOverlay {
    overlay: WarpOverlay,
    mirrored: bool,
    /// The frame was unmapped so the opaque backdrop snapshot shows the area beneath it.
    frame_hidden: bool,
}

/// Logs a fallback notice once per process. Animations are skipped silently afterwards.
fn log_once(once: &std::sync::Once, message: &str) {
    once.call_once(|| eprintln!("flamewm: {message}"));
}

static TRANSITION_SKIP_LOG: std::sync::Once = std::sync::Once::new();
static DESTROYED_WINDOW_RACE_LOG: std::sync::Once = std::sync::Once::new();

/// Desktop-kind windows never raise: a configure from one is forced to the
/// bottom of the root stack with no sibling. Other kinds keep their intent.
fn desktop_stack_policy(mut aux: ConfigureWindowAux, kind: WindowKind) -> ConfigureWindowAux {
    if kind == WindowKind::Desktop {
        aux.sibling = None;
        aux.stack_mode = Some(StackMode::BELOW);
    }
    aux
}

/// True for X errors that mean the target window/drawable no longer exists.
fn is_destroyed_window_race(error: &ReplyOrIdError) -> bool {
    matches!(
        error,
        ReplyOrIdError::X11Error(x11)
            if matches!(x11.error_kind, ErrorKind::Window | ErrorKind::Drawable | ErrorKind::Pixmap)
    )
}

/// Rect a maximized client returns to on unmaximize, when that is a floating restore.
/// `None` when the client is not maximized or its saved placement is not floating.
fn unmaximize_target(placement: &PlacementState) -> Option<RootRect> {
    if placement.mode != PlacementMode::Maximized {
        return None;
    }
    match placement.resume {
        Some(snapshot) if snapshot.mode != PlacementMode::Floating => None,
        Some(snapshot) => Some(snapshot.rect),
        None => Some(placement.floating_restore),
    }
}

/// Consumes one `UnmapNotify` the WM expects for a frame it hid itself. Returns `true` when the
/// unmap must be ignored. Each count must be added exactly where the WM unmaps a frame.
fn consume_expected_unmap(ignore_unmap: &mut u8) -> bool {
    if *ignore_unmap > 0 {
        *ignore_unmap -= 1;
        true
    } else {
        false
    }
}

fn anim_rect(rect: Rect) -> animation::Rect {
    animation::Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width as i32,
        height: rect.height as i32,
    }
}

fn warp_rect(rect: animation::Rect) -> WarpRect {
    WarpRect {
        x: f64::from(rect.x),
        y: f64::from(rect.y),
        width: f64::from(rect.width),
        height: f64::from(rect.height),
    }
}

/// Emits a `restore.trace.*` event whose message is prefixed with wall-clock `t_ms=`.
fn trace_restore(id: &'static str, detail: impl FnOnce() -> String) {
    flamewm_debug::emit(
        flamewm_debug::DebugEventId(id),
        std::time::Duration::ZERO,
        || {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            format!("t_ms={ms} {}", detail())
        },
    );
}

impl<C: Connection> Wm<'_, C> {
    fn session_of(&self, client: Window) -> InteractionSession {
        self.sessions
            .get(&client)
            .map_or(InteractionSession::Idle, |state| state.session)
    }

    fn is_session_active(&self, client: Window) -> bool {
        self.sessions
            .get(&client)
            .is_some_and(|state| !state.session.is_idle())
    }
}

use flamewm_api::shortcuts::{
    ACTION_FULLSCREEN_ESCAPE, ACTION_LAUNCHER, ACTION_SCREENSHOT_FULL, ACTION_SCREENSHOT_REGION,
    ACTION_SCREENSHOT_WINDOW, ACTION_SESSION_LOCK, ACTION_SHOW_DESKTOP,
    ACTION_SHOW_DESKTOP_CTRL_ALT, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_CYCLE_NEXT, ACTION_WINDOW_CYCLE_PREV, ACTION_WINDOW_FLOAT,
    ACTION_WINDOW_FULLSCREEN, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_RESIZE_GROW, ACTION_WINDOW_RESIZE_SHRINK, ACTION_WINDOW_SWITCH,
    ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP,
    focus_direction, move_direction, workspace_activate_index, workspace_move_index,
};
use flamewm_platform::services::workspaces::{count_store_path, load_persisted_count};
use flamewm_platform::shortcuts::{
    ACTION_OPEN_ASSISTANT_CTRL_ALT, ACTION_OPEN_ASSISTANT_SUPER, ACTION_OPEN_FILES_CTRL_ALT,
    ACTION_OPEN_FILES_SUPER, ACTION_OPEN_FINDER, ACTION_OPEN_FINDER_CTRL_ALT,
    ACTION_OPEN_TERMINAL_CTRL_ALT, ACTION_OPEN_TERMINAL_SUPER, ACTION_VOLUME_DOWN,
    ACTION_VOLUME_MUTE, ACTION_VOLUME_UP,
};
use flamewm_platform::system::MediaAudioKey;
use flamewm_window_core::{
    SnapTarget, snap_geometry as core_snap_geometry,
    snap_target_in_output as core_snap_target_in_output,
};
use x11rb::connection::Connection;
use x11rb::errors::{ReplyError, ReplyOrIdError};
use x11rb::protocol::damage::{self};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Allow, Atom, AtomEnum, Button, ButtonIndex, ButtonPressEvent, ButtonReleaseEvent,
    CONFIGURE_NOTIFY_EVENT, ChangeWindowAttributesAux, ClientMessageEvent, ClipOrdering,
    ConfigWindow, ConfigureNotifyEvent, ConfigureRequestEvent, ConfigureWindowAux, ConnectionExt,
    CreateGCAux, CreateWindowAux, EnterNotifyEvent, EventMask, GetGeometryReply, GrabMode,
    GrabStatus, ImageFormat, ImageOrder, InputFocus, KeyButMask, KeyPressEvent, KeyReleaseEvent,
    LeaveNotifyEvent, MapRequestEvent, MapState, ModMask, MotionNotifyEvent, Pixmap, PropMode,
    Screen, SetMode, StackMode, Window, WindowClass,
};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME};

use crate::atoms::{AnyError, Atoms};
use crate::chrome;
use crate::classifier::{WindowKind, classify};
use crate::client::ManagedClient;
use crate::decoration::policy::{
    Decoration, DecorationHints, decoration_policy, gtk_frame_extents, motif_decorations_field,
};
use crate::event_pump::WmEventPump;
use crate::frame::controller::{ControllerState, FrameEffect, FrameEvent};
use crate::frame::coords::{RootPoint, RootRect};
use crate::frame::geometry::{
    FrameExtents, GeometryReason, GeometryRequest, frame_to_client_root, plan_client_configure,
};
use crate::frame::model::{
    FrameControl, FrameRegion, PlacementMode, PlacementSnapshot, PlacementState, ResizeEdges,
    frame_border_for_mode,
};
use crate::frame::resources::{
    FrameRegistry, FrameResources, INPUT_CHILD_EVENT_MASK, cursor_for_region,
};
use crate::frame::session::{InteractionSession, MoveSession};
use crate::geometry::Rect;
use crate::parse_wm_class;
use crate::size_hints::ClientSizeHints;
use crate::snap_preview::{self, SnapPreviewSurface};

pub mod chrome_runtime;
pub mod configure;
pub mod interaction;
pub mod lifecycle;

const BUTTON_PRIMARY: Button = 1;
/// How long after a fullscreen escape press a second press may kill the client that kept the keyboard.
const ESCAPE_KILL_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// Shortcut action ids that were bound but have no WM handler, logged once each.
static UNHANDLED_SHORTCUTS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn log_unhandled_shortcut_once(action: &str) {
    let mut seen = UNHANDLED_SHORTCUTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if seen.insert(action.to_owned()) {
        eprintln!("flamewm: shortcut action {action} has no WM handler; ignored");
    }
}

/// Focus revert target when the focused client is destroyed or unmapped. `NONE` would make the X
/// server discard key events and skip passive root grabs, silently killing every WM shortcut.
const FOCUS_REVERT_TO: InputFocus = InputFocus::POINTER_ROOT;

/// Gated key diagnostics: `FLAMEWM_KEY_DIAG=1` enables stderr key logging.
/// Read once per process; normal runs print nothing.
pub(crate) fn key_diag_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env::var("FLAMEWM_KEY_DIAG").is_ok_and(|value| value == "1"))
}

/// Workspace a navigation shortcut must activate in the product two-row grid, or `None` when the
/// action is not a workspace shortcut or the grid has no neighbour in that direction. Left/right
/// stay inside the current row and up/down switch rows; a missing neighbour is a deliberate no-op,
/// never a wrap. The grid itself is owned by `flamewm_api::workspace`.
fn workspace_shortcut_target(current: usize, count: usize, action: &str) -> Option<usize> {
    flamewm_api::shortcuts::workspace_direction(action)
        .and_then(|direction| flamewm_api::workspace::grid_neighbor(current, count, direction))
}

const WM_STATE_WITHDRAWN: u32 = 0;
const WM_STATE_NORMAL: u32 = 1;
const WM_STATE_ICONIC: u32 = 3;

/// Accumulated invalidation for one reactor turn. Semantic owners mark the
/// affected facets; the reactor flush drains them once per turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct WmChangeSet {
    pub windows: bool,
    pub workspaces: bool,
    pub panels: bool,
    pub work_area: bool,
    /// One-shot request from the `ToggleStartMenu` shortcut; the flush emits it once.
    pub start_menu_toggle: bool,
    /// One-shot request from the Launcher shortcut; the flush emits it once.
    pub launcher_toggle: bool,
    /// One-shot media-key request; the flush dispatches it through the platform host.
    pub media_audio_key: Option<MediaAudioKey>,
}

impl WmChangeSet {
    fn take(&mut self) -> Self {
        let out = *self;
        *self = Self::default();
        out
    }

    #[cfg(test)]
    fn any(self) -> bool {
        self.windows
            || self.workspaces
            || self.panels
            || self.work_area
            || self.start_menu_toggle
            || self.launcher_toggle
            || self.media_audio_key.is_some()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WmConfig {
    pub titlebar_height: u16,
    pub frame_border: u16,
    pub panel_reserve: u16,
    pub snap_zone: i32,
    pub workspaces: usize,
}

impl Default for WmConfig {
    fn default() -> Self {
        Self {
            titlebar_height: chrome::TITLEBAR_HEIGHT,
            frame_border: chrome::FRAME_BORDER,
            panel_reserve: 0,
            snap_zone: 24,
            workspaces: 4,
        }
    }
}

impl WmConfig {
    #[must_use]
    pub fn from_env() -> Self {
        let mut config = Self::default();
        config.titlebar_height = env_u16("FLAMEWM_TITLEBAR_HEIGHT", config.titlebar_height, 20, 72);
        config.panel_reserve = env_u16("FLAMEWM_PANEL_RESERVE", config.panel_reserve, 0, 256);
        config.snap_zone = i32::from(env_u16(
            "FLAMEWM_SNAP_ZONE",
            config.snap_zone as u16,
            4,
            128,
        ));
        config.workspaces = usize::from(env_u16(
            "FLAMEWM_WORKSPACES",
            config.workspaces as u16,
            1,
            flamewm_api::workspace::MAX_WORKSPACES as u16,
        ));
        config
    }
}

impl WmConfig {
    /// Starts from the persisted workspace count; `FLAMEWM_WORKSPACES` still wins when set.
    #[must_use]
    pub fn with_persisted_workspaces(self, settings_path: &std::path::Path) -> Self {
        let env_set = env::var_os("FLAMEWM_WORKSPACES").is_some_and(|value| !value.is_empty());
        let persisted = load_persisted_count(&count_store_path(settings_path));
        self.resolve_workspaces(env_set, persisted)
    }

    fn resolve_workspaces(mut self, env_set: bool, persisted: Option<usize>) -> Self {
        if let (false, Some(count)) = (env_set, persisted) {
            self.workspaces = count;
        }
        self
    }
}

#[cfg(test)]
mod workspace_count_tests;

fn env_u16(name: &str, default: u16, min: u16, max: u16) -> u16 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .map_or(default, |value| value.clamp(min, max))
}

fn any_conn_err(error: x11rb::errors::ConnectionError) -> AnyError {
    Box::new(std::io::Error::other(error.to_string()))
}

/// `_NET_FRAME_EXTENTS` for a server-decorated frame in `mode`: the effective
/// border (configured only while floating) on left/right/bottom, and
/// titlebar plus border on top.
fn frame_extents_for_mode(mode: PlacementMode, configured_border: u16, titlebar: u16) -> [u32; 4] {
    let border = u32::from(frame_border_for_mode(mode, configured_border));
    [border, border, u32::from(titlebar) + border, border]
}

/// True unless `client` already has exactly `extents` published (first publish always true).
fn frame_extents_needs_publish(
    published: &HashMap<Window, [u32; 4]>,
    client: Window,
    extents: [u32; 4],
) -> bool {
    published.get(&client) != Some(&extents)
}

/// True unless the last published client list and stacking are exactly `next` (first publish always true).
fn client_list_needs_publish(
    published: Option<&(Vec<Window>, Vec<Window>)>,
    client_list: &[Window],
    stacking: &[Window],
) -> bool {
    published.is_none_or(|(list, stack)| list != client_list || stack != stacking)
}

fn snap_target_bits(target: SnapTarget) -> u64 {
    match target {
        SnapTarget::None => 0,
        SnapTarget::LeftHalf => 1,
        SnapTarget::RightHalf => 2,
        SnapTarget::TopHalf => 3,
        SnapTarget::BottomHalf => 4,
        SnapTarget::TopLeftQuarter => 5,
        SnapTarget::TopRightQuarter => 6,
        SnapTarget::BottomLeftQuarter => 7,
        SnapTarget::BottomRightQuarter => 8,
        SnapTarget::Maximize => 9,
    }
}

pub(crate) struct Wm<'a, C: Connection> {
    conn: &'a C,
    screen_num: usize,
    config: WmConfig,
    atoms: Atoms,
    support_window: Window,
    catalog: std::sync::Arc<flamewm_applications::ApplicationCatalog>,
    icon_service: Rc<RefCell<flamewm_integrations_linux::icon_service::IconService>>,
    deferred_icon_jobs: HashMap<Window, (u64, flamewm_integrations_linux::icon_service::IconJob)>,
    icon_requests: HashMap<flamewm_integrations_linux::icons::IconKey, Vec<(Window, u64)>>,
    next_client_lifetime: u64,
    clients: HashMap<Window, ManagedClient>,
    managed_order: Vec<Window>,
    frame_to_client: HashMap<Window, Window>,
    /// Last `_NET_FRAME_EXTENTS` value published per client; skips identical re-publishes.
    frame_extents_published: HashMap<Window, [u32; 4]>,
    /// Last `_NET_CLIENT_LIST` and `_NET_CLIENT_LIST_STACKING` pair published; skips identical re-publishes.
    client_list_published: Option<(Vec<Window>, Vec<Window>)>,
    /// Last wallpaper layout published on the root; skips identical re-publishes.
    wallpaper_layout_published: Option<Vec<u32>>,
    /// Frames are redirected automatically through Composite (no compositing manager runs), so
    /// their contents stay valid off screen for translucent windows above them.
    redirect_frames: bool,
    registry: FrameRegistry,
    sessions: HashMap<Window, ControllerState>,
    // Declared before `renderer`: overlays must drop while the renderer's display is open.
    /// Timing owner for minimize/restore/maximize flights.
    animator: Animator,
    overlays: HashMap<Window, TransitionOverlay>,
    /// Finished overlays kept above a remapped window until the client has had time to paint.
    linger: animation::Linger<WarpOverlay>,
    /// Damage objects on clients whose overlay lingers; the first damage releases the overlay.
    damage_watch: HashMap<Window, u32>,
    /// Capture kept from a completed minimize so the matching restore can reuse it.
    minimized_captures: CaptureCache<CapturedImage>,
    /// Window-open effects armed at first map, waiting for a paintable capture on Expose.
    open_gate: OpenGate,
    renderer: Option<Rc<RefCell<flamewm_render_x11::ExternalDecorationRenderer>>>,
    current_workspace: usize,
    workspace_count: usize,
    active: Option<Window>,
    /// Last focused window per workspace, recorded when leaving that workspace.
    workspace_focus: Vec<Option<Window>>,
    screen_rect: Rect,
    output_work_areas: Vec<OutputWorkArea>,
    /// Region map and output bounds for the last controller build; see `ControllerGeometryCache`.
    controller_geometry: ControllerGeometryCache,
    /// Set by `RandR` notifications; the runtime refreshes the display snapshot on the next turn.
    display_topology_dirty: TopologyDirty,
    /// Desktop CRTC modes and the hold/restore state for fullscreen mode changes.
    desktop_mode: desktop_mode::DesktopModeTracker,
    /// `XInput2` raw key feed for escape chords while a client holds the keyboard.
    raw_keys: Option<raw_keys::RawKeys>,
    /// Show-desktop toggle state: the iconified set and the active window to focus on restore.
    show_desktop: show_desktop::ShowDesktop,
    /// Fullscreen client broken out by the last escape press, killed if a second press follows.
    escape_pending: Option<(Window, std::time::Instant)>,
    /// Alt+Tab session state and the focus history that orders it.
    switcher: switcher::Switcher,
    mru: switcher::Mru,
    /// Overlay snapshot queued for the next reactor flush.
    switcher_signal: Option<flamewm_control_wire::ControlSignal>,
    dock_struts: HashMap<Window, [u32; 12]>,
    snap_preview: SnapPreviewSurface,
    chrome_runtimes: HashMap<Window, chrome_runtime::ChromeRuntime>,
    /// Live skin chrome tokens (settings-derived); changes repaint all frames.
    decoration_tokens: flamewm_skin::recipes::decoration::ChromeTokens,
    /// `titlebars` value the frames were last reflowed for; a change resizes every Server frame.
    titlebars_applied: bool,
    /// Clients whose bottom corners were clipped to a rounded frame; used to
    /// restore the unshaped client when the decoration turns square.
    rounded_clients: HashSet<Window>,
    changes: WmChangeSet,
    /// Bare-Super tap state; see `SuperTap`. Start toggles only through it.
    super_tap: SuperTap,
    pointer_ingress_probe: PointerIngressProbe,
    resize_probe: Option<ResizeProbe>,
    edge_drag: Option<EdgeDragPending>,
    mouse_workspace_cooldowns: MouseWorkspaceCooldowns,
    /// Client whose titlebar repaint is paced to the frame interval during a move.
    chrome_deferred: Option<Window>,
    /// When the titlebar was last painted by the paced path.
    chrome_paint_at: Option<std::time::Instant>,
}

#[macro_use]
mod configure_summary;
mod client_state;
mod configure_handlers;
mod construct;
mod controllers;
mod counters;
mod desktop_mode;
mod drag_edge;
mod effects;
mod event_loop;
mod events;
mod icons;
mod manage;
mod outputs;
mod overlay;
mod paint;
mod pointer;
mod probes;
mod publish;
mod raw_keys;
mod rects;
mod release;
mod show_desktop;
mod super_tap;
mod switcher;
mod trace;
mod transition;
mod window_ops;
mod window_state;
mod workspace;
mod workspace_state;

use configure_summary::*;
use construct::*;
use controllers::*;
use counters::{bump, emit_resize_summary, move_counters, read, resize_counters};
use drag_edge::*;
pub(crate) use event_loop::{next_wait_timeout, run_with_hook};
#[cfg(test)]
use icons::{icon_completion_matches, retain_deferred_icon_job};
use outputs::*;
use rects::*;
use release::*;
use super_tap::*;
use trace::*;
pub(crate) use workspace_state::flamewm_workspace_root;
use workspace_state::*;

#[cfg(test)]
const WM_SOURCE: &str = concat!(
    include_str!("manage.rs"),
    include_str!("configure_handlers.rs"),
    include_str!("effects.rs"),
    include_str!("paint.rs"),
    include_str!("publish.rs"),
    include_str!("events.rs"),
    include_str!("pointer.rs"),
    include_str!("window_ops.rs"),
    include_str!("transition.rs"),
    include_str!("overlay.rs"),
    include_str!("workspace.rs"),
    include_str!("client_state.rs"),
    include_str!("configure_summary.rs"),
    include_str!("construct.rs"),
    include_str!("controllers.rs"),
    include_str!("drag_edge.rs"),
    include_str!("event_loop.rs"),
    include_str!("icons.rs"),
    include_str!("outputs.rs"),
    include_str!("probes.rs"),
    include_str!("rects.rs"),
    include_str!("release.rs"),
    include_str!("super_tap.rs"),
    include_str!("trace.rs"),
    include_str!("window_state.rs"),
    include_str!("workspace_state.rs"),
    include_str!("mod.rs"),
);

impl<'a, C: Connection> Wm<'a, C> {
    pub fn take_changes(&mut self) -> WmChangeSet {
        self.changes.take()
    }

    fn mark_windows(&mut self) {
        self.changes.windows = true;
    }

    fn mark_workspaces(&mut self) {
        self.changes.workspaces = true;
    }

    fn mark_work_area(&mut self) {
        self.changes.work_area = true;
    }
}

/// Target frame rect for a managed client after the output layout changes.
/// Floating windows keep a quarter visible on the outputs (may overflow the work area);
/// Snapped and Maximized keep work-area semantics.
fn rehome_outer(
    mode: PlacementMode,
    outer: Rect,
    outputs: &[OutputWorkArea],
    global: Rect,
    window_margin: i32,
) -> Rect {
    match mode {
        PlacementMode::Fullscreen => outer,
        PlacementMode::Maximized => {
            let work_area = to_core_rect(work_area_for_rect_in(outputs, global, outer));
            from_core_rect(flamewm_window_core::apply_window_margin(
                work_area,
                work_area,
                window_margin,
            ))
        }
        PlacementMode::Snapped(_) => {
            outer.clamp_inside(work_area_for_rect_in(outputs, global, outer))
        }
        PlacementMode::Floating => {
            let rects: Vec<RootRect> = outputs
                .iter()
                .map(|output| RootRect::from_legacy(output.geometry))
                .collect();
            from_root_rect(keep_on_outputs(
                RootRect::from_legacy(outer),
                &rects,
                RootRect::from_legacy(global),
            ))
        }
    }
}

/// Stacking for a dock: directly above `frame`, the normal window just raised to the top.
fn dock_above_frame_aux(frame: Window) -> ConfigureWindowAux {
    ConfigureWindowAux::new()
        .sibling(frame)
        .stack_mode(StackMode::ABOVE)
}

fn resolve_frame_event_source(registry: &FrameRegistry, event: Window, child: Window) -> Window {
    registry.lookup_by_xid(child).map_or(event, |_| child)
}

/// A press on an already-active client is replayed without focus or restacking.
fn should_focus_on_press(active: Option<Window>, client: Window) -> bool {
    active != Some(client)
}

/// Client-area press focus action. `Reassert` re-sends X input focus to an already-active
/// client: the cached `active` can diverge from the server's focus (popups, client-initiated
/// focus, unmap reverts), and FocusIn/FocusOut are not tracked, so a press must not trust it.
#[derive(Debug, PartialEq, Eq)]
enum PressFocus {
    Focus,
    Reassert,
    Skip,
}

fn press_focus_action(active: Option<Window>, client: Window, button: u8) -> PressFocus {
    if !matches!(button, 1..=3) {
        PressFocus::Skip
    } else if should_focus_on_press(active, client) {
        PressFocus::Focus
    } else {
        PressFocus::Reassert
    }
}

/// Whether frames may be redirected automatically: the Composite extension exists and no
/// compositing manager owns `_NET_WM_CM_S<screen>` (one would composite the frames itself).
fn automatic_frame_redirect_available<C: Connection>(conn: &C, screen_num: usize) -> bool {
    use x11rb::protocol::composite::ConnectionExt as _;
    let composite = conn
        .composite_query_version(0, 4)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|version| (version.major_version, version.minor_version) >= (0, 2));
    if !composite {
        return false;
    }
    let owner = conn
        .intern_atom(true, format!("_NET_WM_CM_S{screen_num}").as_bytes())
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .filter(|reply| reply.atom != x11rb::NONE)
        .and_then(|reply| conn.get_selection_owner(reply.atom).ok())
        .and_then(|cookie| cookie.reply().ok())
        .map_or(x11rb::NONE, |reply| reply.owner);
    owner == x11rb::NONE
}

fn pointer_grab_target(requested: Window, resources: FrameResources) -> bool {
    requested == resources.frame || resources.children().contains(&requested)
}

impl<C: Connection> Drop for Wm<'_, C> {
    fn drop(&mut self) {
        self.snap_preview.hide(self.conn);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod controller_geometry_tests;

#[cfg(test)]
mod transition_fix_tests;

/// `_NET_WM_MOVERESIZE` direction (EWMH 0 = top-left, clockwise) to frame edges.
/// Whether an input child is mapped for `decoration`. A Flame frame maps all of them. A
/// client-drawn (GTK CSD) frame maps only its eight resize handles, so edges and corners
/// still resize; title drag and controls stay with the client.
fn input_child_mapped(
    decoration: Decoration,
    region: Option<FrameRegion>,
    titlebars: bool,
) -> bool {
    match decoration {
        // With the titlebar hidden only the resize strips keep their input children;
        // the title-drag body and the three control cells must not be mapped.
        Decoration::Server if !titlebars => matches!(region, Some(FrameRegion::Resize(_))),
        Decoration::Server => true,
        Decoration::ClientSide => matches!(region, Some(FrameRegion::Resize(_))),
        Decoration::None => false,
    }
}

fn resize_edges_for_direction(direction: u32) -> ResizeEdges {
    match direction {
        0 => ResizeEdges::top_left(),
        1 => ResizeEdges::top(),
        2 => ResizeEdges::top_right(),
        3 => ResizeEdges::right(),
        4 => ResizeEdges::bottom_right(),
        5 => ResizeEdges::bottom(),
        6 => ResizeEdges::bottom_left(),
        _ => ResizeEdges::left(),
    }
}

#[cfg(test)]
mod csd_direction_tests;

#[cfg(test)]
mod frame_extents_publish_tests;
