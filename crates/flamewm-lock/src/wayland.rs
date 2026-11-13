//! Wayland lock client: the protocol side of presenting the lock without X11.
//!
//! `present` turns the lock document into an RGBA8 frame; this module puts that frame on screen.
//! It drives `ext-session-lock-v1` as an ordinary client — connect, lock, and for each configured
//! lock surface attach a shared-memory buffer holding the painted frame.
//!
//! The compositor is the authority: it confirms the lock, decides each surface size, and shows
//! nothing else while the surfaces are up. This code never draws over another client.

use std::os::unix::io::AsFd;
use wayland_client::WEnum;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_keyboard::WlKeyboard;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::{Event as RegistryEvent, WlRegistry};
use wayland_client::protocol::wl_seat::{Event as SeatEvent, WlSeat};
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::{
    Event as LockSurfaceEvent, ExtSessionLockSurfaceV1,
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::{
    Event as LockEvent, ExtSessionLockV1,
};

use crate::present::Frame;

/// A lock surface the compositor configured, and the size it asked for.
pub struct ConfiguredSurface {
    pub surface: WlSurface,
    pub lock_surface: ExtSessionLockSurfaceV1,
    pub width: u32,
    pub height: u32,
}

/// What the client state needs from the application: pixels for a configured size.
pub type FrameFn = Box<dyn FnMut(&crate::view::LockView, u32, u32) -> Result<Frame, String>>;

/// Reason the lock could not be presented.
#[derive(Debug)]
pub enum LockError {
    /// No `WAYLAND_DISPLAY`, or the connection failed.
    Connect(String),
    /// The compositor does not offer `ext_session_lock_manager_v1`.
    NoGlobal,
    /// The compositor refused the lock and sent `finished`.
    Refused,
    /// A surface was configured but its frame could not be painted.
    Paint(String),
    /// The protocol exchange failed.
    Protocol(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(message) => write!(f, "connect to Wayland: {message}"),
            Self::NoGlobal => write!(f, "compositor does not offer ext_session_lock_manager_v1"),
            Self::Refused => write!(f, "compositor refused the session lock"),
            Self::Paint(message) => write!(f, "paint lock surface: {message}"),
            Self::Protocol(message) => write!(f, "session lock protocol: {message}"),
        }
    }
}

impl std::error::Error for LockError {}

/// Client-side state, owned by the event loop.
pub struct LockClient {
    frame: FrameFn,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    seat: Option<WlSeat>,
    pub lock: Option<ExtSessionLockV1>,
    /// Lock surfaces created but not yet configured.
    pending: Vec<(WlSurface, ExtSessionLockSurfaceV1, WlOutput)>,
    /// Surfaces the compositor configured.
    pub configured: Vec<ConfiguredSurface>,
    /// Set once the compositor reports `locked`; nothing else may be drawn from then on.
    pub locked: bool,
    /// Set when the compositor refused or ended the lock.
    pub finished: bool,
    /// Buffers and pools to keep alive while the compositor reads them.
    retained: Vec<(WlBuffer, WlShmPool, std::fs::File)>,
    /// The lock state machine: password buffer, status, backoff.
    pub(crate) state: crate::state::LockState,
    /// Password decoding for keys the seat sends.
    pub(crate) input: crate::seat::SeatInput,
    /// Where a PAM outcome arrives from the worker thread.
    pub(crate) outcomes: Option<std::sync::mpsc::Receiver<crate::pam::Outcome>>,
    /// User being authenticated.
    pub(crate) username: String,
    /// Settings the lock paints with.
    pub(crate) settings: crate::settings::LockSettings,
    /// How a submitted password is checked.
    pub(crate) authenticate: std::sync::Arc<Authenticator>,
    /// Current projection of the lock state; refreshed before each paint.
    pub(crate) view: crate::view::LockView,
    /// Set when authentication succeeded; the loop unlocks and exits.
    pub(crate) unlock: bool,
    /// A key changed what should be on screen.
    ///
    /// Painting a full lock document is expensive; painting once per loop turn instead of once
    /// per key keeps the event queue from being starved while a password is typed.
    pub(crate) dirty: bool,
}

