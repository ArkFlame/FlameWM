//! X11 root capture and pixel-format conversion. The server supplies the visual masks;
//! assuming BGRA would silently corrupt 16-bit and big-endian desktops.

use std::io::Cursor;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, Window};

pub type Result<T> = std::result::Result<T, String>;

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
    for pixel in data.chunks_exact_mut(4) {
        let native = if little {
            u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]])
        } else {
            u32::from_be_bytes([pixel[0], pixel[1], pixel[2], pixel[3]])
        };
        pixel.copy_from_slice(&[
            component(native, masks[0]),
            component(native, masks[1]),
            component(native, masks[2]),
            255,
        ]);
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
    }
    #[test]
    fn component_masks_scale_565() {
        assert_eq!(component(0xf800, 0xf800), 255);
        assert_eq!(component(0x07e0, 0x07e0), 255);
        assert_eq!(component(0x001f, 0x001f), 255);
        assert_eq!(component(0x0010, 0x001f), 132);
    }
}
