//! X11 root capture and pixel-format conversion. The server supplies the visual masks;
//! assuming BGRA would silently corrupt 16-bit and big-endian desktops.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Cursor;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, Window};

pub type Result<T> = std::result::Result<T, String>;

/// Backend-neutral capture boundary. A future Wayland screencopy implementation can
/// produce the same root-relative RGBA image without changing selection or export.
pub trait Capture {
    fn capture(&self, rect: Rect) -> Result<Image>;
}

pub struct X11Capture<'a, C: Connection> {
    conn: &'a C,
    screen: usize,
    shm: RefCell<ShmState>,
}

impl<'a, C: Connection> X11Capture<'a, C> {
    pub fn new(conn: &'a C, screen: usize) -> Self {
        Self {
            conn,
            screen,
            shm: RefCell::new(ShmState::default()),
        }
    }
}

impl<C: Connection> Capture for X11Capture<'_, C> {
    fn capture(&self, rect: Rect) -> Result<Image> {
        grab_or_shm(self.conn, self.screen, rect, &mut self.shm.borrow_mut())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn intersect(self, bounds: Self) -> Option<Self> {
        let x = i64::from(self.x).max(i64::from(bounds.x));
        let y = i64::from(self.y).max(i64::from(bounds.y));
        let r =
            (i64::from(self.x) + i64::from(self.w)).min(i64::from(bounds.x) + i64::from(bounds.w));
        let b =
            (i64::from(self.y) + i64::from(self.h)).min(i64::from(bounds.y) + i64::from(bounds.h));
        (r > x && b > y).then_some(Self {
            x: x as i32,
            y: y as i32,
            w: (r - x) as u32,
            h: (b - y) as u32,
        })
    }
    pub fn from_points(ax: i32, ay: i32, bx: i32, by: i32) -> Self {
        let x = ax.min(bx);
        let y = ay.min(by);
        Self {
            x,
            y,
            w: ax.abs_diff(bx).saturating_add(1),
            h: ay.abs_diff(by).saturating_add(1),
        }
    }
}

#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Region selection already owns the full-screen capture. Compact into that
    /// same allocation rather than duplicating the selected pixels in another Vec.
    pub fn crop(mut self, rect: Rect) -> Result<Self> {
        let rect = rect
            .intersect(Rect {
                x: 0,
                y: 0,
                w: self.width,
                h: self.height,
            })
            .ok_or("selected region is outside the capture")?;
        let row_bytes = (rect.w as usize)
            .checked_mul(4)
            .ok_or("region row overflow")?;
        let bytes = row_bytes
            .checked_mul(rect.h as usize)
            .ok_or("region size overflow")?;
        let stride = (self.width as usize)
            .checked_mul(4)
            .ok_or("capture stride overflow")?;
        if self.rgba.len()
            != stride
                .checked_mul(self.height as usize)
                .ok_or("capture size overflow")?
        {
            return Err("invalid capture buffer length".into());
        }
        if rect.x == 0 && rect.y == 0 && rect.w == self.width && rect.h == self.height {
            return Ok(self);
        }
        for row in 0..rect.h as usize {
            let start = (rect.y as usize + row) * stride + rect.x as usize * 4;
            self.rgba
                .copy_within(start..start + row_bytes, row * row_bytes);
        }
        self.rgba.truncate(bytes);
        // Large crop reductions should not pin an entire desktop frame while
        // writing PNG; keep capacity for near-full selections to avoid churn.
        if bytes < self.rgba.capacity() / 2 {
            self.rgba.shrink_to_fit();
        }
        self.width = rect.w;
        self.height = rect.h;
        Ok(self)
    }
    pub fn png(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut output), self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer
                .write_image_data(&self.rgba)
                .map_err(|e| e.to_string())?;
        }
        Ok(output)
    }
}

pub fn root_bounds<C: Connection>(conn: &C, screen: usize) -> Rect {
    let root = &conn.setup().roots[screen];
    Rect {
        x: 0,
        y: 0,
        w: u32::from(root.width_in_pixels),
        h: u32::from(root.height_in_pixels),
    }
}

