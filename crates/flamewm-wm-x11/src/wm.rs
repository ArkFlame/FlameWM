use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::animation::{self, Animator, Begin, CaptureCache, Effect, OpenGate, Settings, Tick};
use flamewm_render_x11::{CapturedImage, PanelEdge, TransitionDirection, TransitionStyle, WarpOverlay, WarpRect};

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

use flamewm_api::shortcuts::{
    ACTION_SESSION_LOCK, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE,
    ACTION_WINDOW_MINIMIZE, ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP,
};
use flamewm_window_core::{
    SnapTarget, snap_geometry as core_snap_geometry, snap_target_in_output as core_snap_target_in_output,
};
use x11rb::connection::Connection;
use x11rb::errors::{ReplyError, ReplyOrIdError};
use x11rb::protocol::damage::{self};
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
use crate::event_pump::WmEventPump;
use crate::decoration::policy::{
    decoration_policy, gtk_frame_extents, motif_decorations_field, Decoration, DecorationHints,
};
use crate::frame::controller::{ControllerState, FrameEffect, FrameEvent};
use crate::frame::coords::{RootPoint, RootRect};
use crate::frame::geometry::{
    FrameExtents, GeometryReason, GeometryRequest, frame_to_client_root, plan_client_configure,
};
use crate::frame::model::{
    frame_border_for_mode,
    FrameControl, FrameRegion, PlacementMode, PlacementSnapshot, PlacementState, ResizeEdges,
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

/// Neighbour workspace index for a relative step without wrap-around.
/// Returns `None` when the step would leave `[0, count)`.
/// Focus revert target when the focused client is destroyed or unmapped. `NONE` would make the X
/// server discard key events and skip passive root grabs, silently killing every WM shortcut.
const FOCUS_REVERT_TO: InputFocus = InputFocus::POINTER_ROOT;

/// Gated key diagnostics: `FLAMEWM_KEY_DIAG=1` enables stderr key logging.
/// Read once per process; normal runs print nothing.
pub(crate) fn key_diag_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env::var("FLAMEWM_KEY_DIAG").is_ok_and(|value| value == "1"))
}

fn next_workspace(current: usize, count: usize, delta: isize) -> Option<usize> {
    let target = current.checked_add_signed(delta)?;
    (target < count).then_some(target)
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
}

impl WmChangeSet {
    fn take(&mut self) -> Self {
        let out = *self;
        *self = Self::default();
        out
    }

    #[cfg(test)]
    fn any(self) -> bool {
        self.windows || self.workspaces || self.panels || self.work_area || self.start_menu_toggle
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
            18,
        ));
        config
    }
}

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

/// Profiler point plus the running total read by the summary lines.
struct TotalledCounter {
    point: flamewm_profiler::CounterPoint,
    total: std::sync::atomic::AtomicU64,
}

impl TotalledCounter {
    fn new(label: &'static str) -> Self {
        Self {
            point: flamewm_profiler::CounterPoint::new(label),
            total: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn add(&self, value: u64) {
        self.point.increment_by(value);
        self.total
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);
    }

    fn load(&self) -> u64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }
}

struct ResizeCounterSet {
    received: TotalledCounter,
    coalesced: TotalledCounter,
    committed: TotalledCounter,
    max_batch: std::sync::atomic::AtomicU64,
}

fn resize_counters() -> &'static ResizeCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<ResizeCounterSet> = OnceLock::new();
    SET.get_or_init(|| ResizeCounterSet {
        received: TotalledCounter::new("wm.resize.received"),
        coalesced: TotalledCounter::new("wm.resize.coalesced"),
        committed: TotalledCounter::new("wm.resize.commit"),
        max_batch: std::sync::atomic::AtomicU64::new(0),
    })
}

impl ResizeCounterSet {
    fn record_received(&self, value: u64) {
        self.received.add(value);
    }

    fn record_coalesced(&self, value: u64) {
        self.coalesced.add(value);
    }

    fn record_committed(&self, value: u64) {
        self.committed.add(value);
    }

    fn snapshot(&self) -> (u64, u64, u64) {
        (self.received.load(), self.coalesced.load(), self.committed.load())
    }

    fn max_batch(&self) -> usize {
        self.max_batch.load(std::sync::atomic::Ordering::Relaxed) as usize
    }
    fn set_max_batch(&self, value: usize) {
        self.max_batch
            .store(value as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

fn emit_resize_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_RESIZE_SUMMARY,
        flamewm_debug::WM_RESIZE_SUMMARY_COOLDOWN,
        || {
            let counters = resize_counters();
            let (received, coalesced, committed) = counters.snapshot();
            let max = counters.max_batch();
            format!("received={received} coalesced={coalesced} commit={committed} max_batch={max}")
        },
    );
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
    feedback_reconfigure: flamewm_profiler::CounterPoint,
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
    feedback_total: std::sync::atomic::AtomicU64,
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
        feedback_reconfigure: flamewm_profiler::CounterPoint::new(
            "wm.configure.feedback_reconfigure",
        ),
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
        feedback_total: std::sync::atomic::AtomicU64::new(0),
        synthetic_total: std::sync::atomic::AtomicU64::new(0),
    })
}

macro_rules! configure_bump {
    ($point:ident, $total:ident) => {{
        let counters = configure_counters();
        counters.$point.increment();
        counters
            .$total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }};
}

#[allow(clippy::too_many_arguments)]
fn emit_configure_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_CONFIGURE_SUMMARY,
        std::time::Duration::from_secs(1),
        || {
            let counters = configure_counters();
            let load =
                |v: &std::sync::atomic::AtomicU64| v.load(std::sync::atomic::Ordering::Relaxed);
            format!(
                "requests={} accepted={} refused_interactive={} refused_mode={} notify_frame={} notify_client={} expected={} mismatch={} feedback={} synthetic={}",
                load(&counters.request_received_total),
                load(&counters.request_accepted_total),
                load(&counters.request_refused_interactive_total),
                load(&counters.request_refused_mode_total),
                load(&counters.notify_frame_total),
                load(&counters.notify_client_total),
                load(&counters.notify_expected_total),
                load(&counters.notify_mismatch_total),
                load(&counters.feedback_total),
                load(&counters.synthetic_total),
            )
        },
    );
}

struct MoveCounterSet {
    received: TotalledCounter,
    coalesced: TotalledCounter,
    committed: TotalledCounter,
    snap_target: flamewm_profiler::CounterPoint,
    preview: flamewm_profiler::CounterPoint,
    snap_target_changes: std::sync::atomic::AtomicU64,
    preview_updates: std::sync::atomic::AtomicU64,
    last_target: std::sync::atomic::AtomicU64,
    toggle_max_total: std::sync::atomic::AtomicU64,
    client_message_maximize_total: std::sync::atomic::AtomicU64,
    state_mutations_total: std::sync::atomic::AtomicU64,
    maximized_transitions: std::sync::atomic::AtomicU64,
    resize_snap_blocked_total: std::sync::atomic::AtomicU64,
}

fn move_counters() -> &'static MoveCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<MoveCounterSet> = OnceLock::new();
    SET.get_or_init(|| MoveCounterSet {
        received: TotalledCounter::new("wm.move.received"),
        coalesced: TotalledCounter::new("wm.move.coalesced"),
        committed: TotalledCounter::new("wm.move.commit"),
        snap_target: flamewm_profiler::CounterPoint::new("wm.move.snap_target.count"),
        preview: flamewm_profiler::CounterPoint::new("wm.move.preview.count"),
        snap_target_changes: std::sync::atomic::AtomicU64::new(0),
        preview_updates: std::sync::atomic::AtomicU64::new(0),
        last_target: std::sync::atomic::AtomicU64::new(u64::MAX),
        toggle_max_total: std::sync::atomic::AtomicU64::new(0),
        client_message_maximize_total: std::sync::atomic::AtomicU64::new(0),
        state_mutations_total: std::sync::atomic::AtomicU64::new(0),
        maximized_transitions: std::sync::atomic::AtomicU64::new(0),
        resize_snap_blocked_total: std::sync::atomic::AtomicU64::new(0),
    })
}

impl MoveCounterSet {
    fn record_received(&self, value: u64) {
        self.received.add(value);
    }

    fn record_coalesced(&self, value: u64) {
        self.coalesced.add(value);
    }

    fn record_committed(&self, value: u64) {
        self.committed.add(value);
    }

