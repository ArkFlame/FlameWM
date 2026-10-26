//! Interactive lock loop: one override-redirect surface, keyboard and pointer
//! keyboard grab, PAM and power actions on worker threads, the idle screensaver,
//! and live control snapshots. Input that wakes the screen is consumed in the same turn.
use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use calloop::Interest;
use flamewm_api::Rect;
use flamewm_api::display::DisplaySnapshot;
use flamewm_api::session::SessionCapabilities;
use flamewm_control_dbus::{ControlSignal, ControlSignalClient};
use flamewm_reactor::{FdAction, Reactor};
use flamewm_ui_x11::{
    ActionPhase, RuntimeImage, SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole,
    SurfaceRuntime, UiBackendError, run_surface_runtime_with_reactor_access,
};

use crate::document;
use crate::native_time::local_now;
use crate::pam::{self, Outcome};
use crate::power::{self, Effect as PowerEffect, POWER_ORDER, PowerEvent, PowerUi, PowerView};
use crate::preview::{self, PreviewInput};
use crate::ribbons::{FRAME_INTERVAL, RibbonFrames};
use crate::screensaver::{Phase, Screensaver};
use crate::secret::PasswordBuffer;
use crate::settings::LockSettings;
use crate::signals;
use crate::state::{Effect, Key, LockState, Status};
use crate::view::LockView;
use crate::x11::{KeyBatch, PointerState, Session, grab_with_retry};

const GRAB_TIMEOUT: Duration = Duration::from_secs(2);
/// Handles root `ConfigureNotify` promptly while still keeping activity sampling bounded.
const TICK: Duration = Duration::from_millis(50);
/// Poll interval of the preview: pointer and keyboard are read by polling.
const PREVIEW_POLL: Duration = Duration::from_millis(50);
/// Shown in the status line while Caps Lock is on and no other status applies.
const CAPS_LOCK_HINT: &str = "Caps Lock is on";
/// Primary mouse button in X core numbering.
const BUTTON_PRIMARY: u32 = 1;

fn typography_changed(previous: &LockSettings, next: &LockSettings) -> bool {
    previous.font_family != next.font_family
        || previous.font_bold != next.font_bold
        || previous.font_size_offset != next.font_size_offset
}

fn background_changed(previous: &LockSettings, next: &LockSettings) -> bool {
    previous.background_color != next.background_color
        || previous.background_image != next.background_image
        || previous.background_mode != next.background_mode
        || previous.share_wallpaper != next.share_wallpaper
        || previous.blur_percent != next.blur_percent
        || previous.dim_percent != next.dim_percent
}

fn output_rects_need_rebuild(current: &[Rect], next: &[Rect]) -> bool {
    current != next
}

fn latest_control_snapshots(
    signals: impl IntoIterator<Item = ControlSignal>,
) -> (
    Option<flamewm_api::settings::SettingsSnapshot>,
    Option<DisplaySnapshot>,
) {
    let mut settings = None;
    let mut displays = None;
    for signal in signals {
        match signal {
            ControlSignal::SettingsSnapshotChanged { snapshot } => settings = Some(snapshot),
            ControlSignal::DisplaysSnapshotChanged { snapshot } => displays = Some(snapshot),
            _ => {}
        }
    }
    (settings, displays)
}

/// Shows the lock screen until an authentication succeeds. Returns `Err`
/// before anything is visible when the grabs cannot be taken.
pub fn run(settings: &LockSettings, username: &str) -> Result<(), String> {
    let mut runtime =
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
    runtime.watch_root_changes();
    let (width, height) = runtime.root_geometry();
    let root = Rect::new(0, 0, width as i32, height as i32);
    let outputs = crate::session::lock_output_rects(root);
    lock_on(&mut runtime, settings, username, root, outputs)
}

/// Shows the lock UI as a settings preview: no keyboard or pointer grab, no PAM,
/// no power actions and no screensaver. Closes on the first input after the grace
/// period, or on SIGTERM.
pub fn run_preview(settings: &LockSettings, username: &str) -> Result<(), String> {
    preview_loop(settings, username, false)
}

/// Shows the Ribbons screensaver over the lock UI as a temporary preview. Same
/// dismissal as [`run_preview`]. It takes no grab, never authenticates, never changes
/// DPMS and owns no idle timer, so the real lock's idle state is untouched.
pub fn run_screensaver_preview(settings: &LockSettings, username: &str) -> Result<(), String> {
    preview_loop(settings, username, true)
}

/// Config of the full-screen overlay surface shared by the lock and its previews.
fn overlay_config(width: u32, height: u32, title: &str) -> SurfaceConfig {
    overlay_at(
        Rect::new(0, 0, width as i32, height as i32),
        title,
        SurfaceInputMode::Interactive,
    )
}

fn overlay_at(rect: Rect, title: &str, input: SurfaceInputMode) -> SurfaceConfig {
    SurfaceConfig {
        width: rect.width.max(1) as u32,
        height: rect.height.max(1) as u32,
        title: title.to_string(),
        role: SurfaceRole::Overlay,
        input,
        initially_visible: false,
        x: rect.x,
        y: rect.y,
    }
}

/// Shows and raises `surface`; `what` names it in the error.
fn show_raised(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    what: &str,
) -> Result<(), String> {
    runtime
        .show(surface)
        .map_err(|error| format!("show {what} surface: {error:?}"))?;
    runtime
        .raise(surface)
        .map_err(|error| format!("raise {what} surface: {error:?}"))
}

#[derive(Clone)]
struct LockSurface {
    handle: SurfaceHandle,
    rect: Rect,
    base_background: RuntimeImage,
}

fn update_animated_backgrounds(
    runtime: &mut SurfaceRuntime,
    surfaces: &[LockSurface],
    fallback: Option<&LockSurface>,
    frames: &mut [RibbonFrames],
    fallback_frames: Option<&mut RibbonFrames>,
    now: Instant,
) -> Result<(), String> {
    for (surface, frame) in surfaces.iter().zip(frames) {
        let image = frame.frame(now);
        runtime
            .with_document(surface.handle, |view| document::set_background(view, image))
            .map_err(|error| format!("update lock background: {error:?}"))?;
        runtime
            .redraw(surface.handle)
            .map_err(|error| format!("redraw lock surface: {error:?}"))?;
    }
    if let (Some(surface), Some(frame)) = (fallback, fallback_frames) {
        let image = frame.frame(now);
        runtime
            .with_document(surface.handle, |view| document::set_background(view, image))
            .map_err(|error| format!("update fallback lock background: {error:?}"))?;
        runtime
            .redraw(surface.handle)
            .map_err(|error| format!("redraw fallback lock surface: {error:?}"))?;
    }
    Ok(())
}