fn component(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    (((u64::from((value & mask) >> shift) * 255) + u64::from(max) / 2) / u64::from(max)) as u8
}

/// Decodes `XImage` `ZPixmap` bytes, including native padding and 16/24/32 bpp.
pub fn decode_ximage<C: Connection>(
    conn: &C,
    screen: usize,
    depth: u8,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<Image> {
    let setup = conn.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|p| p.depth == depth)
        .ok_or("X11 pixel format unavailable")?;
    let bpp = usize::from(format.bits_per_pixel);
    if !matches!(bpp, 16 | 24 | 32) {
        return Err(format!("unsupported X11 pixel depth {depth}/{bpp}"));
    }
    let pad = usize::from(format.scanline_pad);
    if pad == 0 {
        return Err("invalid server scanline pad".into());
    }
    let stride = (width as usize)
        .checked_mul(bpp)
        .and_then(|bits| bits.checked_add(pad - 1))
        .map(|bits| bits / pad * pad / 8)
        .ok_or("image stride overflow")?;
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("image size overflow")?;
    let total = stride
        .checked_mul(height as usize)
        .ok_or("image byte size overflow")?;
    if data.len() < total {
        return Err("short X11 image reply".into());
    }
    let visual = setup.roots[screen]
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == setup.roots[screen].root_visual)
        .ok_or("X11 root visual missing")?;
    let mut rgba = Vec::with_capacity(count.checked_mul(4).ok_or("capture too large")?);
    for row in data.chunks_exact(stride).take(height as usize) {
        for pixel in row.chunks_exact(bpp / 8).take(width as usize) {
            let value = if setup.image_byte_order == ImageOrder::LSB_FIRST {
                pixel
                    .iter()
                    .rev()
                    .fold(0u32, |v, b| (v << 8) | u32::from(*b))
            } else {
                pixel.iter().fold(0u32, |v, b| (v << 8) | u32::from(*b))
            };
            rgba.extend_from_slice(&[
                component(value, visual.red_mask),
                component(value, visual.green_mask),
                component(value, visual.blue_mask),
                255,
            ]);
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// Convert a tightly packed 32-bit X11 reply in place. The owned reply already
/// occupies four bytes per pixel, so allocating another full RGBA frame is unnecessary.
/// Non-32-bit or padded replies keep the general decoder below.
fn decode_32bpp_in_place(data: &mut [u8], masks: [u32; 3], little: bool) {
    // 8-bit channels (the common case) need only a shift and mask; the general scaling
    // divides per pixel and dominated the capture time at 1080p.
    let shift_of = |mask: u32| if mask == 0 { 0 } else { mask.trailing_zeros() };
    let eight_bit = masks.map(|m| m != 0 && (m >> shift_of(m)) == 0xff);
    let shifts = masks.map(shift_of);
    for pixel in data.chunks_exact_mut(4) {
        let native = if little {
            u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]])
        } else {
            u32::from_be_bytes([pixel[0], pixel[1], pixel[2], pixel[3]])
        };
        let channel = |i: usize| {
            if eight_bit[i] {
                ((native & masks[i]) >> shifts[i]) as u8
            } else {
                component(native, masks[i])
            }
        };
        pixel.copy_from_slice(&[channel(0), channel(1), channel(2), 255]);
    }
}