    fn record_snap_target(&self, target_bits: u64) {
        self.snap_target.increment();
        if self
            .last_target
            .swap(target_bits, std::sync::atomic::Ordering::Relaxed)
            != target_bits
        {
            self.snap_target_changes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn record_preview(&self) {
        self.preview.increment();
        self.preview_updates
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_toggle_max(&self) {
        self.toggle_max_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_client_message_maximize(&self) {
        self.client_message_maximize_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_placement_transition(&self, before: PlacementMode, after: PlacementMode) {
        if before == after {
            return;
        }
        self.state_mutations_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if matches!(
            (before, after),
            (PlacementMode::Maximized, _) | (_, PlacementMode::Maximized)
        ) {
            self.maximized_transitions
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn record_resize_snap_blocked(&self) {
        self.resize_snap_blocked_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) {
        (
            self.received.load(),
            self.coalesced.load(),
            self.committed.load(),
            self.snap_target_changes
                .load(std::sync::atomic::Ordering::Relaxed),
            self.preview_updates
                .load(std::sync::atomic::Ordering::Relaxed),
            self.last_target.load(std::sync::atomic::Ordering::Relaxed),
            self.toggle_max_total
                .load(std::sync::atomic::Ordering::Relaxed),
            self.client_message_maximize_total
                .load(std::sync::atomic::Ordering::Relaxed),
            self.state_mutations_total
                .load(std::sync::atomic::Ordering::Relaxed),
            self.maximized_transitions
                .load(std::sync::atomic::Ordering::Relaxed),
            self.resize_snap_blocked_total
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
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
            let counters = move_counters();
            let (
                received,
                coalesced,
                committed,
                target_changes,
                preview_updates,
                snap_target_last,
                toggle_max,
                client_message_maximize,
                state_mutations,
                maximized_transitions,
                resize_snap_blocked,
            ) = counters.snapshot();
            format!(
                "received={received} coalesced={coalesced} commit={committed} snap_target_changes={target_changes} snap_target_last={snap_target_last} preview_updates={preview_updates} toggle_max={toggle_max} client_message_maximize={client_message_maximize} state_mutations={state_mutations} maximized_transitions={maximized_transitions} resize_snap_blocked={resize_snap_blocked}"
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
        | FrameEvent::Release { pointer } => Some(pointer),
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
    let commit = commit.map(|commit| {
        let effects = commit
            .effects
            .iter()
            .map(|effect| format!("{effect:?}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "grab={:?} ungrab={} abort_on_grab_fail={} effects=[{effects}]",
            commit.grab, commit.ungrab, commit.abort_on_grab_fail
        )
    });
    let cooldown = if matches!(event, FrameEvent::Motion { .. }) {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_millis(100)
    };
    flamewm_debug::emit(id, cooldown, || {
        // Formatted only when tracing is enabled; keeps the `Some("Target")` text shape.
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
    detail: String,
    result: &Result<T, E>,
) {
    let outcome = match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("error={error:?}"),
    };
    flamewm_debug::emit(id, std::time::Duration::from_millis(100), || {
        format!("client={client} xid={xid} {detail} result={outcome}")
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
                commit
                    .effects
                    .iter()
                    .map(|effect| format!("{effect:?}"))
                    .collect::<Vec<_>>()
                    .join(","),
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

fn pointer_ingress(event: &Event) -> Option<PointerIngress> {
    match event {
        Event::ButtonPress(event) => Some(PointerIngress {
            kind: "ButtonPress",
            event: event.event,
            child: event.child,
            root_x: event.root_x,
            root_y: event.root_y,
        }),
        Event::ButtonRelease(event) => Some(PointerIngress {
            kind: "ButtonRelease",
            event: event.event,
            child: event.child,
            root_x: event.root_x,
            root_y: event.root_y,
        }),
        Event::MotionNotify(event) => Some(PointerIngress {
            kind: "MotionNotify",
            event: event.event,
            child: event.child,
            root_x: event.root_x,
            root_y: event.root_y,
        }),
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
pub(crate) fn next_wait_timeout(now_ms: u64, deadline_ms: Option<u64>) -> Option<std::time::Duration> {
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
    F: FnMut(&mut Wm<'_, RustConnection>, &Rc<RustConnection>, usize, WmChangeSet) -> Result<Option<std::time::Duration>, AnyError>,
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
        // Drain once per reactor turn: semantic owners marked facets above;
        // the hook flushes them to snapshots/signals exactly once.
        let changes = wm.take_changes();
        wm.pump_transitions(std::time::Instant::now())?;
        let wait = hook(&mut wm, &conn, screen_num, changes)?;
        drain_pumped_events(&mut wm, &mut pump, conn.as_ref())?;
        wm.commit_super_tap();
        // After the drain: an effect begun by a drained event (e.g. try_start_open) must cap the wait.
        let wait = wm.transition_wait(wait);
        emit_resize_summary();
        emit_move_summary();
        // Flush once per reactor turn.
        conn.flush()?;
        // Blocking wait: woken by X, provider, audio, or profiler sources.
        reactor.borrow_mut().dispatch(wait)?;
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
    dock_struts: HashMap<Window, [u32; 12]>,
    snap_preview: SnapPreviewSurface,
    chrome_runtimes: HashMap<Window, chrome_runtime::ChromeRuntime>,
    /// Live skin chrome tokens (settings-derived); changes repaint all frames.
    decoration_tokens: flamewm_skin::recipes::decoration::ChromeTokens,
    changes: WmChangeSet,
    /// Bare-Super tap state; see `SuperTap`. Start toggles only through it.
    super_tap: SuperTap,
    pointer_ingress_probe: PointerIngressProbe,
    resize_probe: Option<ResizeProbe>,
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
        Self { output, geometry: from_core_rect(geometry), work_area: from_core_rect(work_area), primary }
    }
}

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
        let animator = Animator::new(
            renderer
                .as_ref()
                .is_some_and(|r| r.borrow().is_available() && r.borrow().supports_picture_transform()),
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
            dock_struts: HashMap::new(),
            snap_preview: SnapPreviewSurface::new(),
            chrome_runtimes: HashMap::new(),
            decoration_tokens: flamewm_skin::recipes::decoration::ChromeTokens::default(),
            changes: WmChangeSet::default(),
            super_tap: SuperTap::default(),
            pointer_ingress_probe: PointerIngressProbe::default(),
            resize_probe: None,
        })
    }

    /// Consume worker completions on the WM owner thread and retry retained
    /// HIGH jobs only after a wake, so queue pressure never causes polling.
    fn drain_catalog_icon_results(&mut self) {
        use flamewm_integrations_linux::icon_service::IconSubmit;

        let ready = self.icon_service.borrow_mut().drain_ready();
        for result in ready {
            let requesters = self.icon_requests.remove(&result.key).unwrap_or_default();
            if let Ok(raster) = result.result {
                for (client_id, token) in requesters {
                    let Some(client) = self.clients.get_mut(&client_id) else { continue };
                    if !icon_completion_matches(
                        client.icon_request_token,
                        client.catalog_icon_key.as_ref(),
                        token,
                        &result.key,
                    )
                    { continue; }
                    client.icon = crate::decoration::cache::catalog_icon_image(&raster);
                    client.icon_fallback = client.icon.is_none().then(|| "catalog-raster-invalid".to_owned());
                    self.mark_windows();
                    let _ = self.paint_chrome(client_id);
                }
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
            match self.icon_service.borrow().submit(job.clone()) {
                Ok(IconSubmit::Enqueued | IconSubmit::Coalesced) => {}
                Ok(IconSubmit::DroppedLow) => unreachable!("catalog icons are HIGH priority"),
                Err(flamewm_integrations_linux::icon_service::IconSubmitError::HighQueueFull(job)) => {
                    retain_deferred_icon_job(&mut self.deferred_icon_jobs, client_id, token, job);
                }
                Err(flamewm_integrations_linux::icon_service::IconSubmitError::Stopped(_)) => {}
            }
        }
    }

    fn request_catalog_icon(&mut self, client_id: Window) {
        use flamewm_integrations_linux::icon_service::{IconJob, IconPriority, IconSubmit, IconSubmitError};
        use flamewm_integrations_linux::icons::{IconKey, IconLookupPurpose, IconRequest, IconSize};

        let Some((instance, class, token)) = self.clients.get(&client_id).map(|client| {
            (client.wm_instance.clone(), client.wm_class.clone(), client.icon_request_token)
        }) else { return };
        let identity = flamewm_applications::WindowApplicationIdentity::new(&instance, &class, String::new());
        let Some(icon_name) = self.catalog.find_by_window_identity(&identity).map(|app| app.icon().to_owned()) else {
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.icon_fallback = Some("catalog-icon-not-found".to_owned());
            }
            return;
        };
        if icon_name.trim().is_empty() { return; }
        let edge = u32::from(crate::chrome::icon_slot_for(crate::chrome::TITLEBAR_HEIGHT));
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
        let job = IconJob { key: key.clone(), priority: IconPriority::High };
        if let Some(Ok(raster)) = service.cached(&key) {
            if let Some(client) = self.clients.get_mut(&client_id) {
                if client.icon_request_token == token && client.catalog_icon_key.as_ref() == Some(&key) {
                    client.icon = crate::decoration::cache::catalog_icon_image(&raster);
                    client.icon_fallback = None;
                }
            }
            return;
        }
        drop(service);
        self.icon_requests.entry(key).or_default().push((client_id, token));
        match self.icon_service.borrow().submit(job.clone()) {
            Ok(IconSubmit::Enqueued | IconSubmit::Coalesced) => {}
            Ok(IconSubmit::DroppedLow) => unreachable!("catalog icons are HIGH priority"),
            Err(IconSubmitError::HighQueueFull(job)) => {
                retain_deferred_icon_job(&mut self.deferred_icon_jobs, client_id, token, job);
            }
            Err(IconSubmitError::Stopped(_)) => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.icon_fallback = Some("catalog-icon-service-stopped".to_owned());
                }
            }
        }
    }

    fn probe_event_dispatch(&mut self, event: &Event, received: usize) {
        let Some(pointer) = pointer_ingress(event) else {
            return;
        };
        let session_client = self.session_client();
        match pointer.kind {
            "ButtonPress" => {
                self.pointer_ingress_probe = PointerIngressProbe::default();
                flamewm_debug::emit(
                    flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                    std::time::Duration::ZERO,
                    || {
                        format!(
                            "probe=pointer_ingress stage=pre_handle_event boundary=first raw={} event={} child={} root=({}, {}) session_client={session_client:?} pump_received={received}",
                            pointer.kind,
                            pointer.event,
                            pointer.child,
                            pointer.root_x,
                            pointer.root_y
                        )
                    },
                );
            }
            "MotionNotify" => {
                self.pointer_ingress_probe.last_motion = Some(pointer);
                if !self.pointer_ingress_probe.motion_dispatched {
                    self.pointer_ingress_probe.motion_dispatched = true;
                    flamewm_debug::emit(
                        flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                        std::time::Duration::ZERO,
                        || {
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
                        },
                    );
                }
            }
            "ButtonRelease" => {
                let last_motion = self.pointer_ingress_probe.last_motion;
                flamewm_debug::emit(
                    flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                    std::time::Duration::ZERO,
                    || {
                        format!(
                            "probe=pointer_ingress stage=pre_handle_event boundary=last raw={} motion_seen_upstream={} release_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?} last_motion={last_motion:?} pump_received={received}",
                            pointer.kind,
                            last_motion.is_some(),
                            pointer.event,
                            pointer.child,
                            pointer.root_x,
                            pointer.root_y,
                        )
                    },
                );
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
        flamewm_debug::emit(
            flamewm_debug::WM_FRAME_EVENT_DISPATCH,
            std::time::Duration::ZERO,
            || {
                format!(
                    "probe=pointer_ingress stage=handle_motion_entry boundary=first_motion raw=MotionNotify motion_seen_upstream=true event={} child={} root=({}, {}) session_client={session_client:?}",
                    event.event, event.child, event.root_x, event.root_y
                )
            },
        );
    }

    fn screen(&self) -> &Screen {
        &self.conn.setup().roots[self.screen_num]
    }

    fn work_area(&self) -> Rect {
        let mut strut = [0_u32; 4];
        for dock in self.dock_struts.values() {
            if dock[0] > 0 && dock[5] >= dock[4] && dock[4] < self.screen_rect.height {
                strut[0] = strut[0].max(dock[0]);
            }
            if dock[1] > 0 && dock[7] >= dock[6] && dock[6] < self.screen_rect.height {
                strut[1] = strut[1].max(dock[1]);
            }
            if dock[2] > 0 && dock[9] >= dock[8] && dock[8] < self.screen_rect.width {
                strut[2] = strut[2].max(dock[2]);
            }
            if dock[3] > 0 && dock[11] >= dock[10] && dock[10] < self.screen_rect.width {
                strut[3] = strut[3].max(dock[3]);
            }
        }
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

    pub(crate) fn set_output_work_areas(
        &mut self,
        areas: Vec<(flamewm_api::OutputId, flamewm_api::Rect, flamewm_api::Rect, bool)>,
    ) -> Result<(), ReplyError> {
        let next = areas.into_iter().map(OutputWorkArea::from_host).collect::<Vec<_>>();
        if self.output_work_areas != next {
            self.finish_all_transitions()?;
            self.output_work_areas = next;
            self.reconcile_work_area()?;
        }
        Ok(())
    }

    fn output_for_rect(&self, rect: Rect) -> Option<&OutputWorkArea> {
        select_output_for_rect(&self.output_work_areas, rect).map(|index| &self.output_work_areas[index])
    }

    fn work_area_for_rect(&self, rect: Rect) -> Rect {
        self.output_for_rect(rect).map_or_else(|| self.work_area(), |output| output.work_area)
    }

    fn output_for_point(&self, x: i32, y: i32, fallback: Rect) -> Option<&OutputWorkArea> {
        self.output_work_areas.iter().find(|output| {
            x >= output.geometry.x && x < output.geometry.right()
                && y >= output.geometry.y && y < output.geometry.bottom()
        }).or_else(|| self.output_for_rect(fallback))
    }

    /// Output geometry and its work area at a root point; falls back to the primary work area.
    fn snap_zones_at(&self, root_x: i16, root_y: i16, fallback: Rect) -> (Rect, Rect) {
        self.output_for_point(i32::from(root_x), i32::from(root_y), fallback)
            .map_or((self.work_area(), self.work_area()), |o| (o.geometry, o.work_area))
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
        FrameExtents::new(i32::from(self.config.titlebar_height), 1)
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
                if new == Decoration::Server {
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
        let work = self.work_area();
        RootRect::new(
            work.x,
            work.y,
            work.width.max(1) as i32,
            work.height.max(1) as i32,
        )
    }

    fn controller_for(&self, client: Window) -> Option<ControllerState> {
        let state = self.clients.get(&client)?;
        let res = self.registry.lookup_by_client(client)?;
        let session = self
            .sessions
            .get(&client)
            .map_or(InteractionSession::Idle, |session| session.session);
        let mut by_xid = std::collections::HashMap::new();
        for xid in res.children() {
            if let Some(target) = self.registry.lookup_by_xid(xid) {
                by_xid.insert(xid, target.region);
            }
        }
        let region_map = interaction::region_map_for(&by_xid, &res.children());
        let screen_rect = RootRect::new(
            0,
            0,
            i32::try_from(self.screen().width_in_pixels).unwrap_or(i32::MAX),
            i32::try_from(self.screen().height_in_pixels).unwrap_or(i32::MAX),
        );
        Some(interaction::build_controller(
            client,
            state.frame,
            state.placement,
            {
                let work = self.work_area_for_rect(state.outer);
                RootRect::new(work.x, work.y, work.width.max(1) as i32, work.height.max(1) as i32)
            },
            screen_rect,
            state.hints,
            self.extents_for(client),
            session,
            region_map,
        ))
    }

    fn store_controller(&mut self, client: Window, controller: &ControllerState) {
        let (sessions, clients) = (&mut self.sessions, &mut self.clients);
        interaction::store_controller(sessions, clients.get_mut(&client), client, controller);
    }

    fn execute_effects(
        &mut self,
        client: Window,
        commit: &interaction::PointerCommit,
    ) -> Result<(), ReplyError> {
        let effect_boundary = self.resize_probe.as_ref().and_then(|probe| {
            if !probe.effect_seen {
                Some("first")
            } else if commit.ungrab {
                Some("last")
            } else {
                None
            }
        });
        if effect_boundary == Some("first") {
            let session = self
                .sessions
                .get(&client)
                .map_or(InteractionSession::Idle, |state| state.session);
            if let Some(probe) = self.resize_probe.as_ref() {
                emit_resize_probe(
                    "effect",
                    "first",
                    probe,
                    Some(probe.last_pointer),
                    session,
                    Some(commit),
                );
            }
            if let Some(probe) = self.resize_probe.as_mut() {
                probe.effect_seen = true;
            }
        }
        if let Some((window, cursor)) = commit.grab {
            let ok = self.interaction_capture(window, cursor).unwrap_or(false);
            flamewm_debug::emit(
                flamewm_debug::WM_FRAME_GRAB,
                std::time::Duration::from_millis(100),
                || format!("client={client} window={window} result={ok}"),
            );
            if !ok {
                if commit.abort_on_grab_fail {
                    self.abort_session_to_idle(client);
                }
                self.resize_probe = None;
                return Ok(());
            }
        }
        let result: Result<(), ReplyError> = (|| {
            for effect in &commit.effects {
                match *effect {
                    // PointerCommit owns the single native grab transaction.
                    FrameEffect::Grab { .. } => {}
                    FrameEffect::Ungrab => {
                        if commit.ungrab {
                            self.release_pointer();
                        }
                    }
                    FrameEffect::ConfigureFrameRoot { x, y, w, h } => {
                        let Some(frame) = self.clients.get(&client).map(|state| state.frame) else {
                            continue;
                        };
                        let border = u32::from(self.effective_frame_border(client));
                        let result = self.conn.configure_window(
                            frame,
                            &ConfigureWindowAux::new()
                                .x(x)
                                .y(y)
                                .border_width(border)
                                .width(w.max(1) as u32)
                                .height(h.max(1) as u32),
                        );
                        if let Some(probe) = self.resize_probe.as_mut() {
                            if probe.client == client {
                                let outcome = match &result {
                                    Ok(_) => String::from("ok"),
                                    Err(error) => format!("error={error:?}"),
                                };
                                let detail =
                                    format!("xid={frame} rect=({x},{y},{w},{h}) result={outcome}");
                                if probe.native_first.is_none() {
                                    probe.native_first = Some(detail.clone());
                                }
                                probe.native_last = Some(detail);
                            }
                        }
                        emit_native_configure(
                            flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE,
                            client,
                            frame,
                            format!("effect=ConfigureFrameRoot rect=({x},{y},{w},{h})"),
                            &result,
                        );
                        result?;
                        self.commit_frame_rect(client, x, y, w, h);
                        self.set_frame_extents(client)?;
                    }
                    // Move is position-only: frame .x/.y + placement x/y commit;
                    // w/h and floating_restore size are preserved exactly.
                    FrameEffect::MoveFrameRoot { x, y } => {
                        let _guard = flamewm_profiler::start("wm.move.frame_position");
                        let Some(frame) = self.clients.get(&client).map(|state| state.frame) else {
                            continue;
                        };
                        let result = self
                            .conn
                            .configure_window(frame, &ConfigureWindowAux::new().x(x).y(y));
                        emit_native_configure(
                            flamewm_debug::WM_FRAME_GEOMETRY_MOVE_NATIVE,
                            client,
                            frame,
                            format!("effect=MoveFrameRoot position=({x},{y})"),
                            &result,
                        );
                        result?;
                        self.commit_frame_position(client, x, y);
                        move_counters().record_committed(1);
                    }
                    FrameEffect::ConfigureClientLocal { x, y, w, h } => {
                        let result = self.conn.configure_window(
                            client,
                            &ConfigureWindowAux::new()
                                .x(x)
                                .y(y)
                                .width(w.max(1))
                                .height(h.max(1)),
                        );
                        emit_native_configure(
                            flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE,
                            client,
                            client,
                            format!("effect=ConfigureClientLocal rect=({x},{y},{w},{h})"),
                            &result,
                        );
                        result?;
                    }
                    FrameEffect::LayoutInput => {
                        self.layout_input_children(client)?;
                    }
                    FrameEffect::PaintChrome => {
                        self.paint_chrome(client)?;
                    }
                    FrameEffect::ApplyShape => {
                        let (frame, floating, outer) = match self.clients.get(&client) {
                            Some(state) => (state.frame, state.is_floating(), state.outer),
                            None => continue,
                        };
                        self.clear_frame_shape(frame, outer, floating)?;
                    }
                    FrameEffect::NotifyClientRoot { x, y, w, h } => {
                        let _guard = flamewm_profiler::start("wm.move.notify");
                        let event = ConfigureNotifyEvent {
                            response_type: CONFIGURE_NOTIFY_EVENT,
                            sequence: 0,
                            event: client,
                            window: client,
                            above_sibling: 0,
                            x: clamp_i16(x),
                            y: clamp_i16(y),
                            width: clamp_u16(w.max(1) as u32),
                            height: clamp_u16(h.max(1)),
                            border_width: 0,
                            override_redirect: false,
                        };
                        self.conn
                            .send_event(false, client, EventMask::STRUCTURE_NOTIFY, event)?;
                    }
                    FrameEffect::SnapPreview { target } => {
                        if target.is_none() {
                            self.snap_preview.hide();
                        }
                    }
                    FrameEffect::Noop => {}
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.abort_session_to_idle(client);
        }
        if effect_boundary == Some("last") {
            let session = self
                .sessions
                .get(&client)
                .map_or(InteractionSession::Idle, |state| state.session);
            if let Some(probe) = self.resize_probe.as_ref() {
                emit_resize_probe(
                    "effect",
                    "last",
                    probe,
                    Some(probe.last_pointer),
                    session,
                    Some(commit),
                );
            }
        }
        if commit.ungrab {
            self.resize_probe = None;
        }
        result
    }

    fn commit_frame_position(&mut self, client: Window, x: i32, y: i32) {
        let (state, session) = match (
            self.clients.get_mut(&client),
            self.sessions.get_mut(&client),
        ) {
            (Some(state), Some(session)) => (state, session),
            _ => return,
        };
        let current = state.placement.current;
        let next = RootRect::new(x, y, current.w.max(1), current.h.max(1));
        state.placement.current = next;
        if state.placement.mode == PlacementMode::Floating {
            let restore = state.placement.floating_restore;
            state.placement.floating_restore =
                RootRect::new(x, y, restore.w.max(1), restore.h.max(1));
        }
        state.outer = crate::client::root_to_rect(next);
        session.placement.current = next;
    }

    fn commit_frame_rect(&mut self, client: Window, x: i32, y: i32, w: i32, h: i32) {
        let (state, session) = match (
            self.clients.get_mut(&client),
            self.sessions.get_mut(&client),
        ) {
            (Some(state), Some(session)) => (state, session),
            _ => return,
        };
        let frame = RootRect::new(x, y, w.max(1), h.max(1)).to_legacy();
        let mode = state.placement.mode;
        state.placement.current = crate::client::rect_to_root(frame);
        if mode == PlacementMode::Floating {
            state.placement.floating_restore = state.placement.current;
        }
        state.outer = frame;
        session.placement.current = state.placement.current;
    }

    fn release_capture_unmap(&self) {
        if let Some(capture) = self.registry.capture() {
            let _ = self.conn.unmap_window(capture);
        }
    }

    fn release_pointer(&self) {
        self.release_capture_unmap();
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
    }

    fn interaction_capture(
        &mut self,
        grab_window: Window,
        cursor: flamewm_render_core::CursorKind,
    ) -> Result<bool, ReplyOrIdError> {
        let Some(grab_target) = self.live_pointer_grab_target(grab_window) else {
            self.release_pointer();
            return Ok(false);
        };
        let capture = match self.registry.capture() {
            Some(existing) => existing,
            None => {
                let id = self.conn.generate_id()?;
                let root = self.screen().root;
                self.conn.create_window(
                    COPY_DEPTH_FROM_PARENT,
                    id,
                    root,
                    clamp_i16(self.screen_rect.x),
                    clamp_i16(self.screen_rect.y),
                    clamp_u16(self.screen_rect.width),
                    clamp_u16(self.screen_rect.height),
                    0,
                    WindowClass::INPUT_ONLY,
                    0,
                    &CreateWindowAux::new(),
                )?;
                self.registry.set_capture(id);
                id
            }
        };
        if let Some(renderer) = self.renderer.as_ref() {
            let _ = renderer.borrow_mut().define_cursor(u64::from(capture), cursor);
        }
        let _ = self.conn.map_window(capture);
        let result = self.conn.configure_window(
            capture,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        );
        emit_native_configure(
            flamewm_debug::WM_FRAME_GEOMETRY_MOVE_NATIVE,
            0,
            capture,
            "effect=GrabCapture stack=above".to_owned(),
            &result,
        );
        let _ = result;
        let reply = match self.conn.grab_pointer(
            false,
            grab_target,
            EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            0u32,
            0u32,
            CURRENT_TIME,
        ) {
            Ok(cookie) => match cookie.reply() {
                Ok(reply) => reply,
                Err(error) => {
                    self.release_pointer();
                    return Err(error.into());
                }
            },
            Err(error) => {
                self.release_pointer();
                return Err(ReplyOrIdError::ConnectionError(error));
            }
        };
        if reply.status == GrabStatus::SUCCESS {
            Ok(true)
        } else {
            self.release_pointer();
            flamewm_debug::emit(
                flamewm_debug::WM_FRAME_GRAB_FAIL,
                std::time::Duration::from_secs(1),
                || "grab refused".to_owned(),
            );
            Ok(false)
        }
    }

    fn abort_session_to_idle(&mut self, client: Window) {
        if let Some(session) = self.sessions.get_mut(&client) {
            session.session.cancel();
        }
        self.snap_preview.hide();
        self.release_pointer();
    }

    fn reduce_and_execute(&mut self, client: Window, event: FrameEvent) -> Result<(), ReplyError> {
        let Some(controller) = self.controller_for(client) else {
            return Ok(());
        };
        let placement = controller.placement.mode;
        let (source, region) = match event {
            FrameEvent::Press { source, .. } => (
                source,
                Some(
                    controller
                        .registry
                        .get(&source)
                        .copied()
                        .unwrap_or(FrameRegion::Client),
                ),
            ),
            _ => (0, None),
        };
        // Move accounting: pre-event session decides. Motion/Release out of a
        // Move session count as received; an all-Noop plan counts as coalesced
        // (zero-delta, no X commit). Committed counts land in MoveFrameRoot.
        let is_move_event = matches!(controller.session, InteractionSession::Move(_))
            && matches!(
                event,
                FrameEvent::Motion { .. } | FrameEvent::Release { .. }
            );
        if is_move_event {
            move_counters().record_received(1);
        }
        let (next, commit) = {
            let _guard = if is_move_event {
                Some(flamewm_profiler::start("wm.move.plan_or_reduce"))
            } else {
                None
            };
            interaction::step(&controller, event)
        };
        let drag_away_from_tiled = matches!(controller.session, InteractionSession::Move(_))
            && matches!(
                placement,
                PlacementMode::Maximized | PlacementMode::Snapped(_)
            )
            && next.placement.mode == PlacementMode::Floating;
        let pointer = match event {
            FrameEvent::Press { pointer, .. }
            | FrameEvent::Motion { pointer }
            | FrameEvent::Release { pointer } => Some(pointer),
            _ => None,
        };
        if let (Some(probe), Some(pointer)) = (self.resize_probe.as_mut(), pointer) {
            probe.last_pointer = pointer;
        }
        let resize_boundary = self.resize_probe.as_ref().and_then(|_| match event {
            FrameEvent::Press { .. } if matches!(region, Some(FrameRegion::Resize(_))) => {
                Some(("first", next.session))
            }
            FrameEvent::Release { .. }
                if matches!(controller.session, InteractionSession::Resize(_)) =>
            {
                Some(("last", controller.session))
            }
            _ => None,
        });
        if let Some((boundary, session)) = resize_boundary {
            if let Some(probe) = self.resize_probe.as_ref() {
                emit_resize_probe("reduce", boundary, probe, pointer, session, Some(&commit));
            }
        }
        if matches!(event, FrameEvent::ToggleMax) {
            move_counters().record_toggle_max();
        }
        move_counters().record_placement_transition(placement, next.placement.mode);
        emit_pointer_trace(
            flamewm_debug::WM_FRAME_GEOMETRY_PLAN,
            "reduce",
            client,
            source,
            region,
            event,
            controller.session,
            Some(next.session),
            Some(&commit),
            Some(placement),
            Some(next.placement.mode),
            match event {
                FrameEvent::Snap { ref target } => Some(target as &dyn std::fmt::Debug),
                _ => None,
            },
            None,
        );
        if is_move_event
            && commit
                .effects
                .iter()
                .all(|e| matches!(e, FrameEffect::Noop))
        {
            move_counters().record_coalesced(1);
        }
        // Zero-X-configure fast path: controller already returned Noop.
        self.store_controller(client, &next);
        let result = self.execute_effects(client, &commit);
        if result.is_ok() && drag_away_from_tiled {
            self.mark_windows();
            self.publish_window_state(client)?;
        }
        result
    }

    /// Apply live chrome tokens from the settings sync point. A change
    /// repaints every frame and reapplies its shape (radius/glass).
    pub(crate) fn apply_decoration_tokens(
        &mut self,
        tokens: flamewm_skin::recipes::decoration::ChromeTokens,
    ) -> Result<(), ReplyError> {
        if tokens == self.decoration_tokens {
            return Ok(());
        }
        self.decoration_tokens = tokens;
        let clients: Vec<Window> = self.clients.keys().copied().collect();
        for client in clients {
            let Some((frame, outer, floating)) = self
                .clients
                .get(&client)
                .map(|state| (state.frame, state.outer, state.is_floating()))
            else {
                continue;
            };
            self.clear_frame_shape(frame, outer, floating)?;
            self.paint_chrome(client)?;
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.borrow().flush();
        }
        Ok(())
    }

    fn paint_chrome(&mut self, client: Window) -> Result<(), ReplyError> {
        if self
            .clients
            .get(&client)
            .is_some_and(|state| state.decoration != Decoration::Server)
        {
            return Ok(());
        }
        // Single chrome paint path: delegate scene build to chrome_runtime
        // (J08 paint_plan seam); this owner only bridges cached rasters.
        let Some((chrome_state, identity, icon, frame)) = self.clients.get(&client).map(|state| {
            (
                chrome_runtime::ChromeState {
                    frame_w: state.outer.width.max(1),
                    frame_h: state.outer.height.max(1),
                    active: self.active.is_none_or(|current| current == client),
                    hover: state.hover_control,
                    pressed: self.pressed_control(client),
                    maximized: state.is_maximized(),
                    fullscreen: state.is_fullscreen(),
                    floating: state.is_floating(),
                    tokens: self.decoration_tokens,
                },
                chrome_runtime::ChromeIdentity {
                    title: state.title.clone(),
                    wm_instance: state.wm_instance.clone(),
                    wm_class: state.wm_class.clone(),
                },
                state.icon.clone(),
                state.frame,
            )
        }) else {
            return Ok(());
        };
        let runtime = self.chrome_runtimes.entry(client).or_default();
        runtime.scene_for(&identity, chrome_state, icon);
        if let Some(renderer) = self.renderer.as_ref() {
            let mut renderer = renderer.borrow_mut();
            let _ = runtime.paint_cached(&mut renderer, u64::from(frame));
        }
        Ok(())
    }

    fn pressed_control(&self, client: Window) -> Option<FrameControl> {
        match self.sessions.get(&client).map(|session| session.session) {
            Some(InteractionSession::ControlPress(session)) if session.armed => {
                Some(session.control)
            }
            _ => None,
        }
    }

    fn layout_input_children(&mut self, client: Window) -> Result<(), ReplyError> {
        let Some(res) = self.registry.lookup_by_client(client) else {
            return Ok(());
        };
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        // Single input-child layout owner: lifecycle placements (J19 wiring).
        let ids = lifecycle::LifecycleIds::new(client, state.frame, res.children());
        for placement in &lifecycle::input_child_placements(
            state.outer.width.max(1) as i32,
            state.outer.height.max(1) as i32,
            self.extents_for(client).titlebar_h,
            ids,
        ) {
            let result = self.conn.configure_window(
                placement.xid,
                &ConfigureWindowAux::new()
                    .x(placement.x)
                    .y(placement.y)
                    .width(placement.w)
                    .height(placement.h),
            );
            emit_native_configure(
                flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE,
                client,
                placement.xid,
                format!(
                    "effect=LayoutInput rect=({},{},{},{})",
                    placement.x, placement.y, placement.w, placement.h
                ),
                &result,
            );
            result?;
        }
        Ok(())
    }

    fn bind_child_cursors(&mut self, client: Window) {
        let Some(res) = self.registry.lookup_by_client(client) else {
            return;
        };
        for xid in res.children() {
            let region = self.registry.lookup_by_xid(xid).map(|target| target.region);
            if let Some(region) = region {
                let kind = cursor_for_region(region);
                if let Some(renderer) = self.renderer.as_ref() {
                    let _ = renderer.borrow_mut().define_cursor(u64::from(xid), kind);
                }
            }
        }
    }

    fn reconcile_work_area(&mut self) -> Result<(), ReplyError> {
        self.snap_preview.hide();
        let ids = self.clients.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let (mode, outer, minimized) = match self.clients.get(&id) {
                Some(state) => (state.placement.mode, state.outer, state.minimized),
                None => continue,
            };
            if minimized {
                continue;
            }
            match mode {
                PlacementMode::Fullscreen => continue,
                PlacementMode::Maximized => {
                    let next = self.work_area_for_rect(outer);
                    if next != outer {
                        if let Some(state) = self.clients.get_mut(&id) {
                            state.set_outer(next);
                        }
                        self.apply_frame_geometry(id)?;
                        self.publish_window_state(id)?;
                    }
                }
                PlacementMode::Floating | PlacementMode::Snapped(_) => {
                    let clamped = outer.clamp_inside(self.work_area_for_rect(outer));
                    if clamped != outer {
                        if let Some(state) = self.clients.get_mut(&id) {
                            state.set_outer(clamped);
                        }
                        self.apply_frame_geometry(id)?;
                        self.publish_window_state(id)?;
                    }
                }
            }
        }
        self.mark_work_area();
        self.mark_windows();
        Ok(())
    }

    fn publish_desktop_state(&self) -> Result<(), ReplyError> {
        let root = self.screen().root;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_supported,
            AtomEnum::ATOM,
            &self.atoms.supported(),
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_supporting_wm_check,
            AtomEnum::WINDOW,
            &[self.support_window],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            self.support_window,
            self.atoms.net_supporting_wm_check,
            AtomEnum::WINDOW,
            &[self.support_window],
        )?;
        self.conn.change_property8(
            PropMode::REPLACE,
            self.support_window,
            self.atoms.net_wm_name,
            self.atoms.utf8_string,
            b"FlameWM",
        )?;
        self.publish_workspace_metadata()?;
        self.publish_current_workspace()?;
        self.publish_client_list()?;
        self.publish_active()?;
        Ok(())
    }

    fn publish_current_workspace(&self) -> Result<(), ReplyError> {
        let _guard = flamewm_profiler::start("wm.workspace.switch.ewmh_current");
        workspace_counters().ewmh_current.increment();
        self.conn.change_property32(
            PropMode::REPLACE,
            self.screen().root,
            self.atoms.net_current_desktop,
            AtomEnum::CARDINAL,
            &[self.current_workspace as u32],
        )?;
        Ok(())
    }

    fn publish_workspace_metadata(&self) -> Result<(), ReplyError> {
        let root = self.screen().root;
        let work = self.work_area();
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_number_of_desktops,
            AtomEnum::CARDINAL,
            &[self.workspace_count as u32],
        )?;
        // _NET_CURRENT_DESKTOP owned by publish_current_workspace only.
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_desktop_geometry,
            AtomEnum::CARDINAL,
            &[self.screen_rect.width, self.screen_rect.height],
        )?;
        let mut viewport = Vec::with_capacity(self.workspace_count * 2);
        let mut workareas = Vec::with_capacity(self.workspace_count * 4);
        let names = desktop_names_payload(self.workspace_count);
        for _ in 0..self.workspace_count {
            viewport.extend_from_slice(&[0, 0]);
            workareas.extend_from_slice(&[work.x as u32, work.y as u32, work.width, work.height]);
        }
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_desktop_viewport,
            AtomEnum::CARDINAL,
            &viewport,
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_workarea,
            AtomEnum::CARDINAL,
            &workareas,
        )?;
        self.conn.change_property8(
            PropMode::REPLACE,
            root,
            self.atoms.net_desktop_names,
            self.atoms.utf8_string,
            &names,
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_showing_desktop,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        Ok(())
    }

    fn publish_client_list(&self) -> Result<(), ReplyError> {
        workspace_counters().query_tree.increment();
        // The root tree is the server's authoritative bottom-to-top order.  HashMap iteration
        // cannot represent stacking and made _NET_CLIENT_LIST_STACKING observably false.
        let tree = self.conn.query_tree(self.screen().root)?.reply()?;
        let windows = tree
            .children
            .iter()
            .filter_map(|frame| self.frame_to_client.get(frame).copied())
            .collect::<Vec<_>>();
        let client_list = self
            .managed_order
            .iter()
            .copied()
            .filter(|window| self.clients.contains_key(window))
            .collect::<Vec<_>>();
        let root = self.screen().root;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_client_list,
            AtomEnum::WINDOW,
            &client_list,
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_client_list_stacking,
            AtomEnum::WINDOW,
            &windows,
        )?;
        Ok(())
    }

    fn publish_active(&self) -> Result<(), ReplyError> {
        let root = self.screen().root;
        self.conn.change_property32(
            PropMode::REPLACE,
            root,
            self.atoms.net_active_window,
            AtomEnum::WINDOW,
            &[self.active.unwrap_or(0)],
        )?;
        Ok(())
    }

    fn scan_existing(&mut self) -> Result<(), ReplyOrIdError> {
        let root = self.screen().root;
        let tree = self.conn.query_tree(root)?.reply()?;
        let mut pending = Vec::with_capacity(tree.children.len());
        for window in tree.children {
            if window == self.support_window {
                continue;
            }
            pending.push((
                window,
                self.conn.get_window_attributes(window)?,
                self.conn.get_geometry(window)?,
            ));
        }
        for (window, attrs, geometry) in pending {
            let Ok(attrs) = attrs.reply() else { continue };
            let Ok(geometry) = geometry.reply() else {
                continue;
            };
            if attrs.map_state != MapState::UNMAPPED {
                let kind = self.window_kind(window, attrs.override_redirect)?;
                match kind {
                    WindowKind::Popup => {
                        self.conn.map_window(window)?;
                    }
                    WindowKind::Desktop => self.map_unframed(window, StackMode::BELOW)?,
                    WindowKind::Dock => self.map_dock(window)?,
                    WindowKind::Normal | WindowKind::Utility => {
                        self.manage(window, &geometry, true, kind)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn window_kind(
        &self,
        window: Window,
        override_redirect: bool,
    ) -> Result<WindowKind, ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                window,
                self.atoms.net_wm_window_type,
                AtomEnum::ATOM,
                0,
                16,
            )?
            .reply()?;
        let types = reply.value32().map_or_else(Vec::new, Iterator::collect);
        Ok(classify(&self.atoms, &types, override_redirect))
    }

    fn map_unframed(&self, window: Window, stack_mode: StackMode) -> Result<(), ReplyError> {
        self.conn.map_window(window)?;
        self.conn
            .configure_window(window, &ConfigureWindowAux::new().stack_mode(stack_mode))?;
        Ok(())
    }

    fn map_dock(&mut self, window: Window) -> Result<(), ReplyOrIdError> {
        self.dock_struts.insert(window, self.read_strut(window)?);
        self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        self.map_unframed(window, StackMode::ABOVE)?;
        self.changes.panels = true;
        self.mark_work_area();
        self.reconcile_work_area()?;
        self.publish_workspace_metadata()?;
        Ok(())
    }

    fn read_strut(&self, window: Window) -> Result<[u32; 12], ReplyError> {
        let partial = self
            .conn
            .get_property(
                false,
                window,
                self.atoms.net_wm_strut_partial,
                AtomEnum::CARDINAL,
                0,
                12,
            )?
            .reply()?;
        if let Some(mut values) = partial.value32() {
            let mut result = [0; 12];
            for value in &mut result {
                *value = values.next().unwrap_or(0);
            }
            if result != [0; 12] {
                return Ok(result);
            }
        }
        let legacy = self
            .conn
            .get_property(
                false,
                window,
                self.atoms.net_wm_strut,
                AtomEnum::CARDINAL,
                0,
                4,
            )?
            .reply()?;
        let mut result = [0; 12];
        if let Some(mut values) = legacy.value32() {
            for value in result.iter_mut().take(4) {
                *value = values.next().unwrap_or(0);
            }
        }
        result[4] = 0;
        result[5] = self.screen_rect.height.saturating_sub(1);
        result[6] = 0;
        result[7] = self.screen_rect.height.saturating_sub(1);
        result[8] = 0;
        result[9] = self.screen_rect.width.saturating_sub(1);
        result[10] = 0;
        result[11] = self.screen_rect.width.saturating_sub(1);
        Ok(result)
    }

    fn manage(
        &mut self,
        window: Window,
        geometry: &GetGeometryReply,
        adopt: bool,
        kind: WindowKind,
    ) -> Result<(), ReplyOrIdError> {
        if self.clients.contains_key(&window) {
            self.conn.map_window(window)?;
            return Ok(());
        }
        if kind == WindowKind::Popup {
            return Ok(());
        }
        if kind == WindowKind::Desktop {
            self.map_unframed(window, StackMode::BELOW)?;
            return Ok(());
        }
        if kind == WindowKind::Dock {
            self.map_dock(window)?;
            return Ok(());
        }

        // Initial-map order: read title/class/hints/icon/transient/type first.
        let title = self
            .read_title(window)
            .unwrap_or_else(|_| "Application".to_owned());
        let (wm_instance, wm_class) = self.wm_identity(window);
        let (icon, icon_fallback) = self.read_native_frame_icon(window);
        let transient_for = self.read_transient_for(window)?;
        let hints = self.read_hints(window);
        let workspace = transient_for
            .and_then(|owner| self.clients.get(&owner).map(|state| state.workspace))
            .unwrap_or(self.current_workspace);

        let screen = self.screen();
        let frame = self.conn.generate_id()?;
        let decoration = self.read_decoration(window, kind);
        let extents = self.extents_for_decoration(decoration);
        let work_root = self.work_root();
        let start = crate::client::placement_floating(RootRect::new(0, 0, 1, 1));
        let initial_req = GeometryRequest {
            reason: GeometryReason::InitialMap,
            start: PlacementSnapshot {
                mode: start.mode,
                rect: start.current,
            },
            pointer_delta: (0, 0),
            edges: crate::frame::model::ResizeEdges::none(),
            work_area: work_root,
            hints,
            frame_extents: extents,
        };
        let initial_plan = crate::frame::geometry::plan_initial_map(
            &initial_req,
            RootPoint::new(i32::from(geometry.x), i32::from(geometry.y)),
            (
                u32::from(geometry.width).max(1),
                u32::from(geometry.height).max(1),
            ),
        );
        let frame_rect = if adopt {
            crate::frame::geometry::plan_adopt(
                RootPoint::new(i32::from(geometry.x), i32::from(geometry.y)),
                (
                    u32::from(geometry.width).max(1),
                    u32::from(geometry.height).max(1),
                ),
                extents,
            )
        } else {
            initial_plan.frame
        };
        let outer = crate::client::root_to_rect(frame_rect);

        // Create frame UNMAPPED with panel bg #1b1e20 (not black).
        self.conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            frame,
            screen.root,
            clamp_i16(outer.x),
            clamp_i16(outer.y),
            clamp_u16(outer.width),
            clamp_u16(outer.height),
            self.config.frame_border,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(0x1b1e20)
                .border_pixel(0x1b1e20)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::SUBSTRUCTURE_NOTIFY
                        | EventMask::SUBSTRUCTURE_REDIRECT,
                ),
        )?;

        // Grab-server only for save-set/reparent/structural-configure.
        // Reparent offset + client configure from the lifecycle contract
        // (single geometry source); this owner performs the X calls.
        self.conn.grab_server()?;
        self.conn.change_save_set(SetMode::INSERT, window)?;
        let (reparent_x, reparent_y) = lifecycle::reparent_offset(extents);
        self.conn
            .reparent_window(window, frame, reparent_x as i16, reparent_y as i16)?;
        self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        // Click-to-focus: a sync passive grab sees presses inside the client area;
        // handle_client_press focuses and then replays the press to the client.
        self.conn.grab_button(
            false,
            window,
            EventMask::BUTTON_PRESS,
            GrabMode::SYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            x11rb::NONE,
            ButtonIndex::ANY,
            ModMask::ANY,
        )?;
        let (client_x, client_y, client_w, client_h) =
            lifecycle::client_configure(crate::client::rect_to_root(outer), extents);
        self.conn.configure_window(
            window,
            &ConfigureWindowAux::new()
                .x(client_x)
                .y(client_y)
                .width(client_w)
                .height(client_h)
                .border_width(0),
        )?;

        // Create real 12 InputOnly children (8 resize + title-drag + 3 controls).
        let child_ids: [u32; 12] = [
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
            self.conn.generate_id()?,
        ];
        // Single lifecycle transaction: ids + placements from lifecycle
        // policy; this owner only performs the X create/configure loop.
        let lifecycle_ids = lifecycle::LifecycleIds::new(window, frame, child_ids);
        let resources = lifecycle_ids.resources();
        let placements = lifecycle::input_child_placements(
            outer.width.max(1) as i32,
            outer.height.max(1) as i32,
            extents.titlebar_h,
            lifecycle_ids,
        );
        for placement in &placements {
            self.conn.create_window(
                COPY_DEPTH_FROM_PARENT,
                placement.xid,
                frame,
                placement.x as i16,
                placement.y as i16,
                placement.w.max(1) as u16,
                placement.h.max(1) as u16,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::from(INPUT_CHILD_EVENT_MASK)),
            )?;
        }
        self.conn.ungrab_server()?;
        self.conn.flush()?;

        // Insert FrameResources + ManagedClient.
        let _ = self.registry.register(resources);
        let title_width = flamewm_render_x11::external_text_measure(&title, 12.0)
            .0
            .round() as i32;
        let client = ManagedClient {
            client: window,
            frame,
            placement: crate::client::placement_floating(crate::client::rect_to_root(outer)),
            hints,
            outer,
            workspace,
            title,
            title_text_width: title_width,
            icon,
            icon_fallback,
            icon_request_token: self.next_client_lifetime,
            catalog_icon_key: None,
            wm_instance,
            wm_class,
            transient_for,
            minimized: false,
            sticky: false,
            ignore_unmap: 1,
            kind,
            hover_control: None,
            decoration,
        };
        self.frame_to_client.insert(frame, window);
        for xid in resources.children() {
            self.frame_to_client.insert(xid, window);
        }
        self.clients.insert(window, client);
        self.next_client_lifetime = self.next_client_lifetime.saturating_add(1);
        self.chrome_runtimes
            .insert(window, chrome_runtime::ChromeRuntime::new());
        if self.clients.get(&window).is_some_and(|client| client.icon.is_none()) {
            self.request_catalog_icon(window);
        }
        self.managed_order.push(window);
        let controller = self.controller_for(window).unwrap_or_else(|| {
            ControllerState::new(
                window,
                self.screen().root,
                crate::client::placement_floating(crate::client::rect_to_root(outer)),
                self.work_root(),
                hints,
                extents,
            )
            .with_screen_rect(crate::client::rect_to_root(Rect::new(
                self.screen_rect.x,
                self.screen_rect.y,
                self.screen_rect.width,
                self.screen_rect.height,
            )))
        });
        self.sessions.insert(window, controller);

        self.set_frame_extents(window)?;
        self.set_client_workspace(window, self.current_workspace)?;
        self.set_wm_state(window, WM_STATE_NORMAL)?;

        // Retarget + paint chrome + shape + cursors.
        self.paint_chrome(window)?;
        let _ = self.renderer.as_ref().map(|renderer| renderer.borrow().flush());
        let floating = self
            .clients
            .get(&window)
            .is_some_and(|state| state.is_floating());
        self.clear_frame_shape(frame, outer, floating)?;
        self.bind_child_cursors(window);

        // Map client + input children + frame (renderer flush above).
        self.conn.map_window(window)?;
        if decoration == Decoration::Server {
            for xid in resources.children() {
                self.conn.map_window(xid)?;
            }
        }
        self.conn.map_window(frame)?;
        self.mark_windows();
        self.publish_client_list()?;
        self.focus(window)?;
        // Window-open effect: only a new, non-adopted normal top-level. It starts on a later
        // Expose, so arming never delays the map or the focus above.
        if !adopt && kind == WindowKind::Normal && self.animator.wants(Effect::Open) {
            self.open_gate.arm(window);
        }
        Ok(())
    }

    fn unmanage(&mut self, window: Window, restore_to_root: bool) -> Result<(), ReplyError> {
        self.snap_preview.hide();
        // The client is leaving: drop its flight and overlay without applying an end state. A
        // running close keeps its afterimage, which finishes on its own timeline.
        if !animation::survives_unmanage(self.animator.running(window)) {
            self.animator.cancel(window);
            self.overlays.remove(&window);
        }
        self.linger.remove(window);
        self.release_damage_watch(window);
        self.minimized_captures.remove(window);
        self.open_gate.clear(window);
        // Abort any in-progress interaction through the reducer so session
        // teardown, ungrab, and snap-preview hide stay in one owner. Runs
        // before `clients.remove` because `controller_for` reads it.
        if let Some(controller) = self.controller_for(window) {
            if !controller.session.is_idle() {
                let (next, commit) = interaction::step(&controller, FrameEvent::Cancel);
                self.store_controller(window, &next);
                self.execute_effects(window, &commit)?;
            }
        }
        let Some(client) = self.clients.remove(&window) else {
            return Ok(());
        };
        self.deferred_icon_jobs.remove(&window);
        self.frame_extents_published.remove(&window);
        for requesters in self.icon_requests.values_mut() {
            requesters.retain(|(id, _)| *id != window);
        }
        self.icon_requests.retain(|_, requesters| !requesters.is_empty());
        self.managed_order.retain(|managed| *managed != window);
        self.frame_to_client.remove(&client.frame);
        self.chrome_runtimes.remove(&window);
        let removed = self.registry.remove_client(window);
        for xid in &removed {
            let _ = self.conn.destroy_window(*xid);
            self.frame_to_client.remove(xid);
        }
        self.release_pointer();
        self.sessions.remove(&window);
        if self.active == Some(window) {
            self.active = None;
        }
        if restore_to_root {
            let _ = self.conn.grab_server();
            let _ = self.conn.change_save_set(SetMode::DELETE, window);
            let _ = self.conn.reparent_window(
                window,
                self.screen().root,
                clamp_i16(client.outer.x),
                clamp_i16(client.outer.y),
            );
            let _ = self.conn.ungrab_server();
        }
        let _ = self.conn.destroy_window(client.frame);
        let _ = self.set_wm_state(window, WM_STATE_WITHDRAWN);
        self.mark_windows();
        self.publish_client_list()?;
        self.publish_active()?;
        Ok(())
    }

    /// Dispatches one event. A request that raced a client's destruction (BadWindow and friends)
    /// is expected on a window manager and must never end the session: the window is gone, and its
    /// DestroyNotify performs the cleanup.
    fn handle_event(&mut self, event: Event) -> Result<(), ReplyOrIdError> {
        match self.handle_event_inner(event) {
            Err(error) if is_destroyed_window_race(&error) => {
                log_once(&DESTROYED_WINDOW_RACE_LOG, &format!("ignored a request that raced a destroyed window: {error}"));
                Ok(())
            }
            other => other,
        }
    }

    fn handle_event_inner(&mut self, event: Event) -> Result<(), ReplyOrIdError> {
        match event {
            Event::MapRequest(event) => self.handle_map_request(event)?,
            Event::ConfigureRequest(event) => self.handle_configure_request(event)?,
            Event::DestroyNotify(event) => {
                if self.dock_struts.remove(&event.window).is_some() {
                    self.changes.panels = true;
                    self.mark_work_area();
                    self.reconcile_work_area()?;
                }
                let client = self.client_for(event.window);
                if let Some(client) = client {
                    self.unmanage(client, false)?;
                }
                self.publish_workspace_metadata()?;
            }
            Event::UnmapNotify(event) => self.handle_unmap(event.window)?,
            Event::MapNotify(event) => self.handle_map_notify(event.window)?,
            Event::ConfigureNotify(event) => self.handle_configure_notify(event)?,
            Event::DamageNotify(event) => self.handle_damage_notify(event.damage),
            Event::Expose(event) => {
                if event.count == 0 {
                    if let Some(client) = self.client_for(event.window) {
                        self.paint_chrome(client)?;
                        self.try_start_open(client)?;
                    }
                }
            }
            Event::EnterNotify(event) => self.handle_enter(event)?,
            Event::LeaveNotify(event) => self.handle_leave(event)?,
            Event::ButtonPress(event) => self.handle_button_press(event)?,
            Event::ButtonRelease(event) => self.handle_button_release(event)?,
            Event::KeyPress(event) => self.handle_key_press(event)?,
            Event::KeyRelease(event) => self.handle_key_release(event)?,
            Event::FocusOut(_) => self.super_tap.focus_lost(),
            Event::MotionNotify(event) => self.handle_motion(event)?,
            Event::PropertyNotify(event) => self.handle_property(event.window, event.atom)?,
            Event::ClientMessage(event) => self.handle_client_message(event)?,
            _ => {}
        }
        Ok(())
    }

    /// Dispatches a passive-grab KeyPress to the bound action. Only actions
    /// with an existing WM path are executed; the rest are logged once.
    fn handle_key_press(&mut self, event: KeyPressEvent) -> Result<(), ReplyError> {
        if let Some(active) = self.active {
            self.cancel_open_for(active)?;
        }
        let state = u16::from(event.state);
        let matched = crate::shortcut_action_for_key(event.detail, state);
        if key_diag_enabled() {
            eprintln!(
                "flamewm: keypress detail={} state={:04x} action={}",
                event.detail,
                state,
                matched.as_deref().unwrap_or("none")
            );
        }
        // A bare Start-bound Super press only arms the tap; Start toggles on its
        // completed release. Any other press completes or cancels a pending tap.
        if matched.as_deref() == Some(ACTION_TOGGLE_START_MENU) && is_bare_state(state) {
            let outcome = self.super_tap.press_super(event.detail, event.time);
            self.apply_super_tap(outcome);
            return Ok(());
        }
        let outcome = self.super_tap.press_other();
        self.apply_super_tap(outcome);
        let Some(action) = matched else {
            return Ok(());
        };
        match action.as_str() {
            ACTION_WINDOW_CLOSE => {
                if let Some(client) = self.active {
                    self.request_close(client)?;
                }
            }
            ACTION_WINDOW_MINIMIZE => {
                if let Some(client) = self.active {
                    self.minimize(client)?;
                }
            }
            ACTION_WINDOW_MAXIMIZE => {
                if let Some(client) = self.active {
                    self.toggle_maximize(client)?;
                }
            }
            ACTION_WORKSPACE_LEFT => {
                if let Some(target) = next_workspace(self.current_workspace, self.workspace_count, -1)
                {
                    self.switch_workspace(target)?;
                }
            }
            ACTION_WORKSPACE_RIGHT => {
                if let Some(target) = next_workspace(self.current_workspace, self.workspace_count, 1)
                {
                    self.switch_workspace(target)?;
                }
            }
            // Single-row desktop layout (KDE default): up/down intentionally do nothing.
            ACTION_WORKSPACE_UP | ACTION_WORKSPACE_DOWN => {}
            // Same spawn and supervision path as the Lock session action.
            ACTION_SESSION_LOCK => {
                if let Err(error) = flamewm_x11_desktop::start_lock_screen() {
                    eprintln!("flamewm: lock shortcut failed: {error:?}");
                }
            }
            // Chorded start-menu binding: toggle on press. Bare Super is handled above
            // and toggles through the SuperTap commit at the end of the batch.
            ACTION_TOGGLE_START_MENU => self.changes.start_menu_toggle = true,
            other => log_unhandled_shortcut_once(other),
        }
        Ok(())
    }

    /// Bare-Super release only completes a tap; Start toggles at batch commit.
    fn handle_key_release(&mut self, event: KeyReleaseEvent) -> Result<(), ReplyError> {
        self.super_tap.release_super(event.detail, event.time);
        Ok(())
    }

    /// Completes a pending bare-Super tap. Called at each event-batch boundary.
    fn commit_super_tap(&mut self) {
        let outcome = self.super_tap.commit();
        self.apply_super_tap(outcome);
    }

    fn apply_super_tap(&mut self, outcome: SuperTapOutcome) {
        if outcome == SuperTapOutcome::Toggle {
            self.changes.start_menu_toggle = true;
        }
    }

    fn handle_configure_notify(&mut self, event: ConfigureNotifyEvent) -> Result<(), ReplyError> {
        // Observation-only: classify and compare through configure policy;
        // never commit geometry from a ConfigureNotify.
        let synthetic = event.response_type & 0x80 != 0;
        if synthetic {
            let _ = configure::classify_notify(
                event.window,
                event.window,
                event.window,
                false,
                true,
                true,
            );
            return Ok(());
        }
        if event.window == self.screen().root {
            configure_bump!(notify_root, notify_root_total);
            self.screen_rect.width = u32::from(event.width).max(1);
            self.screen_rect.height = u32::from(event.height).max(1);
            self.mark_work_area();
            self.reconcile_work_area()?;
            self.publish_workspace_metadata()?;
            return Ok(());
        }
        // Classify the observed window before touching placement. Frame,
        // client, and InputOnly children all resolve to one managed client
        // via `client_for`, so disambiguate with the authoritative frame id
        // and the input-child registry first.
        let Some(client_id) = self.client_for(event.window) else {
            let _ = configure::classify_notify(event.window, 0, 0, false, false, false);
            return Ok(());
        };
        let (frame_id, expected_frame, mode) = match self.clients.get(&client_id) {
            Some(state) => (state.frame, state.placement.current, state.placement.mode),
            None => return Ok(()),
        };
        let _guard = flamewm_profiler::start("wm.configure.notify.observe");
        let extents = self.extents_for(client_id);
        let class = configure::classify_notify(
            event.window,
            frame_id,
            client_id,
            self.registry.lookup_by_xid(event.window).is_some(),
            false,
            true,
        );
        match class {
            configure::NotifyClass::FrameObserved => {
                // Expected frame observation: outer rect in root coords.
                configure_bump!(notify_frame, notify_frame_total);
                let observed = RootRect::new(
                    i32::from(event.x),
                    i32::from(event.y),
                    i32::from(event.width).max(1),
                    i32::from(event.height).max(1),
                );
                match configure::observe_frame(expected_frame, observed) {
                    configure::Observation::Current => {
                        configure_bump!(notify_expected, notify_expected_total);
                    }
                    configure::Observation::Stale => {
                        configure_bump!(notify_expected, notify_expected_total);
                    }
                    configure::Observation::Mismatch => {
                        configure_bump!(notify_mismatch, notify_mismatch_total);
                        flamewm_debug::emit(
                            flamewm_debug::WM_CONFIGURE_MISMATCH,
                            std::time::Duration::from_secs(1),
                            || {
                                format!(
                                    "client={client_id} source=frame expected=({},{},{},{}) observed=({},{},{},{}) mode={mode:?}",
                                    expected_frame.x,
                                    expected_frame.y,
                                    expected_frame.w,
                                    expected_frame.h,
                                    observed.x,
                                    observed.y,
                                    observed.w,
                                    observed.h,
                                )
                            },
                        );
                    }
                }
            }
            configure::NotifyClass::ClientObserved => {
                // Expected client observation: local (0,titlebar) + interior size.
                configure_bump!(notify_client, notify_client_total);
                let (ox, oy, expected_w, expected_h) =
                    configure::expected_client_geometry(expected_frame, extents);
                match configure::observe_client(
                    expected_frame,
                    extents,
                    (
                        i32::from(event.x),
                        i32::from(event.y),
                        i32::from(event.width),
                        i32::from(event.height),
                    ),
                ) {
                    configure::Observation::Current => {
                        configure_bump!(notify_expected, notify_expected_total);
                    }
                    configure::Observation::Stale => {
                        configure_bump!(notify_expected, notify_expected_total);
                    }
                    configure::Observation::Mismatch => {
                        configure_bump!(notify_mismatch, notify_mismatch_total);
                        flamewm_debug::emit(
                            flamewm_debug::WM_CONFIGURE_MISMATCH,
                            std::time::Duration::from_secs(1),
                            || {
                                format!(
                                    "client={client_id} source=client expected=({ox},{oy},{expected_w},{expected_h}) observed=({},{},{},{}) mode={mode:?}",
                                    event.x, event.y, event.width, event.height,
                                )
                            },
                        );
                    }
                }
            }
            configure::NotifyClass::InputChildObserved => {
                // Registered InputOnly child: WM-owned detail, never placement.
                configure_bump!(notify_input_child, notify_input_child_total);
                configure_bump!(notify_expected, notify_expected_total);
            }
            configure::NotifyClass::SyntheticEchoDrop | configure::NotifyClass::StaleUnknown => {}
        }
        emit_configure_summary();
        Ok(())
    }

    fn handle_map_request(&mut self, event: MapRequestEvent) -> Result<(), ReplyOrIdError> {
        if self.clients.contains_key(&event.window) {
            self.conn.map_window(event.window)?;
            if let Some(client) = self.clients.get(&event.window) {
                self.conn.map_window(client.frame)?;
            }
            self.publish_client_list()?;
            return Ok(());
        }
        let kind = self.window_kind(
            event.window,
            self.conn
                .get_window_attributes(event.window)?
                .reply()?
                .override_redirect,
        )?;
        match kind {
            WindowKind::Popup => {
                self.map_unframed(event.window, StackMode::ABOVE)?;
                return Ok(());
            }
            WindowKind::Desktop => {
                self.map_unframed(event.window, StackMode::BELOW)?;
                return Ok(());
            }
            WindowKind::Dock => return self.map_dock(event.window),
            WindowKind::Normal | WindowKind::Utility => {}
        }
        let geometry = self.conn.get_geometry(event.window)?.reply()?;
        self.manage(event.window, &geometry, false, kind)
    }

    fn handle_configure_request(&mut self, event: ConfigureRequestEvent) -> Result<(), ReplyError> {
        let _guard = flamewm_profiler::start("wm.configure.request.total");
        let Some(client_id) = self.client_for(event.window) else {
            let override_redirect = self
                .conn
                .get_window_attributes(event.window)?
                .reply()?
                .override_redirect;
            let kind = self.window_kind(event.window, override_redirect)?;
            self.conn.configure_window(
                event.window,
                &desktop_stack_policy(ConfigureWindowAux::from_configure_request(&event), kind),
            )?;
            return Ok(());
        };
        let Some(controller) = self.controller_for(client_id) else {
            return Ok(());
        };
        configure_bump!(request_received, request_received_total);
        // Stack/sibling intent is orthogonal to the geometry decision:
        // carried alongside via configure policy, never flips accept/refuse.
        let has_stack = event.value_mask.contains(ConfigWindow::STACK_MODE);
        let stack = has_stack.then(|| {
            let frame = self
                .clients
                .get(&client_id)
                .map(|state| state.frame)
                .unwrap_or(client_id);
            let sibling = if event.value_mask.contains(ConfigWindow::SIBLING) && event.sibling != 0
            {
                self.client_for(event.sibling)
                    .and_then(|id| self.clients.get(&id).map(|state| state.frame))
                    .or(Some(event.sibling))
            } else {
                None
            };
            (frame, event.stack_mode, sibling)
        });
        // Single geometry policy: exactly one owner decides accept/refuse.
        let (decision, _) = configure::carry_stack_intent(
            configure::decide_request(&controller.session, controller.placement.mode),
            has_stack,
        );
        // Active gesture owns geometry: acknowledge current root geometry,
        // never mutate the frame from the client request.
        if decision == configure::RequestDecision::RefuseInteractive {
            configure_bump!(
                request_refused_interactive,
                request_refused_interactive_total
            );
            self.send_configure_notify(client_id)?;
            emit_configure_summary();
            if let Some((frame, mode, sibling)) = stack {
                self.restack(frame, mode, sibling)?;
                self.publish_client_list()?;
            }
            return Ok(());
        }
        // WM-owned modes stay authoritative against client geometry.
        if decision == configure::RequestDecision::RefuseMode {
            configure_bump!(request_refused_mode, request_refused_mode_total);
            self.send_configure_notify(client_id)?;
            emit_configure_summary();
            if let Some((frame, mode, sibling)) = stack {
                self.restack(frame, mode, sibling)?;
                self.publish_client_list()?;
            }
            return Ok(());
        }
        let extents = controller.extents;
        let prev_root = frame_to_client_root(controller.placement.current, extents);
        let (prev_origin, prev_size) = (prev_root.origin(), prev_root.size_u32());
        let mask = event.value_mask;
        // ICCCM 4.1.5: ConfigureRequest coordinates from a top-level client
        // are in root coordinates irrespective of reparenting (verbatim
        // client-root origin; no frame-offset subtraction).
        let req_origin = configure::request_origin(
            mask.contains(ConfigWindow::X),
            mask.contains(ConfigWindow::Y),
            i32::from(event.x),
            i32::from(event.y),
            prev_origin,
        );
        let req_size = if mask.contains(ConfigWindow::WIDTH) || mask.contains(ConfigWindow::HEIGHT)
        {
            Some((
                if mask.contains(ConfigWindow::WIDTH) {
                    u32::from(event.width).max(1)
                } else {
                    prev_size.0
                },
                if mask.contains(ConfigWindow::HEIGHT) {
                    u32::from(event.height).max(1)
                } else {
                    prev_size.1
                },
            ))
        } else {
            None
        };
        let req = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            start: PlacementSnapshot {
                mode: controller.placement.mode,
                rect: controller.placement.current,
            },
            pointer_delta: (0, 0),
            edges: crate::frame::model::ResizeEdges::none(),
            work_area: controller.work_area,
            hints: controller.hints,
            frame_extents: extents,
        };
        let plan = plan_client_configure(&req, req_origin, req_size);
        if !plan.noop {
            configure_bump!(request_accepted, request_accepted_total);
            let mode = controller.placement.mode;
            if let Some(state) = self.clients.get_mut(&client_id) {
                state.placement.mode = mode;
                state.placement.current = plan.frame;
                if mode == PlacementMode::Floating {
                    state.placement.floating_restore = plan.frame;
                }
                state.outer = crate::client::root_to_rect(plan.frame);
            }
            if let Some(session) = self.sessions.get_mut(&client_id) {
                session.placement.current = plan.frame;
            }
            self.apply_frame_geometry(client_id)?;
            self.send_configure_notify(client_id)?;
        } else {
            self.send_configure_notify(client_id)?;
        }
        emit_configure_summary();
        if let Some((frame, mode, sibling)) = stack {
            self.restack(frame, mode, sibling)?;
            self.publish_client_list()?;
        }
        Ok(())
    }

    fn restack(
        &self,
        window: Window,
        mode: StackMode,
        sibling: Option<Window>,
    ) -> Result<(), ReplyError> {
        let mut aux = ConfigureWindowAux::new().stack_mode(mode);
        if let Some(sibling) = sibling {
            aux = aux.sibling(sibling);
        }
        self.conn.configure_window(window, &aux)?;
        Ok(())
    }

    /// MapNotify without a preceding MapRequest arrives for windows the WM
    /// itself mapped (frame, client, children: `map_window` generates no
    /// MapRequest). A client MapNotify means the client is visible now; if it
    /// is still unframed (e.g. WM missed the MapRequest because it was not
    /// yet selecting SubstructureRedirect), manage it now. Frame/child
    /// MapNotify events are ignored.
    fn handle_map_notify(&mut self, window: Window) -> Result<(), ReplyOrIdError> {
        if self.registry.capture() == Some(window) {
            return Ok(());
        }
        if window == self.screen().root || self.clients.contains_key(&window) {
            return Ok(());
        }
        if self.client_for(window).is_some() {
            return Ok(());
        }
        if self
            .conn
            .get_window_attributes(window)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_none_or(|attrs| attrs.override_redirect)
        {
            return Ok(());
        }
        let kind = self.window_kind(window, false)?;
        match kind {
            WindowKind::Normal | WindowKind::Utility => {
                let geometry = self.conn.get_geometry(window)?.reply()?;
                return self.manage(window, &geometry, false, kind);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_unmap(&mut self, window: Window) -> Result<(), ReplyError> {
        let Some(client_id) = self.client_for(window) else {
            return Ok(());
        };
        // An unmap the WM expects (the transition's own backdrop hide) must not cancel the flight.
        if let Some(client) = self.clients.get_mut(&client_id) {
            if consume_expected_unmap(&mut client.ignore_unmap) {
                return Ok(());
            }
        }
        self.cancel_open_for(client_id)?;
        if window == client_id {
            self.unmanage(client_id, true)?;
        }
        Ok(())
    }

    fn handle_property(&mut self, window: Window, atom: Atom) -> Result<(), ReplyError> {
        if self.dock_struts.contains_key(&window)
            && (atom == self.atoms.net_wm_strut || atom == self.atoms.net_wm_strut_partial)
        {
            self.dock_struts.insert(window, self.read_strut(window)?);
            self.changes.panels = true;
            self.mark_work_area();
            self.reconcile_work_area()?;
            self.publish_workspace_metadata()?;
            return Ok(());
        }
        if window == self.conn.setup().roots[self.screen_num].root {
            if atom == self.atoms.net_current_desktop {
                // Our own publish echoes back here after the snapshot was already
                // emitted; bumping for it would make the shell's revision stale.
                let value = self
                    .read_cardinals(window, atom, 1)
                    .and_then(|values| values.first().copied());
                if desktop_change_is_external(value, self.current_workspace) {
                    crate::bump_workspace_generation();
                }
            } else if atom == self.atoms.net_number_of_desktops
                || atom == self.atoms.net_desktop_names
            {
                // Our own metadata publish echoes back here; only a value that
                // differs from what we wrote is a real external change.
                let count = self
                    .read_cardinals(window, self.atoms.net_number_of_desktops, 1)
                    .and_then(|values| values.first().copied());
                let names = self
                    .conn
                    .get_property(false, window, self.atoms.net_desktop_names, AtomEnum::ANY, 0, u32::MAX)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .map(|reply| reply.value)
                    .unwrap_or_default();
                let own_names = desktop_names_payload(self.workspace_count);
                if workspace_metadata_is_external(count, &names, self.workspace_count, &own_names)
                {
                    crate::bump_workspace_generation();
                    self.mark_workspaces();
                }
            }
        }
        if (atom == self.atoms.motif_wm_hints || atom == self.atoms.gtk_frame_extents)
            && self.clients.contains_key(&window)
        {
            return self.refresh_decoration(window);
        }
        let Some(client_id) = self.client_for(window) else {
            return Ok(());
        };
        if atom == self.atoms.net_wm_name || atom == AtomEnum::WM_NAME.into() {
            if let Ok(title) = self.read_title(client_id) {
                let width = flamewm_render_x11::external_text_measure(&title, 12.0)
                    .0
                    .round() as i32;
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.title_text_width = width;
                    client.title = title;
                }
                self.mark_windows();
                self.paint_chrome(client_id)?;
            }
        } else if atom == self.atoms.net_wm_icon {
            // A fresh icon property invalidates every catalog result/request.
            let (wm_instance, wm_class) = self.wm_identity(client_id);
            let (icon, fallback) = self.read_native_frame_icon(client_id);
            self.deferred_icon_jobs.remove(&client_id);
            for requesters in self.icon_requests.values_mut() {
                requesters.retain(|(id, _)| *id != client_id);
            }
            self.icon_requests.retain(|_, requesters| !requesters.is_empty());
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.wm_instance = wm_instance;
                client.wm_class = wm_class;
                client.icon = icon;
                client.icon_fallback = fallback;
                client.icon_request_token = self.next_client_lifetime;
                client.catalog_icon_key = None;
            }
            self.next_client_lifetime = self.next_client_lifetime.saturating_add(1);
            if self.clients.get(&client_id).is_some_and(|client| client.icon.is_none()) {
                self.request_catalog_icon(client_id);
            }
            self.mark_windows();
            self.paint_chrome(client_id)?;
        } else if atom == AtomEnum::WM_TRANSIENT_FOR.into() {
            let transient_for = self.read_transient_for(client_id)?;
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.transient_for = transient_for;
            }
            self.mark_windows();
        } else if atom == AtomEnum::WM_NORMAL_HINTS.into() {
            let hints = self.read_hints(client_id);
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.hints = hints;
            }
            if let Some(session) = self.sessions.get_mut(&client_id) {
                session.hints = hints;
            }
            self.layout_input_children(client_id)?;
        }
        Ok(())
    }

    fn handle_enter(&mut self, event: EnterNotifyEvent) -> Result<(), ReplyError> {
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        // Passive observers only: Enter/Motion-to-idle-session repaint paths
        // must not reconfigure native input children, or each Enter =>
        // ConfigureNotify => Enter feedback loop sustains a motion storm.
        // Focus/hover state updates stay; layout stays frozen.
        let active_session = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        if active_session {
            return Ok(());
        }
        // Default is click-to-focus: pointer entry (title bar included) only focuses when the user
        // opted in with focusFollowsMouse.
        if crate::live_settings::focus_follows_mouse() {
            self.focus_hover(client_id)?;
        }
        let _ = self.reduce_and_execute(client_id, FrameEvent::Enter);
        Ok(())
    }

    fn handle_leave(&mut self, event: LeaveNotifyEvent) -> Result<(), ReplyError> {
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        let active_session = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        if active_session {
            return Ok(());
        }
        let _ = self.reduce_and_execute(client_id, FrameEvent::Leave);
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.clear_hover();
        }
        self.paint_chrome(client_id)?;
        Ok(())
    }

    fn handle_button_press(&mut self, event: ButtonPressEvent) -> Result<(), ReplyError> {
        if self.clients.contains_key(&event.event) {
            return self.handle_client_press(event);
        }
        let target = resolve_frame_event_source(&self.registry, event.event, event.child);
        if let Some(client) = self.client_for(target) {
            self.cancel_open_for(client)?;
        }
        if event.detail != BUTTON_PRIMARY {
            return Ok(());
        }
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        self.focus(client_id)?;
        // Route press through controller reduce(); execute FrameEffects diff-aware.
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let had_session = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        let _ = had_session;
        let event = FrameEvent::Press { source, pointer };
        let pre = self
            .sessions
            .get(&client_id)
            .map_or(InteractionSession::Idle, |session| session.session);
        let region = self
            .registry
            .lookup_by_xid(source)
            .map_or(FrameRegion::Client, |target| target.region);
        if let FrameRegion::Resize(_) = region {
            self.resize_probe = Some(ResizeProbe {
                client: client_id,
                source,
                region,
                start_pointer: pointer,
                last_pointer: pointer,
                effect_seen: false,
                native_first: None,
                native_last: None,
            });
            if let Some(probe) = self.resize_probe.as_ref() {
                emit_resize_probe("press", "first", probe, Some(pointer), pre, None);
            }
        }
        emit_pointer_trace(
            flamewm_debug::WM_FRAME_EVENT_DISPATCH,
            "button_press",
            client_id,
            source,
            Some(region),
            event,
            pre,
            None,
            None,
            self.clients
                .get(&client_id)
                .map(|state| state.placement.mode),
            None,
            None,
            None,
        );
        match self.reduce_and_execute(client_id, event) {
            Ok(()) => {
                let grabbed = self
                    .sessions
                    .get(&client_id)
                    .map_or(false, |session| !session.session.is_idle());
                if !grabbed {
                    // No session (grab failed or Noop region): treat as control click.
                    self.handle_control_click(client_id)?;
                }
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    /// Press inside a managed client (sync passive grab). Focus/raise when needed, then
    /// always release the frozen pointer so the client receives the press.
    fn handle_client_press(&mut self, event: ButtonPressEvent) -> Result<(), ReplyError> {
        let client = event.event;
        let focus = if matches!(event.detail, 1..=3) && should_focus_on_press(self.active, client) {
            self.focus(client)
        } else {
            Ok(())
        };
        self.conn.allow_events(Allow::REPLAY_POINTER, event.time)?;
        focus
    }

    fn handle_control_click(&mut self, client_id: Window) -> Result<(), ReplyError> {
        let Some(res) = self.registry.lookup_by_client(client_id) else {
            return Ok(());
        };
        // Control clicks only arm via controller; a press that produced no
        // session means the region was Client/Frame or grab failed. Check
        // which control (if any) via registry region is impossible without
        // coords, so fall back to titlebar control hit via pointer query.
        let _ = res;
        Ok(())
    }

    fn handle_button_release(&mut self, event: ButtonReleaseEvent) -> Result<(), ReplyError> {
        if event.detail != BUTTON_PRIMARY {
            return Ok(());
        }
        // Route release through controller reduce(); control clicks execute here.
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.session_client().or_else(|| self.client_for(source)) else {
            return Ok(());
        };
        // Snapshot the release origin BEFORE the reducer runs: the release
        // reducer always idles the session, so post-reducer reads cannot tell
        // Move from resize/control-terminal origins.
        let release_move =
            self.sessions
                .get(&client_id)
                .and_then(|session| match session.session {
                    InteractionSession::Move(move_session) => Some(move_session),
                    _ => None,
                });
        let release_from_move = release_move.is_some();
        let release_from_resize = self
            .sessions
            .get(&client_id)
            .is_some_and(|session| matches!(session.session, InteractionSession::Resize(_)));
        let (was_control, control) =
            match self.sessions.get(&client_id).map(|session| session.session) {
                Some(InteractionSession::ControlPress(session)) => (
                    true,
                    if session.armed {
                        Some(session.control)
                    } else {
                        None
                    },
                ),
                _ => (false, None),
            };
        if matches!(control, Some(FrameControl::MaximizeRestore)) {
            // The controller would toggle and move the frame during this release, before any
            // overlay can capture it. Only end the press here, then run the animated toggle.
            self.reduce_and_execute(client_id, FrameEvent::Cancel)?;
            move_counters().record_toggle_max();
            self.toggle_maximize(client_id)?;
            return Ok(());
        }
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let _ = self.reduce_and_execute(client_id, FrameEvent::Release { pointer });
        if release_from_move {
            self.snap_preview.hide();
        }
        if let Some(control) = control {
            match control {
                FrameControl::Close => self.request_close(client_id)?,
                FrameControl::MaximizeRestore => {
                    move_counters().record_toggle_max();
                    self.mark_windows();
                    self.publish_window_state(client_id)?;
                }
                FrameControl::Minimize => self.minimize(client_id)?,
            }
            return Ok(());
        }
        if was_control {
            return Ok(());
        }
        // Move-release policy: only a Move session may evaluate/commit a
        // release action. Resize/control-terminal releases never reach this
        // path.
        if !release_from_move {
            if release_from_resize {
                // CONTRACT-REGRESSION: resize release never evaluates snap.
                // OBSERVABLE: bounded counter appears in the move summary.
                move_counters().record_resize_snap_blocked();
            }
            return Ok(());
        }
        let target = {
            let _guard = flamewm_profiler::start("wm.move.snap_target");
            self.snap_target_at(event.root_x, event.root_y, self.clients.get(&client_id).map_or(self.work_area(), |s| s.outer))
        };
        move_counters().record_snap_target(snap_target_bits(target));
        let placement = self
            .clients
            .get(&client_id)
            .map(|state| state.placement.mode);
        match release_action_for_move(release_move, placement, target) {
            ReleaseAction::None => {}
            ReleaseAction::Maximize => self.toggle_maximize(client_id)?,
            ReleaseAction::Snap(target) => {
                self.reduce_and_execute(client_id, FrameEvent::Snap { target })?;
                self.mark_windows();
                self.publish_window_state(client_id)?;
            }
        }
        Ok(())
    }

    fn handle_motion(&mut self, event: MotionNotifyEvent) -> Result<(), ReplyError> {
        self.probe_motion_entry(&event);
        // Root points during active session; route through controller reduce().
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.session_client().or_else(|| self.client_for(source)) else {
            return Ok(());
        };
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let move_session =
            self.sessions
                .get(&client_id)
                .and_then(|session| match session.session {
                    InteractionSession::Move(move_session) => Some(move_session),
                    _ => None,
                });
        let move_active = move_session.is_some();
        let active = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        if active {
            let frame_event = FrameEvent::Motion { pointer };
            let pre = self
                .sessions
                .get(&client_id)
                .map_or(InteractionSession::Idle, |session| session.session);
            // One snap target per motion, shared by the trace and the preview.
            let motion_snap = move_active.then(|| {
                let _guard = flamewm_profiler::start("wm.move.snap_target");
                let fallback = self.clients.get(&client_id).map_or(self.work_area(), |s| s.outer);
                let (output, work) = self.snap_zones_at(event.root_x, event.root_y, fallback);
                let target = core_snap_target_in_output(
                    flamewm_api::Point::new(i32::from(event.root_x), i32::from(event.root_y)),
                    to_core_rect(output),
                    to_core_rect(work),
                );
                (target, work)
            });
            emit_pointer_trace(
                flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                "motion",
                client_id,
                source,
                Some(
                    self.registry
                        .lookup_by_xid(source)
                        .map_or(FrameRegion::Client, |target| target.region),
                ),
                frame_event,
                pre,
                None,
                None,
                self.clients
                    .get(&client_id)
                    .map(|state| state.placement.mode),
                None,
                motion_snap.as_ref().map(|(target, _)| target as &dyn std::fmt::Debug),
                motion_snap.map(|(target, _)| target != SnapTarget::None),
            );
            {
                let _guard = if move_active {
                    Some(flamewm_profiler::start("wm.move.total"))
                } else {
                    None
                };
                let _ = self.reduce_and_execute(client_id, frame_event);
            }
            // Snap preview requires an already activated move and the
            // post-reducer placement to be Floating. Resize and control
            // motion never reach the preview path.
            let preview_allowed = activated_move(move_session).is_some()
                && self
                    .clients
                    .get(&client_id)
                    .is_some_and(|state| state.placement.mode == PlacementMode::Floating);
            if preview_allowed {
                if let Some((target, work)) = motion_snap {
                    self.update_snap_preview(target, work, client_id);
                }
            } else if move_active {
                self.snap_preview.hide();
            }
            return Ok(());
        }
        // Idle hover: track control hover, repaint chrome on change.
        // Never reconfigure input children here: Enter/Motion crossover
        // between adjacent children would otherwise reconfigure on every
        // crossing and sustain ConfigureNotify churn. (WM_NORMAL_HINTS
        // updates still relayout via handle_property; that path is rare.)
        let hover = self.idle_hover(client_id, event.event_x, event.event_y, source);
        let changed = if let Some(state) = self.clients.get_mut(&client_id) {
            let changed = state.hover_control != hover;
            state.hover_control = hover;
            changed
        } else {
            false
        };
        if changed {
            self.paint_chrome(client_id)?;
        }
        Ok(())
    }

    fn session_client(&self) -> Option<Window> {
        self.sessions.iter().find_map(|(client, session)| {
            if !session.session.is_idle() {
                Some(*client)
            } else {
                None
            }
        })
    }

    fn idle_hover(
        &self,
        client_id: Window,
        x: i16,
        y: i16,
        source: Window,
    ) -> Option<FrameControl> {
        let region = self
            .registry
            .lookup_by_xid(source)
            .map(|target| target.region);
        match region {
            Some(FrameRegion::Control(control)) => Some(control),
            _ => {
                let state = self.clients.get(&client_id)?;
                crate::frame::input::frame_control_fallback(
                    state.outer.width,
                    u16::try_from(self.extents_for(client_id).titlebar_h).unwrap_or(0),
                    x,
                    y,
                    state.hover_control,
                )
            }
        }
    }

    fn handle_client_message(&mut self, event: ClientMessageEvent) -> Result<(), ReplyError> {
        if event.type_ == self.atoms.wm_change_state && self.clients.contains_key(&event.window) {
            let data = event.data.as_data32();
            flamewm_debug::emit(
                flamewm_debug::DebugEventId("restore.trace.wm.change_state"),
                std::time::Duration::ZERO,
                || {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("t_ms={ms} window={} state={}", event.window, data[0])
                },
            );
            if data[0] == WM_STATE_ICONIC {
                return self.minimize(event.window);
            }
            return self.restore_minimized(event.window);
        }
        let data = event.data.as_data32();
        if event.type_ == self.atoms.net_current_desktop {
            self.switch_workspace(data[0] as usize)?;
        } else if event.type_ == self.atoms.net_number_of_desktops {
            self.set_workspace_count(data[0] as usize)?;
        } else if event.type_ == self.atoms.net_wm_desktop {
            if self.clients.contains_key(&event.window) {
                self.move_to_workspace(event.window, data[0] as usize)?;
            }
        } else if event.type_ == self.atoms.net_active_window {
            if self.clients.contains_key(&event.window) {
                self.focus(event.window)?;
            }
        } else if event.type_ == self.atoms.net_wm_moveresize
            && self.clients.contains_key(&event.window)
        {
            let pointer = RootPoint::new(data[0] as i32, data[1] as i32);
            self.begin_client_moveresize(event.window, pointer, data[2])?;
        } else if event.type_ == self.atoms.net_close_window {
            if self.clients.contains_key(&event.window) {
                self.request_close(event.window)?;
            }
        } else if event.type_ == self.atoms.net_wm_state && self.clients.contains_key(&event.window)
        {
            let wants_maximize = [data[1], data[2]].iter().any(|atom| {
                *atom == self.atoms.net_wm_state_maximized_horz
                    || *atom == self.atoms.net_wm_state_maximized_vert
            });
            let before = self
                .clients
                .get(&event.window)
                .map(|state| state.placement.mode);
            if wants_maximize {
                move_counters().record_client_message_maximize();
            }
            self.apply_net_wm_state(event.window, data[0], data[1], data[2])?;
            let after = self
                .clients
                .get(&event.window)
                .map(|state| state.placement.mode);
            if let (Some(before), Some(after)) = (before, after) {
                flamewm_debug::emit(
                    flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                    std::time::Duration::ZERO,
                    || {
                        format!(
                            "phase=client_message client={} source={} region=None pointer=None pre_session={:?} post_session={:?} placement={before:?} post_placement={after:?} maximize_request={wants_maximize} state_mutation={}",
                            event.window,
                            event.window,
                            self.sessions
                                .get(&event.window)
                                .map(|session| session.session),
                            self.sessions
                                .get(&event.window)
                                .map(|session| session.session),
                            before != after,
                        )
                    },
                );
            }
        }
        Ok(())
    }

    fn apply_net_wm_state(
        &mut self,
        client: Window,
        action: u32,
        first: Atom,
        second: Atom,
    ) -> Result<(), ReplyError> {
        let atoms = [first, second];
        let wants_maximize = atoms.iter().any(|atom| {
            *atom == self.atoms.net_wm_state_maximized_horz
                || *atom == self.atoms.net_wm_state_maximized_vert
        });
        let wants_fullscreen = atoms.contains(&self.atoms.net_wm_state_fullscreen);
        let wants_hidden = atoms.contains(&self.atoms.net_wm_state_hidden);
        if wants_hidden {
            flamewm_debug::emit(
                flamewm_debug::DebugEventId("restore.trace.wm.net_wm_state_hidden"),
                std::time::Duration::ZERO,
                || {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("t_ms={ms} window={client} action={action}")
                },
            );
            // Add/toggle hides; Remove (the control plane's RestoreWindow) un-minimizes.
            return if action == 0 {
                self.restore_minimized(client)
            } else {
                self.minimize(client)
            };
        }
        if wants_fullscreen {
            let currently = self
                .clients
                .get(&client)
                .is_some_and(|state| state.is_fullscreen());
            let enable = match action {
                0 => false,
                1 => true,
                2 => !currently,
                _ => currently,
            };
            return self.set_fullscreen(client, enable);
        }
        if wants_maximize {
            let currently = self
                .clients
                .get(&client)
                .is_some_and(|state| state.is_maximized());
            let enable = match action {
                0 => false,
                1 => true,
                2 => !currently,
                _ => currently,
            };
            if enable != currently {
                self.toggle_maximize(client)?;
            }
        }
        Ok(())
    }

    fn focus(&mut self, client: Window) -> Result<(), ReplyError> {
        self.focus_with_stacking(client, true)
    }

    /// Focus-follows-mouse entry: takes input focus but never restacks, so hovering a window
    /// cannot raise it above the window the user is working in.
    fn focus_hover(&mut self, client: Window) -> Result<(), ReplyError> {
        self.focus_with_stacking(client, false)
    }

    fn focus_with_stacking(&mut self, client: Window, raise: bool) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if !state.visible_on(self.current_workspace) {
            return Ok(());
        }
        self.conn
            .set_input_focus(FOCUS_REVERT_TO, client, CURRENT_TIME)?;
        if raise && (state.kind != WindowKind::Utility || state.transient_for.is_some()) {
            self.conn.configure_window(
                state.frame,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
        }
        if self.supports_protocol(client, self.atoms.wm_take_focus)? {
            let event = ClientMessageEvent::new(
                32,
                client,
                self.atoms.wm_protocols,
                [self.atoms.wm_take_focus, CURRENT_TIME, 0, 0, 0],
            );
            self.conn
                .send_event(false, client, EventMask::NO_EVENT, event)?;
        }
        self.active = Some(client);
        self.mark_windows();
        self.publish_active()?;
        self.publish_client_list()?;
        Ok(())
    }

    fn supports_protocol(&self, client: Window, protocol: Atom) -> Result<bool, ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.wm_protocols,
                AtomEnum::ATOM,
                0,
                32,
            )?
            .reply()?;
        Ok(reply
            .value32()
            .is_some_and(|mut values| values.any(|atom| atom == protocol)))
    }

    /// Client close request from the keyboard, frame button or `_NET_CLOSE_WINDOW`. The afterimage
    /// is captured first; the close request is sent right after and is never delayed by it.
    fn request_close(&mut self, client: Window) -> Result<(), ReplyError> {
        self.begin_close_overlay(client);
        self.close(client)
    }

    /// Starts the close fade for `client` while its pixels still exist. Skipped without ARGB
    /// (an opaque backdrop would show black) or when the effect is disabled or cannot start.
    fn begin_close_overlay(&mut self, client: Window) {
        let Some(renderer) = self.renderer.clone() else {
            return;
        };
        if !renderer.borrow().warp_argb_usable() {
            return;
        }
        if self.animator.start(client, Effect::Close, std::time::Instant::now()) != Begin::Started {
            return;
        }
        if self.open_overlay(client, Effect::Close).is_err() {
            self.animator.cancel(client);
        }
    }

    fn close(&self, client: Window) -> Result<(), ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.wm_protocols,
                AtomEnum::ATOM,
                0,
                32,
            )?
            .reply()?;
        let supports_delete = reply
            .value32()
            .is_some_and(|mut values| values.any(|atom| atom == self.atoms.wm_delete_window));
        if supports_delete {
            let event = ClientMessageEvent::new(
                32,
                client,
                self.atoms.wm_protocols,
                [self.atoms.wm_delete_window, CURRENT_TIME, 0, 0, 0],
            );
            self.conn
                .send_event(false, client, EventMask::NO_EVENT, event)?;
        } else {
            self.conn.kill_client(client)?;
        }
        Ok(())
    }

    fn minimize(&mut self, client: Window) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if state.minimized && self.animator.running(client).is_none() {
            return Ok(());
        }
        if self.begin_transition(client, Effect::Minimize)? {
            return Ok(());
        }
        self.commit_minimize(client, false)
    }

    /// Applies the minimize end state. `hidden` means the frame was already unmapped
    /// when the transition began, so no second unmap (and no second ignore count) occurs.
    fn commit_minimize(&mut self, client: Window, hidden: bool) -> Result<(), ReplyError> {
        let frame = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            if state.minimized {
                return Ok(());
            }
            state.minimized = true;
            if !hidden {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
            }
            state.frame
        };
        if !hidden {
            self.conn.unmap_window(frame)?;
        }
        self.set_wm_state(client, WM_STATE_ICONIC)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        if self.active == Some(client) {
            self.active = None;
            self.publish_active()?;
        }
        Ok(())
    }

    fn restore_minimized(&mut self, client: Window) -> Result<(), ReplyError> {
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("restore.trace.wm.restore_minimized"),
            std::time::Duration::ZERO,
            || {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                format!(
                    "t_ms={ms} window={client} known={} minimized={:?} animator_running={:?}",
                    self.clients.contains_key(&client),
                    self.clients.get(&client).map(|state| state.minimized),
                    self.animator.running(client)
                )
            },
        );
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if !state.minimized && self.animator.running(client).is_none() {
            return Ok(());
        }
        let owned_by_flight = self.begin_transition(client, Effect::Restore)?;
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("restore.trace.wm.begin_transition"),
            std::time::Duration::ZERO,
            || {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                format!("t_ms={ms} window={client} owned_by_flight={owned_by_flight}")
            },
        );
        if owned_by_flight {
            return Ok(());
        }
        self.commit_restore(client)
    }

    fn commit_restore(&mut self, client: Window) -> Result<(), ReplyError> {
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("restore.trace.wm.commit_restore"),
            std::time::Duration::ZERO,
            || {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                format!("t_ms={ms} window={client}")
            },
        );
        let frame = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            state.minimized = false;
            state.frame
        };
        // Mapping the frame produces no UnmapNotify, so no ignore count is added here.
        self.conn.map_window(frame)?;
        self.set_wm_state(client, WM_STATE_NORMAL)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        self.focus(client)?;
        let _ = self.conn.configure_window(
            frame,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        );
        self.publish_client_list()?;
        Ok(())
    }

    /// Starts `effect` with an overlay unless it is disabled or unavailable. Returns `true`
    /// when a flight owns the end state (the caller must not apply it now). A rapid repeat
    /// or an opposite minimize/restore is absorbed; any other running flight finishes first.
    fn begin_transition(&mut self, client: Window, effect: Effect) -> Result<bool, ReplyError> {
        let now = std::time::Instant::now();
        if let Some(current) = self.animator.running(client) {
            if current == effect {
                flamewm_debug::emit(
                    flamewm_debug::DebugEventId("restore.trace.wm.transition_absorbed"),
                    std::time::Duration::ZERO,
                    || {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis());
                        format!("t_ms={ms} window={client} effect={effect:?} reason=repeat")
                    },
                );
                return Ok(true);
            }
            if matches!(
                (current, effect),
                (Effect::Minimize, Effect::Restore) | (Effect::Restore, Effect::Minimize)
            ) {
                self.animator.start(client, effect, now);
                flamewm_debug::emit(
                    flamewm_debug::DebugEventId("restore.trace.wm.transition_absorbed"),
                    std::time::Duration::ZERO,
                    || {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis());
                        format!(
                            "t_ms={ms} window={client} effect={effect:?} current={current:?} reason=opposite"
                        )
                    },
                );
                return Ok(true);
            }
            if let Some(running) = self.animator.cancel(client) {
                self.finish_transition(client, running)?;
            }
        }
        if self.animator.start(client, effect, now) != Begin::Started {
            return Ok(false);
        }
        let hidden = match self.open_overlay(client, effect) {
            Ok(hidden) => hidden,
            Err(error) => {
                log_once(&TRANSITION_SKIP_LOG, &format!("transition skipped: {error}"));
                self.animator.cancel(client);
                return Ok(false);
            }
        };
        if effect == Effect::Minimize && !hidden {
            // The overlay now shows the window; hide the real frame (counted like minimize does).
            if let Some(state) = self.clients.get_mut(&client) {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                let frame = state.frame;
                self.conn.unmap_window(frame)?;
            }
        }
        Ok(true)
    }

    /// Captures the frame (or reuses the minimize capture for restore) and opens its overlay.
    /// Returns `true` when the frame was unmapped here for the opaque backdrop snapshot.
    fn open_overlay(&mut self, client: Window, effect: Effect) -> Result<bool, String> {
        let (frame, outer) = self
            .clients
            .get(&client)
            .map(|state| (state.frame, state.outer))
            .ok_or("transition: unknown client")?;
        let renderer = self.renderer.clone().ok_or("transition: no renderer")?;
        let (capture, mirrored, target) = match effect {
            Effect::Restore => {
                let capture = self
                    .minimized_captures
                    .take(client)
                    .ok_or("transition: no minimize capture")?;
                (capture, true, self.minimize_target_for(client, outer))
            }
            Effect::Maximize => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, false, anim_rect(self.work_area_for_rect(outer)))
            }
            Effect::Unmaximize => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                let restored = self
                    .clients
                    .get(&client)
                    .and_then(|state| unmaximize_target(&state.placement))
                    .ok_or("transition: no restored rect")?;
                (capture, false, anim_rect(crate::client::root_to_rect(restored)))
            }
            Effect::Open => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, true, animation::open_start_rect(anim_rect(outer)))
            }
            Effect::Close => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, false, anim_rect(outer))
            }
            _ => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, false, self.minimize_target_for(client, outer))
            }
        };
        // The window pixels are captured above. Without ARGB the overlay is opaque and snapshots
        // the root under it, so the frame is unmapped first, then synced before the snapshot.
        // Restore needs no hide: its frame is already unmapped while minimized.
        let hide = !renderer.borrow().warp_argb_usable()
            && matches!(effect, Effect::Minimize | Effect::Maximize | Effect::Unmaximize | Effect::Open);
        if hide {
            self.conn.unmap_window(frame).map_err(|e| e.to_string())?;
            if let Some(state) = self.clients.get_mut(&client) {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
            }
            // Round trip on the WM connection: the unmap is processed before the root snapshot.
            let sync = self
                .conn
                .get_input_focus()
                .map_err(|e| e.to_string())
                .and_then(|cookie| cookie.reply().map_err(|e| e.to_string()));
            if let Err(error) = sync {
                // The unmap was sent and its UnmapNotify stays ignored; restore the frame.
                let _ = self.conn.map_window(frame);
                return Err(error);
            }
        }
        // Fade keeps the window rect (minimize, restore, open) or moves it between the two rects
        // (maximize, unmaximize) while crossfading. Magic Lamp keeps the lamp warp; its panel edge is
        // derived inside the renderer from the same source and target rects.
        let style = self.animator.style(effect);
        let mirrored = mirrored && style != animation::Style::Fade;
        let source = warp_rect(anim_rect(outer));
        let opened = match (effect, style) {
            (Effect::Minimize | Effect::Restore | Effect::Open | Effect::Close, animation::Style::Fade) => {
                let direction = if matches!(effect, Effect::Minimize | Effect::Close) {
                    TransitionDirection::Out
                } else {
                    TransitionDirection::In
                };
                renderer.borrow().open_warp_overlay_styled(
                    capture,
                    source,
                    source,
                    PanelEdge::Bottom,
                    TransitionStyle::Fade,
                    direction,
                )
            }
            (Effect::Maximize | Effect::Unmaximize, animation::Style::Fade) => {
                renderer.borrow().open_warp_overlay_styled(
                    capture,
                    source,
                    warp_rect(target),
                    PanelEdge::Bottom,
                    TransitionStyle::Fade,
                    TransitionDirection::In,
                )
            }
            (Effect::Maximize | Effect::Unmaximize, animation::Style::MagicLamp) => {
                renderer.borrow().open_warp_overlay_styled(
                    capture,
                    source,
                    warp_rect(target),
                    PanelEdge::Bottom,
                    TransitionStyle::Stretch,
                    TransitionDirection::In,
                )
            }
            _ => renderer.borrow().open_warp_overlay(capture, source, warp_rect(target)),
        };
        let mut overlay = match opened {
            Ok(overlay) => overlay,
            Err(error) => {
                if hide {
                    let _ = self.conn.map_window(frame);
                }
                return Err(error);
            }
        };
        // Map and paint the first frame before the caller hides the client, so the overlay
        // takes over the window's pixels with no empty frame in between.
        if let Err(error) = overlay.paint(animation::overlay_t(0.0, mirrored)) {
            if hide {
                let _ = self.conn.map_window(frame);
            }
            return Err(error);
        }
        self.linger.remove(client);
        self.overlays
            .insert(client, TransitionOverlay { overlay, mirrored, frame_hidden: hide });
        Ok(hide)
    }

    /// Starts the window-open effect for an armed client on an Expose. Starting only here
    /// means the capture sees painted contents; the map and focus are never waiting on it.
    fn try_start_open(&mut self, client: Window) -> Result<(), ReplyError> {
        if !self.open_gate.is_armed(client) {
            return Ok(());
        }
        let eligible = self
            .clients
            .get(&client)
            .filter(|state| {
                state.is_floating() && !state.minimized && state.kind == WindowKind::Normal
            })
            .map(|state| (state.frame, state.outer));
        let (Some((frame, outer)), Some(renderer)) = (eligible, self.renderer.clone()) else {
            self.open_gate.clear(client);
            return Ok(());
        };
        let Ok(capture) = renderer.borrow().capture_window(u64::from(frame)) else {
            self.open_gate.clear(client);
            return Ok(());
        };
        // A blank capture is an unpainted window: keep the gate armed for a later Expose.
        if !self.open_gate.attempt(client, animation::is_blank_capture(&capture.argb)) {
            return Ok(());
        }
        if self.animator.start(client, Effect::Open, std::time::Instant::now()) != Begin::Started {
            return Ok(());
        }
        // Fade is a crossfade of the window rect, which works with or without a compositing manager.
        if self.animator.style(Effect::Open) == animation::Style::Fade {
            if self.open_overlay(client, Effect::Open).is_err() {
                self.animator.cancel(client);
            }
            return Ok(());
        }
        // Without a compositing manager the transparent fade would show black, so the open uses
        // the opaque warp from a smaller rect, the same backdrop path as minimize.
        if !renderer.borrow().warp_argb_usable() {
            if self.open_overlay(client, Effect::Open).is_err() {
                self.animator.cancel(client);
            }
            return Ok(());
        }
        let opened = renderer.borrow().open_fade_overlay(capture, outer.x, outer.y);
        match opened {
            Ok(overlay) => {
                self.overlays.insert(
                    client,
                    TransitionOverlay { overlay, mirrored: false, frame_hidden: false },
                );
            }
            Err(_) => {
                self.animator.cancel(client);
            }
        }
        Ok(())
    }

    /// Cancels a pending or running window-open effect for `client` on input to it or its
    /// unmap. A running open ends without completing and never delays anything.
    fn cancel_open_for(&mut self, client: Window) -> Result<(), ReplyError> {
        self.open_gate.clear(client);
        if self.animator.running(client) == Some(Effect::Open) {
            if let Some(effect) = self.animator.cancel(client) {
                self.finish_transition(client, effect)?;
            }
        }
        Ok(())
    }

    fn minimize_target_for(&self, client: Window, outer: Rect) -> animation::Rect {
        let output = self
            .output_for_rect(outer)
            .map_or(self.screen_rect, |output| output.geometry);
        animation::minimize_target(self.read_icon_geometry(client), anim_rect(output))
    }

    /// `_NET_WM_ICON_GEOMETRY` as set by the client's taskbar entry, when present.
    fn read_icon_geometry(&self, client: Window) -> Option<animation::Rect> {
        let reply = self
            .conn
            .get_property(false, client, self.atoms.net_wm_icon_geometry, AtomEnum::CARDINAL, 0, 4)
            .ok()?
            .reply()
            .ok()?;
        let values: Vec<u32> = reply.value32()?.collect();
        if values.len() != 4 {
            return None;
        }
        Some(animation::Rect {
            x: values[0] as i32,
            y: values[1] as i32,
            width: values[2] as i32,
            height: values[3] as i32,
        })
    }

    /// Advances flights once per loop turn: paints frames and applies each completed end state.
    pub(crate) fn pump_transitions(&mut self, now: std::time::Instant) -> Result<(), ReplyError> {
        for client in self.linger.expire(now) {
            // The bounded deadline released a client that never repainted (or was not watched).
            self.release_damage_watch(client);
            if std::env::var_os("FLAMEWM_LINGER_LOG").is_some() {
                eprintln!("flamewm: linger released by deadline client={client}");
            }
        }
        for tick in self.animator.tick(now) {
            match tick {
                Tick::Frame { client, progress } => {
                    let failed = self.overlays.get_mut(&client).is_some_and(|flight| {
                        flight
                            .overlay
                            .paint(animation::overlay_t(progress, flight.mirrored))
                            .is_err()
                    });
                    if failed {
                        if let Some(effect) = self.animator.cancel(client) {
                            self.finish_transition(client, effect)?;
                        }
                    }
                }
                Tick::Complete { client, effect } => {
                    if effect == Effect::Restore {
                        flamewm_debug::emit(
                            flamewm_debug::DebugEventId("restore.trace.wm.pump_complete"),
                            std::time::Duration::ZERO,
                            || {
                                let ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(0, |d| d.as_millis());
                                format!("t_ms={ms} window={client} effect={effect:?}")
                            },
                        );
                    }
                    self.finish_transition(client, effect)?
                }
            }
        }
        Ok(())
    }

    /// Releases the overlay and applies `effect`'s end state exactly once.
    fn finish_transition(&mut self, client: Window, effect: Effect) -> Result<(), ReplyError> {
        let flight = self.overlays.remove(&client);
        let hidden = flight.as_ref().is_some_and(|flight| flight.frame_hidden);
        match effect {
            Effect::Minimize => {
                if let Some(flight) = flight {
                    let capture = flight.overlay.finish();
                    let size = capture.argb.len();
                    self.minimized_captures.insert(client, capture, size);
                }
                self.commit_minimize(client, true)
            }
            Effect::Restore => {
                self.commit_restore(client)?;
                self.linger_overlay(client, flight);
                Ok(())
            }
            Effect::Maximize | Effect::Unmaximize => {
                if hidden {
                    // The frame was unmapped for the backdrop snapshot; show it again.
                    if let Some(frame) = self.clients.get(&client).map(|state| state.frame) {
                        self.conn.map_window(frame)?;
                    }
                }
                self.commit_maximize(client)?;
                self.linger_overlay(client, flight);
                Ok(())
            }
            Effect::Open => {
                if hidden {
                    // The frame was unmapped for the backdrop snapshot; show it again.
                    if let Some(frame) = self.clients.get(&client).map(|state| state.frame) {
                        self.conn.map_window(frame)?;
                    }
                }
                self.linger_overlay(client, flight);
                Ok(())
            }
            // The afterimage is dropped with its flight; nothing lingers over a closed window.
            Effect::Close => Ok(()),
        }
    }

    /// Keeps a finished overlay on top of its remapped window for a bounded linger, so the client
    /// repaints under it instead of showing an empty frame when the overlay is removed.
    fn linger_overlay(&mut self, client: Window, flight: Option<TransitionOverlay>) {
        if let Some(flight) = flight {
            flight.overlay.raise();
            self.linger.hold(client, flight.overlay, std::time::Instant::now());
            self.watch_damage(client);
        }
    }

    /// Watches `client` for its first repaint after remap. A failed damage request leaves the
    /// bounded linger deadline as the only release, so errors are not fatal here.
    fn watch_damage(&mut self, client: Window) {
        if self.damage_watch.contains_key(&client) {
            return;
        }
        let Ok(id) = self.conn.generate_id() else {
            return;
        };
        let version_ok = damage::query_version(&self.conn, 1, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        if version_ok && damage::create(&self.conn, id, client, damage::ReportLevel::NON_EMPTY).is_ok() {
            self.damage_watch.insert(client, id);
        }
    }

    /// The client repainted after remap: release its lingering overlay and stop watching it.
    fn handle_damage_notify(&mut self, damage_id: u32) {
        let Some(client) = self
            .damage_watch
            .iter()
            .find(|(_, id)| **id == damage_id)
            .map(|(client, _)| *client)
        else {
            return;
        };
        self.release_damage_watch(client);
        self.linger.remove(client);
        if std::env::var_os("FLAMEWM_LINGER_LOG").is_some() {
            eprintln!("flamewm: linger released by damage client={client}");
        }
    }

    fn release_damage_watch(&mut self, client: Window) {
        if let Some(id) = self.damage_watch.remove(&client) {
            let _ = damage::destroy(&self.conn, id);
        }
    }

    /// Ends every flight at its final state (output change).
    fn finish_all_transitions(&mut self) -> Result<(), ReplyError> {
        for (client, effect) in self.animator.cancel_all() {
            self.finish_transition(client, effect)?;
        }
        Ok(())
    }

    /// Applies the settings sync point's animation values; disabling ends running flights.
    pub(crate) fn apply_transition_settings(&mut self, settings: Settings) -> Result<(), ReplyError> {
        if let Some(line) = self.animator.take_capability_log() {
            eprintln!("flamewm: {line}");
        }
        for (client, effect) in self.animator.apply_settings(settings) {
            self.finish_transition(client, effect)?;
        }
        Ok(())
    }

    /// Caps the loop's wait at one frame while a flight runs, so frames are painted on time.
    pub(crate) fn transition_wait(&self, wait: Option<std::time::Duration>) -> Option<std::time::Duration> {
        if self.animator.is_active() || !self.linger.is_empty() {
            Some(wait.map_or(TRANSITION_FRAME, |wait| wait.min(TRANSITION_FRAME)))
        } else {
            wait
        }
    }

    fn toggle_maximize(&mut self, client: Window) -> Result<(), ReplyError> {
        let effect = self.clients.get(&client).and_then(|state| {
            if state.is_maximized() {
                unmaximize_target(&state.placement).map(|_| Effect::Unmaximize)
            } else if !state.is_fullscreen() && state.is_floating() {
                Some(Effect::Maximize)
            } else {
                None
            }
        });
        if let Some(effect) = effect {
            if self.begin_transition(client, effect)? {
                return Ok(());
            }
        }
        self.commit_maximize(client)
    }

    fn commit_maximize(&mut self, client: Window) -> Result<(), ReplyError> {
        self.reduce_and_execute(client, FrameEvent::ToggleMax)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        Ok(())
    }

    fn set_fullscreen(&mut self, client: Window, enable: bool) -> Result<(), ReplyError> {
        let fullscreen = self
            .clients
            .get(&client)
            .map_or(false, |state| state.is_fullscreen());
        if enable == fullscreen {
            return Ok(());
        }
        // ToggleFullscreen is a pure toggle; drive it only when the target
        // differs from current, and loop-safe (single reduce).
        if (enable && !fullscreen) || (!enable && fullscreen) {
            self.reduce_and_execute(client, FrameEvent::ToggleFullscreen)?;
        }
        self.mark_windows();
        self.publish_window_state(client)?;
        Ok(())
    }

    /// Drag-motion preview: candidate from shared `snap_target`, geometry from
    /// shared `snap_geometry`. `None` hides. Skips redraw when unchanged.
    /// No new native renderer: the compiled snap-preview surface moves/shows.
    fn update_snap_preview(&mut self, target: SnapTarget, work: Rect, client: Window) {
        // Preview is cosmetic only: snapping on release is decided by the frame controller.
        if !crate::live_settings::snap_preview_enabled() {
            self.snap_preview.hide();
            return;
        }
        move_counters().record_snap_target(snap_target_bits(target));
        if target == SnapTarget::None {
            self.snap_preview.hide();
            return;
        }
        let current = self.clients.get(&client).map_or((800, 600), |state| {
            (
                i32::try_from(state.outer.width).unwrap_or(800),
                i32::try_from(state.outer.height).unwrap_or(600),
            )
        });
        let geometry = core_snap_geometry(
            target,
            to_core_rect(work),
            flamewm_api::Size::new(current.0, current.1),
        );
        let geometry = from_core_rect(geometry);
        let geometry = flamewm_api::Rect::new(
            geometry.x,
            geometry.y,
            i32::try_from(geometry.width).unwrap_or(1),
            i32::try_from(geometry.height).unwrap_or(1),
        );
        if self.snap_preview.is_current(
            target,
            geometry,
            snap_preview::DEFAULT_PREVIEW_OPACITY_PERCENT,
        ) {
            return;
        }
        {
            let _guard = flamewm_profiler::start("wm.move.preview");
            self.snap_preview.update(
                Some(target),
                Some(geometry),
                snap_preview::DEFAULT_PREVIEW_OPACITY_PERCENT,
            );
        }
        move_counters().record_preview();
    }

    fn apply_frame_geometry(&mut self, client: Window) -> Result<(), ReplyError> {
        let (frame, outer, floating) = match self.clients.get(&client) {
            Some(state) => (state.frame, state.outer, state.is_floating()),
            None => return Ok(()),
        };
        {
            let _guard = flamewm_profiler::start("wm.resize.shape");
            self.clear_frame_shape(frame, outer, floating)?;
        }
        {
            let _guard = flamewm_profiler::start("wm.resize.frame_configure");
            let titlebar = u32::try_from(self.extents_for(client).titlebar_h).unwrap_or(0);
            self.conn.configure_window(
                frame,
                &ConfigureWindowAux::new()
                    .x(outer.x)
                    .y(outer.y)
                    .border_width(u32::from(self.effective_frame_border(client)))
                    .width(outer.width.max(1))
                    .height(outer.height.max(titlebar + 1)),
            )?;
            self.set_frame_extents(client)?;
        }
        {
            let _guard = flamewm_profiler::start("wm.resize.client_configure");
            let (x, y, width, height) =
                lifecycle::client_configure(
                    crate::client::rect_to_root(outer),
                    self.extents_for(client),
                );
            self.conn.configure_window(
                client,
                &ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(width)
                    .height(height),
            )?;
        }
        {
            let _guard = flamewm_profiler::start("wm.resize.commit");
            // Geometry commit only: child layout is idempotent for an
            // unchanged outer rect, and reconfiguring 12 children per commit
            // sustains ConfigureNotify churn under SUBSTRUCTURE_NOTIFY.
            // Full relayout stays on manage / WM_NORMAL_HINTS / final resize
            // release (execute_effects LayoutInput arm).
        }
        Ok(())
    }

    fn switch_workspace(&mut self, target: usize) -> Result<(), ReplyError> {
        if target >= self.workspace_count || target == self.current_workspace {
            return Ok(());
        }
        self.snap_preview.hide();
        let old = self.current_workspace;
        if let Some(slot) = self.workspace_focus.get_mut(old) {
            *slot = self.active;
        }
        self.current_workspace = target;
        workspace_counters().switch_total.increment();
        // One server grab so the map/unmap batch presents as a single frame.
        self.conn.grab_server()?;
        let visibility = self.apply_workspace_visibility(old, target);
        let _ = self.conn.ungrab_server();
        let flips = visibility?;
        let previous_active = self.active;
        self.active = None;
        let restored = self
            .workspace_focus
            .get(target)
            .copied()
            .flatten()
            .filter(|w| {
                self.clients
                    .get(w)
                    .is_some_and(|state| state.visible_on(target))
            });
        if let Some(window) = restored {
            self.focus(window)?;
        }
        if workspace_switch_touches_windows(&flips, self.active != previous_active) {
            self.mark_windows();
        }
        self.mark_workspaces();
        self.publish_current_workspace()?;
        {
            let _guard = flamewm_profiler::start("wm.workspace.switch.shell_publish");
            self.publish_active()?;
            workspace_counters().shell_publish.increment();
        }
        Ok(())
    }

    /// Maps/unmaps frames whose visibility differs between `old` and `target`.
    /// Returns the (old_visible, new_visible) pair for every client considered.
    fn apply_workspace_visibility(
        &mut self,
        old: usize,
        target: usize,
    ) -> Result<Vec<(bool, bool)>, ReplyError> {
        let _guard = flamewm_profiler::start("wm.workspace.switch.visibility");
        let ids = self.clients.keys().copied().collect::<Vec<_>>();
        let mut pairs = Vec::with_capacity(ids.len());
        for id in ids {
            let (frame, old_visible, new_visible) = {
                let Some(state) = self.clients.get(&id) else {
                    continue;
                };
                (state.frame, state.visible_on(old), state.visible_on(target))
            };
            pairs.push((old_visible, new_visible));
            if old_visible == new_visible {
                continue;
            }
            if new_visible {
                self.conn.map_window(frame)?;
            } else {
                if let Some(state) = self.clients.get_mut(&id) {
                    state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                }
                self.conn.unmap_window(frame)?;
            }
        }
        workspace_counters().visibility.increment();
        Ok(pairs)
    }

    fn set_workspace_count(&mut self, requested: usize) -> Result<(), ReplyError> {
        let requested = requested.clamp(1, 18);
        if requested == self.workspace_count {
            return Ok(());
        }
        self.snap_preview.hide();
        let mut moved = Vec::new();
        if requested < self.workspace_count {
            for state in self.clients.values_mut() {
                if state.workspace >= requested {
                    state.workspace = requested - 1;
                    moved.push((state.client, state.workspace));
                }
            }
            self.current_workspace = self.current_workspace.min(requested - 1);
        }
        for (client, workspace) in moved {
            self.set_client_workspace(client, workspace)?;
        }
        self.workspace_count = requested;
        self.mark_workspaces();
        self.mark_windows();
        self.publish_workspace_metadata()?;
        self.publish_current_workspace()?;
        self.reconcile_workspace_visibility()?;
        Ok(())
    }

    fn reconcile_workspace_visibility(&mut self) -> Result<(), ReplyError> {
        let ids = self.clients.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let (frame, visible) = {
                let Some(state) = self.clients.get(&id) else {
                    continue;
                };
                (state.frame, state.visible_on(self.current_workspace))
            };
            if visible {
                self.conn.map_window(frame)?;
            } else {
                if let Some(state) = self.clients.get_mut(&id) {
                    state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                }
                self.conn.unmap_window(frame)?;
            }
        }
        Ok(())
    }

    fn move_to_workspace(&mut self, client: Window, target: usize) -> Result<(), ReplyError> {
        if target >= self.workspace_count {
            return Ok(());
        }
        self.snap_preview.hide();
        let (frame, visible) = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            state.workspace = target;
            (state.frame, state.visible_on(self.current_workspace))
        };
        self.set_client_workspace(client, target)?;
        self.mark_windows();
        self.mark_workspaces();
        if visible {
            self.conn.map_window(frame)?;
        } else {
            if let Some(state) = self.clients.get_mut(&client) {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
            }
            self.conn.unmap_window(frame)?;
        }
        Ok(())
    }

    fn set_client_workspace(&self, client: Window, workspace: usize) -> Result<(), ReplyError> {
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_wm_desktop,
            AtomEnum::CARDINAL,
            &[workspace as u32],
        )?;
        Ok(())
    }

    fn set_wm_state(&self, client: Window, state: u32) -> Result<(), ReplyError> {
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.wm_state,
            self.atoms.wm_state,
            &[state, 0],
        )?;
        Ok(())
    }

    fn publish_window_state(&self, client: Window) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        let mut values = Vec::with_capacity(3);
        if state.minimized {
            values.push(self.atoms.net_wm_state_hidden);
        }
        if state.is_maximized() {
            values.push(self.atoms.net_wm_state_maximized_vert);
            values.push(self.atoms.net_wm_state_maximized_horz);
        }
        if state.is_fullscreen() {
            values.push(self.atoms.net_wm_state_fullscreen);
        }
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_wm_state,
            AtomEnum::ATOM,
            &values,
        )?;
        Ok(())
    }

    /// `_NET_FRAME_EXTENTS` (EWMH: left, right, top, bottom). Client-drawn
    /// frames publish zeros: Flame adds no frame, so the window is its own
    /// visible area and GTK's `_GTK_FRAME_EXTENTS` stays client-owned.
    /// Effective X border for `client`'s frame: the configured border only
    /// while floating, zero for snapped/maximized/fullscreen frames.
    fn effective_frame_border(&self, client: Window) -> u16 {
        let configured = self.config.frame_border;
        self.clients.get(&client).map_or(configured, |state| {
            frame_border_for_mode(state.placement.mode, configured)
        })
    }

    fn set_frame_extents(&mut self, client: Window) -> Result<(), ReplyError> {
        let mode = self.clients.get(&client).map_or(PlacementMode::Floating, |state| state.placement.mode);
        let extents = match self.clients.get(&client).map(|state| state.decoration) {
            None | Some(Decoration::Server) => frame_extents_for_mode(
                mode,
                self.config.frame_border,
                self.config.titlebar_height,
            ),
            Some(Decoration::ClientSide | Decoration::None) => [0; 4],
        };
        if !frame_extents_needs_publish(&self.frame_extents_published, client, extents) {
            return Ok(());
        }
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_frame_extents,
            AtomEnum::CARDINAL,
            &extents,
        )?;
        self.frame_extents_published.insert(client, extents);
        Ok(())
    }

    fn send_configure_notify(&self, client: Window) -> Result<(), ReplyError> {
        configure_bump!(synthetic, synthetic_total);
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        // Synthetic contract owned by configure policy: current client-root
        // geometry; the frame is never moved by a notify.
        let notify = configure::synthetic_notify_for(
            state.outer.x,
            state.outer.y,
            state.outer.width.max(1) as i32,
            state.outer.height.max(1) as i32,
            self.extents(),
        );
        let event = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: client,
            window: client,
            above_sibling: 0,
            x: clamp_i16(notify.x),
            y: clamp_i16(notify.y),
            width: clamp_u16(notify.w),
            height: clamp_u16(notify.h),
            border_width: 0,
            override_redirect: false,
        };
        self.conn
            .send_event(false, client, EventMask::STRUCTURE_NOTIFY, event)?;
        Ok(())
    }

    fn clear_frame_shape(
        &self,
        frame: Window,
        outer: crate::geometry::Rect,
        floating: bool,
    ) -> Result<(), ReplyError> {
        use x11rb::protocol::shape::{SK as ShapeSk, SO as ShapeOp};
        let rects = crate::frame::geometry_shape::bounding_rectangles(
            outer,
            floating,
            self.decoration_tokens.style,
        );
        self.conn.shape_rectangles(
            ShapeOp::SET,
            ShapeSk::BOUNDING,
            ClipOrdering::UNSORTED,
            frame,
            0,
            0,
            &rects,
        )?;
        Ok(())
    }

    /// Native EWMH icon only. A missing/invalid native property leaves an
    /// empty slot while the catalog fallback is resolved asynchronously.
    fn read_native_frame_icon(
        &self,
        client: Window,
    ) -> (Option<chrome::IconImage>, Option<String>) {
        match self.read_net_wm_icon(client) {
            Ok(Some(icon)) => (Some(icon), None),
            Ok(None) => (None, Some("native-icon-absent".to_owned())),
            Err(reason) => (None, Some(reason.to_owned())),
        }
    }

    fn read_net_wm_icon(&self, client: Window) -> Result<Option<chrome::IconImage>, &'static str> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.net_wm_icon,
                AtomEnum::CARDINAL,
                0,
                u32::MAX,
            )
            .map_err(|_| "get-property-failed")?
            .reply()
            .map_err(|_| "get-property-reply-failed")?;
        let cardinals = reply.value32().map_or_else(Vec::new, Iterator::collect);
        if cardinals.is_empty() {
            return Ok(None);
        }
        chrome::parse_net_wm_icon(&cardinals)
            .map(Some)
            .ok_or("invalid-payload")
    }

    /// Parsed `WM_CLASS` identity (instance, class) for one client. The raw
    /// property holds two NUL-separated Latin-1 strings: instance first,
    /// class second. Cached on the owner at manage time; paint never reads
    /// the property.
    fn wm_identity(&self, client: Window) -> (String, String) {
        let raw = self
            .conn
            .get_property(false, client, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.value)
            .unwrap_or_default();
        parse_wm_class(&raw)
    }

    fn read_title(&self, client: Window) -> Result<String, ReplyError> {
        let utf8 = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.net_wm_name,
                self.atoms.utf8_string,
                0,
                1024,
            )?
            .reply()?;
        if !utf8.value.is_empty() {
            return Ok(String::from_utf8_lossy(&utf8.value)
                .trim_matches('\0')
                .to_owned());
        }
        let legacy = self
            .conn
            .get_property(false, client, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?
            .reply()?;
        Ok(String::from_utf8_lossy(&legacy.value)
            .trim_matches('\0')
            .to_owned())
    }

    fn read_transient_for(&self, client: Window) -> Result<Option<Window>, ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                0,
                1,
            )?
            .reply()?;
        Ok(reply
            .value32()
            .and_then(|mut values| values.next())
            .filter(|window| *window != 0))
    }

    fn client_for(&self, window: Window) -> Option<Window> {
        if self.clients.contains_key(&window) {
            Some(window)
        } else if let Some(client) = self.frame_to_client.get(&window).copied() {
            Some(client)
        } else if let Some(target) = self.registry.lookup_by_xid(window) {
            Some(target.client)
        } else {
            None
        }
    }

    fn live_pointer_grab_target(&self, requested: Window) -> Option<Window> {
        let client = self.frame_to_client.get(&requested).copied()?;
        let state = self.clients.get(&client)?;
        let resources = self.registry.lookup_by_client(client)?;
        (state.frame == resources.frame && pointer_grab_target(requested, resources))
            .then_some(requested)
    }

    fn read_hints(&self, client: Window) -> ClientSizeHints {
        self.conn
            .get_property(
                false,
                client,
                AtomEnum::WM_NORMAL_HINTS,
                AtomEnum::ANY,
                0,
                18,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(|iter| iter.collect::<Vec<_>>()))
            .map_or_else(ClientSizeHints::default, |values| {
                ClientSizeHints::parse(&values)
            })
    }
}

