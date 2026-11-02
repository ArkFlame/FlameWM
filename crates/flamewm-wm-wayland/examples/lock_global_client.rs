//! Lists the globals a real compositor advertises.
//!
//! Used as an end-to-end check that a running `FlameWM` Wayland session offers
//! `ext_session_lock_manager_v1`: without it the lock UI, which is an ordinary client, has no
//! way to paint. Connects to `WAYLAND_DISPLAY` like any other client.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{Event, WlRegistry};
use wayland_client::{Connection, Dispatch, QueueHandle};

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

fn main() {
    let connection = Connection::connect_to_env().expect("connect to WAYLAND_DISPLAY");
    let (globals, _queue) = registry_queue_init::<State>(&connection).expect("registry");
    let interfaces: Vec<String> = globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|global| global.interface)
        .collect();
    for interface in &interfaces {
        println!("{interface}");
    }
    if interfaces
        .iter()
        .any(|i| i == "ext_session_lock_manager_v1")
    {
        println!("HAS_SESSION_LOCK");
    } else {
        eprintln!("MISSING_SESSION_LOCK");
        std::process::exit(1);
    }
}
