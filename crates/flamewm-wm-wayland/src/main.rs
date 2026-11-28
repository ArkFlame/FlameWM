//! `FlameWM` Wayland compositor: nested under a host session via the winit backend, or on a tty via
//! the `drm` backend (cargo feature `drm`), with `XWayland`.
//!
//! Environment:
//! - `FLAMEWM_WAYLAND_BACKEND` (`drm`/`native` or `winit`/`nested`): backend choice. When unset, a
//!   build with the `drm` feature runs the native backend and every other build runs nested. The
//!   choice never comes from `DISPLAY`/`WAYLAND_DISPLAY`: a session wrapper states its intent.
//! - `FLAMEWM_WAYLAND_SOCKET` (default `wayland-flame`): socket name under `XDG_RUNTIME_DIR`.
//! - `FLAMEWM_WAYLAND_AUTOSTART` (colon-separated programs): started once `XWayland` is ready, with
//!   `DISPLAY` pointing at `XWayland`. The session wrapper passes the shell and desktop clients.
//! - `FLAMEWM_WAYLAND_DUMP=<file.ppm>`: write one framebuffer readback after
//!   `FLAMEWM_WAYLAND_DUMP_AFTER_MS` (default 2000); with `FLAMEWM_WAYLAND_EXIT_AFTER_DUMP=1`
//!   the compositor exits right after the dump (used by the headless evidence run).
//!   `FLAMEWM_WAYLAND_DUMP_POINTER=<x>,<y>` places the pointer there half a second before the dump,
//!   so the frame shows the cursor the client under it asked for.
//! - `FLAMEWM_PROFILE_COMPOSE=1`: print one `FLAMEWM-WAYLAND-COMPOSE` window per second to stderr
//!   with the thread's span shares (Wayland dispatch, control/D-Bus, input, render, timers,
//!   `XWayland`), then the profiler report. Off by default; no timer, no clock read.
//!
//! There is no headless render backend, by design of the stack: smithay 0.7 ships only the winit,
//! x11 and drm/udev backends, its `EGLDisplay` is built from a native display handle, and this
//! crate's render path is `GlowRenderer`-typed throughout, so every frame presents through a winit
//! window. An active-use sample without a host session therefore runs that window on a private
//! Xvfb (`scripts/compose-scenario --xvfb`), where the swap is a local copy instead of a host round
//! trip. Offscreen rendering exists only as the one-shot readback below (`FLAMEWM_WAYLAND_DUMP`).
//!
//! Two non-compositing entry points exist for packaging and diagnostics:
//! - `--capabilities`: machine-readable build facts on stdout, exit 0. Touches no bus, no `/dev`
//!   and nothing on `PATH`, so the packaging gate can run it before installing dependencies.
//! - `--probe`: read-only prerequisite report, exit 0 when every required prerequisite exists.
//!
//! Startup diagnostics: every stage writes
//! `FLAMEWM-WAYLAND-STARTUP stage=<name> status=<ok|fail|skip> code=<0..8> detail=<tokens> t=<ms>ms`
//! to stderr (the session wrapper redirects that into its log and its failure report). `t` is
//! milliseconds since this process started, so a slow stage is visible without a trace tool. A
//! startup that dies before `stage=ready` also emits `stage=ready status=fail`, which is what the
//! wrapper's exit watchdog keys on; a normal exit emits nothing extra.
//!
//! Three markers say three different things about the native backend, and only the last one is
//! readiness:
//! * `stage=ready status=ok code=6 detail=backend_init` — the event loop is running (socket,
//!   `XWayland`, sources installed). It says nothing about the panel.
//! * `stage=firstframe status=ok code=7` — a flip was queued to KMS. Diagnostic only: a queued flip
//!   can still fail, leaving a black panel under a compositor that holds DRM master.
//! * `stage=presented status=ok code=8` — the first flip actually completed. That is the only
//!   evidence a pixel reached the screen, and on the native backend it is also the moment the seat
//!   is grabbed (see `startup` and `drm::Drm::note_presented`).
//!
//! The tty backend paints its first frame inside its own `init`, so on the native backend these two
//! lines come *before* `stage=ready`. A session that reports `ready` but neither of them is the
//! black-VT case; a session that reports `firstframe` but not `presented` is the black-panel case, and
//! the backend's own bounded deadline turns it into a reported failure instead of a dead VT.
//!
//! The native backend also prints one `flamewm-wayland: startup timing session=Nms gpu=Nms gbm=Nms
//! egl=Nms renderer=Nms scan=Nms queue=Nms present=Nms` line right after the presented frame (or, on a
//! failed startup, with the steps that completed and `none` for a boundary that was never reached).
//! Every init step carries its own elapsed milliseconds in the `detail` of its stage line, so the next
//! hardware log localizes a slowdown without a trace tool.
//!
//! # Session activation
//!
//! A session manager may start this process before the VT and the seat are active: at that moment the
//! greeter still owns the tty, and logind reports the session as not yet `Active`. The tty backend
//! therefore activates the session itself (`Session.Activate`, plus `Seat.SwitchTo(XDG_VTNR)`), waits
//! for `Active` with a budget of ~5 s and FAILS the open when it stays inactive: taking DRM master on
//! an inactive session blanks the VT, presents no frame and cannot be escaped, so a reported startup
//! failure is strictly better than a dead black screen. `stage=seat` is what reports this; a session
//! that never becomes active fails with `reason=inactive_after_activate`.
//!
//! `XWayland` is optional. If it cannot be spawned the compositor keeps running as a pure Wayland
//! compositor (`stage=xwayland status=fail`), no X11 client is served and the X11 shell/desktop
//! autostart is skipped instead of aborting the session.

mod chrome_buttons;
mod clip;
// Matrix model: settings -> State -> colour_pass composite pass.
mod colour_filter;
mod colour_pass;
mod control;
mod cursor;
mod debug_outputs;
mod dmabuf;
#[cfg(feature = "drm")]
#[allow(unsafe_code)]
// EGL display and renderer creation are unsafe in smithay; no other module may.
mod drm;
mod font;
mod handlers;
mod input;
mod zoom;
// Compiled without the `drm` feature too: the D-Bus surface and the `Session` impl are the only
// feature-gated parts, which keeps the pause/resume state machine testable on any host.
#[path = "protocols/clipboard.rs"]
mod clipboard;
#[path = "protocols/compat.rs"]
mod compat;
mod lock_cover;
mod lock_global;
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
mod logind;
mod model;
mod output_layout;
mod presentation_scale;
mod probe;
mod profile;
mod render;
mod scene;
mod screencopy;
mod session_lock;
mod snap_preview;
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
// The native backend's first-frame gate; pure, so it is compiled (and tested) without a GPU.
mod startup;
mod state;
mod transient;
mod virtual_output;
mod xwayland;

// Fault-injection tests for the backend fallback decision; the file exists before this declaration.
#[cfg(test)]
mod backend_fallback_tests;

use std::cell::RefCell;
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use flamewm_api::graphics::{DisplayProtocol, choose_provider};
use flamewm_render_core::RenderAcceleration;
use flamewm_session_core::diagnostics::{
    fallback_log_path, guard, install_panic_hook, session_log_path,
};
use rustix::process::getuid;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::winit::{WinitEvent, init_from_attributes};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::reexports::winit::dpi::PhysicalSize;
use smithay::reexports::winit::window::WindowAttributes;
use smithay::utils::Transform;
use smithay::wayland::socket::ListeningSocketSource;

use state::{ClientState, State};