fn restore_surface_backgrounds(
    runtime: &mut SurfaceRuntime,
    surfaces: &[LockSurface],
    fallback: Option<&LockSurface>,
) -> Result<(), String> {
    for surface in surfaces.iter().chain(fallback) {
        let base = surface.base_background.clone();
        runtime
            .with_document(surface.handle, |view| document::set_background(view, base))
            .map_err(|error| format!("restore lock background: {error:?}"))?;
    }
    Ok(())
}

fn create_lock_surface(
    runtime: &mut SurfaceRuntime,
    settings: &LockSettings,
    root: Rect,
    canvas: Option<&RuntimeImage>,
    rect: Rect,
    title: &str,
) -> Result<LockSurface, String> {
    let template = document::template()?;
    let width = rect.width.max(1) as u32;
    let height = rect.height.max(1) as u32;
    let background = document::background_for_output(settings, root, rect, canvas);
    let handle = runtime
        .create_surface(
            template,
            overlay_at(rect, title, SurfaceInputMode::Interactive),
        )
        .map_err(|error| format!("create {title} surface: {error:?}"))?;
    runtime
        .with_document(handle, |view| {
            document::apply_document(view, settings, width, height, background.clone())
        })
        .map_err(|error| format!("theme {title} surface: {error:?}"))?;
    Ok(LockSurface {
        handle,
        rect,
        base_background: background,
    })
}

/// The lock projection for a preview, built only for the non-Ribbons preview. Ribbons
/// shows no lock UI, so `project` is never called for it.
fn preview_lock_ui<T>(ribbons: bool, project: impl FnOnce() -> T) -> Option<T> {
    (!ribbons).then(project)
}

/// Shared preview loop. `ribbons` selects the animated background and nothing else.
fn preview_loop(settings: &LockSettings, username: &str, ribbons: bool) -> Result<(), String> {
    let (width, height) = flamewm_ui_x11::root_geometry()?;
    let template = document::template()?;
    let mut runtime =
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
    let title = if ribbons {
        "FlameWM screensaver preview"
    } else {
        "FlameWM lock preview"
    };
    let surface = runtime
        .create_surface(template, overlay_config(width, height, title))
        .map_err(|error| format!("create preview surface: {error:?}"))?;
    let started = Instant::now();
    let mut frames = ribbons.then(|| RibbonFrames::new(width, height, &settings.saver, started));
    let background = match frames.as_mut() {
        Some(frames) => frames.frame(started),
        None => document::background_image(settings, width, height),
    };
    let mut next_frame = started + FRAME_INTERVAL;
    // Ribbons shows the background alone: its lock projection is never built.
    let lock_ui = preview_lock_ui(ribbons, || {
        (
            LockView::project(
                settings,
                local_now(),
                username,
                &PasswordBuffer::default(),
                Status::default(),
            ),
            PowerUi::new(SessionCapabilities::default()).view(),
        )
    });
    runtime
        .with_document(surface, |document_view| {
            document::apply_document(document_view, settings, width, height, background)?;
            match &lock_ui {
                Some((view, power)) => {
                    document::apply_view(document_view, view)?;
                    document::apply_power(document_view, width, height, power)
                }
                None => document::set_lock_ui_shown(document_view, false),
            }
        })
        .map_err(|error| format!("theme preview surface: {error:?}"))?;
    show_raised(&mut runtime, surface, "preview")?;
    let mapped = Instant::now();

    // Input is read by polling, so no grab is needed. The timer bounds each wait.
    let session = Session::open()?;
    signals::install_preview_terminate();
    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    reactor
        .register_timer(PREVIEW_POLL, true, || {})
        .map_err(|error| format!("register preview tick: {error}"))?;
    let sample = |session: &Session| {
        session
            .pointer()
            .map(|pointer| PreviewInput::new(pointer.x, pointer.y, pointer.mask, session.keymap()))
    };
    let mut previous = sample(&session);
    run_surface_runtime_with_reactor_access(
        &mut runtime,
        &mut reactor,
        |_, _| Ok(()),
        |runtime| {
            let current = sample(&session);
            let dismiss = signals::preview_terminate_requested()
                || matches!((previous, current),
                    (Some(before), Some(now)) if preview::dismisses(mapped.elapsed(), &before, &now));
            if dismiss {
                frames = None;
                runtime
                    .destroy(surface)
                    .map_err(|error| UiBackendError::Renderer(format!("destroy preview: {error:?}")))?;
            }
            if let Some(frames) = frames.as_mut() {
                let now = Instant::now();
                if now >= next_frame {
                    let image = frames.frame(now);
                    next_frame = now + FRAME_INTERVAL;
                    runtime
                        .with_document(surface, |view| document::set_background(view, image))
                        .map_err(|error| UiBackendError::Renderer(format!("update preview background: {error:?}")))?;
                    runtime
                        .redraw(surface)
                        .map_err(|error| UiBackendError::Renderer(format!("redraw preview: {error:?}")))?;
                }
            }
            previous = current;
            Ok(())
        },
    )
    .map_err(|error| format!("preview loop: {error:?}"))
}

