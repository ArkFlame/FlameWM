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

/// Consumes one UnmapNotify the WM expects for a frame it hid itself. Returns `true` when the
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

impl<'a, C: Connection> Wm<'a, C> {
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
    ACTION_OPEN_FILES_CTRL_ALT, ACTION_OPEN_FILES_SUPER, ACTION_OPEN_TERMINAL_CTRL_ALT,
    ACTION_OPEN_TERMINAL_SUPER, ACTION_VOLUME_DOWN, ACTION_VOLUME_MUTE, ACTION_VOLUME_UP,
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
use x11rb::protocol::xproto::*;
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

/// An intentional titlebar drag must dwell at the outer root edge before
/// traversing workspaces. Internal monitor boundaries never count.
const DRAG_EDGE_DWELL_MS: u32 = 220;
// Preserve proximity to the opposite boundary without immediately retriggering.
const DRAG_EDGE_INSET_PX: i32 = 24;
// Mouse-only workspace transitions: Left/Right share one 500ms cooldown;
// Top/Bottom share a separate cooldown. Keyboard and pager actions are exempt.
const MOUSE_WORKSPACE_COOLDOWN_MS: u64 = 500;
const DRAG_EDGE_HIT_PX: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseWorkspaceAxis {
    Horizontal,
    Vertical,
}

impl DragEdge {
    fn axis(self) -> MouseWorkspaceAxis {
        match self {
            Self::Left | Self::Right => MouseWorkspaceAxis::Horizontal,
            Self::Top | Self::Bottom => MouseWorkspaceAxis::Vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MouseWorkspaceCooldowns {
    horizontal: Option<std::time::Instant>,
    vertical: Option<std::time::Instant>,
}

impl MouseWorkspaceCooldowns {
    fn remaining(self, axis: MouseWorkspaceAxis, now: std::time::Instant) -> std::time::Duration {
        let timestamp = match axis {
            MouseWorkspaceAxis::Horizontal => self.horizontal,
            MouseWorkspaceAxis::Vertical => self.vertical,
        };
        timestamp.map_or(std::time::Duration::ZERO, |last| {
            std::time::Duration::from_millis(MOUSE_WORKSPACE_COOLDOWN_MS)
                .saturating_sub(now.saturating_duration_since(last))
        })
    }

    fn record(&mut self, axis: MouseWorkspaceAxis, now: std::time::Instant) {
        match axis {
            MouseWorkspaceAxis::Horizontal => self.horizontal = Some(now),
            MouseWorkspaceAxis::Vertical => self.vertical = Some(now),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EdgeDragPending {
    client: Window,
    side: DragEdge,
    since: std::time::Instant,
}

/// This policy sees only pointer moves from an already-activated Move session;
/// normal pointer motion, resize and titlebar clicks never call it.
fn drag_edge_at(pointer: RootPoint, root: Rect) -> Option<DragEdge> {
    // Only the outer root boundaries are eligible, never a monitor seam.
    // At a corner choose the closest edge (horizontal wins an exact tie),
    // avoiding two competing dwell timers for the same move session.
    let x = pointer.x;
    let y = pointer.y;
    let right = root.right().saturating_sub(1);
    let bottom = root.bottom().saturating_sub(1);
    if x < root.x || x > right || y < root.y || y > bottom {
        return None;
    }
    let mut candidate: Option<(i32, DragEdge)> = None;
    if root.width >= (DRAG_EDGE_INSET_PX * 3) as u32 {
        if x <= root.x.saturating_add(DRAG_EDGE_HIT_PX) {
            candidate = Some((x.saturating_sub(root.x), DragEdge::Left));
        } else if x >= right.saturating_sub(DRAG_EDGE_HIT_PX) {
            candidate = Some((right.saturating_sub(x), DragEdge::Right));
        }
    }
    if root.height >= (DRAG_EDGE_INSET_PX * 3) as u32 {
        let vertical = if y <= root.y.saturating_add(DRAG_EDGE_HIT_PX) {
            Some((y.saturating_sub(root.y), DragEdge::Top))
        } else if y >= bottom.saturating_sub(DRAG_EDGE_HIT_PX) {
            Some((bottom.saturating_sub(y), DragEdge::Bottom))
        } else {
            None
        };
        if let Some(vertical) = vertical {
            if candidate.is_none_or(|(distance, _)| vertical.0 < distance) {
                candidate = Some(vertical);
            }
        }
    }
    candidate.map(|(_, side)| side)
}

fn drag_edge_target(current: usize, count: usize, side: DragEdge) -> Option<usize> {
    // Horizontal mouse traversal wraps ONLY inside the current row; vertical
    // traversal wraps ONLY between the same column in the two-row grid.
    let position = flamewm_api::workspace::grid_position(current, count)?;
    let columns = flamewm_api::workspace::workspace_columns(count);
    let row_start = position.row.checked_mul(columns)?;
    let row_end = row_start.checked_add(columns)?.min(count);
    let target = match side {
        DragEdge::Left if current == row_start => row_end - 1,
        DragEdge::Left => current - 1,
        DragEdge::Right if current + 1 == row_end => row_start,
        DragEdge::Right => current + 1,
        DragEdge::Top | DragEdge::Bottom => {
            let opposite_row = (position.row + 1) % flamewm_api::workspace::WORKSPACE_ROWS;
            return flamewm_api::workspace::grid_index(opposite_row, position.column, count)
                .filter(|target| *target != current);
        }
    };
    (target != current).then_some(target)
}

fn drag_edge_destination(
    pointer: RootPoint,
    root: Rect,
    side: DragEdge,
    frame_height: i32,
) -> RootPoint {
    let inset_x = DRAG_EDGE_INSET_PX.min(root.width.saturating_sub(1) as i32 / 3);
    let inset_y = DRAG_EDGE_INSET_PX.min(root.height.saturating_sub(1) as i32 / 3);
    let x = match side {
        DragEdge::Left => root.right().saturating_sub(1).saturating_sub(inset_x),
        DragEdge::Right => root.x.saturating_add(inset_x),
        DragEdge::Top | DragEdge::Bottom => pointer.x.clamp(root.x, root.right().saturating_sub(1)),
    };
    let y = match side {
        DragEdge::Top => {
            // Warping a tall dragged window to the bottom 24 px would place
            // its titlebar off the active frame: keep at least half the frame
            // visible while retaining the cursor on the titlebar grip.
            let visible = (frame_height.max(1) / 2).max(32);
            root.bottom()
                .saturating_sub(1)
                .saturating_sub(inset_y.max(visible.saturating_sub(16)))
                .clamp(
                    root.y.saturating_add(inset_y),
                    root.bottom().saturating_sub(1).saturating_sub(inset_y),
                )
        }
        DragEdge::Bottom => root.y.saturating_add(inset_y),
        DragEdge::Left | DragEdge::Right => {
            pointer.y.clamp(root.y, root.bottom().saturating_sub(1))
        }
    };
    RootPoint::new(x, y)
}

fn drag_edge_ready(
    previous: Option<EdgeDragPending>,
    client: Window,
    side: DragEdge,
    now: std::time::Instant,
) -> (Option<EdgeDragPending>, bool) {
    if let Some(pending) = previous {
        if pending.client == client && pending.side == side {
            return (
                Some(pending),
                now.saturating_duration_since(pending.since)
                    >= std::time::Duration::from_millis(u64::from(DRAG_EDGE_DWELL_MS)),
            );
        }
    }
    (
        Some(EdgeDragPending {
            client,
            side,
            since: now,
        }),
        false,
    )
}

/// Arms exactly one reactor deadline while a titlebar move is held at an edge.
/// No busy polling: the ordinary X11 reactor blocks until this deadline or an
/// input event. A still pointer can therefore complete the dwell without a
/// second MotionNotify.
fn drag_edge_wait(
    pending: Option<EdgeDragPending>,
    cooldowns: MouseWorkspaceCooldowns,
    now: std::time::Instant,
    wait: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    let Some(pending) = pending else {
        return wait;
    };
    let dwell_remaining = std::time::Duration::from_millis(u64::from(DRAG_EDGE_DWELL_MS))
        .saturating_sub(now.saturating_duration_since(pending.since));
    // Wake once BOTH dwell and cooldown have expired (no zero-timeout loop).
    let remaining = dwell_remaining.max(cooldowns.remaining(pending.side.axis(), now));
    Some(wait.map_or(remaining, |other| other.min(remaining)))
}

/// Keep the grabbed titlebar and most of the frame visible on the arrival
/// side. Reusing the original pointer-to-frame offset can leave nearly the
/// entire frame offscreen after the pointer wraps; the controller will rebase
/// the grip at the new edge instead.
fn drag_entry_frame_x(pointer: RootPoint, width: i32, side: DragEdge) -> i32 {
    let width = width.max(1);
    let grip = (width / 2).clamp(1, DRAG_EDGE_INSET_PX - 8);
    match side {
        DragEdge::Right => pointer.x.saturating_sub(grip),
        DragEdge::Left => pointer.x.saturating_sub(width.saturating_sub(grip)),
        DragEdge::Top | DragEdge::Bottom => unreachable!("vertical transfers preserve x"),
    }
}

/// Vertical crossings keep the cursor in the titlebar after the warp. The
/// destination frame stays under the pointer rather than abandoning the grab.
fn drag_entry_frame_y(pointer: RootPoint, side: DragEdge) -> i32 {
    match side {
        DragEdge::Top | DragEdge::Bottom => pointer.y.saturating_sub(16),
        DragEdge::Left | DragEdge::Right => unreachable!("horizontal transfers preserve y"),
    }
}

/// Separate renderer work from the atomic X11 server-grab phase. The move
/// reducer owns geometry; only its X11-native position/notify effects may run
/// while other X clients are paused. In particular, PaintChrome uses another
/// connection and must run after UngrabServer has been flushed.
fn defer_drag_arrival_paint(
    mut commit: interaction::PointerCommit,
) -> Option<(interaction::PointerCommit, bool)> {
    if commit.grab.is_some()
        || commit.ungrab
        || !commit.effects.iter().all(|effect| {
            matches!(
                effect,
                FrameEffect::MoveFrameRoot { .. }
                    | FrameEffect::NotifyClientRoot { .. }
                    | FrameEffect::PaintChrome
                    | FrameEffect::Noop
            )
        })
    {
        return None;
    }
    let paint = commit
        .effects
        .iter()
        .any(|effect| matches!(effect, FrameEffect::PaintChrome));
    commit
        .effects
        .retain(|effect| !matches!(effect, FrameEffect::PaintChrome));
    Some((commit, paint))
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
    /// One-shot request from the ToggleStartMenu shortcut; the flush emits it once.
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
    Box::new(std::io::Error::new(
        std::io::ErrorKind::Other,
        error.to_string(),
    ))
}

struct ConfigureCounterSet {
    request_received: flamewm_profiler::CounterPoint,
    request_accepted: flamewm_profiler::CounterPoint,
    request_refused_interactive: flamewm_profiler::CounterPoint,
    request_refused_mode: flamewm_profiler::CounterPoint,
    notify_root: flamewm_profiler::CounterPoint,
    notify_frame: flamewm_profiler::CounterPoint,
    notify_client: flamewm_profiler::CounterPoint,
    notify_input_child: flamewm_profiler::CounterPoint,
    notify_expected: flamewm_profiler::CounterPoint,
    notify_mismatch: flamewm_profiler::CounterPoint,
    synthetic: flamewm_profiler::CounterPoint,
    request_received_total: std::sync::atomic::AtomicU64,
    request_accepted_total: std::sync::atomic::AtomicU64,
    request_refused_interactive_total: std::sync::atomic::AtomicU64,
    request_refused_mode_total: std::sync::atomic::AtomicU64,
    notify_root_total: std::sync::atomic::AtomicU64,
    notify_frame_total: std::sync::atomic::AtomicU64,
    notify_client_total: std::sync::atomic::AtomicU64,
    notify_input_child_total: std::sync::atomic::AtomicU64,
    notify_expected_total: std::sync::atomic::AtomicU64,
    notify_mismatch_total: std::sync::atomic::AtomicU64,
    synthetic_total: std::sync::atomic::AtomicU64,
}

fn configure_counters() -> &'static ConfigureCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<ConfigureCounterSet> = OnceLock::new();
    SET.get_or_init(|| ConfigureCounterSet {
        request_received: flamewm_profiler::CounterPoint::new("wm.configure.request.received"),
        request_accepted: flamewm_profiler::CounterPoint::new("wm.configure.request.accepted"),
        request_refused_interactive: flamewm_profiler::CounterPoint::new(
            "wm.configure.request.refused_interactive",
        ),
        request_refused_mode: flamewm_profiler::CounterPoint::new(
            "wm.configure.request.refused_mode",
        ),
        notify_root: flamewm_profiler::CounterPoint::new("wm.configure.notify.root"),
        notify_frame: flamewm_profiler::CounterPoint::new("wm.configure.notify.frame"),
        notify_client: flamewm_profiler::CounterPoint::new("wm.configure.notify.client"),
        notify_input_child: flamewm_profiler::CounterPoint::new("wm.configure.notify.input_child"),
        notify_expected: flamewm_profiler::CounterPoint::new("wm.configure.notify.expected"),
        notify_mismatch: flamewm_profiler::CounterPoint::new("wm.configure.notify.mismatch"),
        synthetic: flamewm_profiler::CounterPoint::new("wm.configure.synthetic"),
        request_received_total: std::sync::atomic::AtomicU64::new(0),
        request_accepted_total: std::sync::atomic::AtomicU64::new(0),
        request_refused_interactive_total: std::sync::atomic::AtomicU64::new(0),
        request_refused_mode_total: std::sync::atomic::AtomicU64::new(0),
        notify_root_total: std::sync::atomic::AtomicU64::new(0),
        notify_frame_total: std::sync::atomic::AtomicU64::new(0),
        notify_client_total: std::sync::atomic::AtomicU64::new(0),
        notify_input_child_total: std::sync::atomic::AtomicU64::new(0),
        notify_expected_total: std::sync::atomic::AtomicU64::new(0),
        notify_mismatch_total: std::sync::atomic::AtomicU64::new(0),
        synthetic_total: std::sync::atomic::AtomicU64::new(0),
    })
}

macro_rules! configure_bump {
    ($point:ident, $total:ident) => {{
        let counters = configure_counters();
        counters.$point.increment();
        bump(&counters.$total);
    }};
}

fn emit_configure_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_CONFIGURE_SUMMARY,
        std::time::Duration::from_secs(1),
        || {
            let counters = configure_counters();
            format!(
                "requests={} accepted={} refused_interactive={} refused_mode={} notify_frame={} notify_client={} expected={} mismatch={} synthetic={}",
                read(&counters.request_received_total),
                read(&counters.request_accepted_total),
                read(&counters.request_refused_interactive_total),
                read(&counters.request_refused_mode_total),
                read(&counters.notify_frame_total),
                read(&counters.notify_client_total),
                read(&counters.notify_expected_total),
                read(&counters.notify_mismatch_total),
                read(&counters.synthetic_total),
            )
        },
    );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseAction {
    None,
    Maximize,
    Snap(crate::frame::model::SnapTarget),
}

fn drag_activated(session: &InteractionSession) -> bool {
    match session {
        InteractionSession::Move(move_session) => move_session.anchor.is_activated(),
        InteractionSession::Resize(_) => true,
        _ => false,
    }
}

fn activated_move(session: Option<MoveSession>) -> Option<MoveSession> {
    match session {
        Some(session) if session.anchor.is_activated() => Some(session),
        _ => None,
    }
}

fn release_action(target: SnapTarget) -> ReleaseAction {
    match target {
        SnapTarget::None => ReleaseAction::None,
        SnapTarget::Maximize => ReleaseAction::Maximize,
        SnapTarget::LeftHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Left),
        SnapTarget::RightHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Right),
        SnapTarget::TopHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Top),
        SnapTarget::BottomHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Bottom),
        SnapTarget::TopLeftQuarter => ReleaseAction::Snap(crate::frame::model::SnapTarget::TopLeft),
        SnapTarget::TopRightQuarter => {
            ReleaseAction::Snap(crate::frame::model::SnapTarget::TopRight)
        }
        SnapTarget::BottomLeftQuarter => {
            ReleaseAction::Snap(crate::frame::model::SnapTarget::BottomLeft)
        }
        SnapTarget::BottomRightQuarter => {
            ReleaseAction::Snap(crate::frame::model::SnapTarget::BottomRight)
        }
    }
}

fn release_action_for_move(
    session: Option<MoveSession>,
    placement: Option<PlacementMode>,
    target: SnapTarget,
) -> ReleaseAction {
    if activated_move(session).is_none() || placement != Some(PlacementMode::Floating) {
        return ReleaseAction::None;
    }
    release_action(target)
}

fn emit_move_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_FRAME_MOVE_COMMITS,
        std::time::Duration::from_secs(1),
        || {
            let c = move_counters();
            format!(
                "received={} coalesced={} commit={} snap_target_changes={} snap_target_last={} preview_updates={} toggle_max={} client_message_maximize={} state_mutations={} maximized_transitions={} resize_snap_blocked={}",
                c.received.load(),
                c.coalesced.load(),
                c.committed.load(),
                read(&c.snap_target_changes),
                read(&c.last_target),
                read(&c.preview_updates),
                read(&c.toggle_max_total),
                read(&c.client_message_maximize_total),
                read(&c.state_mutations_total),
                read(&c.maximized_transitions),
                read(&c.resize_snap_blocked_total),
            )
        },
    );
}