/// Which compositor backend this process runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// Tty backend: logind session, DRM/KMS, libinput.
    Native,
    /// Window on the host session (winit).
    Nested,
}

impl Backend {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Nested => "nested",
        }
    }
}

impl Default for Backend {
    /// The backend a build would choose with no override.
    fn default() -> Self {
        if cfg!(feature = "drm") {
            Self::Native
        } else {
            Self::Nested
        }
    }
}

/// Backend selection. Only the explicit override and the compiled feature decide; there is no
/// inference from `DISPLAY` or `WAYLAND_DISPLAY` anywhere in this crate.
pub(crate) fn select_backend(explicit: Option<&str>) -> Backend {
    match explicit.map(str::trim).filter(|value| !value.is_empty()) {
        Some("drm" | "native") => Backend::Native,
        Some("winit" | "nested") => Backend::Nested,
        Some(_unknown) => Backend::default(),
        None => Backend::default(),
    }
}

/// Outcome of a backend init attempt in the native → nested → headless fallback chain. The step is
/// part of the variant so the fallback rule is a pure mapping, not a read of process state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendFallbackOutcome {
    /// Native (tty) init succeeded; no fallback is needed.
    NativeOk,
    /// Native init failed; the nested backend is the next step.
    NativeFailed,
    /// Nested (winit) init succeeded after a native failure.
    NestedOk,
    /// Nested init failed after a native failure; only the headless step is left.
    NestedFailed,
}

/// The step the compositor runner takes after a backend init outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NextBackend {
    /// Keep the backend that is already running; start nothing else.
    Keep,
    /// Attempt the nested winit backend.
    Nested,
    /// Keep the Wayland socket alive with no rendering backend.
    Headless,
}

/// Pure fallback rule: an init outcome in, the next backend out. It does no I/O and mutates no
/// state; the caller starts the returned backend and appends the record for the stage that failed.
pub(crate) fn next_backend(outcome: BackendFallbackOutcome) -> NextBackend {
    match outcome {
        BackendFallbackOutcome::NativeOk => NextBackend::Keep,
        BackendFallbackOutcome::NativeFailed => NextBackend::Nested,
        BackendFallbackOutcome::NestedOk => NextBackend::Keep,
        BackendFallbackOutcome::NestedFailed => NextBackend::Headless,
    }
}

/// A native request in a build without the `drm` feature is a recoverable init failure, not a
/// reason to end the session: the nested backend can still own a window and serve the socket.
pub(crate) fn native_unavailable_falls_back(backend: Backend, drm_compiled: bool) -> bool {
    backend == Backend::Native && !drm_compiled
}

/// The four native prerequisites that can be missing when the tty backend starts. Every field is an
/// observation made by the caller; the chooser itself never touches a device, bus or environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeAvailability {
    pub(crate) drm_device: bool,
    pub(crate) logind_seat: bool,
    pub(crate) drm_master: bool,
    pub(crate) input_devices: bool,
}

impl NativeAvailability {
    /// All prerequisites present: nothing to recover from.
    pub(crate) const fn complete() -> Self {
        Self {
            drm_device: true,
            logind_seat: true,
            drm_master: true,
            input_devices: true,
        }
    }

    /// Maps the stage a native init failed at onto the prerequisite it could not obtain. The other
    /// facts stay at their complete values: the caller only claims the gap it actually observed.
    pub(crate) fn from_failed_stage(stage: &str) -> Self {
        let mut facts = Self::complete();
        match stage {
            "seat" => facts.logind_seat = false,
            "drm" | "outputs" => {
                facts.drm_device = false;
                facts.drm_master = false;
            }
            "libinput" => facts.input_devices = false,
            _ => facts.drm_device = false,
        }
        facts
    }
}

/// What the runner does with a native start that lost a prerequisite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeFallbackDecision {
    /// Nothing missing: keep the tty backend that is already running.
    KeepNative,
    /// A recoverable hardware gap: start the nested winit window instead.
    NestedWinit,
    /// No logind seat means there is no session to hand back; exit with this clear code.
    ExitWithCode(i32),
}

/// Exit code for a native start with no logind seat. Distinct from the generic startup error (1) so
/// a session wrapper can tell "no seat" from every other failure.
pub(crate) const EXIT_NO_SEAT: i32 = 2;

/// Pure fallback chooser: availability facts in, one decision out. No I/O, no process state and no
/// panic path, so every combination of missing resources has a defined, testable outcome.
pub(crate) fn choose_native_fallback(facts: NativeAvailability) -> NativeFallbackDecision {
    if facts == NativeAvailability::complete() {
        return NativeFallbackDecision::KeepNative;
    }
    if !facts.logind_seat {
        return NativeFallbackDecision::ExitWithCode(EXIT_NO_SEAT);
    }
    NativeFallbackDecision::NestedWinit
}

/// The `--capabilities` payload. Exactly three lines, in this order, never embellished: the
/// packaging gate greps `drm_compiled=true` literally.
pub(crate) fn capabilities_text() -> String {
    format!(
        "flamewm-capabilities=1\ndrm_compiled={}\nbackend={}\n",
        cfg!(feature = "drm"),
        Backend::default().name()
    )
}

/// Startup stages, in the order they must succeed. The stage index is the `code=` of the structured
/// line, so a log reader can order stages without parsing names.
///
/// `Firstframe` and `Presented` are the two exceptions to "in order": both are reported by the tty
/// backend while it builds its first frame, which happens inside the backend's own init and therefore
/// before `Ready` (emitted by `run` once the socket and `XWayland` exist). They keep the next free codes
/// so no existing code number moves.
///
/// They are *not* the same event and must never be read as one:
/// * `Firstframe` (7) means a flip was queued to KMS. Diagnostic only — a queued flip can still fail,
///   which leaves a black panel with a compositor that owns DRM master.
/// * `Presented` (8) means the first flip *completed*. This is the readiness signal, the point where
///   the tty backend grabs the seat and cancels its own startup deadline.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Seat = 0,
    Drm = 1,
    Outputs = 2,
    Libinput = 3,
    Socket = 4,
    Xwayland = 5,
    Ready = 6,
    Firstframe = 7,
    Presented = 8,
}

impl Stage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Seat => "seat",
            Self::Drm => "drm",
            Self::Outputs => "outputs",
            Self::Libinput => "libinput",
            Self::Socket => "socket",
            Self::Xwayland => "xwayland",
            Self::Ready => "ready",
            Self::Firstframe => "firstframe",
            Self::Presented => "presented",
        }
    }

    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Seat => 0,
            Self::Drm => 1,
            Self::Outputs => 2,
            Self::Libinput => 3,
            Self::Socket => 4,
            Self::Xwayland => 5,
            Self::Ready => 6,
            Self::Firstframe => 7,
            Self::Presented => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageStatus {
    Ok,
    Fail,
    Skip,
}

