//! Prints the `wp_fractional_scale_v1` preferred scale the running compositor sends for a surface.
//!
//! Connects to `WAYLAND_DISPLAY` like any client, creates one surface, asks for its fractional
//! scale and prints the numerator of `preferred_scale`. The denominator is fixed at 120 by the
//! protocol, so a 1.25 output scale arrives as numerator 150.

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_compositor, wl_registry, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};

#[derive(Default)]
struct State {
    preferred: Option<u32>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.preferred = Some(scale);
        }
    }
}

delegate_noop!(State: wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let manager: wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _fractional = manager.get_fractional_scale(&surface, &qh, ());
    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    if let Some(numerator) = state.preferred {
        println!("preferred_scale numerator={numerator} denominator=120");
    } else {
        eprintln!("no preferred_scale event received");
        std::process::exit(1);
    }
    Ok(())
}