fn select_output_for_rect(outputs: &[OutputWorkArea], rect: Rect) -> Option<usize> {
    let center_x = rect.x.saturating_add(i32::try_from(rect.width / 2).unwrap_or(i32::MAX));
    let center_y = rect.y.saturating_add(i32::try_from(rect.height / 2).unwrap_or(i32::MAX));
    if let Some(index) = outputs.iter().position(|output| {
        center_x >= output.geometry.x && center_x < output.geometry.right()
            && center_y >= output.geometry.y && center_y < output.geometry.bottom()
    }) {
        return Some(index);
    }
    let mut best: Option<(usize, u64)> = None;
    for (index, output) in outputs.iter().enumerate() {
        let area = rect_overlap_area(rect, output.geometry);
        if area > 0 && best.map_or(true, |(_, score)| area > score) {
            best = Some((index, area));
        }
    }
    best.map(|(index, _)| index).or_else(|| outputs.iter().position(|output| output.primary))
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
    u64::try_from(width).unwrap_or(0).saturating_mul(u64::try_from(height).unwrap_or(0))
}

/// Workspace root for the canonical indexed `IconResolver` (packaged icon
/// assets live under `<workspace>/assets`). Pure path probe for resolver
/// construction; no icon decode happens here.
pub(crate) fn flamewm_workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// True when a `_NET_CURRENT_DESKTOP` value differs from the WM's own current
/// workspace, i.e. the change came from another client rather than our publish.
fn desktop_change_is_external(published: Option<u32>, own_current: usize) -> bool {
    published.map_or(true, |value| value as usize != own_current)
}

