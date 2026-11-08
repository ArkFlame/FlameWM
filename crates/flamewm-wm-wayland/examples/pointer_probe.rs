//! Pointer probe client for the `FlameWM` Wayland compositor.
//!
//! Maps one shm window, prints every `wl_pointer` enter/motion and `zwp_relative_pointer_v1`
//! motion it receives, and optionally requests a pointer constraint. Output is one line per event
//! on stdout, so a run can be checked with plain text matching. Environment:
//! - `FLAMEWM_PROBE_CONSTRAINT`: `none` (default), `lock` or `confine`,
//! - `FLAMEWM_PROBE_HOLD_MS` (default `15000`): how long the window stays mapped.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1, zwp_locked_pointer_v1, zwp_pointer_constraints_v1,
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1, zwp_relative_pointer_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const WIDTH: i32 = 320;
const HEIGHT: i32 = 200;

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    relative_manager: Option<zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1>,
    constraints: Option<zwp_pointer_constraints_v1::ZwpPointerConstraintsV1>,
    pending_serial: Option<u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(5), qh, ())),
                "zwp_relative_pointer_manager_v1" => {
                    state.relative_manager = Some(registry.bind(name, 1, qh, ()));
                }
                "zwp_pointer_constraints_v1" => {
                    state.constraints = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            if let WEnum::Value(caps) = capabilities {
                if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                    state.pointer = Some(seat.get_pointer(qh, ()));
                }
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        _: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                println!("enter {surface_x:.2} {surface_y:.2}");
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                println!("motion {surface_x:.2} {surface_y:.2}");
            }
            wl_pointer::Event::Leave { .. } => println!("leave"),
            _ => {}
        }
    }
}

impl Dispatch<zwp_relative_pointer_v1::ZwpRelativePointerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion {
            dx,
            dy,
            dx_unaccel,
            dy_unaccel,
            ..
        } = event
        {
            println!("rel {dx:.2} {dy:.2} unaccel {dx_unaccel:.2} {dy_unaccel:.2}");
        }
    }
}

impl Dispatch<zwp_locked_pointer_v1::ZwpLockedPointerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        event: zwp_locked_pointer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_locked_pointer_v1::Event::Locked => println!("locked"),
            zwp_locked_pointer_v1::Event::Unlocked => println!("unlocked"),
            _ => {}
        }
    }
}

impl Dispatch<zwp_confined_pointer_v1::ZwpConfinedPointerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        event: zwp_confined_pointer_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_confined_pointer_v1::Event::Confined => println!("confined"),
            zwp_confined_pointer_v1::Event::Unconfined => println!("unconfined"),
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(
        state: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.pending_serial = Some(serial);
        }
    }
}

macro_rules! ignore_events {
    ($($ty:ty),*) => {$(
        impl Dispatch<$ty, ()> for App {
            fn event(_: &mut Self, _: &$ty, _: <$ty as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel,
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    zwp_pointer_constraints_v1::ZwpPointerConstraintsV1
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let constraint = std::env::var("FLAMEWM_PROBE_CONSTRAINT").unwrap_or_else(|_| "none".into());
    let hold = Duration::from_millis(
        std::env::var("FLAMEWM_PROBE_HOLD_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15000),
    );

    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut app = App::default();
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;

    let compositor = app.compositor.clone().ok_or("no wl_compositor")?;
    let base = app.wm_base.clone().ok_or("no xdg_wm_base")?;
    let shm = app.shm.clone().ok_or("no wl_shm")?;
    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("FlameWM pointer probe".into());
    surface.commit();

    let deadline = Instant::now() + Duration::from_secs(10);
    while app.pending_serial.is_none() {
        if Instant::now() > deadline {
            return Err("no configure from compositor".into());
        }
        queue.roundtrip(&mut app)?;
    }
    xdg.ack_configure(app.pending_serial.take().unwrap_or_default());

    let stride = WIDTH * 4;
    let len = (stride * HEIGHT) as usize;
    let path = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?)
        .join(format!("flame-probe-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    file.write_all(&vec![0x80u8; len])?;
    let pool = shm.create_pool(file.as_fd(), len as i32, &qh, ());
    let buffer = pool.create_buffer(0, WIDTH, HEIGHT, stride, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, WIDTH, HEIGHT);
    surface.commit();

    // Pointer objects arrive after the seat capabilities; wait briefly for them.
    let wait = Instant::now() + Duration::from_secs(5);
    while app.pointer.is_none() && Instant::now() < wait {
        queue.roundtrip(&mut app)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let pointer = app.pointer.clone().ok_or("no wl_pointer")?;
    if let Some(manager) = app.relative_manager.clone() {
        let _relative = manager.get_relative_pointer(&pointer, &qh, ());
    }
    let _locked_or_confined = match (constraint.as_str(), app.constraints.clone()) {
        ("lock", Some(c)) => {
            let _ = c.lock_pointer(
                &surface,
                &pointer,
                None,
                zwp_pointer_constraints_v1::Lifetime::Persistent,
                &qh,
                (),
            );
            println!("requested lock");
        }
        ("confine", Some(c)) => {
            let _ = c.confine_pointer(
                &surface,
                &pointer,
                None,
                zwp_pointer_constraints_v1::Lifetime::Persistent,
                &qh,
                (),
            );
            println!("requested confine");
        }
        _ => {}
    };
    println!("probe mapped {WIDTH}x{HEIGHT} constraint={constraint}");

    let end = Instant::now() + hold;
    while Instant::now() < end {
        queue.roundtrip(&mut app)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}
