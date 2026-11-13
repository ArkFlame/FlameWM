//! Minimal wlr-layer-shell test client for the `FlameWM` Wayland compositor.
//!
//! Maps one solid ARGB8888 layer surface anchored to the bottom edge, stretched across the output
//! (requested output `None`, so the compositor picks it), with an exclusive zone equal to its height.
//! Pixels go through an unlinked file in `XDG_RUNTIME_DIR`, so no mmap or unsafe code is needed.
//! Environment:
//! - `FLAMEWM_TEST_COLOR` (hex, default `FF0000`), `FLAMEWM_LAYER_HEIGHT` (default `40`),
//! - `FLAMEWM_TEST_HOLD_MS` (default `6000`): how long the layer surface stays mapped.
//!
//! Prints `layer-client: configured WxH color=AARRGGBB` once the buffer is committed.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    /// `(serial, width, height)` of the last configure.
    configure: Option<(u32, u32, u32)>,
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
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            state.configure = Some((serial, width, height));
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
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let color = 0xFF00_0000
        | u32::from_str_radix(
            std::env::var("FLAMEWM_TEST_COLOR")
                .unwrap_or_else(|_| "FF0000".into())
                .trim_start_matches('#'),
            16,
        )?;
    let height: u32 = std::env::var("FLAMEWM_LAYER_HEIGHT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
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
    let shell = app.layer_shell.clone().ok_or("no zwlr_layer_shell_v1")?;
    let shm = app.shm.clone().ok_or("no wl_shm")?;
    let surface = compositor.create_surface(&qh, ());
    let layer = shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "flame-layer-test".into(),
        &qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Bottom
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_size(0, height);
    layer.set_exclusive_zone(height as i32);
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    surface.commit();

    let deadline = Instant::now() + Duration::from_secs(10);
    while app.configure.is_none() {
        if Instant::now() > deadline {
            return Err("no layer configure from compositor".into());
        }
        queue.roundtrip(&mut app)?;
    }
    let (serial, width, height) = app.configure.unwrap_or_default();
    layer.ack_configure(serial);

    let stride = width * 4;
    let len = (stride * height) as usize;
    let path = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?)
        .join(format!("flame-layer-{}", std::process::id()));
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

    let pool = shm.create_pool(file.as_fd(), len as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.commit();
    println!("layer-client: configured {width}x{height} color={color:08X}");

    let end = Instant::now() + hold;
    while Instant::now() < end {
        queue.roundtrip(&mut app)?;
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("layer-client: done");
    Ok(())
}