/// Own the X11 `GetImage` reply so a normal tightly-packed 32-bpp frame can
/// become the returned RGBA frame without making a second full-size copy.
pub fn decode_ximage_owned<C: Connection>(
    conn: &C,
    screen: usize,
    depth: u8,
    width: u32,
    height: u32,
    mut data: Vec<u8>,
) -> Result<Image> {
    let setup = conn.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|p| p.depth == depth)
        .ok_or("X11 pixel format unavailable")?;
    let row = (width as usize)
        .checked_mul(4)
        .ok_or("image stride overflow")?;
    let len = row
        .checked_mul(height as usize)
        .ok_or("image byte size overflow")?;
    let pad = usize::from(format.scanline_pad);
    if format.bits_per_pixel == 32
        && pad != 0
        && row.checked_mul(8).is_some_and(|bits| bits % pad == 0)
        && data.len() == len
    {
        let visual = setup.roots[screen]
            .allowed_depths
            .iter()
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == setup.roots[screen].root_visual)
            .ok_or("X11 root visual missing")?;
        decode_32bpp_in_place(
            &mut data,
            [visual.red_mask, visual.green_mask, visual.blue_mask],
            setup.image_byte_order == ImageOrder::LSB_FIRST,
        );
        return Ok(Image {
            width,
            height,
            rgba: data,
        });
    }
    decode_ximage(conn, screen, depth, width, height, &data)
}

/// A private root-grab connection owned by a background capture worker.
pub struct Grabber {
    conn: x11rb::rust_connection::RustConnection,
    screen: usize,
    shm: ShmState,
}

/// Opens a connection for capture on a worker thread.
pub fn connect_grabber() -> Result<Grabber> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    Ok(Grabber {
        conn,
        screen,
        shm: ShmState::default(),
    })
}

impl Grabber {
    pub fn grab(&mut self, requested: Rect) -> Result<Image> {
        grab_or_shm(&self.conn, self.screen, requested, &mut self.shm)
    }
}

/// Bytes per row the server uses for a 32-bpp ZPixmap with the given scanline pad.
pub fn image_stride(width: u32, scanline_pad: u8) -> usize {
    let pad = usize::from(scanline_pad).max(8);
    (width as usize * 32).div_ceil(pad) * pad / 8
}

/// MIT-SHM state for one capture owner. The segment is a file descriptor passed to the
/// server (SHM 1.2), so no System V IPC or raw FFI is needed. A failed setup disables SHM
/// for this owner and every capture falls back to GetImage.
#[derive(Default)]
pub struct ShmState {
    segment: Option<ShmSegment>,
    disabled: bool,
}

struct ShmSegment {
    seg: u32,
    /// The server writes into this file's pages; reading it back sees those pixels.
    file: File,
    len: usize,
}

/// Captures through MIT-SHM when available and falls back to GetImage otherwise.
pub fn grab_or_shm<C: Connection>(
    conn: &C,
    screen: usize,
    requested: Rect,
    state: &mut ShmState,
) -> Result<Image> {
    // Clip first so an offscreen request is an error and never disables SHM.
    let rect = requested
        .intersect(root_bounds(conn, screen))
        .ok_or("capture rectangle offscreen")?;
    if !state.disabled {
        match grab_shm(conn, screen, rect, state) {
            Ok(image) => return Ok(image),
            Err(_) => {
                state.disabled = true;
                state.segment = None;
            }
        }
    }
    grab(conn, screen, rect)
}

fn grab_shm<C: Connection>(
    conn: &C,
    screen: usize,
    rect: Rect,
    state: &mut ShmState,
) -> Result<Image> {
    let root_info = &conn.setup().roots[screen];
    let root = root_info.root;
    let pad = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|p| p.depth == root_info.root_depth && p.bits_per_pixel == 32)
        .map(|p| p.scanline_pad)
        .ok_or("shm needs a 32-bpp root format")?;
    let len = image_stride(rect.w, pad) * rect.h as usize;
    let reuse = matches!(&state.segment, Some(s) if s.len >= len);
    if !reuse {
        if let Some(old) = state.segment.take() {
            let _ = conn.shm_detach(old.seg);
        }
        state.segment = Some(create_segment(conn, len)?);
    }
    let segment = state.segment.as_ref().ok_or("shm segment missing")?;
    let reply = conn
        .shm_get_image(
            root,
            i16::try_from(rect.x).map_err(|_| "X11 coordinate overflow")?,
            i16::try_from(rect.y).map_err(|_| "X11 coordinate overflow")?,
            u16::try_from(rect.w).map_err(|_| "capture width exceeds X11 limit")?,
            u16::try_from(rect.h).map_err(|_| "capture height exceeds X11 limit")?,
            u32::MAX,
            u8::from(ImageFormat::Z_PIXMAP),
            segment.seg,
            0,
        )
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    // The reply arrives after the server finished writing, so the bytes are complete here.
    let mut data = vec![0u8; len];
    segment
        .file
        .read_exact_at(&mut data, 0)
        .map_err(|e| e.to_string())?;
    decode_ximage_owned(conn, screen, reply.depth, rect.w, rect.h, data)
}