fn lock_on(
    runtime: &mut SurfaceRuntime,
    settings: &LockSettings,
    username: &str,
    root: Rect,
    outputs: Vec<Rect>,
) -> Result<(), String> {
    let outputs = if outputs.is_empty() {
        vec![root]
    } else {
        outputs
    };
    // Keep full-root coverage under the output surfaces so stale monitor geometry
    // after a layout change cannot reveal the desktop.
    let canvas = document::shared_root_background(settings, root);
    let fallback = Some(create_lock_surface(
        runtime,
        settings,
        root,
        canvas.as_ref(),
        root,
        "FlameWM lock coverage",
    )?);
    let mut surfaces = outputs
        .iter()
        .enumerate()
        .map(|(index, rect)| {
            create_lock_surface(
                runtime,
                settings,
                root,
                canvas.as_ref(),
                *rect,
                &format!("FlameWM lock {}", index + 1),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if surfaces.is_empty() {
        surfaces.push(create_lock_surface(
            runtime,
            settings,
            root,
            canvas.as_ref(),
            root,
            "FlameWM lock",
        )?);
    }

    let mut session = Session::open()?;
    if !grab_with_retry(&mut session, GRAB_TIMEOUT) {
        return Err("cannot grab keyboard; not locking".to_string());
    }
    let session = Rc::new(RefCell::new(session));
    let keys = Rc::new(RefCell::new(KeyBatch::default()));
    let clicks = Rc::new(RefCell::new(Vec::<String>::new()));
    let control_signals = Rc::new(RefCell::new(Vec::<ControlSignal>::new()));
    let (outcome_tx, outcome_rx) = mpsc::channel::<Outcome>();
    let (power_tx, power_rx) = mpsc::channel::<PowerEvent>();
    power::fetch_capabilities_on_worker(power_tx.clone());
    let (mut wake_reader, wake_writer) =
        UnixStream::pair().map_err(|error| format!("frame pacer socket: {error}"))?;
    let frame_pacer = FramePacer::start(wake_writer)?;
    let mut lock = LockLoop {
        surfaces,
        fallback,
        root,
        settings: settings.clone(),
        session: Rc::clone(&session),
        keys: Rc::clone(&keys),
        clicks: Rc::clone(&clicks),
        control_signals: Rc::clone(&control_signals),
        state: LockState::default(),
        power: PowerUi::new(SessionCapabilities::default()),
        power_tx,
        power_rx,
        shown_power: None,
        painted_key: None,
        settings_revision: 0,
        power_revision: 0,
        saver: Screensaver::new(settings.saver.clone()),
        frames: None,
        background_stale: false,
        next_frame: Instant::now(),
        frame_pacer,
        fallback_frames: None,
        last_tick: Instant::now(),
        last_pointer: None,
        pending_root_geometry: None,
        pending_display_snapshot: None,
        pending_auth_success: false,
        live_typography_dirty: false,
        live_background_dirty: false,
        caps_lock: false,
        shown: None,
        ui_shown_wanted: true,
        ui_visibility_dirty: false,
        display_on_pending: false,
        username: username.to_string(),
        outcome_tx,
        outcome_rx,
        unlocked: false,
    };
    lock.repaint(runtime);
    lock.show_surfaces(runtime)?;
    // Every output surface and the full-root fallback receive ordinary pointer events;
    // the lock's security boundary is the shared keyboard grab.

    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    watch_control_signals(&mut reactor, control_signals);
    let key_fd = session.borrow().fd();
    reactor
        .register_raw_fd_with_action(key_fd, Interest::READ, move |_, _| {
            let read = session.borrow_mut().read_keys();
            let mut queue = keys.borrow_mut();
            queue.keys.extend(read.keys);
            queue.presses += read.presses;
            FdAction::Continue
        })
        .map_err(|error| format!("watch keyboard events: {error}"))?;
    wake_reader
        .set_nonblocking(true)
        .map_err(|error| format!("frame pacer socket: {error}"))?;
    reactor
        .register_raw_fd_with_action(wake_reader.as_raw_fd(), Interest::READ, move |_, _| {
            let mut sink = [0u8; 64];
            while wake_reader.read(&mut sink).is_ok_and(|read| read > 0) {}
            FdAction::Continue
        })
        .map_err(|error| format!("watch frame pacer: {error}"))?;
    reactor
        .register_timer(TICK, true, || {})
        .map_err(|error| format!("register lock tick: {error}"))?;

    run_surface_runtime_with_reactor_access(
        runtime,
        &mut reactor,
        move |event, _runtime| {
            // Power icons, the confirm strip and the Other list are action nodes; only
            // a primary press means a click. Queued here, applied in the tick.
            if event.action.phase == ActionPhase::Press && event.action.button == BUTTON_PRIMARY {
                clicks.borrow_mut().push(event.action.action);
            }
            Ok(())
        },
        |runtime| lock.tick(runtime).map_err(UiBackendError::Renderer),
    )
    .map_err(|error| format!("lock loop: {error:?}"))?;

    if lock.unlocked {
        Ok(())
    } else {
        Err("lock surface closed without authentication".to_string())
    }
}

/// Every input the lock projection reads. Equal keys mean the last paint is still current.
/// `settings_revision` and `power_revision` are bumped wherever those inputs change.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PaintKey {
    time: Option<crate::native_time::LocalTime>,
    password_len: usize,
    status: crate::state::Status,
    caps_lock: bool,
    settings_revision: u32,
    power_revision: u32,
}

struct LockLoop {
    surfaces: Vec<LockSurface>,
    fallback: Option<LockSurface>,
    root: Rect,
    settings: LockSettings,
    session: Rc<RefCell<Session>>,
    keys: Rc<RefCell<KeyBatch>>,
    clicks: Rc<RefCell<Vec<String>>>,
    control_signals: Rc<RefCell<Vec<ControlSignal>>>,
    pending_display_snapshot: Option<DisplaySnapshot>,
    state: LockState,
    power: PowerUi,
    power_tx: Sender<PowerEvent>,
    power_rx: Receiver<PowerEvent>,
    shown_power: Option<PowerView>,
    /// Inputs of the last successful paint; `shown` is reset to `None` to force a repaint.
    painted_key: Option<PaintKey>,
    settings_revision: u32,
    power_revision: u32,
    saver: Screensaver,
    frames: Option<Vec<RibbonFrames>>,
    /// Set when the wallpaper restore was refused; the next turn outside Ribbons retries.
    background_stale: bool,
    next_frame: Instant,
    frame_pacer: FramePacer,
    fallback_frames: Option<RibbonFrames>,
    last_tick: Instant,
    last_pointer: Option<PointerState>,
    pending_root_geometry: Option<(u32, u32)>,
    pending_auth_success: bool,
    live_typography_dirty: bool,
    live_background_dirty: bool,
    caps_lock: bool,
    shown: Option<LockView>,
    /// Wanted visibility of the lock widgets; `ui_visibility_dirty` means the document
    /// does not yet match it and the next tick retries.
    ui_shown_wanted: bool,
    ui_visibility_dirty: bool,
    /// Set when waking a dead display; the next tick retries DPMS on.
    display_on_pending: bool,
    username: String,
    outcome_tx: Sender<Outcome>,
    outcome_rx: Receiver<Outcome>,
    unlocked: bool,
}

/// Runs one lock-screen step that must not end the loop. A failure is logged and reported
/// as `false`; the caller keeps its state and the grabs stay held.
fn keep_lock_step(what: &str, step: impl FnOnce() -> Result<(), String>) -> bool {
    match step() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("flamewm-lock: {what}, keeping the lock engaged: {error}");
            false
        }
    }
}

