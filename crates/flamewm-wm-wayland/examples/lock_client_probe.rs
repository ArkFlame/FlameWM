//! A real `ext-session-lock-v1` client, used to verify the compositor end to end.
//!
//! It connects to `WAYLAND_DISPLAY`, locks the session, and reports the configure the compositor
//! sends for the lock surface. That is the same handshake the `FlameWM` lock UI performs, reduced
//! to the protocol mechanics, and it fails loudly if any step is missing.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::{Event as RegistryEvent, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::{
    Event as SurfaceEvent, ExtSessionLockSurfaceV1,
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::{
    Event as LockEvent, ExtSessionLockV1,
};

#[derive(Default)]
struct State {
    compositor: Option<WlCompositor>,
    surfaces: Vec<ExtSessionLockSurfaceV1>,
    locked: bool,
    configure: Option<(u32, u32)>,
    finished: bool,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: RegistryEvent,
        _data: &GlobalListContents,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let RegistryEvent::Global {
            name,
            interface,
            version,
        } = event
            && interface == "wl_compositor"
        {
            state.compositor = Some(registry.bind::<WlCompositor, _, _>(name, version, qh, ()));
        }
    }
}

impl Dispatch<WlCompositor, ()> for State {
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

impl Dispatch<WlSurface, ()> for State {
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

impl Dispatch<WlOutput, ()> for State {
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

impl Dispatch<ExtSessionLockManagerV1, ()> for State {
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

impl Dispatch<ExtSessionLockV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: LockEvent,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // The compositor confirmed the lock: no other client content is visible.
            LockEvent::Locked => state.locked = true,
            LockEvent::Finished => state.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        surface: &ExtSessionLockSurfaceV1,
        event: SurfaceEvent,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let SurfaceEvent::Configure {
            serial,
            width,
            height,
        } = event
        {
            surface.ack_configure(serial);
            state.configure = Some((width, height));
        }
    }
}

fn main() {
    let connection = Connection::connect_to_env().expect("connect to WAYLAND_DISPLAY");
    let (globals, mut queue) = registry_queue_init::<State>(&connection).expect("registry");
    let qh = queue.handle();
    let mut state = State::default();

    let manager = globals
        .bind::<ExtSessionLockManagerV1, _, _>(&qh, 1..=1, ())
        .expect("compositor must advertise ext_session_lock_manager_v1");
    let lock = manager.lock(&qh, ());

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
        .expect("wl_compositor must be advertised");
    state.compositor = Some(compositor);

    let outputs: Vec<WlOutput> = globals
        .contents()
        .clone_list()
        .into_iter()
        .filter(|global| global.interface == "wl_output")
        .map(|global| {
            globals
                .registry()
                .bind::<WlOutput, _, _>(global.name, global.version.min(4), &qh, ())
        })
        .collect();
    if outputs.is_empty() {
        eprintln!("NO_OUTPUT");
        std::process::exit(1);
    }

    // The surface must exist before the lock confirms, so create it up front.
    for output in &outputs {
        let compositor = state
            .compositor
            .clone()
            .expect("wl_compositor must be advertised");
        let surface: WlSurface = compositor.create_surface(&qh, ());
        let lock_surface = lock.get_lock_surface(&surface, output, &qh, ());
        state.surfaces.push(lock_surface);
    }
    connection.flush().expect("flush");
    println!("lock requested for {} output(s)", outputs.len());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while state.configure.is_none() && std::time::Instant::now() < deadline {
        queue.blocking_dispatch(&mut state).expect("dispatch");
    }

    println!(
        "locked={} configure={:?} finished={}",
        state.locked, state.configure, state.finished
    );
    let Some((width, height)) = state.configure else {
        eprintln!("NO_CONFIGURE");
        std::process::exit(1);
    };
    if !state.locked {
        eprintln!("NO_LOCK_CONFIRMATION");
        std::process::exit(1);
    }
    if width == 0 || height == 0 {
        eprintln!("EMPTY_CONFIGURE");
        std::process::exit(1);
    }
    println!("SESSION_LOCK_OK {width}x{height}");
}