impl StageStatus {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// Bit per [`Stage`], so each stage reports exactly once even when several modules contribute.
static REPORTED_STAGES: AtomicU8 = AtomicU8::new(0);

/// Longest `detail` field; longer messages are cut instead of dumping state.
const MAX_DETAIL: usize = 64;

/// How long a presented session may go without `XWayland` becoming ready before it gives the VT back.
///
/// Ordered after the tty backend's 8 s first-frame deadline (a failed flip reports first) and before
/// the session wrapper's 25 s readiness timeout, so this compositor-side gate is the one that names
/// the failure instead of a wrapper kill.
const CONTENT_DEADLINE_MS: u64 = 15_000;

/// Start instant of this process, for the `t=` field of every stage line. A stage that takes seconds
/// (DRM master, libinput, EGL, `XWayland`) is then visible in the log and in the session wrapper's
/// report without a trace tool.
static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since this process started; 0 until [`START`] is initialised.
pub(crate) fn elapsed_ms() -> u128 {
    START.get().map_or(0, |start| start.elapsed().as_millis())
}

/// Per-step monotonic timings of the tty backend startup, in milliseconds.
///
/// Reported exactly once by [`StartupTiming::emit`]: right after the `stage=presented` line of a
/// successful start, or on a failed startup with the steps that completed (`queue=none present=none`).
/// `egl_ms` covers the EGL display and its context. This is the line that localizes a slow or wedged
/// hardware start without a trace tool, and it is the reason each init step also carries its own
/// `elapsed_<n>ms` token in the `detail` of its stage line.
///
/// `queue_ms` and `present_ms` are two different facts: the first is when a flip reached KMS, the
/// second when one came back. A `queue` with no `present` is the black-panel case.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupTiming {
    /// logind session open and activation.
    pub session_ms: u128,
    /// logind `TakeDevice` plus DRM master on the chosen card.
    pub gpu_ms: u128,
    pub gbm_ms: u128,
    /// EGL display and EGL context.
    pub egl_ms: u128,
    pub renderer_ms: u128,
    /// Connector scan: every `DrmCompositor` of every output.
    pub scan_ms: u128,
    /// Time from native backend init to the first flip queued to KMS; `None` when none was queued.
    pub queue_ms: Option<u128>,
    /// Time from native backend init to the first flip that actually completed; `None` when none did.
    pub present_ms: Option<u128>,
}

#[cfg_attr(not(feature = "drm"), allow(dead_code))]
impl StartupTiming {
    /// The exact text of the one-shot summary line:
    /// `flamewm-wayland: startup timing session=Nms gpu=Nms gbm=Nms egl=Nms renderer=Nms scan=Nms
    /// queue=Nms present=Nms` (`none` for a boundary that was never reached).
    pub(crate) fn line(&self) -> String {
        format!(
            "flamewm-wayland: startup timing session={}ms gpu={}ms gbm={}ms egl={}ms renderer={}ms \
             scan={}ms queue={} present={}",
            self.session_ms,
            self.gpu_ms,
            self.gbm_ms,
            self.egl_ms,
            self.renderer_ms,
            self.scan_ms,
            step(self.queue_ms),
            step(self.present_ms)
        )
    }

    /// Emits the summary line once (see [`StartupTiming::line`]).
    pub(crate) fn emit(&self) {
        eprintln!("{}", self.line());
    }
}

/// One optional timing field: `Nms` when the step happened, `none` when it never did.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
fn step(millis: Option<u128>) -> String {
    match millis {
        Some(millis) => format!("{millis}ms"),
        None => "none".to_owned(),
    }
}

/// The exact text of one structured startup line:
/// `FLAMEWM-WAYLAND-STARTUP stage=<stage> status=<ok|fail|skip> code=<0..7> detail=<tokens> t=<ms>ms`.
///
/// `detail` is reduced to short tokens: no fds, addresses, cookies, environment dumps or file
/// contents reach the line. Split out from [`stage_line`] so the format itself is testable.
pub(crate) fn stage_text(stage: Stage, status: StageStatus, detail: &str) -> String {
    format!(
        "FLAMEWM-WAYLAND-STARTUP stage={} status={} code={} detail={} t={}ms",
        stage.name(),
        status.name(),
        stage.code(),
        detail_tokens(detail),
        elapsed_ms()
    )
}

/// The same session log the panic hook uses, so backend failures and crashes interleave in order.
fn backend_failure_log_path() -> PathBuf {
    let fallback = fallback_log_path(getuid().as_raw());
    let home = env::var_os("HOME").map(PathBuf::from);
    session_log_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
        &fallback,
    )
}

/// Appends one typed failure record for a backend init stage to the injected path. `still_works`
/// names what survives the fallback and `degraded` names what the failed stage took away; both are
/// stable subsystem ids. Taking the path as an argument keeps the write testable without touching
/// the session environment.
fn append_backend_failure_record(
    path: &std::path::Path,
    stage: &str,
    cause: impl std::fmt::Display,
    fallback_taken: bool,
) {
    append_failure_record(
        path,
        stage,
        cause,
        ["process", "session_log", "wayland_socket"],
        ["compositor_backend", "hardware_outputs", "input_devices"],
        fallback_taken,
    );
}

/// Appends one typed failure record with an explicit blast radius. A degraded-but-running session
/// can name what still works without claiming that rendering or input died.
fn append_failure_record(
    path: &std::path::Path,
    stage: &str,
    cause: impl std::fmt::Display,
    still_works: impl IntoIterator<Item = &'static str>,
    degraded: impl IntoIterator<Item = &'static str>,
    fallback_taken: bool,
) {
    use flamewm_session_core::diagnostics::{
        BlastRadius, FailureCode, FailureRecord, append_record,
    };
    let record = FailureRecord::at_now(
        stage,
        FailureCode::SubsystemError,
        cause.to_string(),
        BlastRadius::new(still_works, degraded),
        fallback_taken,
    );
    if let Err(error) = append_record(path, &record) {
        eprintln!("flamewm-wayland: failure record write failed: {error}");
    }
}

/// Production entry point: append the record to the same session log the panic hook uses.
pub(crate) fn record_backend_failure(
    stage: &str,
    cause: impl std::fmt::Display,
    fallback_taken: bool,
) {
    append_backend_failure_record(&backend_failure_log_path(), stage, cause, fallback_taken);
}

/// Appends the typed record for entering the headless step. Headless is itself a fallback taken, so
/// it is recorded with `fallback_taken=true`. Taking the path as an argument keeps it testable.
fn append_headless_entry_record(path: &std::path::Path) {
    append_backend_failure_record(
        path,
        "headless",
        "nested backend unavailable; headless/minimal compositor without outputs or input",
        true,
    );
}

/// Whether a failed startup stage leaves a fallback running. A missing logind seat exits with
/// [`EXIT_NO_SEAT`] and takes no fallback, so it records `false`; a DRM or output gap falls through
/// to the nested window and records `true`.
pub(crate) fn stage_failure_fallback(stage: Stage) -> bool {
    matches!(stage, Stage::Drm | Stage::Outputs)
}

/// Emits one structured startup line (see [`stage_text`]). Every hard failure of a backend init
/// stage also leaves a typed failure record, so a falling-back compositor is diagnosable even when
/// the wrapper throws its stderr away.
pub(crate) fn stage_line(stage: Stage, status: StageStatus, detail: &str) {
    REPORTED_STAGES.fetch_or(1u8 << stage.code(), Ordering::Relaxed);
    eprintln!("{}", stage_text(stage, status, detail));
    if status == StageStatus::Fail
        && matches!(
            stage,
            Stage::Seat | Stage::Drm | Stage::Outputs | Stage::Libinput
        )
    {
        record_backend_failure(stage.name(), detail, stage_failure_fallback(stage));
    }
}

fn stage_reported(stage: Stage) -> bool {
    REPORTED_STAGES.load(Ordering::Relaxed) & (1u8 << stage.code()) != 0
}