fn emit_pointer_trace(
    id: flamewm_debug::DebugEventId,
    phase: &str,
    client: Window,
    source: Window,
    region: Option<FrameRegion>,
    event: FrameEvent,
    pre: InteractionSession,
    post: Option<InteractionSession>,
    commit: Option<&interaction::PointerCommit>,
    placement: Option<PlacementMode>,
    post_placement: Option<PlacementMode>,
    snap_target: Option<&dyn std::fmt::Debug>,
    preview: Option<bool>,
) {
    let pointer = match event {
        FrameEvent::Press { pointer, .. }
        | FrameEvent::Motion { pointer }
        | FrameEvent::Release { pointer, .. } => Some(pointer),
        _ => None,
    };
    let delta = pointer.and_then(|point| {
        let start = match pre {
            InteractionSession::Move(session) => session.start_pointer,
            InteractionSession::Resize(session) => session.start_pointer,
            _ => return None,
        };
        Some((
            point.x.saturating_sub(start.x),
            point.y.saturating_sub(start.y),
        ))
    });
    let cooldown = if matches!(event, FrameEvent::Motion { .. }) {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_millis(100)
    };
    flamewm_debug::emit(id, cooldown, || {
        // Formatted only when tracing is enabled; keeps the `Some("Target")` text shape.
        let commit = commit.map(|commit| {
            let effects = join_effects(&commit.effects);
            format!(
                "grab={:?} ungrab={} abort_on_grab_fail={} effects=[{effects}]",
                commit.grab, commit.ungrab, commit.abort_on_grab_fail
            )
        });
        let snap_target = snap_target.map(|target| format!("{target:?}"));
        format!(
            "phase={phase} client={client} source={source} region={region:?} event={event:?} pointer={pointer:?} delta={delta:?} pre={pre:?} post={post:?} placement={placement:?} post_placement={post_placement:?} snap_target={snap_target:?} preview={preview:?} commit={commit:?}"
        )
    });
}