fn create_segment<C: Connection>(conn: &C, len: usize) -> Result<ShmSegment> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = if Path::new("/dev/shm").is_dir() {
        PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    let path = dir.join(format!(
        "flamewm-capture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    // Unlink at once: our handles and the server's descriptor keep the memory alive.
    let _ = std::fs::remove_file(&path);
    file.set_len(len as u64).map_err(|e| e.to_string())?;
    let seg = conn.generate_id().map_err(|e| e.to_string())?;
    let server_fd = file.try_clone().map_err(|e| e.to_string())?;
    conn.shm_attach_fd(seg, server_fd, false)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(ShmSegment { seg, file, len })
}

#[cfg(test)]
mod shm_layout_tests {
    use super::image_stride;

    #[test]
    fn stride_matches_server_padding() {
        // 32 bpp with 32-bit pad: one 4-byte pixel per column.
        assert_eq!(image_stride(3, 32), 12);
        assert_eq!(image_stride(2560, 32), 2560 * 4);
        // 64-bit pad rounds the row up to 8 bytes.
        assert_eq!(image_stride(3, 64), 16);
        assert_eq!(image_stride(1, 8), 4);
    }
}

pub fn grab<C: Connection>(conn: &C, screen: usize, requested: Rect) -> Result<Image> {
    let rect = requested
        .intersect(root_bounds(conn, screen))
        .ok_or("capture rectangle offscreen")?;
    let root = conn.setup().roots[screen].root;
    let reply = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            root,
            i16::try_from(rect.x).map_err(|_| "X11 coordinate overflow")?,
            i16::try_from(rect.y).map_err(|_| "X11 coordinate overflow")?,
            u16::try_from(rect.w).map_err(|_| "capture width exceeds X11 limit")?,
            u16::try_from(rect.h).map_err(|_| "capture height exceeds X11 limit")?,
            u32::MAX,
        )
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    decode_ximage_owned(conn, screen, reply.depth, rect.w, rect.h, reply.data)
}

fn atom<C: Connection>(conn: &C, name: &[u8]) -> Result<u32> {
    conn.intern_atom(false, name)
        .map_err(|e| e.to_string())?
        .reply()
        .map(|r| r.atom)
        .map_err(|e| e.to_string())
}

pub fn active_window<C: Connection>(conn: &C, root: Window) -> Result<Window> {
    let active = atom(conn, b"_NET_ACTIVE_WINDOW")?;
    let property = conn
        .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    property
        .value32()
        .and_then(|mut v| v.next())
        .filter(|id| *id != 0)
        .ok_or("no active window".into())
}

pub fn pointer_window<C: Connection>(conn: &C, root: Window) -> Result<Window> {
    conn.query_pointer(root)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())
        .and_then(|p| {
            (p.child != 0)
                .then_some(p.child)
                .ok_or("no window under pointer".into())
        })
}