/// The stages a successful backend init stands for. `Stage::Libinput` is deliberately absent: the tty
/// backend grabs the seat only after the first frame has been queued, so "backend init ok" is no
/// evidence that input is up, and claiming it would hide a session that never grabbed a keyboard.
/// `Stage::Firstframe` is absent for the same reason: it is only ever reported by a real queued frame.
const BACKEND_STAGES: [Stage; 3] = [Stage::Seat, Stage::Drm, Stage::Outputs];

/// Fills the stages the backend owns but did not report itself: a successful backend init covers
/// them, and a stage never appears twice.
fn fill_backend_stages() {
    for stage in BACKEND_STAGES {
        if !stage_reported(stage) {
            stage_line(stage, StageStatus::Ok, "backend init");
        }
    }
}

fn detail_tokens(detail: &str) -> String {
    let mut tokens = String::with_capacity(detail.len().min(MAX_DETAIL));
    for ch in detail.chars() {
        if tokens.len() >= MAX_DETAIL {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            tokens.push(ch);
        } else if !tokens.ends_with('_') {
            tokens.push('_');
        }
    }
    let trimmed = tokens.trim_matches('_');
    if trimmed.is_empty() {
        "none".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Routes smithay's own diagnostics to stderr.
///
/// Smithay reports DRM, EGL/GBM, libinput and `XWayland` failures through `log`. Without a subscriber
/// every one of them is discarded, which is how a failed hardware start can leave no trace beyond
/// this crate's own stage lines. Warnings and errors only: smithay logs per-frame detail at
/// info/debug, and the `eprintln!` stage lines above stay the canonical startup record.
/// Receives smithay's `tracing` output. Without a subscriber, `tracing` forwards every span open
/// to the `log` facade at the span's own level: smithay's `warn_span!("output_bind")` then printed
/// as WARN on every client bind. This subscriber keeps events at WARN and above and records no spans.
struct StderrSubscriber;

/// Whether the `tracing` subscriber prints a callsite: warnings and errors that are events.
fn subscriber_wants(level: tracing::Level, is_span: bool) -> bool {
    !is_span && level <= tracing::Level::WARN
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

impl tracing::Subscriber for StderrSubscriber {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        subscriber_wants(*metadata.level(), metadata.is_span())
    }

    fn new_span(&self, _attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let metadata = event.metadata();
        if !self.enabled(metadata) {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        eprintln!(
            "flamewm-wayland: {}: {}:{}",
            metadata.level(),
            metadata.target(),
            visitor.0
        );
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

fn install_smithay_logger() {
    struct StderrLogger;

    static LOGGER: StderrLogger = StderrLogger;

    impl log::Log for StderrLogger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if self.enabled(record.metadata()) {
                eprintln!(
                    "flamewm-wayland: {}: {}: {}",
                    record.level(),
                    record.target(),
                    record.args()
                );
            }
        }

        fn flush(&self) {}
    }

    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
    // Replaces `tracing`'s log fallback, which forwarded span opens as WARN records.
    let _ = tracing::subscriber::set_global_default(StderrSubscriber);
}

fn main() {
    let fallback = fallback_log_path(getuid().as_raw());
    let home = env::var_os("HOME").map(PathBuf::from);
    install_panic_hook(session_log_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
        &fallback,
    ));
    // The `t=` field of every stage line is measured from here, so the timer covers argument
    // parsing, the seat, DRM init and the socket, not just the event loop.
    let _ = START.set(Instant::now());
    tune_malloc();
    install_smithay_logger();
    // Parsed before anything else and exits 0: no event loop, no socket, no seat, no bus, no PATH.
    if std::env::args().any(|arg| arg == "--capabilities") {
        print!("{}", capabilities_text());
        return;
    }
    // Diagnostics only: `--probe` reports prerequisites and exits without starting a compositor.
    if std::env::args().any(|arg| arg == "--probe") {
        std::process::exit(probe::run());
    }
    match run() {
        Ok(0) => {}
        Ok(code) => {
            abort_startup(&format!("startup aborted: exit code {code}"));
            std::process::exit(code);
        }
        Err(error) => {
            eprintln!("flamewm-wayland: {error}");
            abort_startup(&format!("startup aborted: {error}"));
            std::process::exit(1);
        }
    }
}

/// Pins glibc's mmap and trim thresholds to 128 KiB before any large buffer exists. Without this,
/// glibc raises its dynamic mmap threshold after the first freed multi-megabyte capture, so the
/// desktop backdrop and blur temporaries land in the brk heap, which fragments and keeps the
/// freed pages resident. Measured on the nested session: `[heap]` anon 45 MB -> 24 MB and
/// compositor anon 67 MB -> 49 MB, identical to `GLIBC_TUNABLES` with the same values.
#[allow(unsafe_code)] // mallopt only sets allocator tunables; called once, before threads start.
fn tune_malloc() {
    const M_TRIM_THRESHOLD: i32 = -1;
    const M_MMAP_THRESHOLD: i32 = -3;
    const THRESHOLD: i32 = 128 * 1024;
    unsafe extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }
    // SAFETY: `mallopt` only reads and writes glibc's global tunables with plain integer
    // arguments; it does not touch any pointer owned by Rust.
    unsafe {
        mallopt(M_MMAP_THRESHOLD, THRESHOLD);
        mallopt(M_TRIM_THRESHOLD, THRESHOLD);
    }
}

/// Marks the startup as aborted when the compositor never reached [`Stage::Ready`]: the session
/// wrapper's exit watchdog reports that case as a failure and needs the stage it died in. Nothing is
/// printed when the loop ran and `stage=ready` was already reported, so a normal exit stays clean.
/// Logs the saved `renderAcceleration` in the shared `RenderAcceleration` vocabulary. The compositor
/// composites through smithay's GL renderer, so the shared pixel backends do not run here yet.
fn report_render_acceleration() {
    let requested = std::fs::read_to_string(control::settings_path())
        .ok()
        .and_then(|text| {
            flamewm_platform::settings::ConfigStore::default()
                .parse_snapshot(&text)
                .ok()
        })
        .map_or(RenderAcceleration::Cpu, |settings| {
            RenderAcceleration::from_setting(&settings.render_acceleration)
        });
    eprintln!(
        "FLAMEWM_RENDER_ACCELERATION {} pixel-backends=x11-only",
        requested.as_setting()
    );
}

fn abort_startup(reason: &str) {
    if !stage_reported(Stage::Ready) {
        stage_line(Stage::Ready, StageStatus::Fail, reason);
    }
}

/// Runs a non-essential startup subsystem behind `guard`, returning `None` when it panicked so the
/// caller degrades instead of ending the session. The installed panic hook already appended the
/// `code=panic` record with this subsystem as its stage, so this only reports the degrade on stderr;
/// the event loop core is deliberately never wrapped.
fn guard_startup<T>(subsystem: &'static str, operation: impl FnOnce() -> T) -> Option<T> {
    match guard(subsystem, operation) {
        Ok(value) => Some(value),
        Err(failure) => {
            eprintln!(
                "flamewm-wayland: {subsystem} panicked ({}); continuing without it",
                failure.cause
            );
            None
        }
    }
}

/// Runs the compositor and returns its exit code.
fn run() -> Result<i32, Box<dyn Error>> {
    // Before any thread or child exists: XWayland, the session clients and the compositor's own
    // pointer read the theme from this process's environment.
    let graphics_request = flamewm_api::graphics::provider_request(
        flamewm_api::graphics::WAYLAND_PROVIDER_VARIABLE,
        flamewm_api::graphics::LEGACY_PROVIDER_VARIABLE,
    );
    let graphics = choose_provider(DisplayProtocol::Wayland, graphics_request.as_deref(), false)?;
    eprintln!(
        "FLAMEWM_RENDER_PROVIDER {} (real GL driver reported as FLAMEWM_GL_RENDERER)",
        graphics.name()
    );
    let _ = guard_startup("render-acceleration", report_render_acceleration);
    let _ = guard_startup("cursor-theme", || {
        let (theme, size) = cursor::saved_appearance();
        cursor::export_session_theme(theme.as_deref(), size);
    });
    let explicit = std::env::var("FLAMEWM_WAYLAND_BACKEND").ok();
    let backend = select_backend(explicit.as_deref());
    if let Some(requested) = explicit
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && !matches!(requested, "drm" | "native" | "winit" | "nested")
    {
        eprintln!(
            "flamewm-wayland: unknown FLAMEWM_WAYLAND_BACKEND={requested}, using {}",
            backend.name()
        );
    }
    if native_unavailable_falls_back(backend, cfg!(feature = "drm")) {
        eprintln!(
            "flamewm-wayland: the native backend needs a build with `--features drm`; falling back to nested"
        );
    }

    let socket_name =
        std::env::var("FLAMEWM_WAYLAND_SOCKET").unwrap_or_else(|_| "wayland-flame".to_owned());
    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    let _ = guard_startup("profiling", || {
        profile::init();
        profile::install_compose_dump(&handle);
    });
    let mut display: Display<State> = Display::new()?;
    let dh = display.handle();
    // calloop watches a duplicate of the display fd; the Display itself is dispatched through the
    // shared cell, which keeps this crate free of `unsafe`.
    let poll_fd = display.backend().poll_fd().try_clone_to_owned()?;
    let xwayland_dh = dh.clone();
    let mut state = State::new(
        dh,
        event_loop.get_signal(),
        socket_name.clone(),
        handle.clone(),
    )?;
    // Advertise ext-session-lock-v1 before clients connect: the lock UI is an ordinary client,
    // so this global is the only way it can paint while the session is locked.
    let _ = guard_startup("session-protocols", || {
        state.init_session_lock();
        state.init_screencopy();
    });
    // Control service: a failed bus connection (or a panic in the bus setup) leaves the compositor
    // usable without the shell API.
    if let Some(Err(error)) = guard_startup("control", || control::install(&mut state)) {
        eprintln!("flamewm-wayland: control service unavailable: {error}");
    }

    // Backend first: the seat is taken here, and a failure must not leave a half-started
    // compositor holding the session. The guard owns the backend handle and finishes it on drop,
    // which also covers the error paths below.
    // A failed native init is a recoverable BackendInitError (its stages already wrote failure
    // records): a hardware gap falls through to the nested window, then to a headless/minimal
    // compositor that still serves the socket. A missing logind seat exits with a clear code.
    let native = match backend {
        Backend::Native => match start_native(&mut state, &handle) {
            Ok(native) => Some(native),
            Err(error) => {
                // The facts come from the failed stage; the pure chooser decides between the nested
                // window and a clear exit. A missing seat cannot be recovered by a session wrapper,
                // so it exits with a distinct code instead of hiding behind a headless compositor.
                match choose_native_fallback(NativeAvailability::from_failed_stage(&error.stage)) {
                    NativeFallbackDecision::ExitWithCode(code) => {
                        eprintln!(
                            "flamewm-wayland: {error}; no logind seat, exiting with code {code}"
                        );
                        return Ok(code);
                    }
                    NativeFallbackDecision::KeepNative | NativeFallbackDecision::NestedWinit => {
                        eprintln!("flamewm-wayland: {error}; falling back to nested");
                        match next_backend(BackendFallbackOutcome::NativeFailed) {
                            NextBackend::Nested => start_nested_or_headless(&mut state, &handle),
                            NextBackend::Keep | NextBackend::Headless => None,
                        }
                    }
                }
            }
        },
        Backend::Nested => start_nested_or_headless(&mut state, &handle),
    };
    fill_backend_stages();
    // Debug-only virtual outputs (env-gated); needs the primary size the backend just set.
    let _ = guard_startup("debug-outputs", || {
        debug_outputs::install_from_env(&mut state)
    });
    // Extra headless outputs for multi-head testing (env-gated, nested backend only).
    let _ = guard_startup("virtual-outputs", || {
        virtual_output::install_from_env(&mut state)
    });

    // The socket exists before XWayland connects back to this compositor.
    let listening = match ListeningSocketSource::with_name(&socket_name) {
        Ok(listening) => {
            stage_line(Stage::Socket, StageStatus::Ok, &socket_name);
            listening
        }
        Err(error) => {
            stage_line(Stage::Socket, StageStatus::Fail, &format!("{error}"));
            return Err(error.into());
        }
    };
    handle.insert_source(listening, |client, (), state| {
        if let Err(error) = state
            .display
            .insert_client(client, Arc::new(ClientState::default()))
        {
            eprintln!("flamewm-wayland: client rejected: {error}");
        }
    })?;
    let display = Rc::new(RefCell::new(display));
    let dispatch_display = Rc::clone(&display);
    handle.insert_source(
        Generic::new(poll_fd, Interest::READ, Mode::Level),
        move |_, _, state| {
            let _span = profile::wayland_dispatch();
            dispatch_display.borrow_mut().dispatch_clients(state)?;
            Ok(PostAction::Continue)
        },
    )?;

    // XWayland reads WAYLAND_DISPLAY, so it starts after the socket exists; it is never gated on a
    // vblank, and autostart still fires from the XWayland ready handler.
    //
    // X11 is optional: a session without XWayland still serves Wayland clients, so a spawn failure is
    // reported and survived instead of aborting a compositor that already owns the seat. Autostart
    // lives in the XWayland ready handler (`State::xwayland_ready`), which can never run without a
    // client, so the list is dropped here to make the skip explicit.
    let xwayland_client = match guard_startup("xwayland", || {
        xwayland::spawn_xwayland(&xwayland_dh, &handle)
    }) {
        Some(Ok(client)) => {
            stage_line(Stage::Xwayland, StageStatus::Ok, "spawned");
            Some(client)
        }
        Some(Err(error)) => {
            stage_line(
                Stage::Xwayland,
                StageStatus::Fail,
                &xwayland_unavailable_detail(&error),
            );
            eprintln!(
                "flamewm-wayland: XWayland unavailable ({error}); continuing without X11 clients"
            );
            state.autostart.clear();
            None
        }
        None => {
            stage_line(Stage::Xwayland, StageStatus::Fail, "panic");
            state.autostart.clear();
            None
        }
    };

    if let Some(path) = std::env::var_os("FLAMEWM_WAYLAND_DUMP") {
        state.exit_after_dump = std::env::var_os("FLAMEWM_WAYLAND_EXIT_AFTER_DUMP").is_some();
        let after = std::env::var("FLAMEWM_WAYLAND_DUMP_AFTER_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(2000);
        let path = PathBuf::from(path);
        let mut place_pointer =
            std::env::var("FLAMEWM_WAYLAND_DUMP_POINTER")
                .ok()
                .and_then(|value| {
                    let (x, y) = value.split_once(',')?;
                    Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))
                });
        // Re-armed until the frame is written: a redraw request is only honoured once the host
        // delivers a frame callback, so a single shot can be lost.
        handle.insert_source(
            Timer::from_duration(Duration::from_millis(after)),
            move |_, (), state| {
                if state.dumped {
                    return TimeoutAction::Drop;
                }
                if let Some((x, y)) = place_pointer.take() {
                    // The client under the pointer answers the enter with its cursor; give it time.
                    state.on_motion((x, y).into(), 0);
                    state.request_redraw();
                    return TimeoutAction::ToDuration(Duration::from_millis(500));
                }
                state.dump = Some(path.clone());
                state.request_redraw();
                TimeoutAction::ToDuration(Duration::from_millis(250))
            },
        )?;
    }

    // Nested measurement: `FLAMEWM_WAYLAND_SCRIPT_DRAG=x0,y0,x1,y1,steps,start_ms,total_ms` presses
    // the titlebar at (x0,y0) after `start_ms`, moves to (x1,y1) in `steps` equal motion events
    // spread over `total_ms`, releases, and prints the backdrop captures made during the drag. It is
    // inert when unset and is read only here, so the tty backend never runs it.
    if let Some(spec) = std::env::var("FLAMEWM_WAYLAND_SCRIPT_DRAG")
        .ok()
        .and_then(|value| {
            let n: Vec<f64> = value
                .split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect();
            (n.len() == 7).then_some(n)
        })
    {
        let (x0, y0, x1, y1) = (spec[0], spec[1], spec[2], spec[3]);
        let steps = spec[4].max(1.0) as u32;
        let (start, total) = (spec[5] as u64, spec[6] as u64);
        let mut step = 0_u32;
        let mut captures_at_start = 0_u64;
        let mut began_at = std::time::Instant::now();
        let mut begun = false;
        handle.insert_source(
            Timer::from_duration(Duration::from_millis(start)),
            move |_, (), state| {
                if !begun {
                    begun = state.script_drag_begin(x0, y0);
                    if !begun {
                        eprintln!("SCRIPT_DRAG no titlebar at ({x0},{y0})");
                        return TimeoutAction::Drop;
                    }
                    captures_at_start = render::backdrop_captures();
                    render::stage_timers::reset();
                    began_at = std::time::Instant::now();
                }
                step += 1;
                let t = f64::from(step) / f64::from(steps);
                state.on_motion((x0 + (x1 - x0) * t, y0 + (y1 - y0) * t).into(), step);
                state.request_redraw();
                if step >= steps {
                    state.grab = None;
                    eprintln!(
                        "SCRIPT_DRAG steps={steps} planned_ms={total} elapsed_ms={} backdrop_captures={} {}",
                        began_at.elapsed().as_millis(),
                        render::backdrop_captures() - captures_at_start,
                        render::stage_timers::report()
                    );
                    return TimeoutAction::Drop;
                }
                TimeoutAction::ToDuration(Duration::from_millis((total / u64::from(steps)).max(1)))
            },
        )?;
    }

    // A completed flip is not a desktop. The first frame is painted inside backend init, before
    // XWayland exists, and contains only the clear colour and the cursor; XWayland readiness is what
    // actually puts windows on screen. If autostart clients are configured but XWayland never
    // becomes ready, the VT holds a near-black frame forever with no way forward, so end the run:
    // `NativeSession::drop` releases DRM master and logind control, `main` exits non-zero and the
    // session wrapper reports the failure, which returns the greeter instead of a dead black VT.
    // A session with no autostart clients (a nested dev run, or one that deliberately skipped
    // XWayland) is not waiting for a desktop and arms no deadline.
    if !state.autostart.is_empty() {
        handle.insert_source(
            Timer::from_duration(Duration::from_millis(CONTENT_DEADLINE_MS)),
            |_, (), state| {
                if state.desktop_pending() {
                    stage_line(
                        Stage::Xwayland,
                        StageStatus::Fail,
                        "no_client_before_deadline",
                    );
                    eprintln!(
                        "flamewm-wayland: no desktop after {CONTENT_DEADLINE_MS}ms \
                         (XWayland never became ready); giving the VT back"
                    );
                    state.report_startup_failure("no_desktop_before_deadline");
                    state.loop_signal.stop();
                }
                TimeoutAction::Drop
            },
        )?;
    }

    // The event loop is about to run, nothing more: `detail=backend_init` keeps this line from being
    // read as "something is on screen" — that is `stage=presented`, reported by the tty backend when
    // the first flip completes.
    stage_line(Stage::Ready, StageStatus::Ok, "backend_init");
    let result = event_loop.run(None, &mut state, |state| {
        let flush_span = profile::wayland_flush();
        let _ = state.display.flush_clients();
        drop(flush_span);
        if let Some(service) = state.control.clone() {
            service.after_dispatch(state);
        }
    });
    // A backend that decided the run cannot continue (the tty first-frame escape hatch) has to exit
    // non-zero after the backend is torn down, so the session wrapper reports a failure and the
    // console comes back instead of a dead black VT.
    let startup_failure = state.startup_failure();
    // Backend teardown after the loop: releases DRM master and the logind session, then the
    // XWayland client (dropping it ends the X server connection).
    #[cfg_attr(not(feature = "drm"), allow(clippy::drop_non_drop))]
    drop(native);
    drop(display);
    drop(xwayland_client);
    result?;
    if let Some(reason) = startup_failure {
        return Err(format!("startup failed: {reason}").into());
    }
    Ok(0)
}