fn emit_native_configure<T, E: std::fmt::Debug>(
    id: flamewm_debug::DebugEventId,
    client: Window,
    xid: Window,
    detail: impl FnOnce() -> String,
    result: &Result<T, E>,
) {
    // Built lazily: this runs on every drag motion, and the strings are only
    // needed when debug emission is enabled.
    flamewm_debug::emit(id, std::time::Duration::from_millis(100), || {
        let outcome = match result {
            Ok(_) => "ok".to_owned(),
            Err(error) => format!("error={error:?}"),
        };
        format!("client={client} xid={xid} {} result={outcome}", detail())
    });
}

#[derive(Debug, Clone, Copy)]
struct PointerIngress {
    kind: &'static str,
    event: Window,
    child: Window,
    root_x: i16,
    root_y: i16,
}

#[derive(Default)]
struct PointerIngressProbe {
    motion_dispatched: bool,
    motion_handled: bool,
    last_motion: Option<PointerIngress>,
}

#[derive(Debug)]
struct ResizeProbe {
    client: Window,
    source: Window,
    region: FrameRegion,
    start_pointer: RootPoint,
    last_pointer: RootPoint,
    effect_seen: bool,
    native_first: Option<String>,
    native_last: Option<String>,
}

fn resize_edges(session: InteractionSession) -> Option<ResizeEdges> {
    match session {
        InteractionSession::Resize(session) => Some(session.edges),
        _ => None,
    }
}

fn emit_resize_probe(
    stage: &str,
    boundary: &str,
    probe: &ResizeProbe,
    pointer: Option<RootPoint>,
    session: InteractionSession,
    commit: Option<&interaction::PointerCommit>,
) {
    let delta = pointer.map(|point| {
        (
            point.x.saturating_sub(probe.start_pointer.x),
            point.y.saturating_sub(probe.start_pointer.y),
        )
    });
    let session_edges = resize_edges(session).or_else(|| match probe.region {
        FrameRegion::Resize(edges) => Some(edges),
        _ => None,
    });
    let (plan_noop, plan_rect, effects) = commit.map_or_else(
        || (None, None, String::from("[]")),
        |commit| {
            let rect = commit.effects.iter().find_map(|effect| match effect {
                FrameEffect::ConfigureFrameRoot { x, y, w, h } => Some((*x, *y, *w, *h)),
                _ => None,
            });
            (
                Some(
                    commit
                        .effects
                        .iter()
                        .all(|effect| matches!(effect, FrameEffect::Noop)),
                ),
                rect,
                join_effects(&commit.effects),
            )
        },
    );
    let id = if stage == "effect" {
        flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE
    } else {
        flamewm_debug::WM_FRAME_GEOMETRY_PLAN
    };
    flamewm_debug::emit(id, std::time::Duration::ZERO, || {
        format!(
            "probe=resize_edge stage={stage} boundary={boundary} client={} source={} region={:?} session_edges={session_edges:?} pointer={pointer:?} delta={delta:?} plan_noop={plan_noop:?} plan_rect={plan_rect:?} effects=[{effects}] native_first={:?} native_last={:?}",
            probe.client, probe.source, probe.region, probe.native_first, probe.native_last,
        )
    });
}