/// Applies a deferred step. Success clears `pending`; a failure is logged and leaves it set
/// so a later turn retries. It never ends the loop or touches the grabs.
fn settle(pending: &mut bool, what: &str, apply: impl FnOnce() -> Result<(), String>) {
    match apply() {
        Ok(()) => *pending = false,
        Err(error) => {
            *pending = true;
            eprintln!("flamewm-lock: {what}, retrying next turn: {error}");
        }
    }
}

fn watch_control_signals(reactor: &mut Reactor, pending: Rc<RefCell<Vec<ControlSignal>>>) {
    let client = match ControlSignalClient::connect_session() {
        Ok(client) => Rc::new(client),
        Err(error) => {
            eprintln!("flamewm-lock: live settings/display updates unavailable: {error}");
            return;
        }
    };
    let watch = client.watch();
    let interest = match (
        watch
            .events
            .contains(flamewm_api::ports::FdEvents::READABLE),
        watch
            .events
            .contains(flamewm_api::ports::FdEvents::WRITABLE),
    ) {
        (true, true) => calloop::Interest::BOTH,
        (false, true) => calloop::Interest::WRITE,
        _ => calloop::Interest::READ,
    };
    let result = reactor.register_raw_fd_with_action(watch.fd, interest, move |_, _| {
        let pending = Rc::clone(&pending);
        if let Err(error) = client.on_ready(|signal| {
            if let signal @ (ControlSignal::SettingsSnapshotChanged { .. }
            | ControlSignal::DisplaysSnapshotChanged { .. }) = signal
            {
                pending.borrow_mut().push(signal);
            }
        }) {
            eprintln!("flamewm-lock: read settings/display update failed: {error}");
        }
        FdAction::Continue
    });
    if let Err(error) = result {
        eprintln!("flamewm-lock: live settings/display watch unavailable: {error}");
    }
}

impl LockLoop {
    fn show_surfaces(&self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if let Some(fallback) = self.fallback.as_ref() {
            show_raised(runtime, fallback.handle, "lock coverage")?;
        }
        for (index, surface) in self.surfaces.iter().enumerate() {
            show_raised(
                runtime,
                surface.handle,
                &format!("lock monitor {}", index + 1),
            )?;
        }
        Ok(())
    }

    fn raise_surfaces(&self, runtime: &SurfaceRuntime) -> Result<(), String> {
        if let Some(fallback) = self.fallback.as_ref() {
            runtime
                .raise(fallback.handle)
                .map_err(|error| format!("raise lock coverage: {error:?}"))?;
        }
        for surface in &self.surfaces {
            runtime
                .raise(surface.handle)
                .map_err(|error| format!("raise lock surface: {error:?}"))?;
        }
        Ok(())
    }

