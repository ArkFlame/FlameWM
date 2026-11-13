//! Minimal xdg-shell test client for the `FlameWM` Wayland compositor.
//!
//! Draws one solid ARGB8888 window through `wl_shm` (pixels are written to an unlinked file in
//! `XDG_RUNTIME_DIR`, so no mmap or unsafe code is needed). Environment:
//! - `FLAMEWM_TEST_COLOR` (hex, default `2E86C1`), `FLAMEWM_TEST_SIZE` (default `320x200`),
//! - `FLAMEWM_TEST_HOLD_MS` (default `6000`): how long the window stays mapped.
//! - `FLAMEWM_TEST_TITLE` (title), `FLAMEWM_TEST_COLOR2` (hex; with `FLAMEWM_TEST_REPAINT_MS` the
//!   window alternates between the two colours every interval, a repainting client),
//! - `FLAMEWM_TEST_MINIMIZE_MS`: after this many ms the client asks to be minimised
//!   (`xdg_toplevel.set_minimized`) and repaints once to `FLAMEWM_TEST_COLOR2` while minimised.
//! - `FLAMEWM_TEST_CONFIGURE_LOG` (optional path): every toplevel configure is appended as
//!   `w h maximized activated` (state flags as 0/1), so a proof can read the client-reported geometry.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
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
    configure_log: Option<PathBuf>,
    pending_configure: Option<(i32, i32, bool, bool)>,
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
            let pending = state.pending_configure.take();
            if let (Some(path), Some((w, h, max, act))) = (state.configure_log.as_ref(), pending)
                && let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path)
            {
                let _ = writeln!(f, "{w} {h} {} {}", u8::from(max), u8::from(act));
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for App {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Close => state.closed = true,
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                // Each state is a native-endian u32 in the byte array.
                let has = |want: xdg_toplevel::State| {
                    states
                        .chunks_exact(4)
                        .any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == want as u32)
                };
                state.pending_configure = Some((
                    width,
                    height,
                    has(xdg_toplevel::State::Maximized),
                    has(xdg_toplevel::State::Activated),
                ));
            }
            _ => {}
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

/// Rewrites the window's pixels in the shared file and commits the buffer again.
fn paint(
    file: &mut std::fs::File,
    surface: &wl_surface::WlSurface,
    buffer: &wl_buffer::WlBuffer,
    color: u32,
    width: i32,
    height: i32,
) -> std::io::Result<()> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&color.to_le_bytes());
    }
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&pixels)?;
    surface.attach(Some(buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
    Ok(())
}

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
    let mut app = App {
        configure_log: std::env::var_os("FLAMEWM_TEST_CONFIGURE_LOG").map(PathBuf::from),
        ..App::default()
    };
    queue.roundtrip(&mut app)?;

    let compositor = app.compositor.clone().ok_or("no wl_compositor")?;
    let base = app.wm_base.clone().ok_or("no xdg_wm_base")?;
    app.shm.as_ref().ok_or("no wl_shm")?;
    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title(
        std::env::var("FLAMEWM_TEST_TITLE").unwrap_or_else(|_| "FlameWM test client".into()),
    );
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

    let color2 = match std::env::var("FLAMEWM_TEST_COLOR2") {
        Ok(raw) => 0xFF00_0000 | u32::from_str_radix(raw.trim_start_matches('#'), 16)?,
        Err(_) => color,
    };
    let env_ms = |name: &str| {
        Duration::from_millis(
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        )
    };
    let repaint_every = env_ms("FLAMEWM_TEST_REPAINT_MS");
    let minimize_after = env_ms("FLAMEWM_TEST_MINIMIZE_MS");
    let start = Instant::now();
    let mut next_paint = start;
    let mut toggle = false;
    let mut minimized = false;
    let end = start + hold;
    while Instant::now() < end && !app.closed {
        queue.roundtrip(&mut app)?;
        if !repaint_every.is_zero() && Instant::now() >= next_paint {
            toggle = !toggle;
            paint(
                &mut file,
                &surface,
                &buffer,
                if toggle { color2 } else { color },
                width,
                height,
            )?;
            next_paint += repaint_every;
        }
        if !minimized && !minimize_after.is_zero() && start.elapsed() >= minimize_after {
            toplevel.set_minimized();
            minimized = true;
            paint(&mut file, &surface, &buffer, color2, width, height)?;
            println!("flame-test: minimised, repainted to color={color2:08X}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("flame-test: done closed={}", app.closed);
    Ok(())
}