fn join_effects(effects: &[FrameEffect]) -> String {
    effects
        .iter()
        .map(|effect| format!("{effect:?}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn emit_frame_dispatch(detail: impl FnOnce() -> String) {
    flamewm_debug::emit(
        flamewm_debug::WM_FRAME_EVENT_DISPATCH,
        std::time::Duration::ZERO,
        detail,
    );
}

fn pointer_ingress(event: &Event) -> Option<PointerIngress> {
    let ingress = |kind, event, child, root_x, root_y| PointerIngress {
        kind,
        event,
        child,
        root_x,
        root_y,
    };
    match event {
        Event::ButtonPress(e) => Some(ingress("ButtonPress", e.event, e.child, e.root_x, e.root_y)),
        Event::ButtonRelease(e) => Some(ingress(
            "ButtonRelease",
            e.event,
            e.child,
            e.root_x,
            e.root_y,
        )),
        Event::MotionNotify(e) => Some(ingress(
            "MotionNotify",
            e.event,
            e.child,
            e.root_x,
            e.root_y,
        )),
        _ => None,
    }
}

struct WorkspaceCounterSet {
    switch_total: flamewm_profiler::CounterPoint,
    visibility: flamewm_profiler::CounterPoint,
    ewmh_current: flamewm_profiler::CounterPoint,
    shell_publish: flamewm_profiler::CounterPoint,
    query_tree: flamewm_profiler::CounterPoint,
}

fn workspace_counters() -> &'static WorkspaceCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<WorkspaceCounterSet> = OnceLock::new();
    SET.get_or_init(|| WorkspaceCounterSet {
        switch_total: flamewm_profiler::CounterPoint::new("wm.workspace.switch.total"),
        visibility: flamewm_profiler::CounterPoint::new("wm.workspace.switch.visibility"),
        ewmh_current: flamewm_profiler::CounterPoint::new("wm.workspace.switch.ewmh_current"),
        shell_publish: flamewm_profiler::CounterPoint::new("wm.workspace.switch.shell_publish"),
        query_tree: flamewm_profiler::CounterPoint::new("wm.workspace.query_tree.count"),
    })
}

/// Wait timeout for the reactor: `None` (block until an event) when no display
/// transaction is pending, otherwise the time left until its revert deadline.
pub(crate) fn next_wait_timeout(
    now_ms: u64,
    deadline_ms: Option<u64>,
) -> Option<std::time::Duration> {
    deadline_ms.map(|deadline| std::time::Duration::from_millis(deadline.saturating_sub(now_ms)))
}

/// Drains buffered X events through the coalescing pump, dispatching each one,
/// then records the batch size. Callers own the commit boundary after it.
fn drain_pumped_events(
    wm: &mut Wm<'_, RustConnection>,
    pump: &mut WmEventPump,
    conn: &RustConnection,
) -> Result<(), AnyError> {
    let mut batch: usize = 0;
    while let Some(pumped) = pump.next(conn).map_err(any_conn_err)? {
        resize_counters().record_received(pumped.received as u64);
        resize_counters().record_coalesced(pumped.coalesced as u64);
        batch += 1;
        {
            let _guard = flamewm_profiler::start("wm.resize.motion.drain");
            wm.probe_event_dispatch(&pumped.event, pumped.received);
            wm.handle_event(pumped.event)?;
        }
    }
    resize_counters().record_committed(batch as u64);
    if batch > resize_counters().max_batch() {
        resize_counters().set_max_batch(batch);
    }
    Ok(())
}

pub(crate) fn run_with_hook<F>(
    config: WmConfig,
    catalog: &std::sync::Arc<flamewm_applications::ApplicationCatalog>,
    reactor: &Rc<RefCell<flamewm_reactor::Reactor>>,
    icon_service: Rc<RefCell<flamewm_integrations_linux::icon_service::IconService>>,
    mut hook: F,
) -> Result<(), AnyError>
where
    F: FnMut(
        &mut Wm<'_, RustConnection>,
        &Rc<RustConnection>,
        usize,
        WmChangeSet,
    ) -> Result<Option<std::time::Duration>, AnyError>,
{
    use std::os::fd::AsRawFd;
    let (conn, screen_num) = x11rb::connect(None)?;
    let conn = Rc::new(conn);
    let screen = &conn.setup().roots[screen_num];
    become_wm(conn.as_ref(), screen)?;

    let mut wm = Wm::new(
        conn.as_ref(),
        screen_num,
        config,
        std::sync::Arc::clone(catalog),
        icon_service,
    )?;
    wm.publish_desktop_state()?;
    wm.scan_existing()?;
    if let Err(error) = wm.select_raw_keys() {
        eprintln!("flamewm: raw escape-key feed unavailable: {error}");
    }
    conn.flush()?;

    // X fd source: borrowed fd wakes dispatch; buffered drain happens below.
    // The connection outlives the registration; calloop never owns the fd.
    let x_fd = conn.stream().as_raw_fd();
    let _x_token = reactor.borrow_mut().register_raw_fd_with_action(
        x_fd,
        calloop::Interest::READ,
        |_, _| flamewm_reactor::FdAction::Continue,
    )?;
    loop {
        // Buffered drain before block: one pump collapses contiguous
        // same event+child MotionNotify; order vs Button/Key/Configure/
        // Property/Enter/Leave/ClientMessage/Expose is preserved.
        let mut pump = WmEventPump::new();
        drain_pumped_events(&mut wm, &mut pump, conn.as_ref())?;
        // Batch boundary: a bare-Super tap completes only after its autorepeat
        // partner (same timestamp) had the chance to cancel it.
        wm.commit_super_tap();
        wm.tick_edge_drag()?;
        // Drain once per reactor turn: semantic owners marked facets above;
        // the hook flushes them to snapshots/signals exactly once.
        let changes = wm.take_changes();
        wm.pump_transitions(std::time::Instant::now())?;
        wm.flush_deferred_chrome(std::time::Instant::now())?;
        let wait = hook(&mut wm, &conn, screen_num, changes)?;
        drain_pumped_events(&mut wm, &mut pump, conn.as_ref())?;
        wm.commit_super_tap();
        wm.tick_edge_drag()?;
        // After the drain: an effect begun by a drained event (e.g. try_start_open) must cap the wait.
        let wait = wm.transition_wait(wait);
        let wait = wm.deferred_chrome_wait(wait);
        let wait = drag_edge_wait(
            wm.edge_drag,
            wm.mouse_workspace_cooldowns,
            std::time::Instant::now(),
            wait,
        );
        emit_resize_summary();
        emit_move_summary();
        wm.reconcile_frame_shadows()?;
        // Flush once per reactor turn.
        conn.flush()?;
        // Blocking wait: woken by X, provider, audio, or profiler sources.
        reactor.borrow_mut().dispatch(wait)?;
        // The control reply is sent by the reactor callback before it returns.
        // Exit successfully on the following turn so the session supervisor
        // can stop and reap its managed dependents.
        if flamewm_x11_desktop::take_session_logout_request() {
            return Ok(());
        }
        wm.drain_catalog_icon_results();
    }
}

/// Result of a `SuperTap` transition. `Toggle` asks the WM to flip Start once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuperTapOutcome {
    Idle,
    Toggle,
}

/// Bare-Super tap detector with KDE Meta semantics: a tap is a bare Super press
/// whose release is not followed by any other key while Super is held.
///
/// X autorepeat on a held key emits a KeyRelease and KeyPress pair that share
/// one server timestamp. A release is therefore held as `candidate` and only
/// completes when the event batch ends (`commit`) or another key arrives; a
/// same-timestamp Super press cancels it, so repeat pairs never toggle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SuperTap {
    /// Keycode of the Super key currently held.
    held: Option<u8>,
    /// Another key was pressed while Super was held, so its release is not a tap.
    chorded: bool,
    /// Keycode and timestamp of a completed bare-Super release awaiting commit.
    candidate: Option<(u8, u32)>,
}

impl SuperTap {
    /// Bare Start-bound Super press.
    fn press_super(&mut self, keycode: u8, time: u32) -> SuperTapOutcome {
        let mut outcome = SuperTapOutcome::Idle;
        if let Some((key, stamp)) = self.candidate.take() {
            if key == keycode && stamp == time {
                // Autorepeat pair: this press shares the release's timestamp.
                self.held = Some(keycode);
                return SuperTapOutcome::Idle;
            }
            // A new press after a completed tap: the previous tap completes now.
            outcome = SuperTapOutcome::Toggle;
        }
        if self.held == Some(keycode) {
            // Repeat press with no intervening release: still the same hold.
            return outcome;
        }
        self.held = Some(keycode);
        self.chorded = false;
        outcome
    }

    /// Any release. Only the release of the held Super key can complete a tap.
    fn release_super(&mut self, keycode: u8, time: u32) {
        if self.held != Some(keycode) {
            return;
        }
        self.held = None;
        if !self.chorded {
            self.candidate = Some((keycode, time));
        }
        self.chorded = false;
    }

    /// Any key press that is not a bare Start-bound Super press.
    fn press_other(&mut self) -> SuperTapOutcome {
        if self.held.is_some() {
            self.chorded = true;
            return SuperTapOutcome::Idle;
        }
        // A completed tap followed by another key still counts as a tap.
        self.commit()
    }

    /// Completes a pending tap at the end of an event batch.
    fn commit(&mut self) -> SuperTapOutcome {
        if self.candidate.take().is_some() {
            SuperTapOutcome::Toggle
        } else {
            SuperTapOutcome::Idle
        }
    }

    /// Forgets all Super state so a later release cannot toggle Start.
    fn focus_lost(&mut self) {
        *self = Self::default();
    }
}

/// True when the modifier state has no bits besides the ignored lock keys.
fn is_bare_state(state: u16) -> bool {
    use flamewm_x11_desktop::shortcuts::{MOD_LOCK, MOD_NUM_LOCK};
    state & !(MOD_LOCK | MOD_NUM_LOCK) == 0
}