    /// One loop turn: wake the screen on input (consuming it), apply keys, clicks,
    /// PAM and power results, animate the screensaver, keep the grabs and stacking,
    /// then repaint only when the projected content changed.
    fn tick(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if let Some(size) = runtime.take_root_geometry_changed() {
            self.pending_root_geometry = Some(size);
        }
        if let Some((width, height)) = self.pending_root_geometry {
            if self.refresh_root_geometry(runtime, width, height) {
                self.pending_root_geometry = None;
            }
        }
        self.apply_control_signals();
        if self.pending_root_geometry.is_none() {
            if let Some(snapshot) = self.pending_display_snapshot.take() {
                self.apply_display_snapshot(runtime, &snapshot);
            }
        }
        self.sync_live_settings(runtime);
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;

        // XQueryPointer reads the socket into Xlib's queue, so the fd no longer signals for
        // those events: drain the queue here, after the pointer poll, every turn.
        let pointer = self.session.borrow().pointer();
        let fresh = self.session.borrow_mut().read_keys();
        let mut batch = std::mem::take(&mut *self.keys.borrow_mut());
        batch.keys.extend(fresh.keys);
        batch.presses += fresh.presses;
        let clicks = std::mem::take(&mut *self.clicks.borrow_mut());
        let moved = matches!((pointer, self.last_pointer),
            (Some(current), Some(previous)) if current.moved_or_buttons_changed(&previous));
        if let Some(state) = pointer {
            self.caps_lock = state.caps_lock();
            self.last_pointer = Some(state);
        }
        let activity = moved || batch.presses > 0 || !clicks.is_empty();
        let consumed = if activity {
            self.wake(runtime)
        } else {
            if let Some(phase) = self.saver.tick(elapsed) {
                self.enter(runtime, phase, now);
            }
            false
        };

        while let Ok(event) = self.power_rx.try_recv() {
            self.power_event(event);
        }
        self.power_event(PowerEvent::Tick(elapsed));
        if !consumed {
            for key in batch.keys {
                self.key(key, now);
            }
            for name in clicks {
                if let Some(event) = power::ui_action(&name) {
                    self.power_event(event);
                }
            }
        }
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            if self.state.finish(outcome, now) {
                if self.pending_root_geometry.is_some() {
                    self.pending_auth_success = true;
                } else {
                    self.unlocked = true;
                }
            }
        }
        if self.pending_root_geometry.is_none() && self.pending_auth_success {
            self.unlocked = true;
        }
        if self.unlocked {
            self.frame_pacer.set_running(false);
            self.destroy_surfaces(runtime)?;
            return Ok(());
        }
        self.settle_display();
        self.sync_ui_visibility(runtime);
        // A refused restore keeps the Ribbons frame up; retried here, never while Ribbons runs.
        if self.saver.phase() != Phase::RibbonsActive {
            self.retry_background_restore(runtime);
        }
        self.animate(runtime, now);
        // A lost grab is retried every turn; the lock is never dropped by a failed retry.
        self.session.borrow_mut().try_grab();
        keep_lock_step("raise lock surfaces", || self.raise_surfaces(runtime));
        self.repaint(runtime);
        Ok(())
    }

    fn apply_control_signals(&mut self) {
        let signals = std::mem::take(&mut *self.control_signals.borrow_mut());
        let (latest_settings, latest_displays) = latest_control_snapshots(signals);
        if let Some(snapshot) = latest_settings {
            self.update_live_settings(crate::session::lock_settings_from_snapshot(&snapshot));
        }
        if let Some(snapshot) = latest_displays {
            self.pending_display_snapshot = Some(snapshot);
        }
    }

    fn update_live_settings(&mut self, settings: LockSettings) {
        // This path is presentation-only; authentication state and PAM channels stay owned by
        // this lock loop and are never rebuilt for a settings revision.
        self.live_typography_dirty |= typography_changed(&self.settings, &settings);
        self.live_background_dirty |= background_changed(&self.settings, &settings);
        self.settings = settings;
        self.settings_revision += 1;
    }

    fn sync_live_settings(&mut self, runtime: &mut SurfaceRuntime) {
        if !self.live_typography_dirty && !self.live_background_dirty {
            return;
        }
        // Borrow the settings field directly: `surfaces` and `fallback` are disjoint fields.
        let settings = &self.settings;
        let root = self.root;
        let typography_dirty = self.live_typography_dirty;
        let background_dirty = self.live_background_dirty;
        let update_visible_background = self.saver.phase() != Phase::RibbonsActive;
        let canvas = if background_dirty {
            document::shared_root_background(settings, root)
        } else {
            None
        };
        let mut typography_ok = true;
        let mut background_ok = true;
        for surface in self.surfaces.iter_mut().chain(self.fallback.iter_mut()) {
            if typography_dirty {
                let result = runtime
                    .with_document(surface.handle, |view| {
                        document::apply_typography(view, settings)
                    })
                    .and_then(|()| runtime.redraw(surface.handle));
                typography_ok &= result.is_ok();
            }
            if background_dirty {
                let background =
                    document::background_for_output(settings, root, surface.rect, canvas.as_ref());
                surface.base_background = background.clone();
                if update_visible_background {
                    let result = runtime
                        .with_document(surface.handle, |view| {
                            document::set_background(view, background)
                        })
                        .and_then(|()| runtime.redraw(surface.handle));
                    background_ok &= result.is_ok();
                }
            }
        }
        if typography_ok {
            self.live_typography_dirty = false;
        }
        if background_ok {
            self.live_background_dirty = false;
        }
    }

    fn apply_display_snapshot(&mut self, runtime: &mut SurfaceRuntime, snapshot: &DisplaySnapshot) {
        let outputs = crate::session::lock_output_rects_from_snapshot(snapshot, self.root);
        let current = self
            .surfaces
            .iter()
            .map(|surface| surface.rect)
            .collect::<Vec<_>>();
        if output_rects_need_rebuild(&current, &outputs)
            && !self.rebuild_output_surfaces(runtime, outputs)
        {
            self.pending_display_snapshot = Some(snapshot.clone());
        }
    }

    fn rebuild_output_surfaces(&mut self, runtime: &mut SurfaceRuntime, rects: Vec<Rect>) -> bool {
        let mut rebuilt = Vec::with_capacity(rects.len());
        let canvas = document::shared_root_background(&self.settings, self.root);
        for (index, rect) in rects.iter().enumerate() {
            match create_lock_surface(
                runtime,
                &self.settings,
                self.root,
                canvas.as_ref(),
                *rect,
                &format!("FlameWM lock {}", index + 1),
            ) {
                Ok(surface) => rebuilt.push(surface),
                Err(error) => {
                    for surface in &rebuilt {
                        let _ = runtime.destroy(surface.handle);
                    }
                    eprintln!("flamewm-lock: monitor surface rebuild failed: {error}");
                    return false;
                }
            }
        }
        for (index, surface) in rebuilt.iter().enumerate() {
            if let Err(error) = show_raised(
                runtime,
                surface.handle,
                &format!("lock monitor {}", index + 1),
            ) {
                for surface in &rebuilt {
                    let _ = runtime.destroy(surface.handle);
                }
                eprintln!("flamewm-lock: monitor surface show failed: {error}");
                return false;
            }
        }
        let old = std::mem::replace(&mut self.surfaces, rebuilt);
        for surface in old {
            let _ = runtime.destroy(surface.handle);
        }
        self.frames = None;
        self.fallback_frames = None;
        self.shown = None;
        self.shown_power = None;
        self.ui_visibility_dirty = true;
        self.repaint(runtime);
        keep_lock_step("raise refreshed lock surfaces", || {
            self.raise_surfaces(runtime)
        });
        true
    }

    fn refresh_root_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        width: u32,
        height: u32,
    ) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let root = Rect::new(0, 0, width as i32, height as i32);
        if root == self.root {
            return true;
        }
        let Some(fallback) = self.fallback.as_mut() else {
            return false;
        };
        let canvas = document::shared_root_background(&self.settings, root);
        let background =
            document::background_for_output(&self.settings, root, root, canvas.as_ref());
        if let Err(error) = runtime.move_resize(fallback.handle, root.x, root.y, width, height) {
            eprintln!("flamewm-lock: resize root coverage failed; retaining prior lock: {error:?}");
            return false;
        }
        if let Err(error) = runtime.raise(fallback.handle) {
            eprintln!("flamewm-lock: raise resized root coverage failed: {error:?}");
            return false;
        }
        if let Err(error) = runtime.with_document(fallback.handle, |view| {
            document::apply_document(view, &self.settings, width, height, background.clone())
        }) {
            eprintln!("flamewm-lock: theme resized root coverage failed: {error:?}");
            return false;
        }
        fallback.rect = root;
        fallback.base_background = background;
        self.root = root;
        self.shown = None;
        self.shown_power = None;
        self.ui_visibility_dirty = true;
        self.repaint(runtime);

        // The full-root lock was resized first, so failures during monitor reprojection
        // keep the desktop covered by the same authentication session.
        let rects = crate::session::lock_output_rects(root);
        if !self.rebuild_output_surfaces(runtime, rects) {
            eprintln!(
                "flamewm-lock: root resized; prior monitor surfaces retained under root lock"
            );
        }
        true
    }

    fn destroy_surfaces(&self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for surface in &self.surfaces {
            runtime
                .destroy(surface.handle)
                .map_err(|error| format!("destroy lock surface: {error:?}"))?;
        }
        if let Some(fallback) = self.fallback.as_ref() {
            runtime
                .destroy(fallback.handle)
                .map_err(|error| format!("destroy lock coverage surface: {error:?}"))?;
        }
        Ok(())
    }

    /// Records input for the screensaver. `true` when it woke the screen: the input
    /// of this turn is consumed and must not reach the password or the power row.
    fn wake(&mut self, runtime: &mut SurfaceRuntime) -> bool {
        let before = self.saver.phase();
        if !self.saver.input() {
            return false;
        }
        self.frame_pacer.set_running(false);
        self.frames = None;
        self.fallback_frames = None;
        if before == Phase::RibbonsActive || before == Phase::DisplayOff {
            self.restore_background(runtime);
        }
        if before == Phase::DisplayOff {
            self.display_on_pending = true;
        }
        if before != Phase::Visible {
            // Showing the widgets again; the repaint this turn re-applies their content.
            self.ui_shown_wanted = true;
            self.ui_visibility_dirty = true;
            self.shown = None;
            self.shown_power = None;
        }
        true
    }

    /// Applies a phase the screensaver entered on idle time.
    fn enter(&mut self, runtime: &mut SurfaceRuntime, phase: Phase, now: Instant) {
        if phase != Phase::Visible {
            self.ui_shown_wanted = false;
            self.ui_visibility_dirty = true;
        }
        match phase {
            Phase::RibbonsActive => {
                self.frames = Some(
                    self.surfaces
                        .iter()
                        .map(|surface| {
                            RibbonFrames::new(
                                surface.rect.width.max(1) as u32,
                                surface.rect.height.max(1) as u32,
                                &self.settings.saver,
                                now,
                            )
                        })
                        .collect(),
                );
                self.fallback_frames = self.fallback.as_ref().map(|surface| {
                    RibbonFrames::new(
                        surface.rect.width.max(1) as u32,
                        surface.rect.height.max(1) as u32,
                        &self.settings.saver,
                        now,
                    )
                });
                self.next_frame = now;
                self.frame_pacer.set_running(true);
            }
            Phase::DisplayOff => {
                self.frame_pacer.set_running(false);
                if self.frames.take().is_some() {
                    self.restore_background(runtime);
                }
                // Fail open on the display only: a refused DPMS call leaves the screen on, still locked.
                self.session.borrow_mut().set_display_power(false);
            }
            Phase::Visible => {}
        }
    }

    /// Pushes the wanted widget visibility to the document while it is dirty.
    fn sync_ui_visibility(&mut self, runtime: &mut SurfaceRuntime) {
        if !self.ui_visibility_dirty {
            return;
        }
        let shown = self.ui_shown_wanted;
        settle(&mut self.ui_visibility_dirty, "lock UI visibility", || {
            for surface in &self.surfaces {
                runtime
                    .with_document(surface.handle, |view| {
                        document::set_lock_ui_shown(view, shown)
                    })
                    .map_err(|error| format!("set lock UI shown: {error:?}"))?;
            }
            if let Some(fallback) = self.fallback.as_ref() {
                runtime
                    .with_document(fallback.handle, |view| {
                        document::set_lock_ui_shown(view, shown)
                    })
                    .map_err(|error| format!("set fallback lock UI shown: {error:?}"))?;
            }
            Ok(())
        });
    }

    /// Retries DPMS on until the server accepts it; a refused call is logged, never fatal.
    fn settle_display(&mut self) {
        if !self.display_on_pending {
            return;
        }
        if self.session.borrow_mut().set_display_power(true) {
            self.display_on_pending = false;
        } else {
            eprintln!("flamewm-lock: display power on refused, retrying next turn");
        }
    }

    /// Renders the next Ribbons frame into the background when one is due.
    /// A failed frame keeps the previous background; `next_frame` already moved on, so the
    /// next due frame retries.
    fn animate(&mut self, runtime: &mut SurfaceRuntime, now: Instant) {
        if self.saver.phase() != Phase::RibbonsActive || now < self.next_frame {
            return;
        }
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        self.next_frame = now + FRAME_INTERVAL;
        keep_lock_step("animate lock background", || {
            update_animated_backgrounds(
                runtime,
                &self.surfaces,
                self.fallback.as_ref(),
                frames,
                self.fallback_frames.as_mut(),
                now,
            )
        });
    }

    fn restore_background(&mut self, runtime: &mut SurfaceRuntime) {
        settle(
            &mut self.background_stale,
            "lock background restore",
            || restore_surface_backgrounds(runtime, &self.surfaces, self.fallback.as_ref()),
        );
    }

    /// Re-applies the wallpaper while a failed restore is pending. Not called during Ribbons.
    fn retry_background_restore(&mut self, runtime: &mut SurfaceRuntime) {
        if self.background_stale {
            self.restore_background(runtime);
        }
    }

    fn key(&mut self, key: Key, now: Instant) {
        match key {
            Key::Enter if self.power.view().confirm.is_some() => {
                self.power_event(PowerEvent::Confirm);
            }
            Key::Function(number) => {
                if let Some(button) = usize::from(number)
                    .checked_sub(1)
                    .and_then(|i| POWER_ORDER.get(i))
                {
                    self.power_event(PowerEvent::Click(*button));
                }
            }
            _ => {
                if key == Key::Escape {
                    self.power_event(PowerEvent::Escape);
                }
                if self.state.key(key, now) == Effect::Submit {
                    self.submit();
                }
            }
        }
    }

    /// Feeds one event to the power reducer; a requested action runs on a worker thread.
    fn power_event(&mut self, event: PowerEvent) {
        // A tick only changes the power view while an error message is on screen (TTL expiry).
        let visible_change = !matches!(event, PowerEvent::Tick(_))
            || self
                .shown_power
                .as_ref()
                .is_some_and(|shown| !shown.message.is_empty());
        if visible_change {
            self.power_revision += 1;
        }
        if let PowerEffect::Perform(action) = self.power.handle(event) {
            power::perform_on_worker(action, self.power_tx.clone());
        }
    }

    fn submit(&mut self) {
        let secret = self.state.begin_authentication();
        let user = self.username.clone();
        let outcome_tx = self.outcome_tx.clone();
        let spawned = thread::Builder::new()
            .name("flamewm-lock-pam".to_string())
            .spawn(move || {
                let outcome = pam::authenticate(&user, secret.as_str());
                drop(secret);
                // The receiver lives as long as the loop; a send error means it is gone.
                let _ = outcome_tx.send(outcome);
            });
        if spawned.is_err() {
            let _ = self.outcome_tx.send(Outcome::Unavailable);
        }
    }

    /// A failed paint keeps the previous frame: `shown` moves only after a full paint, so the
    /// next turn sees the content still differs and retries.
    fn repaint(&mut self, runtime: &mut SurfaceRuntime) {
        // While the screensaver runs the lock UI stays hidden: no content is projected.
        if self.saver.phase() != Phase::Visible {
            return;
        }
        let key = PaintKey {
            time: local_now(),
            password_len: self.state.password().len(),
            status: self.state.status(),
            caps_lock: self.caps_lock,
            settings_revision: self.settings_revision,
            power_revision: self.power_revision,
        };
        if self.shown.is_some() && self.painted_key == Some(key) {
            return;
        }
        let mut view = LockView::project(
            &self.settings,
            key.time,
            &self.username,
            self.state.password(),
            self.state.status(),
        );
        if view.status.is_empty() && self.caps_lock {
            view.status = CAPS_LOCK_HINT;
        }
        let power = self.power.view();
        let view_changed = self.shown.as_ref() != Some(&view);
        let power_changed = self.shown_power.as_ref() != Some(&power);
        if !view_changed && !power_changed {
            self.painted_key = Some(key);
            return;
        }
        let painted = keep_lock_step("repaint lock surface", || {
            for surface in &self.surfaces {
                runtime
                    .with_document(surface.handle, |document_view| {
                        if view_changed {
                            document::apply_view(document_view, &view)?;
                        }
                        if power_changed {
                            document::apply_power(
                                document_view,
                                surface.rect.width.max(1) as u32,
                                surface.rect.height.max(1) as u32,
                                &power,
                            )?;
                        }
                        Ok(())
                    })
                    .map_err(|error| format!("update lock surface: {error:?}"))?;
                runtime
                    .redraw(surface.handle)
                    .map_err(|error| format!("redraw lock surface: {error:?}"))?;
            }
            if let Some(surface) = self.fallback.as_ref() {
                runtime
                    .with_document(surface.handle, |document_view| {
                        if view_changed {
                            document::apply_view(document_view, &view)?;
                        }
                        if power_changed {
                            document::apply_power(
                                document_view,
                                surface.rect.width.max(1) as u32,
                                surface.rect.height.max(1) as u32,
                                &power,
                            )?;
                        }
                        Ok(())
                    })
                    .map_err(|error| format!("update fallback lock surface: {error:?}"))?;
                runtime
                    .redraw(surface.handle)
                    .map_err(|error| format!("redraw fallback lock surface: {error:?}"))?;
            }
            Ok(())
        });
        if painted {
            self.shown = Some(view);
            self.shown_power = Some(power);
            self.painted_key = Some(key);
        }
    }
}

