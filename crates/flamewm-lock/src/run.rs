//! Interactive lock loop: one override-redirect surface, keyboard and pointer
//! grabs, PAM on a worker thread, and a 1 s clock tick.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use calloop::Interest;
use flamewm_reactor::{FdAction, Reactor};
use flamewm_ui_x11::{
    SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiBackendError,
    run_surface_runtime_with_reactor_access,
};

use crate::document;
use crate::native_time::local_now;
use crate::pam::{self, Outcome};
use crate::settings::LockSettings;
use crate::state::{Effect, Key, LockState};
use crate::view::LockView;
use crate::x11::{Session, grab_with_retry};

const GRAB_TIMEOUT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_secs(1);

/// Shows the lock screen until an authentication succeeds. Returns `Err`
/// before anything is visible when the grabs cannot be taken.
pub fn run(settings: &LockSettings, username: &str) -> Result<(), String> {
    let (width, height) = flamewm_ui_x11::root_geometry()?;
    let template = document::template()?;
    let mut runtime =
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
    let outcome = lock_on(&mut runtime, settings, username, width, height, template);
    // The render stack's display owner must not be dropped here: libXft and
    // libXrender install XESetCloseDisplay hooks on the display, and their
    // dlclose'd code is unmapped by the time XCloseDisplay runs (SIGSEGV).
    // The process exits right after this, which closes the connection and
    // releases the grabs in the kernel's X client teardown.
    std::mem::forget(runtime);
    outcome
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
    let keys = Rc::new(RefCell::new(Vec::<Key>::new()));

    let background = document::background_for(settings, width, height);
    runtime
        .with_document(surface, |view| {
            document::apply_theme(view, settings, background)
        })
        .map_err(|error| format!("theme lock surface: {error:?}"))?;

    let (outcome_tx, outcome_rx) = mpsc::channel::<Outcome>();
    let mut lock = LockLoop {
        surface,
        settings: settings.clone(),
        session: Rc::clone(&session),
        keys: Rc::clone(&keys),
        state: LockState::default(),
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

    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    let key_fd = session.borrow().fd();
    let key_queue = Rc::clone(&keys);
    let key_session = Rc::clone(&session);
    reactor
        .register_raw_fd_with_action(key_fd, Interest::READ, move |_, _| {
            let read = key_session.borrow_mut().read_keys();
            key_queue.borrow_mut().extend(read);
            FdAction::Continue
        })
        .map_err(|error| format!("watch keyboard events: {error}"))?;
    // The timer bounds each wait so the clock and PAM results refresh once a second.
    reactor
        .register_timer(TICK, true, || {})
        .map_err(|error| format!("register lock tick: {error}"))?;

    run_surface_runtime_with_reactor_access(
        runtime,
        &mut reactor,
        |_event, _runtime| Ok(()),
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
    keys: Rc<RefCell<Vec<Key>>>,
    state: LockState,
    shown: Option<LockView>,
    username: String,
    outcome_tx: Sender<Outcome>,
    outcome_rx: Receiver<Outcome>,
    unlocked: bool,
}

impl LockLoop {
    /// One loop turn: apply keys and PAM results, keep the grabs and stacking,
    /// then repaint only when the projected content changed.
    fn tick(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        let now = Instant::now();
        let pending: Vec<Key> = self.keys.borrow_mut().drain(..).collect();
        for key in pending {
            if self.state.key(key, now) == Effect::Submit {
                self.submit();
            }
        }
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            if self.state.finish(outcome, now) {
                self.unlocked = true;
            }
        }
        if self.unlocked {
            runtime
                .destroy(self.surface)
                .map_err(|error| format!("destroy lock surface: {error:?}"))?;
            return Ok(());
        }
        // A lost grab is retried every turn; the lock is never dropped by a failed retry.
        self.session.borrow_mut().try_grab();
        runtime
            .raise(self.surface)
            .map_err(|error| format!("raise lock surface: {error:?}"))?;
        self.repaint(runtime)
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
        let view = LockView::project(
            &self.settings,
            local_now(),
            &self.username,
            self.state.password(),
            self.state.status(),
        );
        if self.shown.as_ref() == Some(&view) {
            return Ok(());
        }
        runtime
            .with_document(self.surface, |document_view| {
                document::apply_view(document_view, &view)
            })
            .map_err(|error| format!("update lock surface: {error:?}"))?;
        runtime
            .redraw(self.surface)
            .map_err(|error| format!("redraw lock surface: {error:?}"))?;
        self.shown = Some(view);
        Ok(())
    }
}
