//! Minimal xdg-shell test client for the FlameWM Wayland compositor.
//!
//! Draws one solid ARGB8888 window through wl_shm (pixels are written to an unlinked file in
//! `XDG_RUNTIME_DIR`, so no mmap or unsafe code is needed). Environment:
//! - `FLAMEWM_TEST_COLOR` (hex, default `2E86C1`), `FLAMEWM_TEST_SIZE` (default `320x200`),
//! - `FLAMEWM_TEST_HOLD_MS` (default `6000`): how long the window stays mapped.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    pending_serial: Option<u32>,
    drawn: bool,
    closed: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
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
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
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
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state.pending_serial = Some(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for App {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.closed = true;
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
    wl_surface::WlSurface
);

fn parse_size(raw: &str) -> Option<(i32, i32)> {
    let (w, h) = raw.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let color = 0xFF00_0000
        | u32::from_str_radix(
            std::env::var("FLAMEWM_TEST_COLOR")
                .unwrap_or_else(|_| "2E86C1".into())
                .trim_start_matches('#'),
            16,
        )?;
    let (width, height) = std::env::var("FLAMEWM_TEST_SIZE")
        .ok()
        .and_then(|raw| parse_size(&raw))
        .unwrap_or((320, 200));
    let hold = Duration::from_millis(
        std::env::var("FLAMEWM_TEST_HOLD_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6000),
    );

    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut app = App::default();
    queue.roundtrip(&mut app)?;

    let compositor = app.compositor.clone().ok_or("no wl_compositor")?;
    let base = app.wm_base.clone().ok_or("no xdg_wm_base")?;
    app.shm.as_ref().ok_or("no wl_shm")?;
    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("FlameWM test client".into());
    toplevel.set_app_id("flame-test".into());
    surface.commit();
    app.surface = Some(surface.clone());
    app.xdg_surface = Some(xdg.clone());

    let deadline = Instant::now() + Duration::from_secs(10);
    while app.pending_serial.is_none() {
        if Instant::now() > deadline {
            return Err("no configure from compositor".into());
        }
        queue.roundtrip(&mut app)?;
    }
    xdg.ack_configure(app.pending_serial.take().unwrap_or_default());

    let stride = width * 4;
    let len = (stride * height) as usize;
    let path = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?)
        .join(format!("flame-test-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    let mut pixels = Vec::with_capacity(len);
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&color.to_le_bytes());
    }
    file.write_all(&pixels)?;

    let shm = app.shm.clone().ok_or("no wl_shm")?;
    let pool = shm.create_pool(file.as_fd(), len as i32, &qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
    app.drawn = true;
    println!("flame-test: mapped {width}x{height} color={color:08X}");

    let end = Instant::now() + hold;
    while Instant::now() < end && !app.closed {
        queue.roundtrip(&mut app)?;
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("flame-test: done closed={}", app.closed);
    Ok(())
}