/// `detail` of the `stage=xwayland status=fail` line when `XWayland` cannot be spawned.
///
/// X11 is optional, so the line says so instead of looking like a fatal stage: the fixed tokens come
/// first (a long error message can only truncate itself), then the error. Split out from the startup
/// path so the non-fatal contract is testable without an X server.
pub(crate) fn xwayland_unavailable_detail(error: &dyn std::fmt::Display) -> String {
    format!("continuing,no_x11_client,autostart_skipped,error={error}")
}

/// Backend handle held for the whole run; `Drop` finishes the DRM backend so a failing startup
/// path, a panic and a normal exit all release DRM master and the logind session exactly once.
#[cfg(feature = "drm")]
pub(crate) struct NativeSession(Option<drm::DrmHandle>);

#[cfg(feature = "drm")]
impl Drop for NativeSession {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            drm::finish(&handle);
        }
    }
}

/// Placeholder for builds without the `drm` feature, where the native backend never starts.
#[cfg(not(feature = "drm"))]
pub(crate) struct NativeSession;

/// A recoverable backend-init failure. `run` matches this type to try the next backend; unrelated
/// errors still propagate to `main` and abort startup.
#[derive(Debug)]
pub(crate) struct BackendInitError {
    pub(crate) stage: String,
    pub(crate) cause: String,
}