fn become_wm<C: Connection>(conn: &C, screen: &Screen) -> Result<(), ReplyError> {
    let attrs = ChangeWindowAttributesAux::new().event_mask(
        EventMask::SUBSTRUCTURE_REDIRECT
            | EventMask::SUBSTRUCTURE_NOTIFY
            | EventMask::PROPERTY_CHANGE
            | EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE,
    );
    let result = conn.change_window_attributes(screen.root, &attrs)?.check();
    conn.randr_select_input(
        screen.root,
        randr::NotifyMask::SCREEN_CHANGE
            | randr::NotifyMask::OUTPUT_CHANGE
            | randr::NotifyMask::CRTC_CHANGE,
    )?
    .check()?;
    if let Err(ReplyError::X11Error(ref error)) = result {
        if error.error_kind == ErrorKind::Access {
            eprintln!("flamewm: another window manager is already running on this display");
        }
    }
    result
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
    /// Set by RandR notifications; the runtime refreshes the display snapshot on the next turn.
    display_topology_dirty: TopologyDirty,
    /// Desktop CRTC modes and the hold/restore state for fullscreen mode changes.
    desktop_mode: desktop_mode::DesktopModeTracker,
    /// XInput2 raw key feed for escape chords while a client holds the keyboard.
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
    /// Shadow strips per floating frame (`wm/shadow.rs`).
    frame_shadows: HashMap<Window, shadow::FrameShadow>,
    /// Clients whose bottom corners were clipped to a rounded frame; used to
    /// restore the unshaped client when the decoration turns square.
    rounded_clients: HashSet<Window>,
    /// Root generation and staleness stamps that decide when a strip re-shades.
    shadow_scene: shadow::ShadowScene,
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

fn icon_completion_matches(
    current_token: u64,
    expected_key: Option<&flamewm_integrations_linux::icons::IconKey>,
    result_token: u64,
    result_key: &flamewm_integrations_linux::icons::IconKey,
) -> bool {
    current_token == result_token && expected_key == Some(result_key)
}

fn retain_deferred_icon_job(
    deferred: &mut HashMap<Window, (u64, flamewm_integrations_linux::icon_service::IconJob)>,
    client: Window,
    token: u64,
    job: flamewm_integrations_linux::icon_service::IconJob,
) {
    deferred.insert(client, (token, job));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputWorkArea {
    output: flamewm_api::OutputId,
    geometry: Rect,
    work_area: Rect,
    primary: bool,
}

impl OutputWorkArea {
    fn from_host(
        (output, geometry, work_area, primary): (
            flamewm_api::OutputId,
            flamewm_api::Rect,
            flamewm_api::Rect,
            bool,
        ),
    ) -> Self {
        Self {
            output,
            geometry: from_core_rect(geometry),
            work_area: from_core_rect(work_area),
            primary,
        }
    }
}

mod counters;
mod desktop_mode;
mod raw_keys;
mod show_desktop;
mod switcher;
use counters::{bump, emit_resize_summary, move_counters, read, resize_counters};
mod client_state;
mod configure_handlers;
mod effects;
mod events;
mod manage;
mod overlay;
mod paint;
mod pointer;
mod publish;
mod shadow;
mod transition;
mod window_ops;
mod workspace;

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
    include_str!("mod.rs"),
);

impl<'a, C: Connection> Wm<'a, C> {
    fn new(
        conn: &'a C,
        screen_num: usize,
        config: WmConfig,
        catalog: std::sync::Arc<flamewm_applications::ApplicationCatalog>,
        icon_service: Rc<RefCell<flamewm_integrations_linux::icon_service::IconService>>,
    ) -> Result<Self, AnyError> {
        let screen = &conn.setup().roots[screen_num];
        let atoms = Atoms::new(conn)?;

        let support_window = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            support_window,
            screen.root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;

        let workspace_count = config.workspaces.max(1);
        let renderer = flamewm_render_x11::ExternalDecorationRenderer::open("IBM Plex Sans")
            .ok()
            .map(flamewm_render_x11::ExternalDecorationRenderer::into_cursor_shared);
        // Root gets the theme cursor at start; the renderer records it, so a live
        // cursor-theme reload re-defines it with the frames.
        if let Some(renderer) = renderer.as_ref() {
            let _ = renderer.borrow_mut().define_cursor(
                u64::from(screen.root),
                flamewm_render_core::CursorKind::Default,
            );
        }
        let animator = Animator::new(
            renderer.as_ref().is_some_and(|r| {
                r.borrow().is_available() && r.borrow().supports_picture_transform()
            }),
            Settings::DEFAULT,
        );
        Ok(Self {
            conn,
            screen_num,
            config,
            atoms,
            support_window,
            catalog,
            icon_service,
            deferred_icon_jobs: HashMap::new(),
            icon_requests: HashMap::new(),
            next_client_lifetime: 1,
            clients: HashMap::new(),
            managed_order: Vec::new(),
            frame_to_client: HashMap::new(),
            frame_extents_published: HashMap::new(),
            client_list_published: None,
            registry: FrameRegistry::new(),
            sessions: HashMap::new(),
            renderer,
            animator,
            overlays: HashMap::new(),
            linger: animation::Linger::default(),
            damage_watch: HashMap::new(),
            minimized_captures: CaptureCache::default(),
            open_gate: OpenGate::default(),
            current_workspace: 0,
            workspace_count,
            active: None,
            workspace_focus: vec![None; workspace_count],
            screen_rect: Rect::new(
                0,
                0,
                u32::from(screen.width_in_pixels),
                u32::from(screen.height_in_pixels),
            ),
            output_work_areas: Vec::new(),
            controller_geometry: ControllerGeometryCache::default(),
            display_topology_dirty: TopologyDirty::default(),
            desktop_mode: desktop_mode::DesktopModeTracker::default(),
            raw_keys: None,
            show_desktop: show_desktop::ShowDesktop::default(),
            escape_pending: None,
            switcher: switcher::Switcher::default(),
            mru: switcher::Mru::default(),
            switcher_signal: None,
            dock_struts: HashMap::new(),
            snap_preview: SnapPreviewSurface::new(),
            chrome_runtimes: HashMap::new(),
            decoration_tokens: flamewm_skin::recipes::decoration::ChromeTokens::default(),
            titlebars_applied: crate::live_settings::titlebars(),
            frame_shadows: HashMap::new(),
            rounded_clients: HashSet::new(),
            shadow_scene: shadow::ShadowScene::default(),
            changes: WmChangeSet::default(),
            super_tap: SuperTap::default(),
            pointer_ingress_probe: PointerIngressProbe::default(),
            resize_probe: None,
            edge_drag: None,
            mouse_workspace_cooldowns: MouseWorkspaceCooldowns::default(),
            chrome_deferred: None,
            chrome_paint_at: None,
        })
    }

    /// Submits a HIGH catalog-icon job; parks it on queue pressure. True when the service has stopped.
    fn submit_catalog_icon(
        &mut self,
        client_id: Window,
        token: u64,
        job: flamewm_integrations_linux::icon_service::IconJob,
    ) -> bool {
        use flamewm_integrations_linux::icon_service::{IconSubmit, IconSubmitError};
        match self.icon_service.borrow().submit(job) {
            Ok(IconSubmit::Enqueued | IconSubmit::Coalesced) => false,
            Ok(IconSubmit::DroppedLow) => unreachable!("catalog icons are HIGH priority"),
            Err(IconSubmitError::HighQueueFull(job)) => {
                retain_deferred_icon_job(&mut self.deferred_icon_jobs, client_id, token, job);
                false
            }
            Err(IconSubmitError::Stopped(_)) => true,
        }
    }

    /// Consume worker completions on the WM owner thread and retry retained
    /// HIGH jobs only after a wake, so queue pressure never causes polling.
    fn drain_catalog_icon_results(&mut self) {
        let ready = self.icon_service.borrow_mut().drain_ready();
        for result in ready {
            let requesters = self.icon_requests.remove(&result.key).unwrap_or_default();
            let Ok(raster) = result.result else { continue };
            for (client_id, token) in requesters {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    continue;
                };
                if !icon_completion_matches(
                    client.icon_request_token,
                    client.catalog_icon_key.as_ref(),
                    token,
                    &result.key,
                ) {
                    continue;
                }
                client.icon = crate::decoration::cache::catalog_icon_image(&raster);
                client.icon_fallback = client
                    .icon
                    .is_none()
                    .then(|| "catalog-raster-invalid".to_owned());
                self.mark_windows();
                let _ = self.paint_chrome(client_id);
            }
        }

        let deferred = std::mem::take(&mut self.deferred_icon_jobs);
        for (client_id, (token, job)) in deferred {
            let current = self.clients.get(&client_id).is_some_and(|client| {
                client.icon_request_token == token
                    && client.catalog_icon_key.as_ref() == Some(&job.key)
            });
            if !current {
                continue;
            }
            self.submit_catalog_icon(client_id, token, job);
        }
    }

    fn request_catalog_icon(&mut self, client_id: Window) {
        use flamewm_integrations_linux::icon_service::{IconJob, IconPriority};
        use flamewm_integrations_linux::icons::{
            IconKey, IconLookupPurpose, IconRequest, IconSize,
        };

        let Some((instance, class, token)) = self.clients.get(&client_id).map(|client| {
            (
                client.wm_instance.clone(),
                client.wm_class.clone(),
                client.icon_request_token,
            )
        }) else {
            return;
        };
        let identity = flamewm_applications::WindowApplicationIdentity::new(
            &*instance,
            &*class,
            String::new(),
        );
        let Some(icon_name) = self
            .catalog
            .find_by_window_identity(&identity)
            .map(|app| app.icon().to_owned())
        else {
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.icon_fallback = Some("catalog-icon-not-found".to_owned());
            }
            return;
        };
        if icon_name.trim().is_empty() {
            return;
        }
        let edge = u32::from(crate::chrome::icon_slot_for(self.config.titlebar_height));
        let service = self.icon_service.borrow();
        let key = IconKey::new(
            IconLookupPurpose::Application,
            IconRequest::name(icon_name),
            IconSize::new(edge, edge),
            service.theme_generation(),
            0,
        );
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.catalog_icon_key = Some(key.clone());
            client.icon_fallback = Some("catalog-icon-pending".to_owned());
        }
        let job = IconJob {
            key: key.clone(),
            priority: IconPriority::High,
        };
        if let Some(Ok(raster)) = service.cached(&key) {
            if let Some(client) = self.clients.get_mut(&client_id).filter(|client| {
                client.icon_request_token == token && client.catalog_icon_key.as_ref() == Some(&key)
            }) {
                client.icon = crate::decoration::cache::catalog_icon_image(&raster);
                client.icon_fallback = None;
            }
            return;
        }
        drop(service);
        self.icon_requests
            .entry(key)
            .or_default()
            .push((client_id, token));
        if self.submit_catalog_icon(client_id, token, job) {
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.icon_fallback = Some("catalog-icon-service-stopped".to_owned());
            }
        }
    }

    fn probe_event_dispatch(&mut self, event: &Event, received: usize) {
        let Some(pointer) = pointer_ingress(event) else {
            return;
        };
        match pointer.kind {
            "ButtonPress" => {
                self.pointer_ingress_probe = PointerIngressProbe::default();
                let session_client = self.session_client();
                emit_frame_dispatch(|| {
                    format!(
                        "probe=pointer_ingress stage=pre_handle_event boundary=first raw={} event={} child={} root=({}, {}) session_client={session_client:?} pump_received={received}",
                        pointer.kind, pointer.event, pointer.child, pointer.root_x, pointer.root_y
                    )
                });
            }
            "MotionNotify" => {
                self.pointer_ingress_probe.last_motion = Some(pointer);
                if !self.pointer_ingress_probe.motion_dispatched {
                    self.pointer_ingress_probe.motion_dispatched = true;
                    let session_client = self.session_client();
                    emit_frame_dispatch(|| {
                        format!(
                            "probe=pointer_ingress stage=pre_handle_event boundary=first_motion raw={} motion_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?} pump_received={} pump_coalesced={}",
                            pointer.kind,
                            pointer.event,
                            pointer.child,
                            pointer.root_x,
                            pointer.root_y,
                            received,
                            received.saturating_sub(1),
                        )
                    });
                }
            }
            "ButtonRelease" => {
                let last_motion = self.pointer_ingress_probe.last_motion;
                let session_client = self.session_client();
                emit_frame_dispatch(|| {
                    format!(
                        "probe=pointer_ingress stage=pre_handle_event boundary=last raw={} motion_seen_upstream={} release_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?} last_motion={last_motion:?} pump_received={received}",
                        pointer.kind,
                        last_motion.is_some(),
                        pointer.event,
                        pointer.child,
                        pointer.root_x,
                        pointer.root_y,
                    )
                });
                self.pointer_ingress_probe = PointerIngressProbe::default();
            }
            _ => {}
        }
    }

    fn probe_motion_entry(&mut self, event: &MotionNotifyEvent) {
        if self.pointer_ingress_probe.motion_handled {
            return;
        }
        self.pointer_ingress_probe.motion_handled = true;
        let session_client = self.session_client();
        emit_frame_dispatch(|| {
            format!(
                "probe=pointer_ingress stage=handle_motion_entry boundary=first_motion raw=MotionNotify motion_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?}",
                event.event, event.child, event.root_x, event.root_y
            )
        });
    }

    fn screen(&self) -> &Screen {
        &self.conn.setup().roots[self.screen_num]
    }

    fn work_area(&self) -> Rect {
        let (height, width) = (self.screen_rect.height, self.screen_rect.width);
        // (extent, start, end, limit) per edge: left, top, right, bottom.
        let edges = [
            (0, 4, 5, height),
            (1, 6, 7, height),
            (2, 8, 9, width),
            (3, 10, 11, width),
        ];
        let mut strut = self
            .dock_struts
            .values()
            .fold([0_u32; 4], |mut strut, dock| {
                for (edge, (extent, start, end, limit)) in edges.into_iter().enumerate() {
                    if dock[extent] > 0 && dock[end] >= dock[start] && dock[start] < limit {
                        strut[edge] = strut[edge].max(dock[extent]);
                    }
                }
                strut
            });
        if strut == [0; 4] {
            strut[3] = u32::from(self.config.panel_reserve);
        }
        let left = strut[0].min(self.screen_rect.width.saturating_sub(1));
        let right = strut[1].min(self.screen_rect.width.saturating_sub(left + 1));
        let top = strut[2].min(self.screen_rect.height.saturating_sub(1));
        let bottom = strut[3].min(self.screen_rect.height.saturating_sub(top + 1));
        Rect::new(
            self.screen_rect
                .x
                .saturating_add(i32::try_from(left).unwrap_or(i32::MAX)),
            self.screen_rect
                .y
                .saturating_add(i32::try_from(top).unwrap_or(i32::MAX)),
            self.screen_rect.width.saturating_sub(left + right).max(1),
            self.screen_rect.height.saturating_sub(top + bottom).max(1),
        )
    }

    /// Consumes the RandR topology-change flag; true when the display snapshot must be refreshed.
    pub(crate) fn take_display_topology_dirty(&mut self) -> bool {
        self.display_topology_dirty.take()
    }

    pub(crate) fn set_output_work_areas(
        &mut self,
        areas: Vec<(
            flamewm_api::OutputId,
            flamewm_api::Rect,
            flamewm_api::Rect,
            bool,
        )>,
    ) -> Result<(), ReplyError> {
        let next = areas
            .into_iter()
            .map(OutputWorkArea::from_host)
            .collect::<Vec<_>>();
        if self.output_work_areas != next {
            self.finish_all_transitions()?;
            self.output_work_areas = next;
            // The wallpaper behind titlebars is laid out per output, as the desktop draws it.
            let geometries = self
                .output_work_areas
                .iter()
                .map(|output| {
                    let rect = output.geometry;
                    (rect.x, rect.y, rect.width, rect.height)
                })
                .collect::<Vec<_>>();
            crate::frame::titlebar_backdrop::set_wallpaper_outputs(&geometries);
            self.reconcile_work_area()?;
        }
        Ok(())
    }

    fn output_for_rect(&self, rect: Rect) -> Option<&OutputWorkArea> {
        select_output_for_rect(&self.output_work_areas, rect)
            .map(|index| &self.output_work_areas[index])
    }

    fn work_area_for_rect(&self, rect: Rect) -> Rect {
        work_area_for_rect_in(&self.output_work_areas, self.work_area(), rect)
    }

    fn output_for_point(&self, x: i32, y: i32, fallback: Rect) -> Option<&OutputWorkArea> {
        self.output_work_areas
            .iter()
            .find(|output| {
                x >= output.geometry.x
                    && x < output.geometry.right()
                    && y >= output.geometry.y
                    && y < output.geometry.bottom()
            })
            .or_else(|| self.output_for_rect(fallback))
    }

    /// Output geometry and its work area at a root point; falls back to the primary work area.
    fn snap_zones_at(&self, root_x: i16, root_y: i16, fallback: Rect) -> (Rect, Rect) {
        self.output_for_point(i32::from(root_x), i32::from(root_y), fallback)
            .map_or_else(
                || (self.work_area(), self.work_area()),
                |o| (o.geometry, o.work_area),
            )
    }

    /// Snap target for a root point. Far edges use the physical output so bottom/right
    /// docks do not hide the corners from the pointer.
    fn snap_target_at(&self, root_x: i16, root_y: i16, fallback: Rect) -> SnapTarget {
        let (output, work) = self.snap_zones_at(root_x, root_y, fallback);
        core_snap_target_in_output(
            flamewm_api::Point::new(i32::from(root_x), i32::from(root_y)),
            to_core_rect(output),
            to_core_rect(work),
        )
    }

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

    fn extents(&self) -> FrameExtents {
        FrameExtents::new(i32::from(self.titlebar_height()), 1)
    }

    /// Effective Server titlebar band. The `titlebars` setting is the only authority;
    /// every geometry, shape and chrome read goes through it.
    fn titlebar_height(&self) -> u16 {
        if crate::live_settings::titlebars() {
            self.config.titlebar_height
        } else {
            0
        }
    }

    /// Frame extents for a decoration owner. Only `Server` gets a Flame
    /// titlebar and border; `ClientSide`/`None` frames equal the client.
    fn extents_for_decoration(&self, decoration: Decoration) -> FrameExtents {
        match decoration {
            Decoration::Server => self.extents(),
            Decoration::ClientSide | Decoration::None => FrameExtents::new(0, 0),
        }
    }

    fn extents_for(&self, client: Window) -> FrameExtents {
        let decoration = self
            .clients
            .get(&client)
            .map_or(Decoration::Server, |state| state.decoration);
        self.extents_for_decoration(decoration)
    }

    /// Reads `_MOTIF_WM_HINTS` and `_GTK_FRAME_EXTENTS` and decides who draws
    /// the frame. Conventions: Motif (Qt frameless, OnlyOffice, Electron) and
    /// GTK3/4 CSD (Firefox, GNOME apps) report client-drawn frames; Flame adds
    /// no titlebar, border or resize handles for them.
    fn read_decoration(&self, window: Window, kind: WindowKind) -> Decoration {
        let motif = self.read_cardinals(window, self.atoms.motif_wm_hints, 5);
        let gtk = self.read_cardinals(window, self.atoms.gtk_frame_extents, 4);
        decoration_policy(&DecorationHints {
            override_redirect: false,
            kind,
            motif_decorations: motif.as_deref().and_then(motif_decorations_field),
            gtk_frame_extents: gtk.as_deref().and_then(gtk_frame_extents),
        })
    }

    fn read_cardinals(&self, window: Window, atom: Atom, len: u32) -> Option<Vec<u32>> {
        self.conn
            .get_property(false, window, atom, AtomEnum::ANY, 0, len)
            .ok()?
            .reply()
            .ok()?
            .value32()
            .map(|values| values.collect())
    }

    /// PropertyNotify on Motif/GTK hints: re-evaluate and rebuild in place.
    /// The client area stays fixed; the frame grows or shrinks by the
    /// titlebar, so nothing is unmapped or remapped.
    fn refresh_decoration(&mut self, client: Window) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        let (old, kind) = (state.decoration, state.kind);
        let new = self.read_decoration(client, kind);
        if new == old {
            return Ok(());
        }
        let old_ext = self.extents_for_decoration(old);
        let new_ext = self.extents_for_decoration(new);
        let Some(state) = self.clients.get_mut(&client) else {
            return Ok(());
        };
        let outer = state.outer;
        let y = outer.y + old_ext.titlebar_h - new_ext.titlebar_h;
        let height = (outer.height as i32 - old_ext.titlebar_h + new_ext.titlebar_h).max(1);
        state.decoration = new;
        state.set_outer(Rect::new(outer.x, y, outer.width, height as u32));
        if let Some(resources) = self.registry.lookup_by_client(client) {
            for xid in resources.children() {
                if input_child_mapped(
                    new,
                    resources.region_of(xid),
                    crate::live_settings::titlebars(),
                ) {
                    self.conn.map_window(xid)?;
                } else {
                    self.conn.unmap_window(xid)?;
                }
            }
        }
        self.apply_frame_geometry(client)?;
        self.layout_input_children(client)?;
        self.set_frame_extents(client)?;
        self.paint_chrome(client)?;
        self.mark_windows();
        Ok(())
    }

    /// `_NET_WM_MOVERESIZE` from the client's own header bar or edges.
    /// Directions 0-7 are the edges/corners, 8 is move; keyboard (9-10) and
    /// cancel (11) are not honoured. Starts the same frame reducer press that
    /// a real button press would, so the existing pointer grab drives it.
    fn begin_client_moveresize(
        &mut self,
        client: Window,
        pointer: RootPoint,
        direction: u32,
    ) -> Result<(), ReplyError> {
        const MOVERESIZE_MOVE: u32 = 8;
        let want = match direction {
            MOVERESIZE_MOVE => FrameRegion::TitleDrag,
            0..=7 => FrameRegion::Resize(resize_edges_for_direction(direction)),
            _ => return Ok(()),
        };
        if self
            .sessions
            .get(&client)
            .is_some_and(|session| !session.session.is_idle())
        {
            return Ok(());
        }
        let Some(resources) = self.registry.lookup_by_client(client) else {
            return Ok(());
        };
        let Some(source) = resources.children().into_iter().find(|xid| {
            self.registry
                .lookup_by_xid(*xid)
                .is_some_and(|target| target.region == want)
        }) else {
            return Ok(());
        };
        self.focus(client)?;
        self.reduce_and_execute(client, FrameEvent::Press { source, pointer })
    }

    fn work_root(&self) -> RootRect {
        to_root_rect(self.work_area())
    }

    fn controller_for(&mut self, client: Window) -> Option<ControllerState> {
        let state = self.clients.get(&client)?;
        let res = self.registry.lookup_by_client(client)?;
        let session = self.session_of(client);
        let children = res.children();
        let regions =
            children.map(|xid| self.registry.lookup_by_xid(xid).map(|target| target.region));
        let outputs = self
            .output_work_areas
            .iter()
            .map(|output| RootRect::from_legacy(output.geometry));
        let (region_map, outputs) = {
            let geometry = self
                .controller_geometry
                .fetch(client, children, regions, outputs);
            (
                Rc::clone(&geometry.region_map),
                Rc::clone(&geometry.outputs),
            )
        };
        let screen_rect = RootRect::new(
            0,
            0,
            i32::try_from(self.screen().width_in_pixels).unwrap_or(i32::MAX),
            i32::try_from(self.screen().height_in_pixels).unwrap_or(i32::MAX),
        );
        let last_title_click = self
            .sessions
            .get(&client)
            .and_then(|session| session.last_title_click);
        let mut controller = Some(
            interaction::build_controller(
                client,
                state.frame,
                state.placement,
                to_root_rect(self.work_area_for_rect(state.outer)),
                screen_rect,
                state.hints,
                self.extents_for(client),
                session,
                region_map,
            )
            .with_outputs(outputs)
            .with_contents_while_dragging(crate::live_settings::show_content_while_dragging()),
        );
        if let Some(controller) = controller.as_mut() {
            controller.last_title_click = last_title_click;
        }
        controller
    }

    /// Output geometries in root coordinates: the bound for drags (windows may overflow the work area).
    fn output_root_rects(&self) -> Vec<RootRect> {
        self.output_work_areas
            .iter()
            .map(|output| RootRect::from_legacy(output.geometry))
            .collect()
    }

    fn store_controller(&mut self, client: Window, controller: &ControllerState) {
        let (sessions, clients) = (&mut self.sessions, &mut self.clients);
        interaction::store_controller(sessions, clients.get_mut(&client), client, controller);
        if let Some(entry) = sessions.get_mut(&client) {
            entry.last_title_click = controller.last_title_click;
        }
    }
}