/// Wakes the reactor about every [`FRAME_INTERVAL`] while the Ribbons animation runs.
/// The tick has no reactor handle, so the wake travels over a socket pair the reactor
/// watches. The pacer thread sleeps on a condvar while the animation is off.
struct FramePacer {
    running: Arc<(Mutex<bool>, Condvar)>,
}

impl FramePacer {
    fn start(writer: UnixStream) -> Result<Self, String> {
        writer
            .set_nonblocking(true)
            .map_err(|error| format!("frame pacer socket: {error}"))?;
        let running = Arc::new((Mutex::new(false), Condvar::new()));
        let shared = Arc::clone(&running);
        thread::Builder::new()
            .name("flamewm-lock-frames".to_string())
            .spawn(move || {
                let (flag, changed) = &*shared;
                loop {
                    let mut on = flag.lock().unwrap_or_else(PoisonError::into_inner);
                    while !*on {
                        on = changed.wait(on).unwrap_or_else(PoisonError::into_inner);
                    }
                    drop(on);
                    thread::sleep(FRAME_INTERVAL);
                    // A full socket means a wake is already pending; a closed one means the loop is gone.
                    if let Err(error) = (&writer).write(&[1]) {
                        if error.kind() == ErrorKind::BrokenPipe {
                            return;
                        }
                    }
                }
            })
            .map_err(|error| format!("frame pacer thread: {error}"))?;
        Ok(Self { running })
    }

