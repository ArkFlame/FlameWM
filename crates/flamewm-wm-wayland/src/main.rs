//! FlameWM Wayland compositor: nested under a host session via the winit backend, or on a tty via
//! the `drm` backend (cargo feature `drm`), with XWayland.
//!
//! Environment:
//! - `FLAMEWM_WAYLAND_BACKEND` (`drm`/`native` or `winit`/`nested`): backend choice. When unset, a
//!   build with the `drm` feature runs the native backend and every other build runs nested. The
//!   choice never comes from `DISPLAY`/`WAYLAND_DISPLAY`: a session wrapper states its intent.
//! - `FLAMEWM_WAYLAND_SOCKET` (default `wayland-flame`): socket name under `XDG_RUNTIME_DIR`.
//! - `FLAMEWM_WAYLAND_AUTOSTART` (colon-separated programs): started once XWayland is ready, with
//!   `DISPLAY` pointing at XWayland. The session wrapper passes the shell and desktop clients.
//! - `FLAMEWM_WAYLAND_DUMP=<file.ppm>`: write one framebuffer readback after
//!   `FLAMEWM_WAYLAND_DUMP_AFTER_MS` (default 2000); with `FLAMEWM_WAYLAND_EXIT_AFTER_DUMP=1`
//!   the compositor exits right after the dump (used by the headless evidence run).
//!
//! Two non-compositing entry points exist for packaging and diagnostics:
//! - `--capabilities`: machine-readable build facts on stdout, exit 0. Touches no bus, no `/dev`
//!   and nothing on `PATH`, so the packaging gate can run it before installing dependencies.
//! - `--probe`: read-only prerequisite report, exit 0 when every required prerequisite exists.
//!
//! Startup diagnostics: every stage writes
//! `FLAMEWM-WAYLAND-STARTUP stage=<name> status=<ok|fail|skip> code=<0..6> detail=<tokens> t=<ms>ms`
//! to stderr (the session wrapper redirects that into its log and its failure report). `t` is
//! milliseconds since this process started, so a slow stage is visible without a trace tool. A
//! startup that dies before `stage=ready` also emits `stage=ready status=fail`, which is what the
//! wrapper's exit watchdog keys on; a normal exit emits nothing extra.
//!
//! # Session activation
//!
//! A session manager may start this process before the VT and the seat are active: at that moment the
//! greeter still owns the tty, and logind reports the session as not yet `Active`. The tty backend
//! therefore waits for activation (polling `Active` for ~3 s before taking control, then carrying on)
//! and arms DRM only when logind's `ActivateSession` arrives — the same shape as any other logind
//! compositor. `stage=seat` is what reports this; a session that is still inactive when the wait
//! expires logs `detail=inactive_continuing` inside the `logind` module and keeps running.
//!
//! XWayland is optional. If it cannot be spawned the compositor keeps running as a pure Wayland
//! compositor (`stage=xwayland status=fail`), no X11 client is served and the X11 shell/desktop
//! autostart is skipped instead of aborting the session.

mod control;
#[cfg(feature = "drm")]
#[allow(unsafe_code)]
// EGL display and renderer creation are unsafe in smithay; no other module may.
mod drm;
mod handlers;
mod input;
// Compiled without the `drm` feature too: the D-Bus surface and the `Session` impl are the only
// feature-gated parts, which keeps the pause/resume state machine testable on any host.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
mod logind;
mod model;
mod probe;
mod render;
mod state;
mod xwayland;

use std::cell::RefCell;
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::winit::{WinitEvent, init_from_attributes};
use smithay::output::Scale;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::reexports::winit::dpi::PhysicalSize;
use smithay::reexports::winit::window::WindowAttributes;
use smithay::reexports::x11rb::connection::Connection;
use smithay::reexports::x11rb::rust_connection::RustConnection;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Seat = 0,
    Drm = 1,
    Outputs = 2,
    Libinput = 3,
    Socket = 4,
    Xwayland = 5,
    Ready = 6,
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

/// Start instant of this process, for the `t=` field of every stage line. A stage that takes seconds
/// (DRM master, libinput, EGL, XWayland) is then visible in the log and in the session wrapper's
/// report without a trace tool.
static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since this process started; 0 until [`START`] is initialised.
fn elapsed_ms() -> u128 {
    START
        .get()
        .map(|start| start.elapsed().as_millis())
        .unwrap_or(0)
}

/// The exact text of one structured startup line:
/// `FLAMEWM-WAYLAND-STARTUP stage=<stage> status=<ok|fail|skip> code=<0..6> detail=<tokens> t=<ms>ms`.
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