/// RandR topology-change flag. Set by notifications; consumed once per runtime turn.
#[derive(Debug, Default)]
pub(crate) struct TopologyDirty(bool);

impl TopologyDirty {
    fn mark(&mut self) {
        self.0 = true;
    }

    fn take(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }

    fn is_pending(&self) -> bool {
        self.0
    }
}

/// Work area of the output holding `rect`, else the global work area.
fn work_area_for_rect_in(outputs: &[OutputWorkArea], global: Rect, rect: Rect) -> Rect {
    select_output_for_rect(outputs, rect).map_or(global, |index| outputs[index].work_area)
}

/// Target frame rect for a managed client after the output layout changes.
/// Floating windows keep a quarter visible on the outputs (may overflow the work area);
/// Snapped and Maximized keep work-area semantics.
fn rehome_outer(
    mode: PlacementMode,
    outer: Rect,
    outputs: &[OutputWorkArea],
    global: Rect,
) -> Rect {
    match mode {
        PlacementMode::Fullscreen => outer,
        PlacementMode::Maximized => work_area_for_rect_in(outputs, global, outer),
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

fn from_root_rect(rect: RootRect) -> Rect {
    Rect::new(rect.x, rect.y, rect.w.max(0) as u32, rect.h.max(0) as u32)
}

/// Work-area rect as a root rect; a zero-size area becomes 1px so the controller never sees empty bounds.
fn to_root_rect(rect: Rect) -> RootRect {
    RootRect::new(
        rect.x,
        rect.y,
        rect.width.max(1) as i32,
        rect.height.max(1) as i32,
    )
}

/// Stacking for a dock: directly above `frame`, the normal window just raised to the top.
fn dock_above_frame_aux(frame: Window) -> ConfigureWindowAux {
    ConfigureWindowAux::new()
        .sibling(frame)
        .stack_mode(StackMode::ABOVE)
}

fn select_output_for_rect(outputs: &[OutputWorkArea], rect: Rect) -> Option<usize> {
    let center_x = rect
        .x
        .saturating_add(i32::try_from(rect.width / 2).unwrap_or(i32::MAX));
    let center_y = rect
        .y
        .saturating_add(i32::try_from(rect.height / 2).unwrap_or(i32::MAX));
    if let Some(index) = outputs.iter().position(|output| {
        center_x >= output.geometry.x
            && center_x < output.geometry.right()
            && center_y >= output.geometry.y
            && center_y < output.geometry.bottom()
    }) {
        return Some(index);
    }
    outputs
        .iter()
        .enumerate()
        .map(|(index, output)| (index, rect_overlap_area(rect, output.geometry)))
        .filter(|&(_, area)| area > 0)
        .max_by_key(|&(index, area)| (area, std::cmp::Reverse(index)))
        .map(|(index, _)| index)
        .or_else(|| outputs.iter().position(|output| output.primary))
}

fn resolve_frame_event_source(registry: &FrameRegistry, event: Window, child: Window) -> Window {
    registry.lookup_by_xid(child).map_or(event, |_| child)
}

/// A press on an already-active client is replayed without focus or restacking.
fn should_focus_on_press(active: Option<Window>, client: Window) -> bool {
    active != Some(client)
}

fn pointer_grab_target(requested: Window, resources: FrameResources) -> bool {
    requested == resources.frame || resources.children().contains(&requested)
}

impl<C: Connection> Drop for Wm<'_, C> {
    fn drop(&mut self) {
        self.snap_preview.hide();
    }
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn clamp_u16(value: u32) -> u16 {
    value.clamp(1, u32::from(u16::MAX)) as u16
}

fn to_core_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(
        rect.x,
        rect.y,
        i32::try_from(rect.width).unwrap_or(i32::MAX),
        i32::try_from(rect.height).unwrap_or(i32::MAX),
    )
}

fn from_core_rect(rect: flamewm_api::Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y,
        u32::try_from(rect.width).unwrap_or(1).max(1),
        u32::try_from(rect.height).unwrap_or(1).max(1),
    )
}

