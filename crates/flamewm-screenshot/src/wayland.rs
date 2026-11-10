//! Full-output capture through `zwlr-screencopy-unstable-v1` on a native Wayland session.
//!
//! The X11 path reads the `XWayland` root, which never holds native Wayland windows. When the
//! compositor advertises the screencopy global, full-output captures use it instead. Pixels travel
//! through a `wl_shm` pool backed by an unlinked temporary file: the client writes nothing, reads
//! the file after `ready`, so no `unsafe` mapping is needed.

use std::fs::OpenOptions;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::time::{SystemTime, UNIX_EPOCH};

use wayland_client::globals::{BindError, GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm::{self, Format, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::{
    self, ZwlrScreencopyManagerV1,
};

use crate::capture::Image;

/// Bytes per pixel of `Xrgb8888` / `Argb8888`.
const BPP: usize = 4;

/// Converts the compositor's `Xrgb8888` rows (stride `width * 4`, memory order B, G, R, X) into
/// RGBA. `y_invert` reverses row order, as the protocol flag requires. `None` when the byte count
/// does not match the geometry.
#[must_use]
pub fn xrgb_to_rgba(src: &[u8], width: u32, height: u32, y_invert: bool) -> Option<Vec<u8>> {
    let row = width as usize * BPP;
    if src.len() != row * height as usize {
        return None;
    }
    let mut lines: Vec<&[u8]> = src.chunks_exact(row).collect();
    if y_invert {
        lines.reverse();
    }
    let mut out = Vec::with_capacity(src.len());
    for line in lines {
        for px in line.chunks_exact(BPP) {
            out.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
        }
    }
    Some(out)
}

/// What the compositor announced and reported for one frame.
#[derive(Default)]
struct Frame {
    format: Option<(u32, u32, u32)>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

/// Dispatch target for the connection: owns the frame under capture and the bound globals.
struct Client {
    frame: Frame,
}

/// Captures the whole output on a native Wayland session.
///
/// `Ok(None)` means the Wayland path is not available here (no `WAYLAND_DISPLAY`, or the
/// compositor does not advertise screencopy or an output): the caller falls back to X11. `Err`
/// means screencopy exists but failed, which must be reported rather than silently replaced.
pub fn capture_output() -> Result<Option<Image>, String> {
    // `connect_to_env` reads WAYLAND_DISPLAY itself and fails when it is unset.
    let Ok(conn) = Connection::connect_to_env() else {
        return Ok(None);
    };
    let (globals, mut queue) =
        registry_queue_init::<Client>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
    let qh = queue.handle();
    let Some(manager) = bind_optional::<ZwlrScreencopyManagerV1>(&globals, &qh, 1..=3)? else {
        return Ok(None);
    };
    let Some(output) = bind_optional::<WlOutput>(&globals, &qh, 1..=4)? else {
        return Ok(None);
    };
    let shm: WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_shm: {e}"))?;
    let mut client = Client {
        frame: Frame::default(),
    };
    queue
        .roundtrip(&mut client)
        .map_err(|e| format!("wayland roundtrip: {e}"))?;
    let frame = manager.capture_output(1, &output, &qh, ());
    let image = receive(&mut queue, &mut client, &shm, &qh, &frame)?;
    frame.destroy();
    Ok(Some(image))
}

/// Binds `I` if the compositor advertises it, `None` otherwise.
fn bind_optional<I>(
    globals: &GlobalList,
    qh: &QueueHandle<Client>,
    versions: std::ops::RangeInclusive<u32>,
) -> Result<Option<I>, String>
where
    I: wayland_client::Proxy + 'static,
    Client: Dispatch<I, ()>,
{
    match globals.bind::<I, _, _>(qh, versions, ()) {
        Ok(proxy) => Ok(Some(proxy)),
        Err(BindError::NotPresent) => Ok(None),
        Err(error) => Err(format!("wayland bind: {error}")),
    }
}

/// Drives one capture: announces the buffer, copies into a shm file, waits for `ready`.
fn receive(
    queue: &mut EventQueue<Client>,
    client: &mut Client,
    shm: &WlShm,
    qh: &QueueHandle<Client>,
    frame: &ZwlrScreencopyFrameV1,
) -> Result<Image, String> {
    loop {
        queue
            .blocking_dispatch(client)
            .map_err(|e| format!("wayland dispatch: {e}"))?;
        if client.frame.failed {
            return Err("compositor refused the capture".into());
        }
        let announced =
            client.frame.format.is_some() && (client.frame.buffer_done || frame.version() < 3);
        if announced {
            break;
        }
    }
    let (width, height, stride) = client.frame.format.ok_or("no buffer announced")?;
    let size = u64::from(stride) * u64::from(height);
    let file = temp_file(size)?;
    let pool: WlShmPool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer: WlBuffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        Format::Xrgb8888,
        qh,
        (),
    );
    frame.copy(&buffer);
    while !client.frame.ready && !client.frame.failed {
        queue
            .blocking_dispatch(client)
            .map_err(|e| format!("wayland dispatch: {e}"))?;
    }
    if client.frame.failed {
        return Err("compositor failed the copy".into());
    }
    let mut raw = vec![0u8; size as usize];
    file.read_exact_at(&mut raw, 0)
        .map_err(|e| format!("read capture: {e}"))?;
    let rgba =
        xrgb_to_rgba(&raw, width, height, client.frame.y_invert).ok_or("capture size mismatch")?;
    buffer.destroy();
    pool.destroy();
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// A temporary file for the shm pool, unlinked at once: it lives while the descriptors do.
fn temp_file(size: u64) -> Result<std::fs::File, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("flamewm-shm-{}-{nanos}", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("shm file: {e}"))?;
    let _ = std::fs::remove_file(&path);
    file.set_len(size).map_err(|e| format!("shm size: {e}"))?;
    Ok(file)
}

impl Dispatch<WlRegistry, GlobalListContents> for Client {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: zwlr_screencopy_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for Client {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for Client {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for Client {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wl_shm_pool::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for Client {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wl_buffer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let frame = &mut state.frame;
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if matches!(format, WEnum::Value(Format::Xrgb8888 | Format::Argb8888)) {
                    frame.format = Some((width, height, stride));
                }
            }
            Event::BufferDone => frame.buffer_done = true,
            Event::Flags {
                flags: WEnum::Value(flags),
            } => frame.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert),
            Event::Ready { .. } => frame.ready = true,
            Event::Failed => frame.failed = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrgb_rows_become_rgba_with_opaque_alpha() {
        // One pixel: B=10, G=20, R=30, X=0 in memory.
        let rgba = xrgb_to_rgba(&[10, 20, 30, 0], 1, 1, false).unwrap();
        assert_eq!(rgba, vec![30, 20, 10, 0xff]);
    }

    #[test]
    fn y_invert_reverses_row_order() {
        let top = [1, 1, 1, 0];
        let bottom = [2, 2, 2, 0];
        let src = [top, bottom].concat();
        let flipped = xrgb_to_rgba(&src, 1, 2, true).unwrap();
        assert_eq!(&flipped[..4], &[2, 2, 2, 0xff]);
        let straight = xrgb_to_rgba(&src, 1, 2, false).unwrap();
        assert_eq!(&straight[..4], &[1, 1, 1, 0xff]);
    }

    #[test]
    fn a_short_buffer_is_rejected_not_read_past() {
        assert_eq!(xrgb_to_rgba(&[0; 7], 1, 2, false), None);
    }
}
