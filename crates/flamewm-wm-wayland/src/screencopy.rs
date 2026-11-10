//! `zwlr-screencopy-unstable-v1` output capture.
//!
//! Smithay 0.7 ships no screencopy support, so this module implements the server side of the
//! wlr protocol directly. Scope is deliberately small: whole-output capture into `wl_shm` buffers
//! with full damage. Region capture, dmabuf buffers and cursor compositing are refused or not
//! advertised.
//!
//! Capture is asynchronous. A `copy` request queues a [`PendingCopy`] and asks for a redraw; the
//! render path already owns the renderer, so it reads the frame back once through the shared
//! offscreen path (`render::offscreen_pixels`) and hands the bytes to [`serve`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_shm::Format;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::shm::with_buffer_contents_mut;
use wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::{
    self, ZwlrScreencopyManagerV1,
};

use crate::state::State;

/// Protocol version advertised. Version 3 adds `buffer_done`, which clients use to know every
/// buffer format was announced.
pub const VERSION: u32 = 3;

/// Bytes per pixel of the only advertised format, `Xrgb8888`.
const BPP: usize = 4;

/// A frame whose `copy` request is waiting for the next render.
pub struct PendingCopy {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
}

/// Per-frame user data: whether `copy` has already been used, and the size announced to the
/// client, which the copied buffer must match exactly.
pub struct FrameData {
    used: AtomicBool,
    width: u32,
    height: u32,
}

/// Converts an RGBA readback (`Abgr8888` bytes in memory order R, G, B, A) into `Xrgb8888` rows
/// with a stride of `width * 4`. `flip_rows` reverses row order for flipped outputs, matching the
/// PPM dump. Returns `None` when `rgba` is not exactly `width * height * 4` bytes.
#[must_use]
pub fn xrgb_rows(rgba: &[u8], width: u32, height: u32, flip_rows: bool) -> Option<Vec<u8>> {
    let row = width as usize * BPP;
    if rgba.len() != row * height as usize {
        return None;
    }
    let mut out = Vec::with_capacity(rgba.len());
    let mut lines: Vec<&[u8]> = rgba.chunks_exact(row).collect();
    if flip_rows {
        lines.reverse();
    }
    for line in lines {
        for px in line.chunks_exact(BPP) {
            // Little-endian `Xrgb8888` stores B, G, R, X.
            out.extend_from_slice(&[px[2], px[1], px[0], 0xff]);
        }
    }
    Some(out)
}

/// Seconds and nanoseconds since the Unix epoch, split the way `ready` expects.
fn timestamp() -> (u32, u32, u32) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    ((secs >> 32) as u32, secs as u32, now.subsec_nanos())
}

/// Writes `xrgb` (stride `width * 4`) into a client `wl_shm` buffer.
///
/// Refuses any buffer whose format, size or stride differs from what was announced, so the
/// copy can never run past the pool.
#[allow(unsafe_code)]
fn write_buffer(buffer: &WlBuffer, width: u32, height: u32, xrgb: &[u8]) -> Result<(), String> {
    let row = width as usize * BPP;
    let total = row * height as usize;
    if xrgb.len() != total {
        return Err("readback size mismatch".into());
    }
    with_buffer_contents_mut(buffer, |ptr, pool_len, data| {
        if !matches!(data.format, Format::Xrgb8888 | Format::Argb8888)
            || data.width != width as i32
            || data.height != height as i32
            || data.stride as usize != row
        {
            return Err("buffer does not match the announced frame".to_owned());
        }
        let offset = usize::try_from(data.offset).map_err(|_| "negative buffer offset")?;
        let end = offset.checked_add(total).ok_or("buffer offset overflows")?;
        if end > pool_len {
            return Err("buffer is smaller than its geometry".to_owned());
        }
        // SAFETY: `with_buffer_contents_mut` hands out a pointer to the mapped pool of `pool_len`
        // bytes for the duration of this closure. `end <= pool_len` was checked above, and `xrgb`
        // holds exactly `total` bytes, so the copy stays inside the pool and reads only from
        // `xrgb`.
        unsafe { std::ptr::copy_nonoverlapping(xrgb.as_ptr(), ptr.add(offset), total) };
        Ok(())
    })
    .map_err(|error| format!("not a wl_shm buffer: {error:?}"))?
}