fn rect_overlap_area(a: Rect, b: Rect) -> u64 {
    let width = a.right().min(b.right()).saturating_sub(a.x.max(b.x));
    let height = a.bottom().min(b.bottom()).saturating_sub(a.y.max(b.y));
    u64::try_from(width)
        .unwrap_or(0)
        .saturating_mul(u64::try_from(height).unwrap_or(0))
}

/// Workspace root for the canonical indexed `IconResolver` (packaged icon
/// assets live under `<workspace>/assets`). Pure path probe for resolver
/// construction; no icon decode happens here.
pub(crate) fn flamewm_workspace_root() -> std::path::PathBuf {
    crate::frame::titlebar_backdrop::packaged_root()
}

/// True when a `_NET_CURRENT_DESKTOP` value differs from the WM's own current
/// workspace, i.e. the change came from another client rather than our publish.
fn desktop_change_is_external(published: Option<u32>, own_current: usize) -> bool {
    published.map_or(true, |value| value as usize != own_current)
}

/// Payload of `_NET_DESKTOP_NAMES` for `count` workspaces: NUL-terminated names.
fn desktop_names_payload(count: usize) -> Vec<u8> {
    (1..=count)
        .flat_map(|index| format!("Workspace {index}\0").into_bytes())
        .collect()
}