/// Payload of `_NET_DESKTOP_NAMES` for `count` workspaces: NUL-terminated names.
fn desktop_names_payload(count: usize) -> Vec<u8> {
    let mut names = Vec::new();
    for index in 0..count {
        names.extend_from_slice(format!("Desktop {}", index + 1).as_bytes());
        names.push(0);
    }
    names
}

/// True when `_NET_NUMBER_OF_DESKTOPS` or `_NET_DESKTOP_NAMES` differ from what
/// we last published; an identical echo of our own write must not advance the fence.
fn workspace_metadata_is_external(
    count: Option<u32>,
    names: &[u8],
    own_count: usize,
    own_names: &[u8],
) -> bool {
    count.map_or(true, |value| value as usize != own_count) || names != own_names
}

/// Windows need re-publishing only when a client's visibility flipped across a
/// workspace switch or the focused window changed; a plain workspace change
/// emits workspace state only.
fn workspace_switch_touches_windows(visibility: &[(bool, bool)], focus_changed: bool) -> bool {
    focus_changed || visibility.iter().any(|(old, new)| old != new)
}

#[cfg(test)]
mod tests {

    #[test]
    fn desktop_configure_is_forced_to_bottom_and_keeps_geometry() {
        let raise_request = ConfigureWindowAux::new()
            .x(0)
            .y(0)
            .width(1350)
            .height(641)
            .sibling(0x200000)
            .stack_mode(StackMode::ABOVE);
        let aux = desktop_stack_policy(raise_request, WindowKind::Desktop);
        assert_eq!(aux.stack_mode, Some(StackMode::BELOW));
        assert_eq!(aux.sibling, None);
        assert_eq!((aux.x, aux.y, aux.width, aux.height), (Some(0), Some(0), Some(1350), Some(641)));
        // A desktop geometry-only request (no stack mask) still ends at the bottom.
        let geometry_only = ConfigureWindowAux::new().width(1350).height(600);
        assert_eq!(
            desktop_stack_policy(geometry_only, WindowKind::Desktop).stack_mode,
            Some(StackMode::BELOW)
        );
    }

