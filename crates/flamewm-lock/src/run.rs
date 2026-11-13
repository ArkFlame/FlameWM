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
use crate::preview::PreviewInput;
use crate::ribbons::{FRAME_INTERVAL, RibbonFrames};
use crate::screensaver::{Phase, Screensaver};
use crate::secret::PasswordBuffer;

mod change;
mod control_watch;
mod idle;
mod input;
mod live;
mod paint;
mod preview_mode;
mod steps;
mod surface;
mod surfaces;
mod tick;

use self::change::{
    background_changed, latest_control_snapshots, output_rects_need_rebuild, typography_changed,
};
use self::control_watch::watch_control_signals;
use self::preview_mode::preview_loop;
use self::steps::{keep_lock_step, settle};
use self::surface::{
    create_lock_surface, overlay_config, restore_surface_backgrounds, show_raised,
    update_animated_backgrounds,
};
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
/// Primary mouse button in X core numbering.
const BUTTON_PRIMARY: u32 = 1;

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

#[derive(Clone)]
struct LockSurface {
    handle: SurfaceHandle,
    rect: Rect,
    base_background: RuntimeImage,
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
                    if let Err(error) = (&writer).write(&[1])
                        && error.kind() == ErrorKind::BrokenPipe
                    {
                        return;
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
mod tests;
