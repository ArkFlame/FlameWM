//! Dev-only `wl_data_device` probe for the `FlameWM` Wayland compositor.
//!
//! Two roles, one binary:
//! - `clipboard_client writer <text> <hold_ms>`: maps a window, takes keyboard focus, offers `<text>`
//!   as the clipboard selection, and answers `send` requests until `hold_ms` elapses.
//! - `clipboard_client reader <timeout_ms>`: maps a window, waits for the selection offer, receives
//!   `text/plain;charset=utf-8` through a pipe and prints it as `CLIPBOARD-READ <text>`.
//! - `clipboard_client primary-writer <text> <hold_ms>` / `primary-reader <timeout_ms>`: the same pair
//!   over the PRIMARY selection (`zwp_primary_selection_device_manager_v1`), printing `PRIMARY-WRITE` /
//!   `PRIMARY-READ`.
//!
//! Both need the compositor to grant keyboard focus (the selection is only accepted from the focused
//! client), so each maps a window with a solid `wl_shm` buffer like `test_client`.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_data_source, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1 as pdm, zwp_primary_selection_device_v1 as pdev,
    zwp_primary_selection_offer_v1 as poffer, zwp_primary_selection_source_v1 as psource,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const MIME: &str = "text/plain;charset=utf-8";

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    ddm: Option<wl_data_device_manager::WlDataDeviceManager>,
    ddev: Option<wl_data_device::WlDataDevice>,
    pm: Option<pdm::ZwpPrimarySelectionDeviceManagerV1>,
    pdevice: Option<pdev::ZwpPrimarySelectionDeviceV1>,
    /// Current PRIMARY selection offer (`None` when cleared).
    primary_offer: Option<poffer::ZwpPrimarySelectionOfferV1>,
    pending_serial: Option<u32>,
    key_serial: Option<u32>,
    /// MIME types announced per offer, keyed by the offer's protocol id.
    offer_mimes: HashMap<u32, Vec<String>>,
    /// Current selection offer (`None` when the selection is cleared).
    selection: Option<wl_data_offer::WlDataOffer>,
    /// Text this client offers as the selection (writer role).
    text: String,
    cancelled: bool,
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
                "wl_data_device_manager" => {
                    state.ddm = Some(registry.bind(name, version.min(3), qh, ()));
                }
                "zwp_primary_selection_device_manager_v1" => {
                    state.pm = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for App {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Enter { serial, .. } = event {
            state.key_serial = Some(serial);
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for App {
    fn event(
        state: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_device::Event::Selection { id } = event {
            state.selection = id;
        }
    }

    wayland_client::event_created_child!(App, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for App {
    fn event(
        state: &mut Self,
        offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state
                .offer_mimes
                .entry(offer.id().protocol_id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for App {
    fn event(
        state: &mut Self,
        _: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } if mime_type == MIME => {
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(state.text.as_bytes());
            }
            wl_data_source::Event::Cancelled => state.cancelled = true,
            _ => {}
        }
    }
}

impl Dispatch<pdev::ZwpPrimarySelectionDeviceV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &pdev::ZwpPrimarySelectionDeviceV1,
        event: pdev::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let pdev::Event::Selection { id } = event {
            state.primary_offer = id;
        }
    }

    wayland_client::event_created_child!(App, pdev::ZwpPrimarySelectionDeviceV1, [
        pdev::EVT_DATA_OFFER_OPCODE => (poffer::ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<poffer::ZwpPrimarySelectionOfferV1, ()> for App {
    fn event(
        state: &mut Self,
        offer: &poffer::ZwpPrimarySelectionOfferV1,
        event: poffer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let poffer::Event::Offer { mime_type } = event {
            state
                .offer_mimes
                .entry(offer.id().protocol_id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<psource::ZwpPrimarySelectionSourceV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &psource::ZwpPrimarySelectionSourceV1,
        event: psource::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            psource::Event::Send { mime_type, fd } if mime_type == MIME => {
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(state.text.as_bytes());
            }
            psource::Event::Cancelled => state.cancelled = true,
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
            fn event(_: &mut Self, _: &$ty, _: <$ty as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
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
    wl_data_device_manager::WlDataDeviceManager,
    pdm::ZwpPrimarySelectionDeviceManagerV1
);

/// Maps a 64x64 solid window with focus-eligible state; returns once the first buffer is committed.
fn map_window(
    queue: &mut EventQueue<App>,
    app: &mut App,
    title: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let qh = queue.handle();
    let compositor = app.compositor.clone().ok_or("no wl_compositor")?;
    let base = app.wm_base.clone().ok_or("no xdg_wm_base")?;
    let surface = compositor.create_surface(&qh, ());
    let xdg = base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title(title.into());
    toplevel.set_app_id("flame-clipboard-test".into());
    surface.commit();

    let deadline = Instant::now() + Duration::from_secs(10);
    while app.pending_serial.is_none() {
        if Instant::now() > deadline {
            return Err("no configure from compositor".into());
        }
        queue.roundtrip(app)?;
    }
    xdg.ack_configure(app.pending_serial.take().unwrap_or_default());

    let (width, height) = (64_i32, 64_i32);
    let stride = width * 4;
    let len = (stride * height) as usize;
    let path = PathBuf::from(std::env::var("XDG_RUNTIME_DIR")?)
        .join(format!("flame-clip-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    let pixels = 0xFF2E_86C1_u32
        .to_le_bytes()
        .repeat((width * height) as usize);
    file.write_all(&pixels)?;
    let shm = app.shm.clone().ok_or("no wl_shm")?;
    let pool = shm.create_pool(file.as_fd(), len as i32, &qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
    queue.roundtrip(app)?;
    Ok(())
}

/// Blocks until the compositor sends keyboard enter; returns its serial, needed to set a selection.
fn await_keyboard_serial(
    queue: &mut EventQueue<App>,
    app: &mut App,
) -> Result<u32, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.key_serial.is_none() {
        if Instant::now() > deadline {
            return Err("no keyboard focus (enter) from compositor".into());
        }
        queue.roundtrip(app)?;
    }
    Ok(app.key_serial.unwrap_or_default())
}

/// Reads `MIME` from the offer that `selection` currently holds, failing after `timeout`.
fn read_offer(
    conn: &Connection,
    queue: &mut EventQueue<App>,
    app: &mut App,
    primary: bool,
    timeout: Duration,
) -> Result<String, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        let id = if primary {
            app.primary_offer.as_ref().map(|o| o.id().protocol_id())
        } else {
            app.selection.as_ref().map(|o| o.id().protocol_id())
        };
        let has_text = id.is_some_and(|id| {
            app.offer_mimes
                .get(&id)
                .is_some_and(|m| m.iter().any(|t| t == MIME))
        });
        if has_text {
            break;
        }
        if Instant::now() > deadline {
            return Err("no text selection offered within timeout".into());
        }
        queue.roundtrip(app)?;
        std::thread::sleep(Duration::from_millis(20));
    }
    let (mut rd, wr): (std::io::PipeReader, OwnedFd) = {
        let (r, w) = std::io::pipe()?;
        (r, w.into())
    };
    if primary {
        let offer = app
            .primary_offer
            .clone()
            .ok_or("primary selection vanished")?;
        offer.receive(MIME.into(), wr.as_fd());
    } else {
        let offer = app.selection.clone().ok_or("selection vanished")?;
        offer.receive(MIME.into(), wr.as_fd());
    }
    conn.flush()?;
    drop(wr);
    let mut text = String::new();
    rd.read_to_string(&mut text)?;
    Ok(text)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut app = App::default();
    queue.roundtrip(&mut app)?;
    let seat = app.seat.clone().ok_or("no wl_seat")?;
    let ddm = app.ddm.clone().ok_or("no wl_data_device_manager")?;
    app.ddev = Some(ddm.get_data_device(&seat, &qh, ()));
    if let Some(pm) = app.pm.clone() {
        app.pdevice = Some(pm.get_device(&seat, &qh, ()));
    }
    queue.roundtrip(&mut app)?;

    match args.get(1).map(String::as_str) {
        Some("writer") => {
            app.text = args.get(2).cloned().ok_or("writer needs <text>")?;
            let hold = Duration::from_millis(args.get(3).and_then(|v| v.parse().ok()).unwrap_or(15_000));
            map_window(&mut queue, &mut app, "FlameWM clipboard writer")?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while app.key_serial.is_none() {
                if Instant::now() > deadline {
                    return Err("no keyboard focus (enter) from compositor".into());
                }
                queue.roundtrip(&mut app)?;
            }
            let source = ddm.create_data_source(&qh, ());
            source.offer(MIME.into());
            let ddev = app.ddev.clone().ok_or("no data device")?;
            ddev.set_selection(Some(&source), app.key_serial.unwrap_or_default());
            queue.roundtrip(&mut app)?;
            println!("CLIPBOARD-WRITE set text={:?}", app.text);
            let end = Instant::now() + hold;
            while Instant::now() < end && !app.cancelled {
                queue.roundtrip(&mut app)?;
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("CLIPBOARD-WRITE done cancelled={}", app.cancelled);
        }
        Some("reader") => {
            let timeout = Duration::from_millis(args.get(2).and_then(|v| v.parse().ok()).unwrap_or(10_000));
            map_window(&mut queue, &mut app, "FlameWM clipboard reader")?;
            let deadline = Instant::now() + timeout;
            loop {
                let offer = app.selection.clone();
                let has_text = offer.as_ref().is_some_and(|o| {
                    app.offer_mimes
                        .get(&o.id().protocol_id())
                        .is_some_and(|m| m.iter().any(|t| t == MIME))
                });
                if has_text {
                    break;
                }
                if Instant::now() > deadline {
                    println!("CLIPBOARD-READ none selection={}", app.selection.is_some());
                    return Err("no text selection offered within timeout".into());
                }
                queue.roundtrip(&mut app)?;
                std::thread::sleep(Duration::from_millis(20));
            }
            let offer = app.selection.clone().ok_or("selection vanished")?;
            let (mut rd, wr): (std::io::PipeReader, OwnedFd) = {
                let (r, w) = std::io::pipe()?;
                (r, w.into())
            };
            offer.receive(MIME.into(), wr.as_fd());
            conn.flush()?;
            drop(wr);
            let mut text = String::new();
            rd.read_to_string(&mut text)?;
            println!("CLIPBOARD-READ {text}");
        }
        Some("primary-writer") => {
            app.text = args.get(2).cloned().ok_or("primary-writer needs <text>")?;
            let hold = Duration::from_millis(args.get(3).and_then(|v| v.parse().ok()).unwrap_or(15_000));
            map_window(&mut queue, &mut app, "FlameWM primary writer")?;
            let serial = await_keyboard_serial(&mut queue, &mut app)?;
            let pm = app.pm.clone().ok_or("no zwp_primary_selection_device_manager_v1")?;
            let source = pm.create_source(&qh, ());
            source.offer(MIME.into());
            let device = app.pdevice.clone().ok_or("no primary selection device")?;
            device.set_selection(Some(&source), serial);
            queue.roundtrip(&mut app)?;
            println!("PRIMARY-WRITE set text={:?}", app.text);
            let end = Instant::now() + hold;
            while Instant::now() < end && !app.cancelled {
                queue.roundtrip(&mut app)?;
                std::thread::sleep(Duration::from_millis(50));
            }
            println!("PRIMARY-WRITE done cancelled={}", app.cancelled);
        }
        Some("primary-reader") => {
            let timeout = Duration::from_millis(args.get(2).and_then(|v| v.parse().ok()).unwrap_or(10_000));
            map_window(&mut queue, &mut app, "FlameWM primary reader")?;
            match read_offer(&conn, &mut queue, &mut app, true, timeout) {
                Ok(text) => println!("PRIMARY-READ {text}"),
                Err(error) => {
                    println!("PRIMARY-READ none selection={}", app.primary_offer.is_some());
                    return Err(error);
                }
            }
        }
        _ => return Err("usage: clipboard_client writer <text> [hold_ms] | reader [timeout_ms] | primary-writer <text> [hold_ms] | primary-reader [timeout_ms]".into()),
    }
    Ok(())
}