    #[test]
    fn non_desktop_configure_keeps_stack_intent() {
        let request = ConfigureWindowAux::new()
            .sibling(0x200000)
            .stack_mode(StackMode::ABOVE);
        for kind in [WindowKind::Normal, WindowKind::Utility, WindowKind::Dock, WindowKind::Popup] {
            let aux = desktop_stack_policy(request, kind);
            assert_eq!(aux.stack_mode, Some(StackMode::ABOVE), "{kind:?}");
            assert_eq!(aux.sibling, Some(0x200000), "{kind:?}");
        }
    }

    #[test]
    fn destroyed_window_races_are_not_fatal_but_other_errors_are() {
        use x11rb::x11_utils::X11Error;
        let error = |kind: ErrorKind| {
            ReplyOrIdError::X11Error(X11Error {
                error_kind: kind,
                error_code: 3,
                sequence: 1,
                bad_value: 0,
                minor_opcode: 0,
                major_opcode: 20,
                extension_name: None,
                request_name: Some("GetProperty"),
            })
        };
        assert!(is_destroyed_window_race(&error(ErrorKind::Window)));
        assert!(is_destroyed_window_race(&error(ErrorKind::Drawable)));
        assert!(!is_destroyed_window_race(&error(ErrorKind::Access)));
        assert!(!is_destroyed_window_race(&ReplyOrIdError::IdsExhausted));
    }
    use super::*;
    use crate::frame::model::ResizeEdges;
    use crate::frame::session::{MoveAnchor, MoveSession};