impl LockClient {
    pub fn new(frame: FrameFn) -> Self {
        Self {
            frame,
            compositor: None,
            shm: None,
            seat: None,
            lock: None,
            pending: Vec::new(),
            configured: Vec::new(),
            locked: false,
            finished: false,
            retained: Vec::new(),
            state: crate::state::LockState::default(),
            input: crate::seat::SeatInput::default(),
            outcomes: None,
            username: String::new(),
            settings: crate::settings::LockSettings::default(),
            authenticate: std::sync::Arc::new(Box::new(crate::pam::authenticate)),
            view: crate::view::LockView::project(
                &crate::settings::LockSettings::default(),
                None,
                "",
                &crate::secret::PasswordBuffer::default(),
                crate::state::Status::Idle,
                false,
            ),
            unlock: false,
            dirty: false,
        }
    }

    /// Requests the lock and one lock surface per output.
    ///
    /// # Errors
    /// Returns [`LockError::NoGlobal`] when the compositor does not offer the protocol, and
    /// [`LockError::Connect`] when the connection or registry handshake fails.
    pub fn lock(
        &mut self,
        connection: &Connection,
    ) -> Result<(wayland_client::EventQueue<Self>, QueueHandle<Self>), LockError> {
        let (globals, queue) = registry_queue_init::<Self>(connection)
            .map_err(|error| LockError::Connect(error.to_string()))?;
        let qh = queue.handle();

        let manager = globals
            .bind::<ExtSessionLockManagerV1, _, _>(&qh, 1..=1, ())
            .map_err(|_| LockError::NoGlobal)?;

        let compositor = globals
            .contents()
            .clone_list()
            .into_iter()
            .find(|global| global.interface == "wl_compositor")
            .map(|global| {
                globals.registry().bind::<WlCompositor, _, _>(
                    global.name,
                    global.version.min(6),
                    &qh,
                    (),
                )
            })
            .ok_or_else(|| LockError::Connect("no wl_compositor".to_string()))?;
        let shm = globals
            .contents()
            .clone_list()
            .into_iter()
            .find(|global| global.interface == "wl_shm")
            .map(|global| {
                globals
                    .registry()
                    .bind::<WlShm, _, _>(global.name, global.version.min(1), &qh, ())
            })
            .ok_or_else(|| LockError::Connect("no wl_shm".to_string()))?;

        let outputs: Vec<WlOutput> = globals
            .contents()
            .clone_list()
            .into_iter()
            .filter(|global| global.interface == "wl_output")
            .map(|global| {
                globals.registry().bind::<WlOutput, _, _>(
                    global.name,
                    global.version.min(4),
                    &qh,
                    (),
                )
            })
            .collect();

        // The seat carries the password: without it the lock could never be unlocked. The
        // keyboard is created now, before the lock is requested, because a compositor only sends
        // key events to a client that already holds a keyboard resource when it gives the lock
        // surface focus. Creating it later would leave the lock permanently deaf.
        let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=9, ()).ok();
        let keyboard = seat.as_ref().map(|seat| seat.get_keyboard(&qh, ()));

        let lock = manager.lock(&qh, ());
        // Surfaces must exist before the lock confirms, so create them now.
        for output in outputs {
            let surface = compositor.create_surface(&qh, ());
            let lock_surface = lock.get_lock_surface(&surface, &output, &qh, ());
            self.pending.push((surface, lock_surface, output));
        }