    fn set_running(&self, on: bool) {
        let (flag, changed) = &*self.running;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = on;
        changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use flamewm_ui_x11::SurfaceRuntime;

    use super::*;

    #[test]
    fn live_settings_detect_typography_and_wallpaper_changes() {
        let defaults = LockSettings::default();
        let bold = LockSettings {
            font_bold: true,
            ..defaults.clone()
        };
        assert!(typography_changed(&defaults, &bold));
        assert!(!background_changed(&defaults, &bold));
        let shared = LockSettings {
            share_wallpaper: true,
            ..defaults.clone()
        };
        assert!(background_changed(&defaults, &shared));
        assert!(!typography_changed(&defaults, &shared));
    }

    #[test]
    fn control_signal_reducer_keeps_the_latest_typed_snapshots() {
        let settings = flamewm_api::settings::SettingsSnapshot::new(2);
        let displays = DisplaySnapshot {
            generation: 3,
            outputs: Vec::new(),
            pending: None,
        };
        let (settings, displays) = latest_control_snapshots([
            ControlSignal::SettingsSnapshotChanged {
                snapshot: flamewm_api::settings::SettingsSnapshot::new(1),
            },
            ControlSignal::SettingsSnapshotChanged { snapshot: settings },
            ControlSignal::DisplaysSnapshotChanged { snapshot: displays },
        ]);
        assert_eq!(settings.expect("settings snapshot").revision, 2);
        assert_eq!(displays.expect("display snapshot").generation, 3);
    }

    #[test]
    fn display_rect_changes_rebuild_even_when_root_size_is_unchanged() {
        let root_before = Rect::new(0, 0, 1920, 1080);
        let root_after = Rect::new(0, 0, 1920, 1080);
        let current = [Rect::new(0, 0, 1920, 1080)];
        let rearranged = [Rect::new(0, 0, 1280, 720), Rect::new(1280, 0, 640, 1080)];
        assert!(!output_rects_need_rebuild(&current, &current));
        assert!(output_rects_need_rebuild(&current, &rearranged));
        assert_eq!(root_before, root_after, "root geometry remains unchanged");
    }

    #[test]
    fn live_settings_projection_does_not_replace_authentication_owners() {
        let body = method_body("fn update_live_settings(");
        assert!(!body.contains("self.state"));
        assert!(!body.contains("outcome_tx") && !body.contains("outcome_rx"));
        assert!(body.contains("live_typography_dirty"));
    }

    #[test]
    fn live_output_projection_uses_the_same_canonical_clipping_fallback() {
        let root = Rect::new(0, 0, 1920, 1080);
        let empty = DisplaySnapshot {
            generation: 1,
            outputs: Vec::new(),
            pending: None,
        };
        assert_eq!(
            crate::session::lock_output_rects_from_snapshot(&empty, root),
            vec![root]
        );
    }

    /// Needs a live X display (`DISPLAY`). Creates and drops the render stack
    /// normally: a regression here is a SIGSEGV at `XCloseDisplay`.
    #[test]
    #[ignore = "requires DISPLAY"]
    fn runtime_drops_cleanly_after_xft_and_xrender_load() {
        let runtime = SurfaceRuntime::new().expect("create UI runtime");
        drop(runtime);
    }

    /// The shared preview body must never reach a grab, PAM, a screensaver or DPMS.
    #[test]
    fn preview_loop_has_no_grab_auth_screensaver_or_dpms_call_sites() {
        let source = include_str!("run.rs");
        let start = source.find("fn preview_loop(").expect("preview loop");
        let end = source
            .find("fn lock_on(")
            .expect("lock loop follows the preview");
        assert!(start < end, "preview loop must sit before the lock loop");
        let body = &source[start..end];
        for forbidden in [
            "set_display_power",
            "grab_pointer",
            "grab_with_retry",
            "try_grab",
            "pam::",
            "Screensaver::new",
            "perform_on_worker",
        ] {
            assert!(
                !body.contains(forbidden),
                "preview path reaches {forbidden}"
            );
        }
    }

    /// The Ribbons preview builds no lock projection; the lock preview builds exactly one.
    #[test]
    fn ribbons_preview_never_projects_the_lock_ui() {
        let mut calls = 0;
        assert_eq!(preview_lock_ui(true, || calls += 1), None);
        assert_eq!(calls, 0, "ribbons preview called the lock projection");
        assert_eq!(
            preview_lock_ui(false, || {
                calls += 1;
                7
            }),
            Some(7)
        );
        assert_eq!(calls, 1);
    }

    /// A failing lock-screen step is logged and reported, never propagated: the loop
    /// keeps its grabs and retries on a later turn.
    #[test]
    fn failing_lock_step_is_logged_and_never_fatal() {
        let mut attempts = 0;
        let ok = keep_lock_step("test step", || {
            attempts += 1;
            Err("document unavailable".to_string())
        });
        assert!(!ok);
        assert_eq!(attempts, 1);
        assert!(keep_lock_step("test step", || Ok(())));
    }

    /// Body of the method in this file whose signature starts with `name`.
    fn method_body(name: &str) -> &'static str {
        let source = include_str!("run.rs");
        let start = source.find(name).expect("function present");
        let end = start + source[start..].find("\n    }\n").expect("function end");
        &source[start..end]
    }

