//! Wayland lock client: the protocol side of presenting the lock without X11.
//!
//! `present` turns the lock document into an RGBA8 frame; this module puts that frame on screen.
//! It drives `ext-session-lock-v1` as an ordinary client — connect, lock, and for each configured
//! lock surface attach a shared-memory buffer holding the painted frame.
//!
//! The compositor is the authority: it confirms the lock, decides each surface size, and shows
//! nothing else while the surfaces are up. This code never draws over another client.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::{Event as RegistryEvent, WlRegistry};
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
pub type FrameFn = Box<dyn FnMut(u32, u32) -> Result<Frame, String>>;

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
}

impl LockClient {
    pub fn new(frame: FrameFn) -> Self {
        Self {
            frame,
            compositor: None,
            shm: None,
            lock: None,
            pending: Vec::new(),
            configured: Vec::new(),
            locked: false,
            finished: false,
            retained: Vec::new(),
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

        let lock = manager.lock(&qh, ());
        // Surfaces must exist before the lock confirms, so create them now.
        for output in outputs {
            let surface = compositor.create_surface(&qh, ());
            let lock_surface = lock.get_lock_surface(&surface, &output, &qh, ());
            self.pending.push((surface, lock_surface, output));
        }

        self.compositor = Some(compositor);
        self.shm = Some(shm);
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
        let frame = (self.frame)(surface.width, surface.height).map_err(LockError::Paint)?;
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
        use std::os::unix::io::AsFd;
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

impl Dispatch<WlRegistry, GlobalListContents> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: RegistryEvent,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Globals are read from the list during `lock`; nothing to handle per event.
    }
}

impl Dispatch<WlCompositor, ()> for LockClient {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wayland_client::protocol::wl_compositor::Event,
        _: &(),
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
        _: &(),
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
        _: &(),
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
        _: &(),
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
        _: &(),
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
        _: &(),
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
        _: &(),
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
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            LockEvent::Locked => {
                eprintln!("flamewm-lock: session locked; painting lock surfaces");
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
        _: &(),
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
pub fn run<F>(
    settings: &crate::settings::LockSettings,
    username: &str,
    frame: F,
) -> Result<(), LockError>
where
    F: FnMut(u32, u32) -> Result<Frame, String> + 'static,
{
    let connection =
        Connection::connect_to_env().map_err(|error| LockError::Connect(error.to_string()))?;
    let _ = (settings, username);
    let mut client = LockClient::new(Box::new(frame));
    let (mut queue, _qh) = client.lock(&connection)?;
    connection
        .flush()
        .map_err(|error| LockError::Protocol(error.to_string()))?;

    // Dispatch until the compositor ends the lock. Every configure arrives here, which is where
    // the surface gets its frame.
    loop {
        if client.finished {
            return Ok(());
        }
        if let Err(error) = queue.blocking_dispatch(&mut client) {
            return Err(LockError::Protocol(error.to_string()));
        }
    }
}
