//! Interactive lock loop: one override-redirect surface, keyboard and pointer
//! grabs, PAM and power actions on worker threads, the idle screensaver, and a
//! 1 s clock tick. Input that wakes the screen is consumed in the same turn.
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
use flamewm_api::session::SessionCapabilities;
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
const TICK: Duration = Duration::from_secs(1);
/// Poll interval of the preview: pointer and keyboard are read by polling.
const PREVIEW_POLL: Duration = Duration::from_millis(50);
/// Shown in the status line while Caps Lock is on and no other status applies.
const CAPS_LOCK_HINT: &str = "Caps Lock is on";
/// Primary mouse button in X core numbering.
const BUTTON_PRIMARY: u32 = 1;

/// Shows the lock screen until an authentication succeeds. Returns `Err`
/// before anything is visible when the grabs cannot be taken.
pub fn run(settings: &LockSettings, username: &str) -> Result<(), String> {
    let (width, height) = flamewm_ui_x11::root_geometry()?;
    let template = document::template()?;
    let mut runtime =
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
    lock_on(&mut runtime, settings, username, width, height, template)
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
        .create_surface(
            template,
            SurfaceConfig {
                width,
                height,
                title: title.to_string(),
                role: SurfaceRole::Overlay,
                input: SurfaceInputMode::Interactive,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )
        .map_err(|error| format!("create preview surface: {error:?}"))?;
    let started = Instant::now();
    let mut frames = ribbons.then(|| RibbonFrames::new(width, height, &settings.saver, started));
    let background = match frames.as_mut() {
        Some(frames) => frames.frame(started),
        None => document::background_image(settings, width, height),
    };
    let mut next_frame = started + FRAME_INTERVAL;
    let power = PowerUi::new(SessionCapabilities::default()).view();
    let view = LockView::project(
        settings,
        local_now(),
        username,
        &PasswordBuffer::default(),
        Status::default(),
    );
    runtime
        .with_document(surface, |document_view| {
            document::apply_document(document_view, settings, width, height, background)?;
            document::apply_view(document_view, &view)?;
            document::apply_power(document_view, width, height, &power)
        })
        .map_err(|error| format!("theme preview surface: {error:?}"))?;
    runtime
        .show(surface)
        .map_err(|error| format!("show preview surface: {error:?}"))?;
    runtime
        .raise(surface)
        .map_err(|error| format!("raise preview surface: {error:?}"))?;
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
    width: u32,
    height: u32,
    template: flamewm_ui_x11::UiTemplate,
) -> Result<(), String> {
    let surface = runtime
        .create_surface(
            template,
            SurfaceConfig {
                width,
                height,
                title: "FlameWM lock".to_string(),
                role: SurfaceRole::Overlay,
                input: SurfaceInputMode::Interactive,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )
        .map_err(|error| format!("create lock surface: {error:?}"))?;

    let mut session = Session::open()?;
    if !grab_with_retry(&mut session, GRAB_TIMEOUT) {
        return Err("cannot grab keyboard and pointer; not locking".to_string());
    }
    let session = Rc::new(RefCell::new(session));
    let keys = Rc::new(RefCell::new(KeyBatch::default()));
    let clicks = Rc::new(RefCell::new(Vec::<String>::new()));

    let background = document::background_image(settings, width, height);
    let base_background = background.clone();
    runtime
        .with_document(surface, |view| {
            document::apply_document(view, settings, width, height, background)
        })
        .map_err(|error| format!("theme lock surface: {error:?}"))?;

    let (outcome_tx, outcome_rx) = mpsc::channel::<Outcome>();
    let (power_tx, power_rx) = mpsc::channel::<PowerEvent>();
    power::fetch_capabilities_on_worker(power_tx.clone());
    let (wake_reader, wake_writer) =
        UnixStream::pair().map_err(|error| format!("frame pacer socket: {error}"))?;
    let frame_pacer = FramePacer::start(wake_writer)?;
    let mut lock = LockLoop {
        surface,
        settings: settings.clone(),
        session: Rc::clone(&session),
        keys: Rc::clone(&keys),
        clicks: Rc::clone(&clicks),
        state: LockState::default(),
        power: PowerUi::new(SessionCapabilities::default()),
        power_tx,
        power_rx,
        shown_power: None,
        saver: Screensaver::new(settings.saver.clone()),
        frames: None,
        next_frame: Instant::now(),
        frame_pacer,
        base_background,
        width,
        height,
        last_tick: Instant::now(),
        last_pointer: None,
        caps_lock: false,
        shown: None,
        username: username.to_string(),
        outcome_tx,
        outcome_rx,
        unlocked: false,
    };
    lock.repaint(runtime)?;
    runtime
        .show(surface)
        .map_err(|error| format!("show lock surface: {error:?}"))?;
    runtime
        .raise(surface)
        .map_err(|error| format!("raise lock surface: {error:?}"))?;
    // The lock is already visible under its keyboard grab; a pointer grab that is not yet
    // granted is retried every tick, so the lock never drops for it.
    let _ = runtime.grab_pointer(surface);

    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    let key_fd = session.borrow().fd();
    let key_queue = Rc::clone(&keys);
    let key_session = Rc::clone(&session);
    reactor
        .register_raw_fd_with_action(key_fd, Interest::READ, move |_, _| {
            let read = key_session.borrow_mut().read_keys();
            let mut queue = key_queue.borrow_mut();
            queue.keys.extend(read.keys);
            queue.presses += read.presses;
            FdAction::Continue
        })
        .map_err(|error| format!("watch keyboard events: {error}"))?;
    let mut wake_reader = wake_reader;
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
    // The timer bounds each wait so the clock, PAM and power results refresh once a second.
    reactor
        .register_timer(TICK, true, || {})
        .map_err(|error| format!("register lock tick: {error}"))?;

    let click_queue = Rc::clone(&clicks);
    run_surface_runtime_with_reactor_access(
        runtime,
        &mut reactor,
        move |event, _runtime| {
            // Power icons, the confirm strip and the Other list are action nodes; only
            // a primary press means a click. Queued here, applied in the tick.
            if event.action.phase == ActionPhase::Press && event.action.button == BUTTON_PRIMARY {
                click_queue.borrow_mut().push(event.action.action);
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

struct LockLoop {
    surface: SurfaceHandle,
    settings: LockSettings,
    session: Rc<RefCell<Session>>,
    keys: Rc<RefCell<KeyBatch>>,
    clicks: Rc<RefCell<Vec<String>>>,
    state: LockState,
    power: PowerUi,
    power_tx: Sender<PowerEvent>,
    power_rx: Receiver<PowerEvent>,
    shown_power: Option<PowerView>,
    saver: Screensaver,
    frames: Option<RibbonFrames>,
    next_frame: Instant,
    frame_pacer: FramePacer,
    base_background: RuntimeImage,
    width: u32,
    height: u32,
    last_tick: Instant,
    last_pointer: Option<PointerState>,
    caps_lock: bool,
    shown: Option<LockView>,
    username: String,
    outcome_tx: Sender<Outcome>,
    outcome_rx: Receiver<Outcome>,
    unlocked: bool,
}

impl LockLoop {
    /// One loop turn: wake the screen on input (consuming it), apply keys, clicks,
    /// PAM and power results, animate the screensaver, keep the grabs and stacking,
    /// then repaint only when the projected content changed.
    fn tick(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
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
            self.wake(runtime)?
        } else {
            if let Some(phase) = self.saver.tick(elapsed) {
                self.enter(runtime, phase, now)?;
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
                self.unlocked = true;
            }
        }
        if self.unlocked {
            self.frame_pacer.set_running(false);
            runtime
                .destroy(self.surface)
                .map_err(|error| format!("destroy lock surface: {error:?}"))?;
            return Ok(());
        }
        self.animate(runtime, now)?;
        // A lost grab is retried every turn; the lock is never dropped by a failed retry.
        self.session.borrow_mut().try_grab();
        if runtime.is_pointer_grabbed(self.surface) != Ok(true) {
            let _ = runtime.grab_pointer(self.surface);
        }
        runtime
            .raise(self.surface)
            .map_err(|error| format!("raise lock surface: {error:?}"))?;
        self.repaint(runtime)
    }

    /// Records input for the screensaver. `true` when it woke the screen: the input
    /// of this turn is consumed and must not reach the password or the power row.
    fn wake(&mut self, runtime: &mut SurfaceRuntime) -> Result<bool, String> {
        let before = self.saver.phase();
        if !self.saver.input() {
            return Ok(false);
        }
        self.frame_pacer.set_running(false);
        self.frames = None;
        if before == Phase::RibbonsActive || before == Phase::DisplayOff {
            self.restore_background(runtime)?;
        }
        if before == Phase::DisplayOff {
            self.session.borrow_mut().set_display_power(true);
        }
        Ok(true)
    }

    /// Applies a phase the screensaver entered on idle time.
    fn enter(
        &mut self,
        runtime: &mut SurfaceRuntime,
        phase: Phase,
        now: Instant,
    ) -> Result<(), String> {
        match phase {
            Phase::RibbonsActive => {
                self.frames = Some(RibbonFrames::new(
                    self.width,
                    self.height,
                    &self.settings.saver,
                    now,
                ));
                self.next_frame = now;
                self.frame_pacer.set_running(true);
            }
            Phase::DisplayOff => {
                self.frame_pacer.set_running(false);
                if self.frames.take().is_some() {
                    self.restore_background(runtime)?;
                }
                // Fail open on the display only: a refused DPMS call leaves the screen on, still locked.
                self.session.borrow_mut().set_display_power(false);
            }
            Phase::Visible => {}
        }
        Ok(())
    }

    /// Renders the next Ribbons frame into the background when one is due.
    fn animate(&mut self, runtime: &mut SurfaceRuntime, now: Instant) -> Result<(), String> {
        if self.saver.phase() != Phase::RibbonsActive || now < self.next_frame {
            return Ok(());
        }
        let Some(frames) = self.frames.as_mut() else {
            return Ok(());
        };
        let image = frames.frame(now);
        self.next_frame = now + FRAME_INTERVAL;
        runtime
            .with_document(self.surface, |view| document::set_background(view, image))
            .map_err(|error| format!("update lock background: {error:?}"))?;
        runtime
            .redraw(self.surface)
            .map_err(|error| format!("redraw lock surface: {error:?}"))
    }

    fn restore_background(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        let base = self.base_background.clone();
        runtime
            .with_document(self.surface, |view| document::set_background(view, base))
            .map_err(|error| format!("restore lock background: {error:?}"))
    }

    fn key(&mut self, key: Key, now: Instant) {
        match key {
            Key::Enter if self.power.view().confirm.is_some() => {
                self.power_event(PowerEvent::Confirm)
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

    fn repaint(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        let mut view = LockView::project(
            &self.settings,
            local_now(),
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
            return Ok(());
        }
        let (width, height) = (self.width, self.height);
        runtime
            .with_document(self.surface, |document_view| {
                if view_changed {
                    document::apply_view(document_view, &view)?;
                }
                if power_changed {
                    document::apply_power(document_view, width, height, &power)?;
                }
                Ok(())
            })
            .map_err(|error| format!("update lock surface: {error:?}"))?;
        runtime
            .redraw(self.surface)
            .map_err(|error| format!("redraw lock surface: {error:?}"))?;
        self.shown = Some(view);
        self.shown_power = Some(power);
        Ok(())
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

    /// Needs a live X display (`DISPLAY`). Creates and drops the render stack
    /// normally: a regression here is a SIGSEGV at XCloseDisplay.
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