        self.compositor = Some(compositor);
        self.shm = Some(shm);
        self.seat = seat;
        self.input.keyboard = keyboard;
        self.lock = Some(lock);
        Ok((queue, qh))
    }

    /// Attaches a freshly painted frame to `surface` and commits it.
    fn present(
        &mut self,
        surface: &ConfiguredSurface,
        qh: &QueueHandle<Self>,
    ) -> Result<(), LockError> {
        let shm = self.shm.clone().ok_or(LockError::NoGlobal)?;
        let frame =
            (self.frame)(&self.view, surface.width, surface.height).map_err(LockError::Paint)?;
        if frame.width != surface.width || frame.height != surface.height {
            return Err(LockError::Paint(format!(
                "frame is {}x{} but the surface is {}x{}",
                frame.width, frame.height, surface.width, surface.height
            )));
        }
        let (buffer, pool, file) =
            self.shm_buffer(&shm, qh, &frame.pixels, surface.width, surface.height)?;
        surface.surface.attach(Some(&buffer), 0, 0);
        surface
            .surface
            .damage_buffer(0, 0, surface.width as i32, surface.height as i32);
        surface.surface.commit();
        // The pool must outlive the buffer upload; keep both until the client exits.
        self.retained.push((buffer, pool, file));
        Ok(())
    }

    /// Fills an `wl_shm` pool with `pixels` and returns the buffer plus the resources to retain.
    fn shm_buffer(
        &mut self,
        shm: &WlShm,
        qh: &QueueHandle<Self>,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(WlBuffer, WlShmPool, std::fs::File), LockError> {
        let stride = (width * 4) as i32;
        let len = pixels.len();
        let file = shm_file(len).map_err(|error| LockError::Protocol(error.to_string()))?;
        {
            use std::io::Write;
            let mut writer = &file;
            writer
                .write_all(pixels)
                .map_err(|error| LockError::Protocol(error.to_string()))?;
        }
        let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride,
            // The painter writes straight RGBA8 bytes in native order.
            Format::Argb8888,
            qh,
            (),
        );
        Ok((buffer, pool, file))
    }
}

/// A private, unlinked file for one shm buffer.
fn shm_file(len: usize) -> std::io::Result<std::fs::File> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "flamewm-lock-shm-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.set_len(len as u64)?;
    // Unlink immediately: the descriptor keeps it alive and nothing leaks on a crash.
    let _ = std::fs::remove_file(&path);
    Ok(file)
}

/// Whether a global announced by the registry needs a lock surface of its own. Only an output
/// that appears after the lock was requested, while it is still live, is covered here: outputs
/// present at `lock` time get their surfaces there. Precedent: ext-session-lock-v1 leaves the
/// per-output surface to the client, and Vela's `lock/main.cpp` covers every active output.
pub(crate) fn covers_late_output(interface: &str, lock_requested: bool, finished: bool) -> bool {
    interface == "wl_output" && lock_requested && !finished
}

impl Dispatch<WlRegistry, GlobalListContents> for LockClient {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: RegistryEvent,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Outputs present at `lock` time were read from the list there. A hotplugged output gets
        // its lock surface here, or it would stay blank with no password prompt.
        let RegistryEvent::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        if !covers_late_output(&interface, state.lock.is_some(), state.finished) {
            return;
        }
        let (Some(compositor), Some(lock)) = (state.compositor.clone(), state.lock.clone()) else {
            return;
        };
        let output = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ());
        let surface = compositor.create_surface(qh, ());
        let lock_surface = lock.get_lock_surface(&surface, &output, qh, ());
        state.pending.push((surface, lock_surface, output));
    }
}