/// Emits one structured startup line (see [`stage_text`]).
pub(crate) fn stage_line(stage: Stage, status: StageStatus, detail: &str) {
    REPORTED_STAGES.fetch_or(1u8 << stage.code(), Ordering::Relaxed);
    eprintln!("{}", stage_text(stage, status, detail));
}

fn stage_reported(stage: Stage) -> bool {
    REPORTED_STAGES.load(Ordering::Relaxed) & (1u8 << stage.code()) != 0
}

/// Fills the stages the backend owns but did not report itself: a successful backend init covers
/// them, and a stage never appears twice.
fn fill_backend_stages() {
    for stage in [Stage::Seat, Stage::Drm, Stage::Outputs, Stage::Libinput] {
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

fn main() {
    // The `t=` field of every stage line is measured from here, so the timer covers argument
    // parsing, the seat, DRM init and the socket, not just the event loop.
    let _ = START.set(Instant::now());
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

/// Marks the startup as aborted when the compositor never reached [`Stage::Ready`]: the session
/// wrapper's exit watchdog reports that case as a failure and needs the stage it died in. Nothing is
/// printed when the loop ran and `stage=ready` was already reported, so a normal exit stays clean.
fn abort_startup(reason: &str) {
    if !stage_reported(Stage::Ready) {
        stage_line(Stage::Ready, StageStatus::Fail, reason);
    }
}

/// Runs the compositor and returns its exit code.
fn run() -> Result<i32, Box<dyn Error>> {
    let explicit = std::env::var("FLAMEWM_WAYLAND_BACKEND").ok();
    let backend = select_backend(explicit.as_deref());
    if let Some(requested) = explicit
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !matches!(requested, "drm" | "native" | "winit" | "nested") {
            eprintln!(
                "flamewm-wayland: unknown FLAMEWM_WAYLAND_BACKEND={requested}, using {}",
                backend.name()
            );
        }
    }
    if backend == Backend::Native && !cfg!(feature = "drm") {
        eprintln!(
            "flamewm-wayland: the native backend needs a build with `--features drm`; this build only has the nested backend"
        );
        return Ok(2);
    }

    let socket_name =
        std::env::var("FLAMEWM_WAYLAND_SOCKET").unwrap_or_else(|_| "wayland-flame".to_owned());
    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let handle = event_loop.handle();
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
    // Control service: a failed bus connection leaves the compositor usable without the shell API.
    if let Err(error) = control::install(&mut state) {
        eprintln!("flamewm-wayland: control service unavailable: {error}");
    }

    // Backend first: the seat is taken here, and a failure must not leave a half-started
    // compositor holding the session. The guard owns the backend handle and finishes it on drop,
    // which also covers the error paths below.
    let native = match backend {
        Backend::Native => Some(start_native(&mut state, &handle)?),
        Backend::Nested => {
            stage_line(Stage::Seat, StageStatus::Skip, "nested backend");
            stage_line(Stage::Drm, StageStatus::Skip, "nested backend");
            init_winit(&mut state, &handle)?;
            stage_line(Stage::Outputs, StageStatus::Ok, "winit window");
            stage_line(Stage::Libinput, StageStatus::Skip, "host input");
            None
        }
    };
    fill_backend_stages();

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
    handle.insert_source(listening, |client, _, state| {
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
    let xwayland_client = match xwayland::spawn_xwayland(&xwayland_dh, &handle) {
        Ok(client) => {
            stage_line(Stage::Xwayland, StageStatus::Ok, "spawned");
            Some(client)
        }
        Err(error) => {
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
    };

    if let Some(path) = std::env::var_os("FLAMEWM_WAYLAND_DUMP") {
        state.exit_after_dump = std::env::var_os("FLAMEWM_WAYLAND_EXIT_AFTER_DUMP").is_some();
        let after = std::env::var("FLAMEWM_WAYLAND_DUMP_AFTER_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(2000);
        let path = PathBuf::from(path);
        // Re-armed until the frame is written: a redraw request is only honoured once the host
        // delivers a frame callback, so a single shot can be lost.
        handle.insert_source(
            Timer::from_duration(Duration::from_millis(after)),
            move |_, _, state| {
                if state.dumped {
                    return TimeoutAction::Drop;
                }
                state.dump = Some(path.clone());
                state.request_redraw();
                TimeoutAction::ToDuration(Duration::from_millis(250))
            },
        )?;
    }

    // Reap exited children twice a second; the loop may otherwise sleep indefinitely.
    handle.insert_source(
        Timer::from_duration(Duration::from_millis(500)),
        |_, _, state| {
            state.reap_children();
            TimeoutAction::ToDuration(Duration::from_millis(500))
        },
    )?;

    stage_line(Stage::Ready, StageStatus::Ok, backend.name());
    let result = event_loop.run(None, &mut state, |state| {
        let _ = state.display.flush_clients();
        if let Some(service) = state.control.clone() {
            service.after_dispatch(state);
        }
    });
    // Backend teardown after the loop: releases DRM master and the logind session, then the
    // XWayland client (dropping it ends the X server connection).
    drop(native);
    drop(display);
    drop(xwayland_client);
    result?;
    Ok(0)
}

/// `detail` of the `stage=xwayland status=fail` line when XWayland cannot be spawned.
///
/// X11 is optional, so the line says so instead of looking like a fatal stage: the fixed tokens come
/// first (a long error message can only truncate itself), then the error. Split out from the startup
/// path so the non-fatal contract is testable without an X server.
fn xwayland_unavailable_detail(error: &dyn std::fmt::Display) -> String {
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

/// Starts the native (tty) backend. On failure the `drm` module already released its devices, the
/// stage line names the failure and the process exits non-zero.
fn start_native(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Result<NativeSession, Box<dyn Error>> {
    #[cfg(feature = "drm")]
    {
        match drm::init(state, handle) {
            Ok(drm_handle) => Ok(NativeSession(Some(drm_handle))),
            Err(error) => {
                stage_line(Stage::Drm, StageStatus::Fail, &format!("{error}"));
                Err(format!("native backend init failed: {error}").into())
            }
        }
    }
    #[cfg(not(feature = "drm"))]
    {
        let _ = (state, handle);
        Err("native backend requested but this build has no `drm` feature".into())
    }
}

/// Nested winit backend: a window on the host session.
fn init_winit(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Result<(), Box<dyn Error>> {
    let (backend, winit_source) = init_from_attributes::<GlowRenderer>(nested_window_attributes())?;
    let size = backend.window_size();
    let mode = smithay::output::Mode {
        size,
        refresh: 60_000,
    };
    // The winit/EGL window surface is y-inverted: the output transform carries the flip and
    // cancels the renderer's own y-flip. Upstream's winit backend uses the same transform.
    state.output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    state.output.set_preferred(mode);
    state.output_size = (size.w, size.h).into();
    state.backend = Some(backend);
    handle.insert_source(winit_source, |event, _, state| match event {
        WinitEvent::Resized { size, .. } => state.on_resized(size.w, size.h),
        WinitEvent::Input(event) => state.process_input(event),
        WinitEvent::Redraw => render::render_frame(state),
        WinitEvent::CloseRequested => state.loop_signal.stop(),
        WinitEvent::Focus(_) => {}
    })?;
    Ok(())
}

/// Initial size of the nested window. Without one winit picks 1280x800, so the compositor shows up
/// smaller than the host screen and the rest of the host is unpainted. An explicit override wins;
/// a Wayland host is asked to maximize us; an X11 host gets the root size, because there may be no
/// window manager to tile the window (Xephyr).
fn nested_window_attributes() -> WindowAttributes {
    if let Some((width, height)) = explicit_window_size() {
        return WindowAttributes::default().with_inner_size(PhysicalSize::new(width, height));
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        // Wayland host: let its compositor tile us to the usable area.
        return WindowAttributes::default().with_maximized(true);
    }
    match host_root_size() {
        Some((width, height)) => {
            WindowAttributes::default().with_inner_size(PhysicalSize::new(width, height))
        }
        None => WindowAttributes::default(),
    }
}

fn explicit_window_size() -> Option<(u32, u32)> {
    let width = std::env::var("FLAMEWM_WAYLAND_WIDTH").ok()?.parse().ok()?;
    let height = std::env::var("FLAMEWM_WAYLAND_HEIGHT").ok()?.parse().ok()?;
    Some((width, height))
}

fn host_root_size() -> Option<(u32, u32)> {
    let (connection, screen) = RustConnection::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?;
    Some((
        u32::from(root.width_in_pixels),
        u32::from(root.height_in_pixels),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        Backend, MAX_DETAIL, Stage, StageStatus, capabilities_text, detail_tokens, select_backend,
        stage_text, xwayland_unavailable_detail,
    };

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
        assert_eq!(Stage::Seat.name(), "seat");
        assert_eq!(Stage::Xwayland.name(), "xwayland");
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