pub fn window_rect<C: Connection>(conn: &C, root: Window, window: Window) -> Result<Rect> {
    let mut window = window;
    // Managed client windows can be reparented into a decoration frame.
    for _ in 0..12 {
        let tree = conn
            .query_tree(window)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        if tree.parent == root || tree.parent == 0 {
            break;
        }
        window = tree.parent;
    }
    let geom = conn
        .get_geometry(window)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let translated = conn
        .translate_coordinates(window, root, 0, 0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let border = i32::from(geom.border_width);
    Ok(Rect {
        x: i32::from(translated.dst_x) - border,
        y: i32::from(translated.dst_y) - border,
        w: u32::from(geom.width) + border as u32 * 2,
        h: u32::from(geom.height) + border as u32 * 2,
    })
}

/// Blend the live ARGB cursor image into a root-relative screenshot. `XGetImage`
/// excludes hardware cursor sprites; this explicit option avoids surprising users.
pub fn blend_cursor<C: Connection>(conn: &C, image: &mut Image, root_rect: Rect) -> Result<()> {
    let cursor = conn
        .xfixes_get_cursor_image()
        .map_err(|e| format!("XFixes cursor unavailable: {e}"))?
        .reply()
        .map_err(|e| format!("XFixes cursor unavailable: {e}"))?;
    let left = i32::from(cursor.x) - i32::from(cursor.xhot) - root_rect.x;
    let top = i32::from(cursor.y) - i32::from(cursor.yhot) - root_rect.y;
    for sy in 0..u32::from(cursor.height) {
        for sx in 0..u32::from(cursor.width) {
            let dx = left + sx as i32;
            let dy = top + sy as i32;
            if dx < 0 || dy < 0 || dx as u32 >= image.width || dy as u32 >= image.height {
                continue;
            }
            let argb = cursor.cursor_image[sy as usize * cursor.width as usize + sx as usize];
            let alpha = (argb >> 24) & 255;
            if alpha == 0 {
                continue;
            }
            let at = (dy as usize * image.width as usize + dx as usize) * 4;
            for (channel, shift) in [16, 8, 0].into_iter().enumerate() {
                let source = (argb >> shift) & 255;
                let old = u32::from(image.rgba[at + channel]);
                image.rgba[at + channel] =
                    ((source * alpha + old * (255 - alpha) + 127) / 255) as u8;
            }
        }
    }
    Ok(())
}

pub fn current_screen<C: Connection>(conn: &C, root: Window, full: Rect) -> Result<Rect> {
    let pointer = conn
        .query_pointer(root)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    if let Ok(cookie) = conn.randr_get_screen_resources_current(root) {
        if let Ok(res) = cookie.reply() {
            for crtc in res.crtcs {
                if let Ok(cookie) = conn.randr_get_crtc_info(crtc, x11rb::CURRENT_TIME) {
                    if let Ok(info) = cookie.reply() {
                        if info.width == 0 || info.height == 0 {
                            continue;
                        }
                        let rect = Rect {
                            x: i32::from(info.x),
                            y: i32::from(info.y),
                            w: u32::from(info.width),
                            h: u32::from(info.height),
                        };
                        if i32::from(pointer.root_x) >= rect.x
                            && i32::from(pointer.root_y) >= rect.y
                            && i32::from(pointer.root_x) < rect.x + rect.w as i32
                            && i32::from(pointer.root_y) < rect.y + rect.h as i32
                        {
                            return Ok(rect.intersect(full).unwrap_or(full));
                        }
                    }
                }
            }
        }
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipped_capture_and_reverse_drag() {
        let r = Rect::from_points(10, 30, 0, 2);
        assert_eq!(
            r,
            Rect {
                x: 0,
                y: 2,
                w: 11,
                h: 29
            }
        );
        assert_eq!(
            r.intersect(Rect {
                x: 4,
                y: 5,
                w: 3,
                h: 10
            }),
            Some(Rect {
                x: 4,
                y: 5,
                w: 3,
                h: 10
            })
        );
        assert!(
            r.intersect(Rect {
                x: 500,
                y: 0,
                w: 2,
                h: 2
            })
            .is_none()
        );
    }
    #[test]
    fn crop_preserves_rgba_layout() {
        let image = Image {
            width: 2,
            height: 2,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
        };
        assert_eq!(
            image
                .crop(Rect {
                    x: 1,
                    y: 0,
                    w: 1,
                    h: 2
                })
                .unwrap()
                .rgba,
            [4, 5, 6, 255, 10, 11, 12, 255]
        );
    }
    #[test]
    fn crop_reuses_full_frame_storage_and_works_with_overlapping_rows() {
        let mut original = Vec::new();
        for i in 0..12u8 {
            original.extend_from_slice(&[i, i, i, 255]);
        }
        let image = Image {
            width: 4,
            height: 3,
            rgba: original,
        };
        let cropped = image
            .crop(Rect {
                x: 1,
                y: 1,
                w: 3,
                h: 2,
            })
            .unwrap();
        assert_eq!((cropped.width, cropped.height), (3, 2));
        assert_eq!(
            cropped
                .rgba
                .chunks_exact(4)
                .map(|p| p[0])
                .collect::<Vec<_>>(),
            [5, 6, 7, 9, 10, 11]
        );
        let image = Image {
            width: 1,
            height: 1,
            rgba: vec![2, 3, 4, 255],
        };
        let ptr = image.rgba.as_ptr();
        assert_eq!(
            image
                .crop(Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1
                })
                .unwrap()
                .rgba
                .as_ptr(),
            ptr
        );
    }

    #[test]
    fn owned_ximage_fast_path_matches_mask_decoding_and_reuses_pixels() {
        let expected = [[0x12, 0x34, 0x56, 255], [0xa0, 0xb0, 0xc0, 255]];
        for little in [true, false] {
            let mut bytes = Vec::new();
            for word in [0x0012_3456u32, 0x00a0_b0c0] {
                bytes.extend_from_slice(&if little {
                    word.to_le_bytes()
                } else {
                    word.to_be_bytes()
                });
            }
            let ptr = bytes.as_ptr();
            decode_32bpp_in_place(&mut bytes, [0x00ff_0000, 0x0000_ff00, 0x0000_00ff], little);
            assert_eq!(bytes.as_ptr(), ptr, "must preserve reply allocation");
            for (pixel, wanted) in bytes.chunks_exact(4).zip(expected) {
                assert_eq!(pixel, wanted);
            }
        }
    }

    #[test]
    fn owned_ximage_fast_path_preserves_nonstandard_channel_masks() {
        let mut bytes = 0x0045_2311u32.to_le_bytes().to_vec();
        decode_32bpp_in_place(&mut bytes, [0x0000_00ff, 0x0000_ff00, 0x00ff_0000], true);
        assert_eq!(bytes, [0x11, 0x23, 0x45, 0xff]);
    }

    #[test]
    fn png_encoder_writes_full_rgba() {
        let img = Image {
            width: 1,
            height: 1,
            rgba: vec![0xef, 0x40, 0x48, 255],
        };
        let encoded = img.png().unwrap();
        assert!(encoded.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(encoded.len() > 32);
        let mut reader = png::Decoder::new(Cursor::new(encoded)).read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((frame.width, frame.height), (1, 1));
        assert_eq!(frame.color_type, png::ColorType::Rgba);
        assert_eq!(&decoded[..frame.buffer_size()], img.rgba);
    }

    #[test]
    fn png_encoder_rejects_short_pixel_buffer() {
        let img = Image {
            width: 2,
            height: 1,
            rgba: vec![0; 4],
        };
        assert!(img.png().is_err());
    }
    #[test]
    fn fast_8bit_path_matches_general_scaling() {
        // Same answer whether a channel takes the shift path or the scaling path.
        let masks = [0x00ff_0000, 0x0000_ff00, 0x0000_00ff];
        for value in [0u32, 0x0012_3456, 0x00ff_ffff, 0x0080_8080] {
            let mut fast = value.to_le_bytes();
            decode_32bpp_in_place(&mut fast, masks, true);
            let expected = [
                component(value, masks[0]),
                component(value, masks[1]),
                component(value, masks[2]),
                255,
            ];
            assert_eq!(fast, expected, "value {value:#x}");
        }
    }

    #[test]
    fn component_masks_scale_565() {
        assert_eq!(component(0xf800, 0xf800), 255);
        assert_eq!(component(0x07e0, 0x07e0), 255);
        assert_eq!(component(0x001f, 0x001f), 255);
        assert_eq!(component(0x0010, 0x001f), 132);
    }
}