impl Dispatch<WlCompositor, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wayland_client::protocol::wl_compositor::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wayland_client::protocol::wl_surface::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: wayland_client::protocol::wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wayland_client::protocol::wl_shm::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockManagerV1, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &ExtSessionLockManagerV1,
        _: wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtSessionLockV1, ()> for LockClient {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: LockEvent,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            LockEvent::Locked => {
                eprintln!("flamewm-lock: session locked; painting lock surfaces");
                // The seat may have advertised its keyboard before the lock confirmed; bind then
                // so no keystroke is lost.
                state.ensure_keyboard(qh);
                state.locked = true;
            }
            LockEvent::Finished => state.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, ()> for LockClient {
    fn event(
        state: &mut Self,
        surface: &ExtSessionLockSurfaceV1,
        event: LockSurfaceEvent,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let LockSurfaceEvent::Configure {
            serial,
            width,
            height,
        } = event
        else {
            return;
        };
        surface.ack_configure(serial);
        // Find the surface this configure belongs to and paint it.
        let Some(index) = state
            .pending
            .iter()
            .position(|(_, lock_surface, _)| lock_surface == surface)
        else {
            return;
        };
        let (wl_surface, lock_surface, _output) = state.pending.remove(index);
        let configured = ConfiguredSurface {
            surface: wl_surface,
            lock_surface,
            width,
            height,
        };
        if let Err(error) = state.present(&configured, qh) {
            eprintln!("flamewm-lock: {error}");
            state.finished = true;
            return;
        }
        eprintln!(
            "flamewm-lock: presented lock surface {}x{}",
            configured.width, configured.height
        );
        state.configured.push(configured);
    }
}

/// Locks the session and runs the Wayland lock loop until the compositor ends it.
///
/// `frame` produces the pixels for each configured surface; the caller supplies the document.
///
/// # Errors
/// Returns [`LockError`] when the connection, the global, the handshake or a paint fails.
pub fn run<L, F>(load_settings: L, username: &str, frame: F) -> Result<(), LockError>
where
    L: FnOnce() -> crate::settings::LockSettings + 'static,
    F: FnMut(&crate::view::LockView, u32, u32) -> Result<Frame, String> + 'static,
{
    run_with(
        load_settings,
        username,
        Box::new(crate::pam::authenticate),
        frame,
    )
}

/// How a typed secret is checked. The real presenter uses PAM; a test can substitute a closure
/// so the unlock path is exercisable without a live account.
pub type Authenticator = Box<dyn Fn(&str, &str) -> crate::pam::Outcome + Send + Sync + 'static>;

/// Runs the presenter with an injected authenticator.
pub fn run_with<L, F>(
    load_settings: L,
    username: &str,
    authenticate: Authenticator,
    frame: F,
) -> Result<(), LockError>
where
    L: FnOnce() -> crate::settings::LockSettings + 'static,
    F: FnMut(&crate::view::LockView, u32, u32) -> Result<Frame, String> + 'static,
{
    let connection =
        Connection::connect_to_env().map_err(|error| LockError::Connect(error.to_string()))?;
    let mut client = LockClient::new(Box::new(frame));
    client.username = username.to_string();
    client.authenticate = std::sync::Arc::new(authenticate);
    // The lock is requested before any settings work: the session is covered only once the
    // compositor confirms the lock, so a slow settings service must never delay this request.
    // The keyboard is already created inside `lock()`, so input is captured before the settings wait.
    let (mut queue, _qh) = client.lock(&connection)?;
    connection
        .flush()
        .map_err(|error| LockError::Protocol(error.to_string()))?;
    client.settings = load_settings();
    client.view = crate::view::LockView::project(
        &client.settings,
        None,
        username,
        client.state.password(),
        client.state.status(),
        false,
    );

    // Dispatch until the session is unlocked or the compositor ends the lock. Configures arrive
    // here (each paints its surface) and so does keyboard input, which is what makes the lock
    // openable at all.
    loop {
        if client.unlock {
            client.release_lock();
            connection
                .flush()
                .map_err(|error| LockError::Protocol(error.to_string()))?;
            return Ok(());
        }
        if client.finished {
            return Ok(());
        }
        // A PAM outcome can land while no event is pending, so check before blocking.
        client.poll_outcome();
        if client.dirty && !client.configured.is_empty() {
            client.dirty = false;
            client.repaint(&queue.handle());
        }
        if client.unlock {
            continue;
        }

        // A lock has no other event source, so blocking until the compositor speaks would never
        // notice a PAM result. Poll the connection for a short interval instead.
        if let Err(error) = queue.dispatch_pending(&mut client) {
            return Err(LockError::Protocol(error.to_string()));
        }
        if client.dirty && !client.configured.is_empty() {
            client.dirty = false;
            client.repaint(&queue.handle());
        }
        connection
            .flush()
            .map_err(|error| LockError::Protocol(error.to_string()))?;
        if let Some(guard) = connection.prepare_read() {
            let readable = wait_readable(&connection);
            if readable && let Err(error) = guard.read() {
                return Err(LockError::Protocol(error.to_string()));
            }
            // Dropping the guard cancels the read; the loop re-dispatches next turn.
        }
    }
}

impl Dispatch<WlSeat, ()> for LockClient {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: SeatEvent,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let SeatEvent::Capabilities { capabilities } = event {
            let WEnum::Value(capabilities) = capabilities else {
                return;
            };
            // The seat may advertise a keyboard later than the initial bind.
            if crate::seat::advertises_keyboard(std::slice::from_ref(&capabilities))
                && !state.input.has_keyboard()
            {
                let keyboard = seat.get_keyboard(qh, ());
                state.input.keyboard = Some(keyboard);
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for LockClient {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wayland_client::protocol::wl_keyboard::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Collect the keys first: the handler needs `&mut self` for repaint and submit.
        let mut keys = Vec::new();
        let caps_before = state.input.caps_lock();
        state.input.handle(event, |key| keys.push(key));
        // A Caps Lock toggle changes the status hint, so the next turn must repaint.
        state.dirty |= state.input.caps_lock() != caps_before;
        if keys.is_empty() {
            return;
        }
        for key in keys {
            state.apply_key(key, qh);
        }
    }
}

impl LockClient {
    /// Binds the keyboard once the seat advertises one and it is not bound yet.
    fn ensure_keyboard(&mut self, qh: &QueueHandle<Self>) {
        if self.input.has_keyboard() {
            return;
        }
        if let Some(seat) = self.seat.clone() {
            self.input.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }

    /// Applies one decoded key: edits the password, submits, or arms a repaint.
    fn apply_key(&mut self, key: crate::state::Key, qh: &QueueHandle<Self>) {
        use crate::state::Effect;
        let now = std::time::Instant::now();
        match self.state.key(key, now) {
            Effect::None => {}
            Effect::Edited => self.dirty = true,
            Effect::Submit => {
                self.dirty = true;
                self.submit();
            }
        }
        let _ = qh;
    }

    /// Moves the typed secret to the PAM worker and enters the authenticating state.
    fn submit(&mut self) {
        let secret = self.state.begin_authentication();
        let user = self.username.clone();
        let authenticate = std::sync::Arc::clone(&self.authenticate);
        let (tx, rx) = std::sync::mpsc::channel();
        self.outcomes = Some(rx);
        let spawned = std::thread::Builder::new()
            .name("flamewm-lock-pam".to_string())
            .spawn(move || {
                let outcome = authenticate(&user, secret.as_str());
                drop(secret);
                let _ = tx.send(outcome);
            });
        if spawned.is_err() {
            if let Some(rx) = self.outcomes.take() {
                drop(rx);
            }
            self.state
                .finish(crate::pam::Outcome::Unavailable, std::time::Instant::now());
        }
    }

    /// Applies a PAM outcome, unlocking when it is the single success path.
    pub(crate) fn poll_outcome(&mut self) {
        let Some(rx) = self.outcomes.as_ref() else {
            return;
        };
        let Ok(outcome) = rx.try_recv() else {
            return;
        };
        self.outcomes = None;
        match outcome {
            crate::pam::Outcome::Unlocked => eprintln!("flamewm-lock: unlocked"),
            crate::pam::Outcome::Denied => eprintln!("flamewm-lock: authentication denied"),
            crate::pam::Outcome::Unavailable => {
                eprintln!("flamewm-lock: authentication unavailable");
            }
        }
        if self.state.finish(outcome, std::time::Instant::now()) {
            self.unlock = true;
        }
    }

    /// Repaints every configured surface from the current lock state.
    fn repaint(&mut self, qh: &QueueHandle<Self>) {
        // Project the live state: the masked dots and the status line follow the keystrokes.
        self.view = crate::view::LockView::project(
            &self.settings,
            None,
            &self.username,
            self.state.password(),
            self.state.status(),
            self.input.caps_lock(),
        );
        let surfaces = std::mem::take(&mut self.configured);
        for surface in &surfaces {
            if let Err(error) = self.present(surface, qh) {
                eprintln!("flamewm-lock: {error}");
            }
        }
        for surface in surfaces {
            self.configured.push(surface);
        }
    }

    /// Unlocks the session, which is the only way to leave the lock.
    fn release_lock(&mut self) {
        // `unlock_and_destroy` is required once the compositor sent `locked`; the plain `destroy`
        // request is a protocol error in that state.
        if let Some(lock) = self.lock.take() {
            lock.unlock_and_destroy();
        }
        self.locked = false;
    }
}

/// Waits up to one tick for `connection` to become readable.
///
/// Returns `false` on timeout so the caller can service work that does not arrive on the
/// Wayland socket, such as a PAM result.
fn wait_readable(connection: &Connection) -> bool {
    use rustix::event::{PollFd, PollFlags, poll};
    let backend = connection.backend();
    let fd = backend.poll_fd();
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    match poll(
        &mut fds,
        Some(&rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 16_000_000,
        }),
    ) {
        Ok(count) => count > 0,
        // A poll failure is treated as "not ready"; the next dispatch reports the real error.
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Effect, Key, Status};

    #[test]
    fn only_a_live_lock_covers_an_output_announced_later() {
        assert!(covers_late_output("wl_output", true, false));
        assert!(
            !covers_late_output("wl_output", false, false),
            "before lock: lock() covers it"
        );
        assert!(
            !covers_late_output("wl_output", true, true),
            "finished lock takes no surfaces"
        );
        assert!(
            !covers_late_output("wl_seat", true, false),
            "other globals are ignored"
        );
    }

    fn client() -> LockClient {
        LockClient::new(Box::new(|_, width, height| {
            Ok(Frame {
                width,
                height,
                pixels: vec![0u8; (width * height * 4) as usize],
            })
        }))
    }

    #[test]
    fn typing_reaches_the_password_buffer() {
        let mut client = client();
        let now = std::time::Instant::now();
        assert_eq!(client.state.key(Key::Char('s'), now), Effect::Edited);
        assert_eq!(client.state.key(Key::Char('e'), now), Effect::Edited);
        assert_eq!(
            client.state.password().len(),
            2,
            "the field holds the secret"
        );
    }

    #[test]
    fn enter_with_text_submits() {
        let mut client = client();
        let now = std::time::Instant::now();
        client.state.key(Key::Char('x'), now);
        assert_eq!(client.state.key(Key::Enter, now), Effect::Submit);
    }

    #[test]
    fn enter_on_an_empty_field_does_not_submit() {
        // An empty submit would burn a PAM attempt for nothing.
        let mut client = client();
        assert_eq!(
            client.state.key(Key::Enter, std::time::Instant::now()),
            Effect::None
        );
    }

    #[test]
    fn the_view_follows_the_typed_state() {
        let mut client = client();
        let now = std::time::Instant::now();
        client.state.key(Key::Char('a'), now);
        client.view = crate::view::LockView::project(
            &client.settings,
            None,
            "tester",
            client.state.password(),
            client.state.status(),
            false,
        );
        // The projection carries the username so the lock names who it will authenticate.
        assert_eq!(client.view.username, "tester");
    }

    #[test]
    fn a_successful_outcome_requests_an_unlock() {
        let mut client = client();
        let (tx, rx) = std::sync::mpsc::channel();
        client.outcomes = Some(rx);
        tx.send(crate::pam::Outcome::Unlocked).expect("send");
        client.poll_outcome();
        assert!(client.unlock, "an unlocked outcome must release the lock");
    }

    #[test]
    fn a_denied_outcome_keeps_the_session_locked() {
        // The single most important invariant: a wrong password never unlocks.
        let mut client = client();
        let (tx, rx) = std::sync::mpsc::channel();
        client.outcomes = Some(rx);
        tx.send(crate::pam::Outcome::Denied).expect("send");
        client.poll_outcome();
        assert!(!client.unlock, "a denial must leave the session locked");
        assert_eq!(client.state.status(), Status::Failed);
    }

    #[test]
    fn an_unavailable_outcome_keeps_the_session_locked() {
        let mut client = client();
        let (tx, rx) = std::sync::mpsc::channel();
        client.outcomes = Some(rx);
        tx.send(crate::pam::Outcome::Unavailable).expect("send");
        client.poll_outcome();
        assert!(
            !client.unlock,
            "a missing PAM must leave the session locked"
        );
        assert_eq!(client.state.status(), Status::Unavailable);
    }
}