    #[test]
    fn focus_reverts_to_pointer_root_so_root_shortcut_grabs_survive_focus_loss() {
        assert_eq!(FOCUS_REVERT_TO, InputFocus::POINTER_ROOT);
    }

    #[test]
    fn wm_class_parse_characterization_trims_and_keeps_empty_tokens() {
        assert_eq!(
            parse_wm_class(b"firefox\0Firefox\0"),
            ("firefox".to_owned(), "Firefox".to_owned())
        );
        assert_eq!(
            parse_wm_class(b" app \0class\0"),
            ("app".to_owned(), "class".to_owned())
        );
        assert_eq!(
            parse_wm_class(b"firefox\0Firefox\0\0\0"),
            ("firefox".to_owned(), "Firefox".to_owned())
        );
        assert_eq!(
            parse_wm_class(b"firefox"),
            ("firefox".to_owned(), String::new())
        );
        assert_eq!(parse_wm_class(b""), (String::new(), String::new()));
        assert_eq!(
            parse_wm_class(b"\xffapp\0Cl\xe9ss\0"),
            ("\u{FFFD}app".to_owned(), "Cl\u{FFFD}ss".to_owned())
        );
    }

    fn test_icon_key(name: &str) -> flamewm_integrations_linux::icons::IconKey {
        use flamewm_integrations_linux::icons::{IconKey, IconLookupPurpose, IconRequest, IconSize};
        IconKey::new(
            IconLookupPurpose::Application,
            IconRequest::name(name),
            IconSize::new(24, 24),
            7,
            0,
        )
    }