impl std::fmt::Display for BackendInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} backend init failed: {}", self.stage, self.cause)
    }
}

impl Error for BackendInitError {}

/// Starts the native (tty) backend. On failure the `drm` module already released its devices, the
/// failing stage wrote a failure record, and the distinct error lets `run` fall back.
fn start_native(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Result<NativeSession, BackendInitError> {
    #[cfg(feature = "drm")]
    {
        match drm::init(state, handle) {
            Ok(drm_handle) => Ok(NativeSession(Some(drm_handle))),
            Err(error) => {
                let cause = error.to_string();
                stage_line(Stage::Drm, StageStatus::Fail, &cause);
                Err(BackendInitError {
                    stage: Stage::Drm.name().to_owned(),
                    cause,
                })
            }
        }
    }
    #[cfg(not(feature = "drm"))]
    {
        let _ = (state, handle);
        let cause = "native backend requested but this build has no `drm` feature";
        record_backend_failure(Stage::Drm.name(), cause, true);
        Err(BackendInitError {
            stage: Stage::Drm.name().to_owned(),
            cause: cause.to_owned(),
        })
    }
}

/// Try the nested winit window; if that fails too, keep the compositor alive headless/minimal.
fn start_nested_or_headless(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Option<NativeSession> {
    if !stage_reported(Stage::Seat) {
        stage_line(Stage::Seat, StageStatus::Skip, "nested backend");
    }
    if !stage_reported(Stage::Drm) {
        stage_line(Stage::Drm, StageStatus::Skip, "nested backend");
    }
    match init_winit(state, handle) {
        Ok(()) => {
            stage_line(Stage::Outputs, StageStatus::Ok, "winit window");
            stage_line(Stage::Libinput, StageStatus::Skip, "host input");
            None
        }
        Err(error) => {
            record_backend_failure("nested_winit", &error, true);
            eprintln!(
                "flamewm-wayland: nested backend unavailable ({error}); continuing headless/minimal"
            );
            // The pure rule turns the second failed attempt into the headless step; the other arm
            // keeps the pre-extraction behaviour if the rule is ever widened.
            if next_backend(BackendFallbackOutcome::NestedFailed) == NextBackend::Headless {
                append_headless_entry_record(&backend_failure_log_path());
                // `init_winit` installs the renderer before it registers the source, so a late
                // failure can leave a half-started window behind. Drop it before the socket-only run.
                state.backend = None;
                for (stage, detail) in [
                    (Stage::Seat, "headless_fallback"),
                    (Stage::Drm, "headless_fallback"),
                    (Stage::Outputs, "headless_no_output"),
                    (Stage::Libinput, "headless_no_input"),
                ] {
                    if !stage_reported(stage) {
                        stage_line(stage, StageStatus::Skip, detail);
                    }
                }
            }
            None
        }
    }
}

/// Nested winit backend: a window on the host session.
fn init_winit(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Result<(), Box<dyn Error>> {
    let (mut backend, winit_source) =
        init_from_attributes::<GlowRenderer>(nested_window_attributes())?;
    render::log_gl_renderer(backend.renderer(), "winit");
    dmabuf::init(state, backend.renderer());
    let size = backend.window_size();
    let mode = smithay::output::Mode {
        size,
        refresh: 60_000,
    };
    let percent = control::nested_output_scale_percent();
    // The winit/EGL window surface is y-inverted: the output transform carries the flip and
    // cancels the renderer's own y-flip. Upstream's winit backend uses the same transform.
    state.output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(presentation_scale::output_scale(percent)),
        Some((0, 0).into()),
    );
    state.output.set_preferred(mode);
    let (w, h) = presentation_scale::logical_output_size((size.w, size.h), percent);
    state.output_size = (w, h).into();
    // The host window draws named pointers (no software frame per motion); surfaces stay software.
    state.backend = Some(backend);
    state.sync_host_cursor();
    handle.insert_source(winit_source, |event, (), state| match event {
        WinitEvent::Resized { size, .. } => {
            let _span = profile::resize();
            state.on_resized(size.w, size.h);
        }
        WinitEvent::Input(event) => {
            let _span = profile::input();
            state.process_input(event);
        }
        WinitEvent::Redraw => render::render_frame(state),
        WinitEvent::CloseRequested => state.loop_signal.stop(),
        WinitEvent::Focus(_) => {}
    })?;
    Ok(())
}

/// Match the Xephyr development viewport unless the caller supplies a size. Using the
/// host root size (or maximizing the nested winit window) used to hide `FlameWM`'s bottom
/// panel behind the host desktop panel/titlebar, especially at fractional host scaling.
/// Both native and X11 development harnesses must start with the same *content* size.
const NESTED_DEFAULT_WIDTH: u32 = 1350;
const NESTED_DEFAULT_HEIGHT: u32 = 641;

fn nested_window_attributes() -> WindowAttributes {
    let (width, height) =
        explicit_window_size().unwrap_or((NESTED_DEFAULT_WIDTH, NESTED_DEFAULT_HEIGHT));
    WindowAttributes::default()
        .with_title("FlameWM Wayland (nested)")
        .with_inner_size(PhysicalSize::new(width, height))
}

fn explicit_window_size() -> Option<(u32, u32)> {
    let width = std::env::var("FLAMEWM_WAYLAND_WIDTH").ok()?.parse().ok()?;
    let height = std::env::var("FLAMEWM_WAYLAND_HEIGHT").ok()?.parse().ok()?;
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::{
        BACKEND_STAGES, Backend, MAX_DETAIL, Stage, StageStatus, StartupTiming, capabilities_text,
        detail_tokens, select_backend, stage_text, subscriber_wants, xwayland_unavailable_detail,
    };

    #[test]
    fn subscriber_prints_warn_events_and_never_spans() {
        use tracing::Level;
        // smithay's `warn_span!("output_bind")` is a WARN span: it must not print.
        assert!(!subscriber_wants(Level::WARN, true));
        assert!(!subscriber_wants(Level::ERROR, true));
        assert!(subscriber_wants(Level::WARN, false));
        assert!(subscriber_wants(Level::ERROR, false));
        assert!(!subscriber_wants(Level::INFO, false));
    }

    #[test]
    fn capabilities_lines_are_exact() {
        let expected = format!(
            "flamewm-capabilities=1\ndrm_compiled={}\nbackend={}\n",
            cfg!(feature = "drm"),
            Backend::default().name()
        );
        assert_eq!(capabilities_text(), expected);
    }

    #[test]
    fn capabilities_keep_the_packaging_gate_greppable() {
        let text = capabilities_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert_eq!(lines[0], "flamewm-capabilities=1");
        let drm_line = if cfg!(feature = "drm") {
            "drm_compiled=true"
        } else {
            "drm_compiled=false"
        };
        assert_eq!(lines[1], drm_line);
        assert!(
            matches!(lines[2], "backend=native" | "backend=nested"),
            "{text:?}"
        );
        assert!(
            text.contains("drm_compiled="),
            "gate string missing: {text:?}"
        );
    }

    #[test]
    fn backend_selection_follows_override_then_feature() {
        assert_eq!(select_backend(Some("drm")), Backend::Native);
        assert_eq!(select_backend(Some("native")), Backend::Native);
        assert_eq!(select_backend(Some(" drm ")), Backend::Native);
        assert_eq!(select_backend(Some("winit")), Backend::Nested);
        assert_eq!(select_backend(Some("nested")), Backend::Nested);
        // No override, no value, or an unknown value: the compiled feature decides.
        assert_eq!(select_backend(None), Backend::default());
        assert_eq!(select_backend(Some("")), Backend::default());
        assert_eq!(select_backend(Some("   ")), Backend::default());
        assert_eq!(select_backend(Some("weston")), Backend::default());
    }

    #[test]
    fn backend_default_matches_the_compiled_feature() {
        let expected = if cfg!(feature = "drm") {
            Backend::Native
        } else {
            Backend::Nested
        };
        assert_eq!(Backend::default(), expected);
    }

    #[test]
    fn stage_codes_match_their_order() {
        assert_eq!(Stage::Seat.code(), 0);
        assert_eq!(Stage::Drm.code(), 1);
        assert_eq!(Stage::Outputs.code(), 2);
        assert_eq!(Stage::Libinput.code(), 3);
        assert_eq!(Stage::Socket.code(), 4);
        assert_eq!(Stage::Xwayland.code(), 5);
        assert_eq!(Stage::Ready.code(), 6);
        // The two frame markers take the next free codes so no existing number moves, and they keep
        // their order: a queued flip (7) before a completed one (8).
        assert_eq!(Stage::Firstframe.code(), 7);
        assert_eq!(Stage::Presented.code(), 8);
        assert!(Stage::Firstframe.code() < Stage::Presented.code());
        assert_eq!(Stage::Seat.name(), "seat");
        assert_eq!(Stage::Xwayland.name(), "xwayland");
        assert_eq!(Stage::Firstframe.name(), "firstframe");
        assert_eq!(Stage::Presented.name(), "presented");
    }

    /// The queue marker is what a reader of a black-screen log looks for, but it is *not* readiness:
    /// stage, status, code and the GPU token must be exactly this, and it is emitted once per session
    /// (`stage_line` never dedupes, the backend does). The wrapper logs it as a timing diagnostic only.
    #[test]
    fn first_frame_marker_has_the_documented_shape() {
        let line = stage_text(Stage::Firstframe, StageStatus::Ok, "card0");
        let (head, stamp) = line.split_once(" t=").unwrap_or((line.as_str(), ""));
        assert_eq!(
            head,
            "FLAMEWM-WAYLAND-STARTUP stage=firstframe status=ok code=7 detail=card0"
        );
        assert!(stamp.ends_with("ms"), "{line}");
        // A failure of the same stage must never look like that marker.
        let failed = stage_text(
            Stage::Firstframe,
            StageStatus::Fail,
            "pageflip_timeout queued=1 presented=0 crtc=63 elapsed=8003ms",
        );
        assert!(
            failed.starts_with(
                "FLAMEWM-WAYLAND-STARTUP stage=firstframe status=fail code=7 \
                 detail=pageflip_timeout_queued_1_presented_0_crtc_63_elapsed_8003ms t="
            ),
            "{failed}"
        );
    }

    /// The presented marker is the readiness signal: `code=8`, the GPU and the CRTC in `detail`, and a
    /// trailing timestamp. The wrapper's readiness gate greps exactly this prefix, so the field order
    /// must not drift.
    #[test]
    fn presented_marker_has_the_documented_shape() {
        let line = stage_text(Stage::Presented, StageStatus::Ok, "card0 crtc=63");
        let (head, stamp) = line.split_once(" t=").unwrap_or((line.as_str(), ""));
        assert_eq!(
            head,
            "FLAMEWM-WAYLAND-STARTUP stage=presented status=ok code=8 detail=card0_crtc_63"
        );
        assert!(stamp.ends_with("ms"), "{line}");
        // A queued flip is a different line: dropping the code would make the two indistinguishable.
        assert_ne!(Stage::Presented.code(), Stage::Firstframe.code());
    }

    /// A backend init success only vouches for seat, DRM and outputs. Input is grabbed after the first
    /// *presented* frame, and neither frame marker comes from a queued flip alone, so neither may be
    /// filled in by `fill_backend_stages`.
    #[test]
    fn backend_stage_fill_never_claims_input_or_a_frame() {
        assert_eq!(
            BACKEND_STAGES,
            [Stage::Seat, Stage::Drm, Stage::Outputs],
            "a backend that never grabbed the seat must not report libinput"
        );
        assert!(!BACKEND_STAGES.contains(&Stage::Libinput));
        assert!(!BACKEND_STAGES.contains(&Stage::Firstframe));
        assert!(
            !BACKEND_STAGES.contains(&Stage::Presented),
            "only a completed flip may report presented"
        );
    }

    /// The one-shot timing line keeps its documented field order: the next hardware log is read with a
    /// grep for `startup timing` plus these keys. `queue` and `present` are separate boundaries.
    #[test]
    fn startup_timing_line_lists_every_step_in_order() {
        let timing = StartupTiming {
            session_ms: 412,
            gpu_ms: 180,
            gbm_ms: 3,
            egl_ms: 95,
            renderer_ms: 240,
            scan_ms: 310,
            queue_ms: Some(5912),
            present_ms: Some(5931),
        };
        assert_eq!(
            timing.line(),
            "flamewm-wayland: startup timing session=412ms gpu=180ms gbm=3ms egl=95ms \
             renderer=240ms scan=310ms queue=5912ms present=5931ms"
        );
        // A failed startup reports the steps that completed and says neither boundary was reached.
        let failed = StartupTiming {
            session_ms: 1,
            ..StartupTiming::default()
        };
        assert_eq!(
            failed.line(),
            "flamewm-wayland: startup timing session=1ms gpu=0ms gbm=0ms egl=0ms renderer=0ms \
             scan=0ms queue=none present=none"
        );
        // A flip that was queued and never completed is the black-panel case and must look like it.
        let queued_only = StartupTiming {
            queue_ms: Some(250),
            ..StartupTiming::default()
        };
        assert!(queued_only.line().ends_with("queue=250ms present=none"));
    }

    #[test]
    fn stage_detail_is_reduced_to_short_tokens() {
        assert_eq!(detail_tokens("backend init"), "backend_init");
        assert_eq!(detail_tokens("/dev/dri/card0"), "dev_dri_card0");
        assert_eq!(detail_tokens(""), "none");
        assert_eq!(detail_tokens("   "), "none");
        assert!(!detail_tokens("fd 7 leaked").contains('/'));
        assert_eq!(detail_tokens(&"a".repeat(200)).len(), MAX_DETAIL);
    }

    /// X11 is optional: a spawn failure is a `status=fail` line that also says `continuing`, and the
    /// session keeps running. The error text may be truncated; the tokens that matter come first.
    #[test]
    fn xwayland_failure_is_reported_as_non_fatal() {
        let detail = xwayland_unavailable_detail(&"missing");
        let line = stage_text(Stage::Xwayland, StageStatus::Fail, &detail);
        assert!(
            line.starts_with(
                "FLAMEWM-WAYLAND-STARTUP stage=xwayland status=fail code=5 \
                 detail=continuing_no_x11_client_autostart_skipped_error_missing"
            ),
            "{line}"
        );
        // A pathological error message cannot push the fixed tokens out of the field.
        let long = xwayland_unavailable_detail(&"x".repeat(200));
        let line = stage_text(Stage::Xwayland, StageStatus::Fail, &long);
        assert!(
            line.contains("detail=continuing_no_x11_client_autostart_skipped_error_"),
            "{line}"
        );
        assert!(!line.contains("status=ok"), "{line}");
    }

    /// The wrapper's exit watchdog reads `stage=ready` with `code=6`: the field order and the trailing
    /// `t=<ms>ms` timestamp must stay exactly this, or a failure report silently loses its evidence.
    #[test]
    fn stage_text_keeps_the_documented_field_order_and_the_timestamp() {
        let line = stage_text(Stage::Socket, StageStatus::Ok, "wayland-flame");
        let (head, stamp) = line.split_once(" t=").unwrap_or((line.as_str(), ""));
        assert_eq!(
            head,
            "FLAMEWM-WAYLAND-STARTUP stage=socket status=ok code=4 detail=wayland-flame"
        );
        let millis = stamp.strip_suffix("ms").unwrap_or("");
        assert!(
            !millis.is_empty() && millis.chars().all(|digit| digit.is_ascii_digit()),
            "{line}"
        );
        // The abort line must never look like a success to the watchdog.
        let abort = stage_text(Stage::Ready, StageStatus::Fail, "startup aborted: no seat");
        assert!(
            abort.starts_with(
                "FLAMEWM-WAYLAND-STARTUP stage=ready status=fail code=6 \
                 detail=startup_aborted_no_seat t="
            ),
            "{abort}"
        );
    }
}