/// Finishes queued copies from one readback. Each frame gets `ready` on success or `failed`
/// when its buffer cannot take the pixels. Dead frames are skipped silently.
pub fn serve(pending: Vec<PendingCopy>, width: u32, height: u32, rgba: &[u8], flip_rows: bool) {
    let Some(xrgb) = xrgb_rows(rgba, width, height, flip_rows) else {
        fail(pending);
        return;
    };
    for copy in pending {
        match write_buffer(&copy.buffer, width, height, &xrgb) {
            Ok(()) => {
                let (hi, lo, nsec) = timestamp();
                copy.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
                copy.frame.ready(hi, lo, nsec);
            }
            Err(reason) => {
                eprintln!("flamewm-wayland: screencopy refused: {reason}");
                copy.frame.failed();
            }
        }
    }
}

/// Fails every queued copy, used when the readback itself cannot run.
pub fn fail(pending: Vec<PendingCopy>) {
    for copy in pending {
        copy.frame.failed();
    }
}

impl State {
    /// Advertises `zwlr_screencopy_manager_v1` to every client. A locked session refuses
    /// capture outright (see `capture_frame`).
    pub fn init_screencopy(&mut self) {
        let _ = self
            .display
            .create_global::<State, ZwlrScreencopyManagerV1, ()>(VERSION, ());
    }

    /// Size of the whole output in physical pixels: what every capture reports.
    #[must_use]
    pub fn screencopy_output_size(&self) -> (u32, u32) {
        let size = self.physical_output_size();
        (size.w.max(0) as u32, size.h.max(0) as u32)
    }
}

/// Sends the `Xrgb8888` buffer description for a new frame, or `failed` while locked. A locked
/// session never yields pixels, so a capture cannot reveal what the lock covers.
fn announce(frame: &ZwlrScreencopyFrameV1, size: (u32, u32), locked: bool) {
    if locked {
        frame.failed();
        return;
    }
    let (width, height) = size;
    frame.buffer(Format::Xrgb8888, width, height, width * BPP as u32);
    if frame.version() >= 3 {
        frame.buffer_done();
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, .. } => {
                let size = state.screencopy_output_size();
                let data = FrameData {
                    used: AtomicBool::new(false),
                    width: size.0,
                    height: size.1,
                };
                let frame = data_init.init(frame, data);
                announce(&frame, size, state.session_locked);
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion { frame, .. } => {
                // Region capture is not implemented: refuse, the client falls back to full output.
                let data = FrameData {
                    used: AtomicBool::new(false),
                    width: 0,
                    height: 0,
                };
                let frame = data_init.init(frame, data);
                frame.failed();
            }
            zwlr_screencopy_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer }
            | zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => {
                if data.used.swap(true, Ordering::Relaxed) {
                    resource.post_error(
                        zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                        "frame already copied",
                    );
                    return;
                }
                if data.width == 0 || data.height == 0 {
                    resource.failed();
                    return;
                }
                state.screencopy_pending.push(PendingCopy {
                    frame: resource.clone(),
                    buffer,
                });
                state.request_redraw();
            }
            zwlr_screencopy_frame_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_readback_becomes_xrgb_rows_with_opaque_alpha() {
        // One pixel read back as R=30, G=20, B=10, A=0.
        let xrgb = xrgb_rows(&[30, 20, 10, 0], 1, 1, false).unwrap();
        assert_eq!(xrgb, vec![10, 20, 30, 0xff]);
    }

    #[test]
    fn flipped_outputs_reverse_row_order() {
        let src = [[1, 1, 1, 0], [2, 2, 2, 0]].concat();
        let flipped = xrgb_rows(&src, 1, 2, true).unwrap();
        assert_eq!(&flipped[..4], &[2, 2, 2, 0xff]);
        let straight = xrgb_rows(&src, 1, 2, false).unwrap();
        assert_eq!(&straight[..4], &[1, 1, 1, 0xff]);
    }

    #[test]
    fn a_readback_that_does_not_match_the_geometry_is_rejected() {
        assert_eq!(xrgb_rows(&[0; 7], 1, 2, false), None);
    }
}