    #[test]
    fn catalog_result_requires_matching_client_lifetime_and_full_icon_key() {
        let key = test_icon_key("terminal");
        assert!(icon_completion_matches(42, Some(&key), 42, &key));
        assert!(!icon_completion_matches(43, Some(&key), 42, &key));
        assert!(!icon_completion_matches(42, Some(&test_icon_key("other")), 42, &key));
        let mut stale_theme = key.clone();
        stale_theme.theme_generation += 1;
        assert!(!icon_completion_matches(42, Some(&stale_theme), 42, &key));
        assert!(!icon_completion_matches(42, None, 42, &key));
    }

    #[test]
    fn deferred_high_icon_work_is_bounded_to_one_current_job_per_client() {
        use flamewm_integrations_linux::icon_service::{IconJob, IconPriority};
        let mut deferred = HashMap::new();
        retain_deferred_icon_job(
            &mut deferred,
            5,
            10,
            IconJob { key: test_icon_key("old"), priority: IconPriority::High },
        );
        retain_deferred_icon_job(
            &mut deferred,
            5,
            11,
            IconJob { key: test_icon_key("new"), priority: IconPriority::High },
        );
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred.get(&5).map(|(token, job)| (*token, job.key.clone())),
            Some((11, test_icon_key("new"))));
    }

    fn output(id: &str, geometry: Rect, work_area: Rect, primary: bool) -> OutputWorkArea {
        OutputWorkArea { output: flamewm_api::OutputId::new(id), geometry, work_area, primary }
    }

    #[test]
    fn local_output_selection_handles_adjacent_vertical_gap_and_spanning_windows() {
        let outputs = vec![
            output("left", Rect::new(0, 0, 1000, 800), Rect::new(0, 0, 1000, 756), true),
            output("right", Rect::new(1000, 0, 1200, 900), Rect::new(1000, 0, 1200, 845), false),
            output("above", Rect::new(0, -700, 900, 700), Rect::new(0, -700, 900, 656), false),
        ];
        assert_eq!(select_output_for_rect(&outputs, Rect::new(1100, 40, 100, 100)), Some(1));
        assert_eq!(select_output_for_rect(&outputs, Rect::new(50, -650, 200, 200)), Some(2));
        assert_eq!(select_output_for_rect(&outputs, Rect::new(900, 100, 200, 100)), Some(1));
        assert_eq!(select_output_for_rect(&outputs, Rect::new(2300, 100, 100, 100)), Some(0));
    }

    #[test]
    fn cross_output_window_selects_output_containing_centre_for_unequal_layouts() {
        // Horizontal: 1920x1080 left, 1280x720 right offset down by 180.
        let horizontal = vec![
            output("left", Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1050), true),
            output("right", Rect::new(1920, 180, 1280, 720), Rect::new(1920, 180, 1280, 690), false),
        ];
        // 400x300 window straddling x=1920 with centre (2020, 330) on the right output.
        assert_eq!(select_output_for_rect(&horizontal, Rect::new(1820, 180, 400, 300)), Some(1));
        // Centre (1900, 330) stays on the left output even though the window spills right.
        assert_eq!(select_output_for_rect(&horizontal, Rect::new(1700, 180, 400, 300)), Some(0));

        // Negative-coordinate: 1280x720 left at x=-1280, 1920x1080 primary right.
        let negative = vec![
            output("left", Rect::new(-1280, 0, 1280, 720), Rect::new(-1280, 0, 1280, 690), false),
            output("right", Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1050), true),
        ];
        assert_eq!(select_output_for_rect(&negative, Rect::new(-500, 100, 400, 300)), Some(0));
        assert_eq!(select_output_for_rect(&negative, Rect::new(-100, 100, 400, 300)), Some(1));
        // The selected output's work area (panel strut) is the one used for maximize/snap.
        assert_eq!(
            negative[select_output_for_rect(&negative, Rect::new(-500, 100, 400, 300)).unwrap()].work_area,
            Rect::new(-1280, 0, 1280, 690)
        );
        assert_eq!(
            horizontal[select_output_for_rect(&horizontal, Rect::new(1820, 180, 400, 300)).unwrap()].work_area,
            Rect::new(1920, 180, 1280, 690)
        );
        // Vertical: right output stacked above at negative y.
        let vertical = vec![
            output("below", Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1050), true),
            output("above", Rect::new(0, -720, 1280, 720), Rect::new(0, -720, 1280, 690), false),
        ];
        assert_eq!(select_output_for_rect(&vertical, Rect::new(100, -600, 300, 200)), Some(1));
        assert_eq!(select_output_for_rect(&vertical, Rect::new(100, -200, 300, 400)), Some(0));
    }

    #[test]
    fn output_reconciliation_drops_unplugged_and_accepts_replugged_geometry() {
        let mut outputs = vec![output("panel", Rect::new(0, 0, 100, 100), Rect::new(0, 0, 100, 56), true)];
        assert_eq!(select_output_for_rect(&outputs, Rect::new(20, 20, 10, 10)), Some(0));
        outputs.clear();
        assert_eq!(select_output_for_rect(&outputs, Rect::new(20, 20, 10, 10)), None);
        outputs.push(output("panel", Rect::new(0, 0, 100, 100), Rect::new(0, 0, 100, 45), true));
        assert_eq!(outputs[0].work_area.height, 45);
    }

    #[test]
    fn next_wait_timeout_is_none_without_pending_deadline() {
        assert_eq!(next_wait_timeout(1_000, None), None);
    }

    #[test]
    fn next_wait_timeout_returns_remaining_time_until_deadline() {
        assert_eq!(
            next_wait_timeout(1_000, Some(1_500)),
            Some(std::time::Duration::from_millis(500))
        );
    }

    #[test]
    fn next_wait_timeout_clamps_past_deadline_to_zero() {
        assert_eq!(
            next_wait_timeout(2_000, Some(1_500)),
            Some(std::time::Duration::ZERO)
        );
        assert_eq!(
            next_wait_timeout(1_500, Some(1_500)),
            Some(std::time::Duration::ZERO)
        );
    }

    fn observed_release_mapping(target: SnapTarget) -> ReleaseAction {
        release_action(target)
    }

    fn observed_release_action(
        session: InteractionSession,
        mode: PlacementMode,
        target: SnapTarget,
    ) -> ReleaseAction {
        let move_session = match session {
            InteractionSession::Move(session) => Some(session),
            _ => None,
        };
        release_action_for_move(move_session, Some(mode), target)
    }

    fn pending_move() -> InteractionSession {
        InteractionSession::Move(MoveSession {
            client: 7,
            start_pointer: RootPoint::new(520, 230),
            start_placement: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: RootRect::new(500, 200, 400, 300),
            },
            anchor: MoveAnchor::new(20, 400, 30),
            grab_window: 99,
        })
    }

    #[test]
    fn own_desktop_publish_echo_does_not_bump_workspace_revision() {
        // Echo of our own _NET_CURRENT_DESKTOP write must not advance the fence.
        assert!(!desktop_change_is_external(Some(2), 2));
        assert!(!desktop_change_is_external(Some(0), 0));
        // A foreign pager/client change must still advance the fence.
        assert!(desktop_change_is_external(Some(1), 2));
        // Unreadable property is treated as external (conservative fence advance).
        assert!(desktop_change_is_external(None, 2));
    }

    #[test]
    fn own_workspace_metadata_publish_echo_does_not_bump_workspace_revision() {
        let names = desktop_names_payload(2);
        // Echo of our own publish (count + names) must not advance the fence.
        assert!(!workspace_metadata_is_external(Some(2), &names, 2, &names));
        // A foreign count or name change must still advance the fence.
        assert!(workspace_metadata_is_external(Some(3), &names, 2, &names));
        assert!(workspace_metadata_is_external(
            Some(2),
            b"Web\0Mail\0",
            2,
            &names
        ));
        // Unreadable count is treated as external (conservative fence advance).
        assert!(workspace_metadata_is_external(None, &names, 2, &names));
    }

    #[test]
    fn workspace_switch_marks_windows_only_on_visibility_or_focus_change() {
        assert!(!workspace_switch_touches_windows(&[], false));
        assert!(!workspace_switch_touches_windows(&[(true, true), (false, false)], false));
        assert!(workspace_switch_touches_windows(&[(true, false)], false));
        assert!(workspace_switch_touches_windows(&[(false, true)], false));
        assert!(workspace_switch_touches_windows(&[], true));
    }

    #[test]
    fn resize_from_left_preserves_right_edge() {
        let original = Rect::new(100, 100, 600, 400);
        let edges = ResizeEdges {
            left: true,
            ..ResizeEdges::default()
        };
        let resized = ResizeEdges::resized_rect(original, edges, 100, 0);
        assert_eq!(resized, Rect::new(200, 100, 500, 400));
    }

    #[test]
    fn resize_enforces_minimum_size() {
        let original = Rect::new(100, 100, 600, 400);
        let edges = ResizeEdges {
            left: true,
            top: true,
            ..ResizeEdges::default()
        };
        let resized = ResizeEdges::resized_rect(original, edges, 1_000, 1_000);
        assert_eq!(resized.width, 160);
        assert_eq!(resized.height, 96);
    }

    #[test]
    fn cursor_kind_covers_eight_directions() {
        use flamewm_render_core::CursorKind;
        let corner_nw = ResizeEdges {
            left: true,
            top: true,
            ..ResizeEdges::default()
        };
        assert_eq!(
            corner_nw.cursor_kind(),
            CursorKind::ResizeNorthWestSouthEast
        );
        let corner_se = ResizeEdges {
            right: true,
            bottom: true,
            ..ResizeEdges::default()
        };
        assert_eq!(
            corner_se.cursor_kind(),
            CursorKind::ResizeNorthWestSouthEast
        );
        let corner_ne = ResizeEdges {
            right: true,
            top: true,
            ..ResizeEdges::default()
        };
        assert_eq!(
            corner_ne.cursor_kind(),
            CursorKind::ResizeNorthEastSouthWest
        );
        let corner_sw = ResizeEdges {
            left: true,
            bottom: true,
            ..ResizeEdges::default()
        };
        assert_eq!(
            corner_sw.cursor_kind(),
            CursorKind::ResizeNorthEastSouthWest
        );
        let west = ResizeEdges {
            left: true,
            ..ResizeEdges::default()
        };
        assert_eq!(west.cursor_kind(), CursorKind::ResizeHorizontal);
        let east = ResizeEdges {
            right: true,
            ..ResizeEdges::default()
        };
        assert_eq!(east.cursor_kind(), CursorKind::ResizeHorizontal);
        let north = ResizeEdges {
            top: true,
            ..ResizeEdges::default()
        };
        assert_eq!(north.cursor_kind(), CursorKind::ResizeVertical);
        let south = ResizeEdges {
            bottom: true,
            ..ResizeEdges::default()
        };
        assert_eq!(south.cursor_kind(), CursorKind::ResizeVertical);
        assert_eq!(ResizeEdges::default().cursor_kind(), CursorKind::Default);
    }

    #[test]
    fn start_menu_toggle_flag_defaults_to_false() {
        assert!(!WmChangeSet::default().start_menu_toggle);
        let mut changes = WmChangeSet::default();
        changes.start_menu_toggle = true;
        let drained = changes.take();
        assert!(drained.start_menu_toggle);
        assert!(!changes.start_menu_toggle);
    }

    /// Replays one event batch: events in order, then the batch-boundary commit.
    /// Returns how many times Start toggled in that batch.
    fn run_batch(tap: &mut SuperTap, events: &[(&str, u8, u32)]) -> usize {
        let mut toggles = 0;
        for &(kind, keycode, time) in events {
            let outcome = match kind {
                "super-press" => tap.press_super(keycode, time),
                "super-release" => {
                    tap.release_super(keycode, time);
                    SuperTapOutcome::Idle
                }
                "other-press" => tap.press_other(),
                other => panic!("unknown test event kind {other}"),
            };
            toggles += usize::from(outcome == SuperTapOutcome::Toggle);
        }
        toggles + usize::from(tap.commit() == SuperTapOutcome::Toggle)
    }

    const SUPER_KEY: u8 = 133;
    const RIGHT_KEY: u8 = 114;

    #[test]
    fn super_tap_toggles_exactly_once() {
        let mut tap = SuperTap::default();
        assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
        assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 150)]), 1);
        assert_eq!(tap.commit(), SuperTapOutcome::Idle);
    }

    #[test]
    fn super_autorepeat_pairs_while_held_never_toggle() {
        let mut tap = SuperTap::default();
        assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
        // Each repeat is a KeyRelease and KeyPress sharing one timestamp.
        for stamp in [200, 225, 250] {
            let batch = [
                ("super-release", SUPER_KEY, stamp),
                ("super-press", SUPER_KEY, stamp),
            ];
            assert_eq!(run_batch(&mut tap, &batch), 0, "repeat at {stamp}");
        }
        // The final real release is the single toggle for the whole hold.
        assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 400)]), 1);
    }

    #[test]
    fn super_press_with_no_release_yet_is_a_repeat_not_a_new_tap() {
        let mut tap = SuperTap::default();
        assert_eq!(
            run_batch(
                &mut tap,
                &[
                    ("super-press", SUPER_KEY, 100),
                    ("super-press", SUPER_KEY, 125),
                    ("super-press", SUPER_KEY, 150),
                ],
            ),
            0
        );
        assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 175)]), 1);
    }

    #[test]
    fn super_right_chord_never_toggles() {
        let mut tap = SuperTap::default();
        assert_eq!(
            run_batch(
                &mut tap,
                &[
                    ("super-press", SUPER_KEY, 100),
                    ("other-press", RIGHT_KEY, 120),
                    ("super-release", SUPER_KEY, 150),
                ],
            ),
            0
        );
        // The Right key's own autorepeat must not start a tap either.
        assert_eq!(
            run_batch(
                &mut tap,
                &[
                    ("other-press", RIGHT_KEY, 200),
                    ("other-press", RIGHT_KEY, 225),
                ],
            ),
            0
        );
    }

    #[test]
    fn ctrl_super_right_never_starts_a_tap() {
        // Ctrl is a plain other-key press; Super with Ctrl held is not bare.
        let mut tap = SuperTap::default();
        assert_eq!(
            run_batch(
                &mut tap,
                &[
                    ("other-press", 37, 100),
                    ("other-press", SUPER_KEY, 110),
                    ("other-press", RIGHT_KEY, 120),
                    ("other-press", RIGHT_KEY, 150),
                ],
            ),
            0
        );
        assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 200)]), 0);
    }

    #[test]
    fn tap_followed_by_other_key_in_same_batch_still_toggles_once() {
        let mut tap = SuperTap::default();
        assert_eq!(
            run_batch(
                &mut tap,
                &[
                    ("super-press", SUPER_KEY, 100),
                    ("super-release", SUPER_KEY, 150),
                    ("other-press", RIGHT_KEY, 160),
                ],
            ),
            1
        );
    }

    #[test]
    fn focus_loss_while_held_cancels_the_tap() {
        let mut tap = SuperTap::default();
        assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
        tap.focus_lost();
        assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 150)]), 0);
    }

    #[test]
    fn focus_loss_after_release_cancels_pending_commit() {
        let mut tap = SuperTap::default();
        assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
        tap.release_super(SUPER_KEY, 150);
        tap.focus_lost();
        assert_eq!(tap.commit(), SuperTapOutcome::Idle);
    }

    #[test]
    fn two_separate_taps_toggle_twice() {
        let mut tap = SuperTap::default();
        assert_eq!(
            run_batch(
                &mut tap,
                &[
                    ("super-press", SUPER_KEY, 100),
                    ("super-release", SUPER_KEY, 150),
                    ("super-press", SUPER_KEY, 300),
                    ("super-release", SUPER_KEY, 350),
                ],
            ),
            2
        );
    }

    #[test]
    fn bare_state_ignores_lock_bits_only() {
        assert!(is_bare_state(0));
        assert!(is_bare_state(0x0002)); // CapsLock
        assert!(is_bare_state(0x0010)); // NumLock
        assert!(!is_bare_state(0x0040)); // Super held as modifier
        assert!(!is_bare_state(0x0004)); // Ctrl
    }

    #[test]
    fn change_set_drains_once_per_turn() {
        let mut changes = WmChangeSet::default();
        assert!(!changes.any());
        changes.windows = true;
        changes.work_area = true;
        let drained = changes.take();
        assert!(drained.windows && drained.work_area);
        assert!(!changes.any());
    }

    #[test]
    fn frame_control_hit_targets_unchanged() {
        use crate::frame::input::frame_control_at;
        use crate::frame::model::FrameControl as EngineControl;
        let titlebar = chrome::TITLEBAR_HEIGHT;
        // Skin chrome: button_width=38, controls tile the right edge.
        // 400-wide frame: min [286,324), max [324,362), close [362,400).
        assert_eq!(
            frame_control_at(400, titlebar, 390),
            Some(EngineControl::Close)
        );
        assert_eq!(
            frame_control_at(400, titlebar, 360),
            Some(EngineControl::MaximizeRestore)
        );
        assert_eq!(
            frame_control_at(400, titlebar, 328),
            Some(EngineControl::MaximizeRestore)
        );
        assert_eq!(frame_control_at(400, titlebar, 100), None);
        assert_eq!(
            frame_control_at(400, titlebar, 362),
            Some(EngineControl::Close)
        );
        assert_eq!(
            frame_control_at(400, titlebar, 324),
            Some(EngineControl::MaximizeRestore)
        );
        assert_eq!(
            frame_control_at(400, titlebar, 286),
            Some(EngineControl::Minimize)
        );
        assert_eq!(frame_control_at(400, titlebar, 285), None);
    }

    #[test]
    fn frame_event_source_prefers_live_registered_child() {
        let mut registry = FrameRegistry::new();
        registry.register(crate::frame::resources::FrameResources::new(
            1,
            2,
            3,
            [4, 5, 6],
            [7, 8, 9, 10, 11, 12, 13, 14],
        ));
        assert_eq!(resolve_frame_event_source(&registry, 99, 7), 7);
        assert_eq!(resolve_frame_event_source(&registry, 99, 0), 99);
        assert_eq!(resolve_frame_event_source(&registry, 99, 42), 99);
    }

    #[test]
    fn pointer_grab_target_accepts_live_frame_or_input_child() {
        let resources = crate::frame::resources::FrameResources::new(
            1,
            2,
            3,
            [4, 5, 6],
            [7, 8, 9, 10, 11, 12, 13, 14],
        );
        assert!(pointer_grab_target(2, resources));
        assert!(pointer_grab_target(7, resources));
        assert!(!pointer_grab_target(99, resources));
    }

    // CONTRACT-REGRESSION T10: release mapping is total and typed.
    // TRIGGER: release policy receives each snap candidate, including None.
    // OBSERVABLE RESULT: None is no action, Maximize stays Maximize, and the
    // eight half/quarter candidates become their matching engine snaps.
    // UNCOVERED GAP: release mapping collapses Maximize and None to Left.
    // FAILURE MUTATION: retain a Maximize/None -> Left fallback arm.
    // REPRESENTATIVE CASE: the complete release-candidate mapping table.
    #[test]
    fn t10_release_action_mapping_is_total() {
        use crate::frame::model::SnapTarget as EngineSnapTarget;

        let cases = [
            (SnapTarget::None, ReleaseAction::None),
            (SnapTarget::Maximize, ReleaseAction::Maximize),
            (
                SnapTarget::LeftHalf,
                ReleaseAction::Snap(EngineSnapTarget::Left),
            ),
            (
                SnapTarget::RightHalf,
                ReleaseAction::Snap(EngineSnapTarget::Right),
            ),
            (
                SnapTarget::TopHalf,
                ReleaseAction::Snap(EngineSnapTarget::Top),
            ),
            (
                SnapTarget::BottomHalf,
                ReleaseAction::Snap(EngineSnapTarget::Bottom),
            ),
            (
                SnapTarget::TopLeftQuarter,
                ReleaseAction::Snap(EngineSnapTarget::TopLeft),
            ),
            (
                SnapTarget::TopRightQuarter,
                ReleaseAction::Snap(EngineSnapTarget::TopRight),
            ),
            (
                SnapTarget::BottomLeftQuarter,
                ReleaseAction::Snap(EngineSnapTarget::BottomLeft),
            ),
            (
                SnapTarget::BottomRightQuarter,
                ReleaseAction::Snap(EngineSnapTarget::BottomRight),
            ),
        ];
        let mismatches = cases
            .into_iter()
            .filter_map(|(target, expected)| {
                let actual = observed_release_mapping(target);
                (actual != expected).then_some((target, expected, actual))
            })
            .collect::<Vec<_>>();
        assert!(
            mismatches.is_empty(),
            "release mapping mismatches: {mismatches:?}"
        );
    }

    // CONTRACT-REGRESSION T11: an unactivated move cannot snap on release.
    // TRIGGER: title press arms Move, then release reaches an edge before any
    // activating motion crosses the threshold.
    // OBSERVABLE RESULT: release eligibility is None; no Snap action exists.
    // UNCOVERED GAP: release_from_move currently treats a pending Move as
    // activated and evaluates the edge target anyway.
    // FAILURE MUTATION: admit every InteractionSession::Move at release.
    // REPRESENTATIVE CASE: floating title press/release with LeftHalf target.
    #[test]
    fn t11_unactivated_move_release_is_not_snap_eligible() {
        let session = pending_move();
        assert!(matches!(session, InteractionSession::Move(_)));
        assert_eq!(
            observed_release_action(session, PlacementMode::Floating, SnapTarget::LeftHalf),
            ReleaseAction::None
        );
    }

    #[test]
    fn grab_reaches_dismissed_counter_static_shape() {
        // Serial-owner static check: switch path must carry the split
        // metadata/current pair plus the renamed counters/spans without
        // touching query_tree or client-list publish.
        let src = include_str!("wm.rs");
        assert!(src.contains("fn publish_workspace_metadata("));
        assert!(src.contains("fn publish_current_workspace("));
        assert!(src.contains("\"wm.workspace.switch.total\""));
        assert!(src.contains("\"wm.workspace.switch.visibility\""));
        assert!(src.contains("\"wm.workspace.switch.ewmh_current\""));
        assert!(src.contains("\"wm.workspace.switch.shell_publish\""));
        assert!(src.contains("\"wm.workspace.query_tree.count\""));
        assert!(src.contains("NotifyClientRoot"));
        assert!(src.contains("ConfigureClientLocal"));
        assert!(src.contains("ConfigureFrameRoot"));
        assert!(src.contains("with_screen_rect"));
    }

    #[test]
    fn next_workspace_clamps_without_wrap() {
        assert_eq!(next_workspace(0, 3, -1), None);
        assert_eq!(next_workspace(2, 3, 1), None);
        assert_eq!(next_workspace(0, 3, 1), Some(1));
        assert_eq!(next_workspace(2, 3, -1), Some(1));
        assert_eq!(next_workspace(0, 1, 1), None);
        assert_eq!(next_workspace(0, 1, -1), None);
    }
}

