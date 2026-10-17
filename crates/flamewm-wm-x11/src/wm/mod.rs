use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::animation::{self, Animator, Begin, CaptureCache, Effect, OpenGate, Settings, Tick};
use crate::frame::geometry::keep_on_outputs;
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

/// Emits a `restore.trace.*` event whose message is prefixed with wall-clock `t_ms=`.
fn trace_restore(id: &'static str, detail: impl FnOnce() -> String) {
    flamewm_debug::emit(flamewm_debug::DebugEventId(id), std::time::Duration::ZERO, || {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        format!("t_ms={ms} {}", detail())
    });
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
    ACTION_SESSION_LOCK, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE,
    ACTION_WINDOW_MINIMIZE, ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP,
};
use flamewm_platform::services::workspaces::{count_store_path, load_persisted_count};
use flamewm_platform::shortcuts::{ACTION_VOLUME_DOWN, ACTION_VOLUME_MUTE, ACTION_VOLUME_UP};
use flamewm_platform::system::MediaAudioKey;
use flamewm_window_core::{
    SnapTarget, snap_geometry as core_snap_geometry, snap_target_in_output as core_snap_target_in_output,
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

/// Maps a workspace shortcut action to its step through the desktop list.
fn workspace_action_delta(action: &str) -> Option<isize> {
    match action {
        ACTION_WORKSPACE_LEFT | ACTION_WORKSPACE_UP => Some(-1),
        ACTION_WORKSPACE_RIGHT | ACTION_WORKSPACE_DOWN => Some(1),
        _ => None,
    }
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
mod workspace_count_tests {
    use super::WmConfig;

    #[test]
    fn persisted_count_applies_only_without_env_override() {
        let base = WmConfig::default();
        assert_eq!(base.resolve_workspaces(false, Some(7)).workspaces, 7);
        assert_eq!(base.resolve_workspaces(true, Some(7)).workspaces, 4);
        assert_eq!(base.resolve_workspaces(false, None).workspaces, 4);
    }

    #[test]
    fn start_path_reads_persisted_count_from_settings_dir() {
        let dir = std::env::temp_dir().join(format!("flamewm-wm-start-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let settings = dir.join("settings.toml");
        std::fs::write(dir.join("workspaces.conf"), "workspaces.count=6\n").expect("write");
        let config = WmConfig::default().with_persisted_workspaces(&settings);
        assert_eq!(config.workspaces, 6);
        assert_eq!(WmConfig::default().resolve_workspaces(true, Some(6)).workspaces, 4);
        let _ = std::fs::remove_dir_all(&dir);
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

fn bump(counter: &std::sync::atomic::AtomicU64) {
    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn read(counter: &std::sync::atomic::AtomicU64) -> u64 {
    counter.load(std::sync::atomic::Ordering::Relaxed)
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
        bump(&counters.$total);
    }};
}

#[allow(clippy::too_many_arguments)]
fn emit_configure_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_CONFIGURE_SUMMARY,
        std::time::Duration::from_secs(1),
        || {
            let counters = configure_counters();
            format!(
                "requests={} accepted={} refused_interactive={} refused_mode={} notify_frame={} notify_client={} expected={} mismatch={} feedback={} synthetic={}",
                read(&counters.request_received_total),
                read(&counters.request_accepted_total),
                read(&counters.request_refused_interactive_total),
                read(&counters.request_refused_mode_total),
                read(&counters.notify_frame_total),
                read(&counters.notify_client_total),
                read(&counters.notify_expected_total),
                read(&counters.notify_mismatch_total),
                read(&counters.feedback_total),
                read(&counters.synthetic_total),
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
            bump(&self.snap_target_changes);
        }
    }

    fn record_preview(&self) {
        self.preview.increment();
        bump(&self.preview_updates);
    }

    fn record_toggle_max(&self) {
        bump(&self.toggle_max_total);
    }

    fn record_client_message_maximize(&self) {
        bump(&self.client_message_maximize_total);
    }

    fn record_placement_transition(&self, before: PlacementMode, after: PlacementMode) {
        if before == after {
            return;
        }
        bump(&self.state_mutations_total);
        if matches!(
            (before, after),
            (PlacementMode::Maximized, _) | (_, PlacementMode::Maximized)
        ) {
            bump(&self.maximized_transitions);
        }
    }

    fn record_resize_snap_blocked(&self) {
        bump(&self.resize_snap_blocked_total);
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

fn drag_activated(session: &InteractionSession) -> bool {
    matches!(session, InteractionSession::Move(move_session) if move_session.anchor.is_activated())
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
    let commit = commit.map(|commit| {
        let effects = join_effects(&commit.effects);
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
    effects.iter().map(|effect| format!("{effect:?}")).collect::<Vec<_>>().join(",")
}

fn emit_frame_dispatch(detail: impl FnOnce() -> String) {
    flamewm_debug::emit(flamewm_debug::WM_FRAME_EVENT_DISPATCH, std::time::Duration::ZERO, detail);
}

fn pointer_ingress(event: &Event) -> Option<PointerIngress> {
    let ingress = |kind, event, child, root_x, root_y| PointerIngress { kind, event, child, root_x, root_y };
    match event {
        Event::ButtonPress(e) => Some(ingress("ButtonPress", e.event, e.child, e.root_x, e.root_y)),
        Event::ButtonRelease(e) => Some(ingress("ButtonRelease", e.event, e.child, e.root_x, e.root_y)),
        Event::MotionNotify(e) => Some(ingress("MotionNotify", e.event, e.child, e.root_x, e.root_y)),
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
    conn.randr_select_input(
        screen.root,
        randr::NotifyMask::SCREEN_CHANGE | randr::NotifyMask::OUTPUT_CHANGE | randr::NotifyMask::CRTC_CHANGE,
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

mod manage;
mod configure_handlers;
mod effects;
mod paint;
mod publish;
mod events;
mod pointer;
mod window_ops;
mod transition;
mod overlay;
mod workspace;
mod client_state;

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
            let _ = renderer
                .borrow_mut()
                .define_cursor(u64::from(screen.root), flamewm_render_core::CursorKind::Default);
        }
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
            controller_geometry: ControllerGeometryCache::default(),
            display_topology_dirty: TopologyDirty::default(),
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
            match self.icon_service.borrow().submit(job) {
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
        match self.icon_service.borrow().submit(job) {
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
                emit_frame_dispatch(|| {
                    format!(
                        "probe=pointer_ingress stage=pre_handle_event boundary=first raw={} event={} child={} root=({}, {}) session_client={session_client:?} pump_received={received}",
                        pointer.kind,
                        pointer.event,
                        pointer.child,
                        pointer.root_x,
                        pointer.root_y
                    )
                });
            }
            "MotionNotify" => {
                self.pointer_ingress_probe.last_motion = Some(pointer);
                if !self.pointer_ingress_probe.motion_dispatched {
                    self.pointer_ingress_probe.motion_dispatched = true;
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

    /// Consumes the RandR topology-change flag; true when the display snapshot must be refreshed.
    pub(crate) fn take_display_topology_dirty(&mut self) -> bool {
        self.display_topology_dirty.take()
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
        work_area_for_rect_in(&self.output_work_areas, self.work_area(), rect)
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

    fn controller_for(&mut self, client: Window) -> Option<ControllerState> {
        let state = self.clients.get(&client)?;
        let res = self.registry.lookup_by_client(client)?;
        let session = self.session_of(client);
        let children = res.children();
        let regions = children.map(|xid| self.registry.lookup_by_xid(xid).map(|target| target.region));
        let outputs = self.output_work_areas.iter().map(|output| to_root_rect(output.geometry));
        let (region_map, outputs) = {
            let geometry = self.controller_geometry.fetch(client, children, regions, outputs);
            (geometry.region_map.clone(), geometry.outputs.clone())
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
                {
                    let work = self.work_area_for_rect(state.outer);
                    RootRect::new(work.x, work.y, work.width.max(1) as i32, work.height.max(1) as i32)
                },
                screen_rect,
                state.hints,
                self.extents_for(client),
                session,
                region_map,
            )
            .with_outputs(outputs),
        );
        if let Some(controller) = controller.as_mut() {
            controller.last_title_click = last_title_click;
        }
        controller
    }

    /// Output geometries in root coordinates: the bound for drags (windows may overflow the work area).
    fn output_root_rects(&self) -> Vec<RootRect> {
        self.output_work_areas.iter().map(|output| to_root_rect(output.geometry)).collect()
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
}

/// Work area of the output holding `rect`, else the global work area.
fn work_area_for_rect_in(outputs: &[OutputWorkArea], global: Rect, rect: Rect) -> Rect {
    select_output_for_rect(outputs, rect).map_or(global, |index| outputs[index].work_area)
}

/// Target frame rect for a managed client after the output layout changes.
/// Floating windows keep a quarter visible on the outputs (may overflow the work area);
/// Snapped and Maximized keep work-area semantics.
fn rehome_outer(mode: PlacementMode, outer: Rect, outputs: &[OutputWorkArea], global: Rect) -> Rect {
    match mode {
        PlacementMode::Fullscreen => outer,
        PlacementMode::Maximized => work_area_for_rect_in(outputs, global, outer),
        PlacementMode::Snapped(_) => {
            outer.clamp_inside(work_area_for_rect_in(outputs, global, outer))
        }
        PlacementMode::Floating => {
            let rects: Vec<RootRect> = outputs.iter().map(|output| to_root_rect(output.geometry)).collect();
            from_root_rect(keep_on_outputs(to_root_rect(outer), &rects, to_root_rect(global)))
        }
    }
}

fn to_root_rect(rect: Rect) -> RootRect {
    RootRect::new(
        rect.x,
        rect.y,
        i32::try_from(rect.width).unwrap_or(i32::MAX),
        i32::try_from(rect.height).unwrap_or(i32::MAX),
    )
}

fn from_root_rect(rect: RootRect) -> Rect {
    Rect::new(rect.x, rect.y, rect.w.max(0) as u32, rect.h.max(0) as u32)
}

/// Stacking for a dock: directly above `frame`, the normal window just raised to the top.
fn dock_above_frame_aux(frame: Window) -> ConfigureWindowAux {
    ConfigureWindowAux::new().sibling(frame).stack_mode(StackMode::ABOVE)
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
        names.extend_from_slice(format!("Workspace {}", index + 1).as_bytes());
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
mod tests;

/// Controller geometry derived from live registry and output state.
struct ControllerGeometry {
    client: Window,
    children: [Window; 12],
    regions: [Option<FrameRegion>; 12],
    region_map: HashMap<u32, FrameRegion>,
    outputs: Vec<RootRect>,
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
                region_map,
                outputs: outputs.collect(),
            });
        }
        self.entry.as_ref().expect("entry populated above")
    }
}

#[cfg(test)]
mod controller_geometry_tests {
    use super::*;

    fn children(base: u32) -> [u32; 12] {
        std::array::from_fn(|i| base + i as u32)
    }

    fn regions() -> [Option<FrameRegion>; 12] {
        std::array::from_fn(|i| (i == 8).then_some(FrameRegion::TitleDrag))
    }

    fn outputs(width: i32) -> [RootRect; 1] {
        [RootRect::new(0, 0, width, 1080)]
    }

    #[test]
    fn unchanged_motions_rebuild_once() {
        let mut cache = ControllerGeometryCache::default();
        for _ in 0..1000 {
            let geometry = cache.fetch(7, children(100), regions(), outputs(1920).into_iter());
            assert_eq!(geometry.region_map.get(&108), Some(&FrameRegion::TitleDrag));
            assert_eq!(geometry.region_map.len(), 1);
            assert_eq!(geometry.outputs, outputs(1920));
        }
        assert_eq!(cache.rebuilds, 1);
    }

    #[test]
    fn client_region_or_output_change_rebuilds() {
        let mut cache = ControllerGeometryCache::default();
        cache.fetch(7, children(100), regions(), outputs(1920).into_iter());
        cache.fetch(8, children(100), regions(), outputs(1920).into_iter());
        assert_eq!(cache.rebuilds, 2);
        cache.fetch(8, children(200), regions(), outputs(1920).into_iter());
        assert_eq!(cache.rebuilds, 3);
        let mut moved = regions();
        moved[8] = None;
        let geometry = cache.fetch(8, children(200), moved, outputs(1920).into_iter());
        assert!(geometry.region_map.is_empty());
        assert_eq!(cache.rebuilds, 4);
        cache.fetch(8, children(200), moved, outputs(1280).into_iter());
        assert_eq!(cache.rebuilds, 5);
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
        let src = WM_SOURCE;
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