/// True when `_NET_NUMBER_OF_DESKTOPS` or `_NET_DESKTOP_NAMES` differ from what
/// we last published; an identical echo of our own write must not advance the fence.
fn workspace_metadata_is_external(count: Option<u32>, names: &[u8], own_count: usize) -> bool {
    count.map_or(true, |value| value as usize != own_count)
        || !desktop_names_match(names, own_count)
}

/// True when `names` equals `desktop_names_payload(count)`, checked without building the
/// payload: this runs on every root desktop-metadata PropertyNotify.
fn desktop_names_match(names: &[u8], count: usize) -> bool {
    use std::io::Write;
    let mut rest = names;
    for index in 1..=count {
        let mut buf = [0u8; 32];
        let mut cursor: &mut [u8] = &mut buf;
        // "Workspace " (10) + at most 20 digits + NUL fits in 32 bytes.
        if write!(cursor, "Workspace {index}\0").is_err() {
            return false;
        }
        let written = 32 - cursor.len();
        match rest.strip_prefix(&buf[..written]) {
            Some(next) => rest = next,
            None => return false,
        }
    }
    rest.is_empty()
}

/// Mapping a frame is necessary only when workspace membership changes its
/// actual visibility. In particular, a minimized frame remains unmapped when
/// moved, and a hidden frame must not accumulate unmatched ignore_unmap counts.
fn frame_visibility_delta(before: bool, after: bool) -> Option<bool> {
    (before != after).then_some(after)
}

/// Windows need re-publishing only when a client's visibility flipped across a
/// workspace switch or the focused window changed; a plain workspace change
/// emits workspace state only.
fn workspace_switch_touches_windows(visibility_changed: bool, focus_changed: bool) -> bool {
    visibility_changed || focus_changed
}

#[cfg(test)]
mod tests;

/// Controller geometry derived from live registry and output state.
struct ControllerGeometry {
    client: Window,
    children: [Window; 12],
    regions: [Option<FrameRegion>; 12],
    region_map: Rc<HashMap<u32, FrameRegion>>,
    outputs: Rc<Vec<RootRect>>,
}

/// Reuses the last `ControllerGeometry` while its inputs are unchanged. The key is the
/// live inputs themselves (no revision counter to forget to bump), so a hit is exact.
#[derive(Default)]
struct ControllerGeometryCache {
    entry: Option<ControllerGeometry>,
    #[cfg(test)]
    rebuilds: usize,
}

impl ControllerGeometryCache {
    fn fetch(
        &mut self,
        client: Window,
        children: [Window; 12],
        regions: [Option<FrameRegion>; 12],
        outputs: impl Iterator<Item = RootRect> + Clone,
    ) -> &ControllerGeometry {
        let fresh = self.entry.as_ref().is_some_and(|entry| {
            entry.client == client
                && entry.children == children
                && entry.regions == regions
                && entry.outputs.iter().copied().eq(outputs.clone())
        });
        if !fresh {
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
            let region_map = children
                .iter()
                .zip(regions.iter())
                .filter_map(|(xid, region)| region.map(|region| (*xid, region)))
                .collect();
            self.entry = Some(ControllerGeometry {
                client,
                children,
                regions,
                region_map: Rc::new(region_map),
                outputs: Rc::new(outputs.collect()),
            });
        }
        self.entry.as_ref().expect("entry populated above")
    }
}

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