#[cfg(test)]
mod transition_fix_tests {
    use super::*;
    use crate::frame::coords::RootRect;

    fn maximized(resume: Option<PlacementSnapshot>, restore: RootRect) -> PlacementState {
        PlacementState {
            mode: PlacementMode::Maximized,
            current: RootRect::new(0, 0, 1920, 1080),
            floating_restore: restore,
            resume,
        }
    }

    #[test]
    fn unmaximize_target_returns_floating_rect_for_maximized_client() {
        let floating = RootRect::new(500, 200, 400, 300);
        let snap = PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: floating,
        };
        assert_eq!(unmaximize_target(&maximized(Some(snap), floating)), Some(floating));
        assert_eq!(unmaximize_target(&maximized(None, floating)), Some(floating));
    }

    #[test]
    fn unmaximize_target_is_none_when_not_maximized_or_not_floating_restore() {
        let floating = RootRect::new(500, 200, 400, 300);
        let mut floating_state = maximized(None, floating);
        floating_state.mode = PlacementMode::Floating;
        assert_eq!(unmaximize_target(&floating_state), None);
        let fullscreen_snap = PlacementSnapshot {
            mode: PlacementMode::Fullscreen,
            rect: floating,
        };
        assert_eq!(unmaximize_target(&maximized(Some(fullscreen_snap), floating)), None);
    }

    #[test]
    fn expected_unmap_is_consumed_exactly_once() {
        let mut ignore = 1u8;
        assert!(consume_expected_unmap(&mut ignore));
        assert!(!consume_expected_unmap(&mut ignore), "no count left for a real unmap");
    }

    /// Models the frame's UnmapNotify stream. Minimize hides the frame and counts one expected
    /// unmap; restore maps the frame (no UnmapNotify). A later client-initiated unmap must
    /// reach the WM. Before the fix, restore's extra count swallowed that unmap.
    #[test]
    fn restore_does_not_leave_an_ignore_count_that_swallows_a_later_unmap() {
        let mut ignore_unmap = 0u8;
        // minimize: WM unmaps the frame and expects exactly one UnmapNotify.
        ignore_unmap = ignore_unmap.saturating_add(1);
        assert!(consume_expected_unmap(&mut ignore_unmap), "minimize unmap is ours");
        // restore: WM maps the frame; mapping produces no UnmapNotify, so no count is added.
        assert_eq!(ignore_unmap, 0);
        // later: the client withdraws its own window; this unmap must not be swallowed.
        assert!(!consume_expected_unmap(&mut ignore_unmap), "client unmap is handled");
    }
}

/// `_NET_WM_MOVERESIZE` direction (EWMH 0 = top-left, clockwise) to frame edges.
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
mod csd_direction_tests {
    use super::*;

    #[test]
    fn moveresize_directions_map_clockwise_from_top_left() {
        // EWMH _NET_WM_MOVERESIZE: 0 TL, 1 T, 2 TR, 3 R, 4 BR, 5 B, 6 BL, 7 L.
        assert_eq!(resize_edges_for_direction(0), ResizeEdges::top_left());
        assert_eq!(resize_edges_for_direction(1), ResizeEdges::top());
        assert_eq!(resize_edges_for_direction(2), ResizeEdges::top_right());
        assert_eq!(resize_edges_for_direction(3), ResizeEdges::right());
        assert_eq!(resize_edges_for_direction(4), ResizeEdges::bottom_right());
        assert_eq!(resize_edges_for_direction(5), ResizeEdges::bottom());
        assert_eq!(resize_edges_for_direction(6), ResizeEdges::bottom_left());
        assert_eq!(resize_edges_for_direction(7), ResizeEdges::left());
    }

    #[test]
    fn moveresize_edge_mapping_rejects_plain_move() {
        // Negative control: direction 8 (move) must not be a resize edge.
        assert_eq!(resize_edges_for_direction(8), ResizeEdges::left());
    }

    #[test]
    fn click_on_inactive_client_requests_focus_and_active_client_does_not() {
        let client: Window = 0x400002;
        assert!(super::should_focus_on_press(None, client));
        assert!(super::should_focus_on_press(Some(0x400001), client));
        // Negative control: a plain click on the already-active window must not refocus/restack.
        assert!(!super::should_focus_on_press(Some(client), client));
    }
}

#[cfg(test)]
mod frame_extents_publish_tests {
    use super::*;

    /// Mirrors `set_frame_extents`: a publish is sent only when the value differs.
    fn publish_all(values: &[[u32; 4]]) -> usize {
        let mut published = HashMap::new();
        let mut sent = 0;
        for &extents in values {
            if frame_extents_needs_publish(&published, 7, extents) {
                sent += 1;
                published.insert(7, extents);
            }
        }
        sent
    }

    #[test]
    fn first_publish_always_sends_and_identical_repeats_are_suppressed() {
        let floating = [1, 1, 25, 1];
        let published: HashMap<Window, [u32; 4]> = HashMap::new();
        assert!(frame_extents_needs_publish(&published, 7, floating));
        assert_eq!(publish_all(&[floating, floating, floating, floating]), 1);
    }

    #[test]
    fn any_change_republishes_including_border_mode_switch() {
        let floating = [1, 1, 25, 1];
        let snapped = [0, 0, 25, 0];
        let client_side = [0, 0, 0, 0];
        // Floating -> snapped (border 1 -> 0) -> floating -> client-side decoration.
        assert_eq!(publish_all(&[floating, snapped, floating, client_side]), 4);
        // Titlebar-only change must also be detected (no border-only shortcut).
        assert_eq!(publish_all(&[floating, [1, 1, 0, 1], [1, 1, 0, 1]]), 2);
    }

    #[test]
    fn frame_extents_follow_effective_border_per_mode() {
        let (border, titlebar) = (1, 31);
        let floating = [1, 1, 32, 1];
        let snapped = [0, 0, 31, 0];
        assert_eq!(frame_extents_for_mode(PlacementMode::Floating, border, titlebar), floating);
        assert_eq!(
            frame_extents_for_mode(PlacementMode::Snapped(crate::frame::model::SnapTarget::Left), border, titlebar),
            snapped
        );
        assert_eq!(frame_extents_for_mode(PlacementMode::Maximized, border, titlebar), snapped);
        assert_eq!(frame_extents_for_mode(PlacementMode::Fullscreen, border, titlebar), snapped);
        // Unsnap returns to the floating border and titlebar extents.
        assert_eq!(frame_extents_for_mode(PlacementMode::Floating, border, titlebar), floating);
    }

    /// Client interior per mode: the published EWMH side/bottom extents equal
    /// the effective X border (the only side inset), and the client rect sits
    /// at (0, titlebar) spanning the full frame width. No side inset leaks
    /// into the client for snapped/maximized/fullscreen frames.
    #[test]
    fn client_interior_matches_effective_border_per_mode() {
        let outer = crate::frame::coords::RootRect { x: 10, y: 20, w: 800, h: 600 };
        let titlebar: u16 = 31;
        let configured: u16 = 1;
        let modes = [
            PlacementMode::Floating,
            PlacementMode::Snapped(crate::frame::model::SnapTarget::TopLeft),
            PlacementMode::Maximized,
            PlacementMode::Fullscreen,
        ];
        for mode in modes {
            let border = frame_border_for_mode(mode, configured);
            let [left, right, top, bottom] = frame_extents_for_mode(mode, configured, titlebar);
            assert_eq!(left, u32::from(border), "{mode:?} left extent");
            assert_eq!(right, u32::from(border), "{mode:?} right extent");
            assert_eq!(bottom, u32::from(border), "{mode:?} bottom extent");
            assert_eq!(top, u32::from(titlebar) + u32::from(border), "{mode:?} top extent");
            let ext = FrameExtents::new(i32::from(titlebar), i32::from(border));
            let (x, y, w, h) = lifecycle::client_configure(outer, ext);
            assert_eq!((x, y), (0, i32::from(titlebar)), "{mode:?} client origin");
            assert_eq!(w, outer.w as u32, "{mode:?} client width equals frame width");
            assert_eq!(h, (outer.h - i32::from(titlebar)) as u32, "{mode:?} client height");
        }
        // Client-drawn side insets must never appear for borderless modes.
        let snapped = FrameExtents::new(i32::from(titlebar), 0);
        assert_eq!(lifecycle::client_configure(outer, snapped).2, 800);
    }

    /// The ConfigureFrameRoot arm commits the border/geometry, then publishes
    /// the effective extents through the single owner, `set_frame_extents`.
    #[test]
    fn configure_frame_root_arm_publishes_extents_after_commit() {
        let src = include_str!("wm.rs");
        let start = src
            .find("FrameEffect::ConfigureFrameRoot { x, y, w, h } => {")
            .expect("ConfigureFrameRoot arm");
        let end = start
            + src[start..]
                .find("FrameEffect::MoveFrameRoot")
                .expect("arm following ConfigureFrameRoot");
        let arm = &src[start..end];
        let commit = arm
            .find("self.commit_frame_rect(client, x, y, w, h);")
            .expect("arm commits frame rect");
        let publish = arm
            .find("self.set_frame_extents(client)?;")
            .expect("arm publishes frame extents");
        assert!(publish > commit, "extents must publish after the rect is committed");
    }
}