    /// The loop-reachable functions must not propagate with `?`: a rendering or
    /// background error there would end `tick()` and drop the lock. The only tolerated
    /// propagation is the destroy after a successful authentication.
    #[test]
    fn lock_loop_paths_never_propagate_render_errors() {
        let body = method_body;
        for name in [
            "fn wake(",
            "fn enter(",
            "fn animate(",
            "fn restore_background(",
            "fn repaint(",
        ] {
            assert!(!body(name).contains("-> Result"), "{name} returns Result");
        }
        for name in [
            "fn wake(",
            "fn enter(",
            "fn animate(",
            "fn restore_background(",
        ] {
            assert_eq!(body(name).matches(")?").count(), 0, "{name} propagates");
        }
        assert_eq!(body("fn tick(").matches(")?").count(), 1, "tick propagates");
    }

    /// Hiding on entry and restoring on wake must both exist, or the widgets stay hidden.
    #[test]
    fn saver_hides_lock_ui_on_entry_and_restores_on_wake() {
        let body = method_body;
        assert!(body("fn enter(").contains("ui_shown_wanted = false"));
        assert!(body("fn wake(").contains("ui_shown_wanted = true"));
        assert!(body("fn sync_ui_visibility(").contains("set_lock_ui_shown(view, shown)"));
    }

    /// The projection is skipped only while the paint key matches; every input bumps a revision.
    #[test]
    fn repaint_skips_projection_on_unchanged_paint_key() {
        let body = method_body;
        assert!(body("fn repaint(").contains("self.painted_key == Some(key)"));
        assert!(body("fn power_event(").contains("self.power_revision += 1"));
        assert!(body("fn update_live_settings(").contains("self.settings_revision += 1"));
    }

    /// A refused wallpaper restore must stay pending and be retried; a success clears it.
    #[test]
    fn failed_wallpaper_restore_stays_pending_until_it_succeeds() {
        let mut stale = false;
        let mut attempts = 0;
        settle(&mut stale, "test restore", || {
            attempts += 1;
            Err("document unavailable".to_string())
        });
        assert!(
            stale,
            "a refused restore must stay pending, not drop the Ribbons frame silently"
        );
        settle(&mut stale, "test restore", || {
            attempts += 1;
            Ok(())
        });
        assert!(!stale, "a successful retry clears the pending restore");
        assert_eq!(attempts, 2);
    }

    /// A refused widget update must keep the loop alive: it is retried on the next turn,
    /// and a success clears the dirty flag.
    #[test]
    fn failed_widget_visibility_is_retried_next_turn() {
        let mut dirty = true;
        let mut attempts = 0;
        settle(&mut dirty, "test visibility", || {
            attempts += 1;
            Err("document unavailable".to_string())
        });
        assert!(dirty, "a failed update stays dirty for the next tick");
        settle(&mut dirty, "test visibility", || {
            attempts += 1;
            Ok(())
        });
        assert!(!dirty, "a successful retry clears the flag");
        assert_eq!(attempts, 2, "the retry re-applies the same wanted state");
    }

    /// Counts the frame wakes that arrive within `window`.
    fn wakes_within(reader: &mut UnixStream, window: Duration) -> usize {
        let deadline = Instant::now() + window;
        let mut count = 0;
        let mut byte = [0u8; 1];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return count;
            }
            reader
                .set_read_timeout(Some(remaining))
                .expect("read timeout");
            match reader.read(&mut byte) {
                Ok(1) => count += 1,
                _ => return count,
            }
        }
    }

    #[test]
    fn frame_pacer_wakes_only_while_the_animation_runs() {
        let (mut reader, writer) = UnixStream::pair().expect("socket pair");
        let pacer = FramePacer::start(writer).expect("start pacer");
        assert_eq!(
            wakes_within(&mut reader, Duration::from_millis(150)),
            0,
            "idle pacer must stay silent"
        );
        pacer.set_running(true);
        assert!(
            wakes_within(&mut reader, Duration::from_millis(300)) >= 3,
            "active pacer ticks at frame rate"
        );
        pacer.set_running(false);
        thread::sleep(FRAME_INTERVAL * 2);
        let _ = wakes_within(&mut reader, Duration::from_millis(10));
        assert_eq!(
            wakes_within(&mut reader, Duration::from_millis(150)),
            0,
            "stopped pacer must stay silent"
        );
    }
}
