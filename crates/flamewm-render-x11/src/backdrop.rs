//! Snap-preview backdrop: root readback under the preview rect.
//! No compositor, no XSHM. Pure conversion + one X11-backed capture helper.
//! Flow (caller-gated on target/rect change): capture under rect via
//! XGetImage on the root, convert pixels with live server masks/byte order
//! (never hardcoded RGB order), retain as preview background, then draw the
//! desktop-selection material over it. Failure -> border-only preview.
//! No capture runs unless the caller reports a geometry change.

use crate::xlib::*;

/// Accent from the shared `flamewm-skin` selection contract (0xef4048).
pub const BACKDROP_ACCENT_RGB: (u8, u8, u8) = (0xef, 0x40, 0x48);

/// Border alpha family: strong accent edge over the captured backdrop.
pub const BACKDROP_BORDER_ALPHA: u8 = 0xdd;
/// Border width family in device px: matches the compiled snap-preview CSS.
pub const BACKDROP_BORDER_WIDTH_PX: u32 = 2;

/// Straight RGBA8 capture plus the server format it was decoded from.
#[derive(Debug, Clone)]
pub struct BackdropCapture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Bit position of the lowest set bit in a channel mask. Zero mask -> 0.
fn mask_shift(mask: u64) -> u32 {
    if mask == 0 {
        return 0;
    }
    mask.trailing_zeros()
}

/// Bit width of the contiguous set field in a channel mask, from its lowest
/// set bit. Zero mask -> 0.
fn mask_width(mask: u64) -> u32 {
    if mask == 0 {
        return 0;
    }
    64 - (mask >> mask_shift(mask)).leading_zeros()
}

/// Scale one extracted channel field to 8 bits using the live mask geometry.
fn scale_channel(value: u64, mask: u64) -> u8 {
    let width = mask_width(mask);
    if width == 0 {
        return 0;
    }
    let field = (value & mask) >> mask_shift(mask);
    if width >= 8 {
        (field >> (width - 8)) as u8
    } else {
        ((field * 255) / ((1u64 << width) - 1)) as u8
    }
}

/// Decode one server pixel word into (r, g, b) using live visual masks.
/// No hardcoded channel order: shifts/widths come from the server.
#[must_use]
pub fn decode_pixel(pixel: u64, red_mask: u64, green_mask: u64, blue_mask: u64) -> (u8, u8, u8) {
    (
        scale_channel(pixel, red_mask),
        scale_channel(pixel, green_mask),
        scale_channel(pixel, blue_mask),
    )
}

/// Translucent accent fill over a captured backdrop. Pure and testable:
/// 20% default maps to alpha 51 to match the existing preview contract.
#[must_use]
pub fn fill_color(opacity_percent: u8) -> (u8, u8, u8, u8) {
    let opacity = u32::from(opacity_percent.min(100));
    let alpha = ((opacity * 255 + 50) / 100) as u8;
    (
        BACKDROP_ACCENT_RGB.0,
        BACKDROP_ACCENT_RGB.1,
        BACKDROP_ACCENT_RGB.2,
        alpha,
    )
}

pub type SelectionMaterial = ((u8, u8, u8, u8), (u8, u8, u8, u8));

/// Shared desktop-selection material for one material, two consumers
/// (desktop selection renderer and snap-preview backdrop): translucent
/// accent fill plus strong accent border ring. Pure and headless-testable.
#[must_use]
pub fn selection_material(opacity_percent: u8) -> SelectionMaterial {
    (
        fill_color(opacity_percent),
        (
            BACKDROP_ACCENT_RGB.0,
            BACKDROP_ACCENT_RGB.1,
            BACKDROP_ACCENT_RGB.2,
            BACKDROP_BORDER_ALPHA,
        ),
    )
}

/// Channel masks to decode with. XGetImage on a pixmap reports zero masks, so a zero triple
/// falls back to the 24/32-bpp TrueColor layout (red, green, blue in the low three bytes).
/// Window images keep the masks their visual reports.
pub fn channel_masks(red: u64, green: u64, blue: u64) -> (u64, u64, u64) {
    if red | green | blue == 0 {
        (0xff_0000, 0xff_00, 0xff)
    } else {
        (red, green, blue)
    }
}

/// XGetImage of any window or pixmap drawable (`rect` is drawable-local),
/// decoded to straight RGBA8 with the live server masks.
pub(crate) fn capture_drawable_rect(
    display: *mut Display,
    drawable: u64,
    rect: (i32, i32, u32, u32),
) -> Result<BackdropCapture, String> {
    use std::os::raw::{c_int, c_ulong};
    let (x, y, width, height) = rect;
    if width == 0 || height == 0 {
        return Err("backdrop capture: empty rect".to_string());
    }
    if display.is_null() || drawable == 0 {
        return Err("backdrop capture: no display/root".to_string());
    }
    let image = unsafe {
        XGetImage(
            display,
            drawable as c_ulong,
            x as c_int,
            y as c_int,
            width,
            height,
            c_ulong::MAX,
            ZPIXMAP,
        )
    };
    if image.is_null() {
        eprintln!("debug render.overlay.capture reason=xgetimage-null");
        return Err("backdrop capture unsupported (XGetImage returned NULL)".to_string());
    }
    let head = unsafe { &*(image as *const XImageHead) };
    if head.depth != 24 && head.depth != 32 {
        unsafe { XDestroyImage(image) };
        eprintln!(
            "debug render.overlay.capture reason=unsupported-depth depth={}",
            head.depth
        );
        return Err(format!("backdrop capture unsupported depth {}", head.depth));
    }
    if head.width <= 0 || head.height <= 0 || head.data.is_null() {
        unsafe { XDestroyImage(image) };
        eprintln!("debug render.overlay.capture reason=invalid-image");
        return Err("backdrop capture produced an invalid XImage".to_string());
    }
    let width_px = head.width as u32;
    let height_px = head.height as u32;
    let (red_mask, green_mask, blue_mask) = channel_masks(
        head.red_mask as u64,
        head.green_mask as u64,
        head.blue_mask as u64,
    );
    let byte_order = head.byte_order as u32;
    let bits_per_pixel = head.bits_per_pixel as u32;
    let bytes_per_line = head.bytes_per_line as usize;
    // XImage data is owned by the XImage; copy out row by row honoring
    // bytes_per_line, bits_per_pixel, and server byte order.
    let data = unsafe {
        std::slice::from_raw_parts(
            head.data as *const u8,
            bytes_per_line.saturating_mul(height_px as usize),
        )
    };
    let bytes_per_pixel = match bits_per_pixel {
        24 | 32 => 4usize,
        16 => 2usize,
        _ => {
            unsafe { XDestroyImage(image) };
            eprintln!("debug render.overlay.capture reason=unsupported-bpp bpp={bits_per_pixel}");
            return Err(format!("backdrop capture unsupported bpp {bits_per_pixel}"));
        }
    };
    let row_out_len = (width_px as usize) * 4;
    let mut pixels = vec![0u8; row_out_len * (height_px as usize)];
    let plain_bgrx = bytes_per_pixel == 4
        && byte_order == 0
        && (red_mask, green_mask, blue_mask) == (0xff_0000, 0xff_00, 0xff)
        && bytes_per_line >= row_out_len
        && data.len() >= bytes_per_line * (height_px as usize - 1) + row_out_len;
    if plain_bgrx {
        for (row, out_row) in pixels.chunks_exact_mut(row_out_len).enumerate() {
            let source = &data[row * bytes_per_line..row * bytes_per_line + row_out_len];
            for (out, pixel) in out_row.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
                out.copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        unsafe { XDestroyImage(image) };
        return Ok(BackdropCapture {
            width: width_px,
            height: height_px,
            pixels,
        });
    }
    for (row, out_row) in pixels.chunks_exact_mut(row_out_len).enumerate() {
        let row_base = row.saturating_mul(bytes_per_line);
        for (col, out) in out_row.chunks_exact_mut(4).enumerate() {
            let off = row_base.saturating_add(col.saturating_mul(bytes_per_pixel));
            if off + bytes_per_pixel > data.len() {
                unsafe { XDestroyImage(image) };
                eprintln!("debug render.overlay.capture reason=short-row");
                return Err("backdrop capture row overrun".to_string());
            }
            let word: u64 = match bytes_per_pixel {
                4 => {
                    let raw = [data[off], data[off + 1], data[off + 2], data[off + 3]];
                    // Server byte order decides word assembly; LSBFirst
                    // (0) vs MSBFirst (1) per XImage.byte_order.
                    if byte_order == 1 {
                        u32::from_be_bytes(raw) as u64
                    } else {
                        u32::from_le_bytes(raw) as u64
                    }
                }
                2 => {
                    let raw = [data[off], data[off + 1]];
                    if byte_order == 1 {
                        u16::from_be_bytes(raw) as u64
                    } else {
                        u16::from_le_bytes(raw) as u64
                    }
                }
                _ => 0,
            };
            let (r, g, b) = decode_pixel(word, red_mask, green_mask, blue_mask);
            out.copy_from_slice(&[r, g, b, 255]);
        }
    }
    unsafe { XDestroyImage(image) };
    Ok(BackdropCapture {
        width: width_px,
        height: height_px,
        pixels,
    })
}

/// Blur radius at 100 percent (`blur / 100 * 24`), shared by every pseudo-transparency surface.
pub const MAX_BLUR_RADIUS_PX: f32 = 24.0;

/// Radius for a blur percent. Blur only applies below 100% opacity, so 100% never blurs.
#[must_use]
pub fn blur_radius_px(opacity_percent: u8, blur_percent: u8) -> f32 {
    if opacity_percent >= 100 {
        return 0.0;
    }
    f32::from(blur_percent.min(100)) / 100.0 * MAX_BLUR_RADIUS_PX
}

/// Blur runs on a snapshot shrunk by this factor once the radius is large: the blur cost falls with
/// the pixel count, and the soft result is upscaled bilinearly. Below twice the factor the blur is
/// already cheap, so it stays at full resolution.
pub(crate) const BLUR_DOWNSCALE: u32 = 4;

/// Opaque ARGB32 words (0xFFRRGGBB) for a straight RGBA8 snapshot, blurred per
/// [`blur_radius_px`]. Takes ownership so the blur runs in place without a copy.
/// Blur failure keeps the unblurred pixels.
#[must_use]
pub fn blurred_argb_words(
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
    opacity_percent: u8,
    blur_percent: u8,
) -> Vec<u32> {
    let radius = blur_radius_px(opacity_percent, blur_percent);
    if radius > 0.0 && crate::accel::gpu_blur_rgba(&mut rgba, width, height, radius) {
        return opaque_argb_words(&rgba);
    }
    if radius >= 2.0 * BLUR_DOWNSCALE as f32 {
        let (mut small, small_w, small_h) = box_downscale(&rgba, width, height, BLUR_DOWNSCALE);
        // The original full-resolution input is no longer needed. Release it
        // before the costly blur/upsample rather than keeping both alive.
        drop(rgba);
        blur_in_place_slice(&mut small, small_w, small_h, radius / BLUR_DOWNSCALE as f32);
        return bilinear_argb_words(&small, small_w, small_h, width, height);
    }
    if radius > 0.0 {
        blur_in_place_slice(&mut rgba, width, height, radius);
    }
    opaque_argb_words(&rgba)
}

fn opaque_argb_words(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|p| 0xff00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
        .collect()
}

/// Gaussian blur through the shared scratch buffers. A failure keeps the input pixels.
pub(crate) fn blur_in_place_slice(pixels: &mut [u8], width: u32, height: u32, radius: f32) {
    crate::accel::blur_rgba(pixels, width, height, radius);
}

/// Averages `factor` by `factor` blocks (edge blocks average what is inside the image).
fn box_downscale(rgba: &[u8], width: u32, height: u32, factor: u32) -> (Vec<u8>, u32, u32) {
    let (small_w, small_h) = (width.div_ceil(factor), height.div_ceil(factor));
    let mut out = vec![0u8; (small_w * small_h * 4) as usize];
    for sy in 0..small_h {
        for sx in 0..small_w {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for y in sy * factor..((sy + 1) * factor).min(height) {
                for x in sx * factor..((sx + 1) * factor).min(width) {
                    let i = ((y * width + x) * 4) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(rgba[i + channel]);
                    }
                    count += 1;
                }
            }
            let o = ((sy * small_w + sx) * 4) as usize;
            for (channel, total) in sum.iter().enumerate() {
                out[o + channel] = (total / count) as u8;
            }
        }
    }
    (out, small_w, small_h)
}

/// Bilinear taps `(lower index, upper index, weight of the upper tap out of 256)` mapping each of
/// `out` target coordinates onto a source of `len` samples.
fn bilinear_taps(len: u32, out: u32) -> Vec<(usize, usize, u32)> {
    (0..out).map(|i| bilinear_tap(len, out, i)).collect()
}

/// The [`bilinear_taps`] entry for target coordinate `i`.
#[inline]
fn bilinear_tap(len: u32, out: u32, i: u32) -> (usize, usize, u32) {
    let scale = len as f32 / out as f32;
    let f = ((i as f32 + 0.5) * scale - 0.5).clamp(0.0, (len - 1) as f32);
    let lo = f.floor();
    let hi = (lo as u32 + 1).min(len - 1);
    (lo as usize, hi as usize, ((f - lo) * 256.0).round() as u32)
}

/// One source row blended horizontally at every column tap, unrounded (`channel * 256`), so the
/// vertical blend in [`BlurredCanvas::crop`] yields exactly the words of [`bilinear_word`].
fn horizontal_line(row: &[u8], cols: &[(usize, usize, u32)], line: &mut [u32]) {
    for (&(a, b, wx), out) in cols.iter().zip(line.chunks_exact_mut(3)) {
        let (a_off, b_off) = (a * 4, b * 4);
        for (channel, value) in out.iter_mut().enumerate() {
            *value =
                u32::from(row[a_off + channel]) * (256 - wx) + u32::from(row[b_off + channel]) * wx;
        }
    }
}

/// One opaque ARGB32 word blended from two source rows at a column tap and a row weight.
#[inline]
fn bilinear_word(top_row: &[u8], bottom_row: &[u8], col: (usize, usize, u32), wy: u32) -> u32 {
    let (a, b, wx) = (col.0 * 4, col.1 * 4, col.2);
    let mut word = 0xff00_0000u32;
    for channel in 0..3usize {
        let top =
            u32::from(top_row[a + channel]) * (256 - wx) + u32::from(top_row[b + channel]) * wx;
        let bottom = u32::from(bottom_row[a + channel]) * (256 - wx)
            + u32::from(bottom_row[b + channel]) * wx;
        let value = (top * (256 - wy) + bottom * wy + (1 << 15)) >> 16;
        word |= value << (16 - 8 * channel);
    }
    word
}

/// Bilinear upscale of a small RGBA image to opaque ARGB32 words for a `width` x `height` target.
/// Weights are 8-bit fixed point and the taps are computed once per column and per row.
fn bilinear_argb_words(
    small: &[u8],
    small_w: u32,
    small_h: u32,
    width: u32,
    height: u32,
) -> Vec<u32> {
    let (cols, rows) = (
        bilinear_taps(small_w, width),
        bilinear_taps(small_h, height),
    );
    let stride = small_w as usize * 4;
    let mut words = Vec::with_capacity((width * height) as usize);
    for &(r0, r1, wy) in &rows {
        let (top_row, bottom_row) = (&small[r0 * stride..], &small[r1 * stride..]);
        words.extend(
            cols.iter()
                .map(|&col| bilinear_word(top_row, bottom_row, col, wy)),
        );
    }
    words
}

/// A blurred root-sized image stored at the resolution its blur leaves meaningful.
///
/// A radius of at least twice [`BLUR_DOWNSCALE`] blurs a 1/`BLUR_DOWNSCALE` copy and upsamples it
/// (see [`blurred_argb_words`]); that small copy is all this keeps, 1/16 of the full image, and a
/// crop upsamples only the pixels it returns. A crop is pixel-identical to cropping the full
/// upsampled image. Smaller radii keep full-resolution words.
#[derive(Debug)]
pub struct BlurredCanvas {
    width: u32,
    height: u32,
    pixels: CanvasPixels,
}

#[derive(Debug)]
enum CanvasPixels {
    Full(Vec<u32>),
    Scaled {
        small: Vec<u8>,
        small_w: u32,
        small_h: u32,
    },
}

impl BlurredCanvas {
    /// Blurs a `width` x `height` RGBA image per [`blur_radius_px`].
    pub fn new(
        width: u32,
        height: u32,
        mut rgba: Vec<u8>,
        opacity_percent: u8,
        blur_percent: u8,
    ) -> Self {
        let radius = blur_radius_px(opacity_percent, blur_percent);
        let pixels = if radius >= 2.0 * BLUR_DOWNSCALE as f32 {
            let (mut small, small_w, small_h) = box_downscale(&rgba, width, height, BLUR_DOWNSCALE);
            drop(rgba); // Release large input before blur; only the small canvas is retained.
            blur_in_place_slice(&mut small, small_w, small_h, radius / BLUR_DOWNSCALE as f32);
            CanvasPixels::Scaled {
                small,
                small_w,
                small_h,
            }
        } else {
            CanvasPixels::Full(blurred_argb_words(
                width,
                height,
                std::mem::take(&mut rgba),
                opacity_percent,
                blur_percent,
            ))
        };
        Self {
            width,
            height,
            pixels,
        }
    }

    /// A canvas from a small image stored earlier (see [`Self::scaled_bytes`]); `None` when the
    /// bytes do not describe a `width` x `height` canvas.
    pub(crate) fn from_scaled(width: u32, height: u32, small: Vec<u8>) -> Option<Self> {
        let (small_w, small_h) = (
            width.div_ceil(BLUR_DOWNSCALE),
            height.div_ceil(BLUR_DOWNSCALE),
        );
        (width > 0 && height > 0 && small.len() == (small_w * small_h * 4) as usize).then_some(
            Self {
                width,
                height,
                pixels: CanvasPixels::Scaled {
                    small,
                    small_w,
                    small_h,
                },
            },
        )
    }

    /// The stored small image, when this canvas keeps one.
    pub(crate) fn scaled_bytes(&self) -> Option<&[u8]> {
        match &self.pixels {
            CanvasPixels::Scaled { small, .. } => Some(small),
            CanvasPixels::Full(_) => None,
        }
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The `size` region at `origin` as opaque ARGB32 words. Pixels off the canvas replicate its
    /// nearest edge.
    #[must_use]
    pub fn crop(&self, origin: (i32, i32), size: (u32, u32)) -> Vec<u32> {
        if self.width == 0 || self.height == 0 {
            return Vec::new();
        }
        let clamp = |start: i32, offset: u32, len: u32| {
            (i64::from(start) + i64::from(offset)).clamp(0, i64::from(len) - 1) as usize
        };
        let mut out = Vec::with_capacity(size.0 as usize * size.1 as usize);
        match &self.pixels {
            CanvasPixels::Full(words) => {
                for y in 0..size.1 {
                    let row = &words[clamp(origin.1, y, self.height) * self.width as usize..]
                        [..self.width as usize];
                    out.extend((0..size.0).map(|x| row[clamp(origin.0, x, self.width)]));
                }
            }
            CanvasPixels::Scaled {
                small,
                small_w,
                small_h,
            } => {
                // Separable: each source row is blended horizontally once per crop, and every
                // output row only blends two of those lines. Taps cover the crop, not the root.
                let cols = (0..size.0)
                    .map(|x| {
                        bilinear_tap(*small_w, self.width, clamp(origin.0, x, self.width) as u32)
                    })
                    .collect::<Vec<_>>();
                let stride = *small_w as usize * 4;
                let line_len = size.0 as usize * 3;
                let mut top = (usize::MAX, vec![0u32; line_len]);
                let mut bottom = (usize::MAX, vec![0u32; line_len]);
                for y in 0..size.1 {
                    let (r0, r1, wy) = bilinear_tap(
                        *small_h,
                        self.height,
                        clamp(origin.1, y, self.height) as u32,
                    );
                    if top.0 != r0 {
                        if bottom.0 == r0 {
                            std::mem::swap(&mut top, &mut bottom);
                        } else {
                            horizontal_line(&small[r0 * stride..], &cols, &mut top.1);
                            top.0 = r0;
                        }
                    }
                    if bottom.0 != r1 && r1 != r0 {
                        horizontal_line(&small[r1 * stride..], &cols, &mut bottom.1);
                        bottom.0 = r1;
                    }
                    let lower = if r1 == r0 { &top.1 } else { &bottom.1 };
                    out.extend(top.1.chunks_exact(3).zip(lower.chunks_exact(3)).map(
                        |(upper, lower)| {
                            let mut word = 0xff00_0000u32;
                            for channel in 0..3usize {
                                let value =
                                    (upper[channel] * (256 - wy) + lower[channel] * wy + (1 << 15))
                                        >> 16;
                                word |= value << (16 - 8 * channel);
                            }
                            word
                        },
                    ));
                }
            }
        }
        out
    }

    /// [`Self::crop`] at `origin`, reusing `previous`, the same-sized crop at `previous_origin`:
    /// the overlap is copied and only the newly exposed strips are computed.
    #[must_use]
    pub fn recrop(
        &self,
        previous: &[u32],
        previous_origin: (i32, i32),
        origin: (i32, i32),
        size: (u32, u32),
    ) -> Vec<u32> {
        let (width, height) = (size.0 as usize, size.1 as usize);
        let dx = i64::from(origin.0) - i64::from(previous_origin.0);
        let dy = i64::from(origin.1) - i64::from(previous_origin.1);
        if previous.len() != width * height
            || dx.unsigned_abs() >= width as u64
            || dy.unsigned_abs() >= height as u64
        {
            return self.crop(origin, size);
        }
        let (dx, dy) = (dx as isize, dy as isize);
        let (w, h) = (width as isize, height as isize);
        // New columns x0..x1 and rows y0..y1 were inside the previous crop.
        let (x0, x1) = ((-dx).max(0) as usize, (w - dx).min(w) as usize);
        let (y0, y1) = ((-dy).max(0) as usize, (h - dy).min(h) as usize);
        let mut out = vec![0u32; width * height];
        for y in y0..y1 {
            let source = (y as isize + dy) as usize * width + (x0 as isize + dx) as usize;
            out[y * width + x0..y * width + x1]
                .copy_from_slice(&previous[source..source + (x1 - x0)]);
        }
        let mut fill = |left: usize, top: usize, columns: usize, rows: usize| {
            if columns == 0 || rows == 0 {
                return;
            }
            let block = self.crop(
                (origin.0 + left as i32, origin.1 + top as i32),
                (columns as u32, rows as u32),
            );
            for (row, words) in block.chunks_exact(columns).enumerate() {
                let start = (top + row) * width + left;
                out[start..start + columns].copy_from_slice(words);
            }
        };
        fill(0, 0, width, y0);
        fill(0, y1, width, height - y1);
        fill(0, y0, x0, y1 - y0);
        fill(x1, y0, width - x1, y1 - y0);
        out
    }
}

/// Magic and format version of a cached wallpaper canvas file.
const CANVAS_CACHE_MAGIC: &[u8; 8] = b"FWBCNV01";
/// Cached canvases kept; older ones are removed when a new one is written.
const CANVAS_CACHE_FILES: usize = 6;

/// The desktop layer every translucent surface blends over: the wallpaper under the whole
/// `root`, laid out per output like the desktop draws it (`outputs` in root coordinates; empty is
/// one wallpaper over the root), blurred once per [`BlurredCanvas::new`].
///
/// The first FlameWM process to need it decodes, composes and blurs the wallpaper and stores the
/// small canvas (about half a megabyte at 1080p) under `$XDG_CACHE_HOME/flamewm/backdrops`; every
/// later surface, process and session loads that file instead of decoding the wallpaper again.
/// The key covers the wallpaper file (path, size, modification time), the fit, the output layout,
/// the root size and the blur, so a changed wallpaper or setting never reads a stale canvas. The
/// decoded wallpaper is dropped as soon as the canvas exists.
pub fn desktop_canvas(
    wallpaper: &std::path::Path,
    fit: flamewm_image_core::wallpaper::WallpaperFit,
    outputs: &[(i32, i32, u32, u32)],
    root: (u32, u32),
    blur_percent: u8,
) -> Result<BlurredCanvas, String> {
    let outputs = if outputs == [(0, 0, root.0, root.1)] {
        &[][..]
    } else {
        outputs
    };
    let cache = canvas_cache_path(wallpaper, fit, outputs, root, blur_percent);
    if let Some(canvas) = cache.as_deref().and_then(|file| load_canvas(file, root)) {
        return Ok(canvas);
    }
    let natural = flamewm_image_core::wallpaper::decode_file(wallpaper)?;
    let composed = flamewm_image_core::wallpaper::compose_outputs_region(
        &natural,
        root,
        outputs,
        fit,
        (0, 0, root.0, root.1),
    )
    .ok_or("desktop canvas: wallpaper region unavailable")?;
    drop(natural);
    let canvas = BlurredCanvas::new(root.0, root.1, composed.pixels, 0, blur_percent);
    if let (Some(file), Some(small)) = (cache.as_deref(), canvas.scaled_bytes()) {
        store_canvas(file, root, small);
    }
    Ok(canvas)
}

/// Root property on which the desktop surface publishes its presented frame for the WM:
/// `CARDINAL[6]` = backbuffer pixmap, window root `x`, `y` (two's complement), `width`, `height`,
/// present serial. The pixmap holds what the desktop shows, so the WM shades frame shadows from it
/// without reading the root. It is rewritten after every present: the serial change tells the WM
/// that a read racing that repaint must be redone. Deleted when the desktop surface goes away.
pub const DESKTOP_FRAME_PROPERTY: &str = "_FLAMEWM_DESKTOP_FRAME";

/// Root property on which the WM lists the frames it redirects automatically through Composite
/// (`WINDOW[]`, bottom to top). Their contents stay valid off screen even where other windows
/// cover them, so translucent app windows compose what lies behind them from these pixmaps.
/// Absent when the WM does not redirect (no Composite, or a compositing manager runs).
pub const REDIRECTED_FRAMES_PROPERTY: &str = "_FLAMEWM_REDIRECTED_FRAMES";

/// A desktop frame read back from [`DESKTOP_FRAME_PROPERTY`]: the pixmap and its window's root
/// origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopFrame {
    pub(crate) pixmap: Pixmap,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// Parses the [`DESKTOP_FRAME_PROPERTY`] words; `None` when malformed.
fn desktop_frame_from_words(words: &[u32]) -> Option<DesktopFrame> {
    let [pixmap, x, y, width, height, _serial] = *words else {
        return None;
    };
    (pixmap != 0 && width > 0 && height > 0).then_some(DesktopFrame {
        pixmap: Pixmap::from(pixmap),
        x: x as i32,
        y: y as i32,
    })
}

/// The desktop frame currently published on the root of `display`; `None` without a desktop.
///
/// # Safety
///
/// `display` must be live.
pub(crate) unsafe fn published_desktop_frame(display: *mut Display) -> Option<DesktopFrame> {
    // SAFETY: live display per the contract.
    desktop_frame_from_words(&unsafe { root_cardinals(display, DESKTOP_FRAME_PROPERTY, 6) })
}

/// Root property on which the WM publishes the wallpaper layout the desktop draws:
/// `CARDINAL[4n]` = `x`, `y` (two's complement), `width`, `height` per output in root coordinates.
/// Empty or absent means one wallpaper over the whole root (a single output or `shareWallpaper`).
pub const WALLPAPER_OUTPUTS_PROPERTY: &str = "_FLAMEWM_WALLPAPER_OUTPUTS";

/// Most outputs read from [`WALLPAPER_OUTPUTS_PROPERTY`].
const WALLPAPER_OUTPUTS_MAX: usize = 16;

/// Parses [`WALLPAPER_OUTPUTS_PROPERTY`] words; empty rects and a trailing partial rect are
/// dropped.
fn wallpaper_outputs_from_words(words: &[u32]) -> Vec<(i32, i32, u32, u32)> {
    words
        .chunks_exact(4)
        .filter(|rect| rect[2] > 0 && rect[3] > 0)
        .map(|rect| (rect[0] as i32, rect[1] as i32, rect[2], rect[3]))
        .collect()
}

/// The wallpaper layout the WM published on the root of `display`; empty means one wallpaper
/// over the root.
///
/// # Safety
///
/// `display` must be live.
pub(crate) unsafe fn published_wallpaper_outputs(
    display: *mut Display,
) -> Vec<(i32, i32, u32, u32)> {
    // SAFETY: live display per the contract.
    wallpaper_outputs_from_words(&unsafe {
        root_cardinals(
            display,
            WALLPAPER_OUTPUTS_PROPERTY,
            WALLPAPER_OUTPUTS_MAX * 4,
        )
    })
}

/// Up to `max_words` format-32 `CARDINAL` words of the root property `name`; empty when absent.
///
/// # Safety
///
/// `display` must be live.
unsafe fn root_cardinals(display: *mut Display, name: &str, max_words: usize) -> Vec<u32> {
    let Ok(name) = std::ffi::CString::new(name) else {
        return Vec::new();
    };
    // SAFETY: live display per the contract; `only_if_exists` keeps the lookup from creating it.
    let atom = unsafe { XInternAtom(display, name.as_ptr(), 1) };
    if atom == 0 {
        return Vec::new();
    }
    let root = unsafe { XRootWindow(display, XDefaultScreen(display)) };
    let (mut actual_type, mut actual_format, mut items, mut after) = (0, 0, 0, 0);
    let mut data: *mut u8 = std::ptr::null_mut();
    // SAFETY: all out-pointers reference live locals; Xlib allocates `data`, freed below.
    let status = unsafe {
        crate::xlib_ext::XGetWindowProperty(
            display,
            root,
            atom,
            0,
            max_words as _,
            0,
            XA_CARDINAL,
            &mut actual_type,
            &mut actual_format,
            &mut items,
            &mut after,
            &mut data,
        )
    };
    if status != 0 || data.is_null() {
        return Vec::new();
    }
    let words: Vec<u32> = if actual_format == 32 {
        // SAFETY: format-32 properties come back as `items` C longs at `data`.
        unsafe {
            std::slice::from_raw_parts(data.cast::<std::os::raw::c_long>(), items as usize)
                .iter()
                .map(|&word| word as u32)
                .collect()
        }
    } else {
        Vec::new()
    };
    unsafe { XFree(data.cast()) };
    words
}

/// What the desktop layer is made of, as the desktop draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopSource {
    Solid([u8; 3]),
    Image {
        path: std::path::PathBuf,
        fit: flamewm_image_core::wallpaper::WallpaperFit,
    },
}

/// Flat colour of the layer when the wallpaper cannot be decoded (the desktop's solid default).
const DESKTOP_FALLBACK_RGB: [u8; 3] = [0x23, 0x26, 0x29];

/// One process's handle on the desktop layer that popups and panels blend over. Blurs that the
/// canvas stores downscaled share [`desktop_canvas`] (one small canvas, cached on disk); smaller
/// blurs and unblurred crops are composed per request from the decoded wallpaper and never kept,
/// so no root-sized full-resolution copy stays resident.
#[derive(Debug)]
pub struct DesktopLayer {
    source: DesktopSource,
    canvas: Option<(CanvasLayout, Option<BlurredCanvas>)>,
}

/// Root size, output layout and blur a [`DesktopLayer`] canvas was built for.
type CanvasLayout = ((u32, u32), Vec<(i32, i32, u32, u32)>, u8);

impl DesktopLayer {
    #[must_use]
    pub fn new(source: DesktopSource) -> Self {
        Self {
            source,
            canvas: None,
        }
    }

    #[must_use]
    pub fn source(&self) -> &DesktopSource {
        &self.source
    }

    /// Opaque ARGB32 words of the layer under the root-space `rect`, blurred as a surface at
    /// `opacity_percent` and `blur_percent` would be, with the wallpaper laid out over `outputs`
    /// like the desktop draws it. A solid or undecodable wallpaper is flat.
    pub fn crop(
        &mut self,
        root: (u32, u32),
        outputs: &[(i32, i32, u32, u32)],
        rect: (i32, i32, u32, u32),
        opacity_percent: u8,
        blur_percent: u8,
    ) -> Vec<u32> {
        let (x, y, width, height) = rect;
        let flat = |rgb: [u8; 3]| {
            let word =
                0xff00_0000 | u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
            vec![word; width as usize * height as usize]
        };
        let (path, fit) = match &self.source {
            DesktopSource::Solid(rgb) => return flat(*rgb),
            DesktopSource::Image { path, fit } => (path, *fit),
        };
        let radius = blur_radius_px(opacity_percent, blur_percent);
        if radius < 2.0 * BLUR_DOWNSCALE as f32 {
            return flamewm_image_core::wallpaper::decode_file(path)
                .ok()
                .and_then(|natural| {
                    flamewm_image_core::wallpaper::compose_outputs_region(
                        &natural, root, outputs, fit, rect,
                    )
                })
                .map_or_else(
                    || flat(DESKTOP_FALLBACK_RGB),
                    |region| {
                        blurred_argb_words(
                            width,
                            height,
                            region.pixels,
                            opacity_percent,
                            blur_percent,
                        )
                    },
                );
        }
        if self
            .canvas
            .as_ref()
            .is_none_or(|((size, layout, blur), _)| {
                (*size, layout.as_slice(), *blur) != (root, outputs, blur_percent)
            })
        {
            let canvas = desktop_canvas(path, fit, outputs, root, blur_percent)
                .map_err(|error| eprintln!("flamewm-render-x11: desktop layer: {error}"))
                .ok();
            self.canvas = Some(((root, outputs.to_vec(), blur_percent), canvas));
        }
        match self.canvas.as_ref().and_then(|(_, canvas)| canvas.as_ref()) {
            Some(canvas) => canvas.crop((x, y), (width, height)),
            None => flat(DESKTOP_FALLBACK_RGB),
        }
    }
}

/// [`desktop_canvas`] for a window backdrop spec, laid out over `outputs`.
pub(crate) fn wallpaper_canvas(
    spec: &WindowBackdrop,
    root: (u32, u32),
    outputs: &[(i32, i32, u32, u32)],
) -> Result<BlurredCanvas, String> {
    desktop_canvas(
        &wallpaper_file(&spec.wallpaper, &spec.packaged_root),
        flamewm_image_core::wallpaper::parse(&spec.fit),
        outputs,
        root,
        spec.blur_percent,
    )
}

/// Cache file for a desktop canvas; `None` without a cache directory or a readable wallpaper.
fn canvas_cache_path(
    wallpaper: &std::path::Path,
    fit: flamewm_image_core::wallpaper::WallpaperFit,
    outputs: &[(i32, i32, u32, u32)],
    root: (u32, u32),
    blur_percent: u8,
) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let metadata = std::fs::metadata(wallpaper).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        wallpaper,
        metadata.len(),
        modified.as_nanos(),
        format!("{fit:?}"),
        outputs,
        root,
        blur_percent,
        BLUR_DOWNSCALE,
    )
        .hash(&mut hasher);
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| std::path::PathBuf::from(home).join(".cache"))
        })?;
    Some(
        base.join("flamewm/backdrops")
            .join(format!("{:016x}.canvas", hasher.finish())),
    )
}

fn load_canvas(file: &std::path::Path, root: (u32, u32)) -> Option<BlurredCanvas> {
    let bytes = std::fs::read(file).ok()?;
    let header = CANVAS_CACHE_MAGIC.len() + 8;
    if bytes.len() < header || &bytes[..CANVAS_CACHE_MAGIC.len()] != CANVAS_CACHE_MAGIC {
        return None;
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    let size = (
        word(CANVAS_CACHE_MAGIC.len()),
        word(CANVAS_CACHE_MAGIC.len() + 4),
    );
    if size != root {
        return None;
    }
    BlurredCanvas::from_scaled(root.0, root.1, bytes[header..].to_vec())
}

/// Writes the canvas atomically (temporary file, then rename) and keeps only the newest
/// [`CANVAS_CACHE_FILES`] canvases. Any failure only costs the next window a decode.
fn store_canvas(file: &std::path::Path, root: (u32, u32), small: &[u8]) {
    let Some(dir) = file.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut bytes = Vec::with_capacity(CANVAS_CACHE_MAGIC.len() + 8 + small.len());
    bytes.extend_from_slice(CANVAS_CACHE_MAGIC);
    bytes.extend_from_slice(&root.0.to_le_bytes());
    bytes.extend_from_slice(&root.1.to_le_bytes());
    bytes.extend_from_slice(small);
    let temporary = file.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&temporary, &bytes).is_err() || std::fs::rename(&temporary, file).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut canvases: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "canvas"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    canvases.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, stale) in canvases.into_iter().skip(CANVAS_CACHE_FILES) {
        let _ = std::fs::remove_file(stale);
    }
}

/// Window-level pseudo-transparency for a Normal window (for example the explorer): the
/// wallpaper under the whole window, blurred per `blur_percent`, is laid under the window's
/// translucent fills. Wallpaper pixels come from the shared wallpaper owner, never a root read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowBackdrop {
    /// `wallpaper` setting. Empty selects the packaged default wallpaper.
    pub wallpaper: String,
    /// `wallpaperFit` setting text, parsed by `flamewm_image_core::wallpaper::parse`.
    pub fit: String,
    /// Packaged asset root that the default wallpaper resolves against.
    pub packaged_root: std::path::PathBuf,
    /// Surface opacity percent. 100 is opaque and installs no backdrop.
    pub opacity_percent: u8,
    /// Blur percent applied to the snapshot (`blur_radius_px` policy).
    pub blur_percent: u8,
}

impl WindowBackdrop {
    /// Whether translucent fills need a backdrop at all.
    #[must_use]
    pub fn is_translucent(&self) -> bool {
        self.opacity_percent < 100
    }
}

/// Wallpaper file for a `wallpaper` setting value (empty means the packaged default).
#[must_use]
pub fn wallpaper_file(wallpaper: &str, packaged_root: &std::path::Path) -> std::path::PathBuf {
    if wallpaper.trim().is_empty() {
        packaged_root.join(flamewm_image_core::wallpaper::DEFAULT_WALLPAPER_PATH)
    } else {
        std::path::PathBuf::from(wallpaper)
    }
}

/// Whether the window snapshot must be recaptured now.
/// A move keeps the window size, so it recaptures at once. A size change (interactive resize)
/// keeps the stale snapshot out of the scene (paint skips a size mismatch) and waits until no
/// X events are queued, so only the settled size pays for a capture.
#[must_use]
pub fn recapture_due(
    snapshot_size: Option<(u32, u32)>,
    window_size: (u32, u32),
    events_queued: bool,
) -> bool {
    match snapshot_size {
        None => true,
        Some(size) if size == window_size => true,
        Some(_) => !events_queued,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recrop_matches_a_fresh_crop_for_every_shift() {
        let (width, height) = (96, 64);
        let rgba: Vec<u8> = (0..width * height * 4)
            .map(|i| ((i * 37) ^ (i >> 5)) as u8)
            .collect();
        for blur in [0, 100] {
            let canvas = BlurredCanvas::new(width, height, rgba.clone(), 40, blur);
            let size = (31, 23);
            let previous_origin = (10, -4);
            let previous = canvas.crop(previous_origin, size);
            for dx in [-40, -31, -7, -1, 0, 1, 5, 30, 31, 80] {
                for dy in [-30, -23, -3, 0, 2, 22, 23, 70] {
                    let origin = (previous_origin.0 + dx, previous_origin.1 + dy);
                    assert_eq!(
                        canvas.recrop(&previous, previous_origin, origin, size),
                        canvas.crop(origin, size),
                        "blur={blur} shift=({dx}, {dy})"
                    );
                }
            }
        }
    }

    #[test]
    fn wallpaper_output_words_parse_signed_rects_and_drop_empty_or_partial() {
        let left = (-1280_i32) as u32;
        assert_eq!(
            wallpaper_outputs_from_words(&[left, 0, 1280, 800, 0, 0, 1920, 1080, 5, 5, 0, 9, 1]),
            vec![(-1280, 0, 1280, 800), (0, 0, 1920, 1080)]
        );
        assert!(wallpaper_outputs_from_words(&[]).is_empty());
    }

    #[test]
    fn desktop_frame_words_parse_and_reject_malformed() {
        assert_eq!(
            desktop_frame_from_words(&[7, 0, (-3i32) as u32, 1280, 800, 9]),
            Some(DesktopFrame {
                pixmap: 7,
                x: 0,
                y: -3,
            })
        );
        assert_eq!(desktop_frame_from_words(&[0, 0, 0, 1280, 800, 9]), None);
        assert_eq!(desktop_frame_from_words(&[7, 0, 0, 1280, 800]), None);
    }

    #[test]
    fn full_opacity_and_zero_blur_never_blur() {
        assert_eq!(blur_radius_px(100, 80), 0.0);
        assert_eq!(blur_radius_px(80, 0), 0.0);
        assert!((blur_radius_px(80, 100) - 24.0).abs() < f32::EPSILON);
        assert!((blur_radius_px(80, 250) - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn unblurred_words_are_opaque_argb_in_rgb_order() {
        let words = blurred_argb_words(
            2,
            1,
            vec![0x11, 0x22, 0x33, 0x00, 0x11, 0x22, 0x33, 0x00],
            100,
            80,
        );
        assert_eq!(words, vec![0xff11_2233, 0xff11_2233]);
    }

    #[test]
    fn blur_of_uniform_snapshot_keeps_colour() {
        let words = blurred_argb_words(8, 8, [0x40, 0x80, 0xc0, 0xff].repeat(64), 40, 100);
        assert!(words.iter().all(|&w| w == 0xff40_80c0));
    }

    #[test]
    fn downscaled_blur_keeps_output_size_and_softens_an_edge() {
        // 66 is not a multiple of the downscale factor, so the last blocks are partial.
        let (width, height) = (66u32, 3u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let v = if i % width < 33 { 0u8 } else { 255 };
                [v, v, v, 0xff]
            })
            .collect();
        let words = blurred_argb_words(width, height, rgba, 40, 100);
        assert_eq!(words.len(), (width * height) as usize);
        let channel = |x: usize| (words[x] & 0xff) as u8;
        assert!(
            channel(0) < channel(16) && channel(16) < channel(33),
            "dark side ramps up"
        );
        assert!(
            channel(33) < channel(50) && channel(50) < channel(65),
            "light side ramps up"
        );
        assert!(
            (64..192).contains(&channel(33)),
            "edge is soft: {}",
            channel(33)
        );
    }

    /// A scaled canvas never stores the full image, yet every crop (inside, straddling or off the
    /// canvas) equals cropping the full upsampled image that `blurred_argb_words` returns.
    #[test]
    fn scaled_canvas_crops_match_the_full_upsampled_image() {
        let (width, height) = (61u32, 37u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    ((x * y) % 251) as u8,
                    0xff,
                ]
            })
            .collect();
        let full = blurred_argb_words(width, height, rgba.clone(), 0, 80);
        let canvas = BlurredCanvas::new(width, height, rgba, 0, 80);
        assert!(
            canvas.scaled_bytes().is_some(),
            "80% blur keeps only the small image"
        );
        let reference = |origin: (i32, i32), size: (u32, u32)| -> Vec<u32> {
            let mut out = Vec::new();
            for y in 0..size.1 as i32 {
                let sy = (origin.1 + y).clamp(0, height as i32 - 1) as u32;
                for x in 0..size.0 as i32 {
                    let sx = (origin.0 + x).clamp(0, width as i32 - 1) as u32;
                    out.push(full[(sy * width + sx) as usize]);
                }
            }
            out
        };
        for &(origin, size) in &[
            ((0, 0), (61, 37)),
            ((7, 3), (20, 11)),
            ((-5, -2), (12, 9)),
            ((55, 30), (14, 14)),
        ] {
            assert_eq!(
                canvas.crop(origin, size),
                reference(origin, size),
                "origin {origin:?} size {size:?}"
            );
        }
        let stored = canvas.scaled_bytes().expect("scaled").to_vec();
        let reloaded = BlurredCanvas::from_scaled(width, height, stored).expect("same dimensions");
        assert_eq!(
            reloaded.crop((3, 4), (10, 10)),
            canvas.crop((3, 4), (10, 10))
        );
        assert!(BlurredCanvas::from_scaled(width + 9, height, vec![0; 16]).is_none());
    }

    #[test]
    fn a_cached_canvas_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("flamewm-canvas-{}", std::process::id()));
        let file = dir.join("one.canvas");
        let canvas = BlurredCanvas::new(40, 24, vec![90; 40 * 24 * 4], 0, 80);
        store_canvas(&file, (40, 24), canvas.scaled_bytes().expect("scaled"));
        let loaded = load_canvas(&file, (40, 24)).expect("cached canvas");
        assert_eq!(loaded.crop((0, 0), (40, 24)), canvas.crop((0, 0), (40, 24)));
        assert!(
            load_canvas(&file, (41, 24)).is_none(),
            "another root size is a miss"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn downscaled_blur_tracks_full_resolution_blur() {
        let (width, height) = (96u32, 64u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [(x * 255 / width) as u8, (y * 255 / height) as u8, 96, 0xff]
            })
            .collect();
        let mut full = rgba.clone();
        flamewm_image_core::blur::gaussian_blur_rgba(&mut full, width, height, 24.0).unwrap();
        let fast = blurred_argb_words(width, height, rgba, 40, 100);
        let total: u32 = full
            .chunks_exact(4)
            .zip(&fast)
            .map(|(p, w)| {
                let got = [(w >> 16) & 0xff, (w >> 8) & 0xff, w & 0xff];
                (0..3)
                    .map(|c| (i32::from(p[c]) - got[c] as i32).unsigned_abs())
                    .sum::<u32>()
            })
            .sum();
        let mean = total as f32 / (width * height * 3) as f32;
        assert!(mean < 3.0, "mean channel error {mean}");
    }

    #[test]
    fn in_place_blur_matches_copying_reference_on_random_pixels() {
        // Reference: the pre-move path (copy, blur the copy, then convert).
        fn reference(width: u32, height: u32, rgba: &[u8], radius: f32) -> Vec<u32> {
            let mut pixels = rgba.to_vec();
            if radius > 0.0 {
                flamewm_image_core::blur::gaussian_blur_rgba(&mut pixels, width, height, radius)
                    .unwrap();
            }
            pixels
                .chunks_exact(4)
                .map(|p| {
                    0xff00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2])
                })
                .collect()
        }
        let mut seed: u32 = 0x1234_5678;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) as u8
        };
        for &(width, height) in &[(1u32, 1u32), (7, 5), (31, 17)] {
            let len = (width * height * 4) as usize;
            let rgba: Vec<u8> = (0..len).map(|_| next()).collect();
            // Radii below twice BLUR_DOWNSCALE keep the full-resolution path this reference models.
            for &(opacity, blur) in &[(100u8, 80u8), (80, 0), (95, 25), (60, 33)] {
                let radius = blur_radius_px(opacity, blur);
                let expected = reference(width, height, &rgba, radius);
                let actual = blurred_argb_words(width, height, rgba.clone(), opacity, blur);
                assert_eq!(
                    actual, expected,
                    "{width}x{height} opacity={opacity} blur={blur}"
                );
            }
        }
    }

    #[test]
    fn pixmap_zero_masks_fall_back_to_truecolor_layout() {
        assert_eq!(channel_masks(0, 0, 0), (0xff_0000, 0xff_00, 0xff));
        // A window image keeps the masks its visual reports, even in BGR order.
        assert_eq!(
            channel_masks(0xff, 0xff00, 0xff_0000),
            (0xff, 0xff00, 0xff_0000)
        );
    }

    #[test]
    fn pixmap_decode_with_fallback_masks_is_rgb_and_ignores_alpha_byte() {
        let (r, g, b) = channel_masks(0, 0, 0);
        // Server word 0xAA112233: alpha 0xAA in the top byte must not leak into any channel.
        assert_eq!(decode_pixel(0xAA_11_22_33, r, g, b), (0x11, 0x22, 0x33));
    }

    #[test]
    fn decode_uses_masks_not_hardcoded_order() {
        // BGR-ordered server: red in low byte, blue in high byte.
        assert_eq!(
            decode_pixel(0x00_11_22_33, 0x0000ff, 0x00ff00, 0xff0000),
            (0x33, 0x22, 0x11)
        );
        // Standard RGB order.
        assert_eq!(
            decode_pixel(0x00_11_22_33, 0xff0000, 0x00ff00, 0x0000ff),
            (0x11, 0x22, 0x33)
        );
        // 16-bit 565.
        assert_eq!(decode_pixel(0xf800, 0xf800, 0x07e0, 0x001f).0, 255);
        assert_eq!(decode_pixel(0x001f, 0xf800, 0x07e0, 0x001f).2, 255);
    }

    #[test]
    fn fill_default_matches_twenty_percent_contract() {
        assert_eq!(fill_color(20), (0xef, 0x40, 0x48, 51));
        assert_eq!(BACKDROP_BORDER_ALPHA, 0xdd);
        assert_eq!(BACKDROP_BORDER_WIDTH_PX, 2);
        assert_eq!(
            selection_material(20),
            ((0xef, 0x40, 0x48, 51), (0xef, 0x40, 0x48, 0xdd))
        );
    }

    #[test]
    fn capture_rejects_empty_rect_without_x() {
        assert!(capture_drawable_rect(std::ptr::null_mut(), 0, (0, 0, 0, 10)).is_err());
        assert!(capture_drawable_rect(std::ptr::null_mut(), 0, (0, 0, 10, 10)).is_err());
    }
}

#[cfg(test)]
mod window_backdrop_tests {
    use super::*;

    #[test]
    fn empty_wallpaper_setting_selects_packaged_default() {
        let root = std::path::Path::new("/pkg");
        assert_eq!(
            wallpaper_file("  ", root),
            root.join(flamewm_image_core::wallpaper::DEFAULT_WALLPAPER_PATH)
        );
        assert_eq!(
            wallpaper_file("/home/u/bg.png", root),
            std::path::PathBuf::from("/home/u/bg.png")
        );
    }

    #[test]
    fn only_translucent_opacity_needs_a_backdrop() {
        let mut spec = WindowBackdrop {
            wallpaper: String::new(),
            fit: String::new(),
            packaged_root: std::path::PathBuf::new(),
            opacity_percent: 100,
            blur_percent: 40,
        };
        assert!(!spec.is_translucent());
        spec.opacity_percent = 99;
        assert!(spec.is_translucent());
    }

    #[test]
    fn recapture_policy_moves_immediately_and_freezes_resize_until_settled() {
        assert!(recapture_due(None, (800, 600), true));
        assert!(
            recapture_due(Some((800, 600)), (800, 600), true),
            "move recaptures"
        );
        assert!(
            !recapture_due(Some((800, 600)), (900, 600), true),
            "resize frozen while queued"
        );
        assert!(
            recapture_due(Some((800, 600)), (900, 600), false),
            "settled resize captures"
        );
    }

    #[test]
    fn blurred_window_snapshot_is_opaque_argb() {
        let rgba = [0x40, 0x80, 0xc0, 0xff].repeat(4 * 3);
        let words = blurred_argb_words(4, 3, rgba, 0, 50);
        assert_eq!(words.len(), 12);
        assert!(words.iter().all(|w| w >> 24 == 0xff));
    }
}

/// Root-space rect the wallpaper is actually composed from: the intersection of `rect` with
/// the root bounds, or a 1x1 anchor at the nearest root pixel when `rect` lies fully outside.
/// The result is always non-empty and inside the root, so the compose never misses.
#[must_use]
pub fn root_anchor_rect(rect: (i32, i32, u32, u32), root: (u32, u32)) -> (i32, i32, u32, u32) {
    let (rx, ry, w, h) = (
        i64::from(rect.0),
        i64::from(rect.1),
        i64::from(rect.2),
        i64::from(rect.3),
    );
    let (root_w, root_h) = (i64::from(root.0.max(1)), i64::from(root.1.max(1)));
    let (x0, y0) = (rx.max(0), ry.max(0));
    let (x1, y1) = ((rx + w).min(root_w), (ry + h).min(root_h));
    if x0 < x1 && y0 < y1 {
        return (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32);
    }
    let (ax, ay) = (rx.clamp(0, root_w - 1), ry.clamp(0, root_h - 1));
    (ax as i32, ay as i32, 1, 1)
}

/// Fills a `win_w`x`win_h` RGBA buffer from `src` (`src_w`x`src_h`). Window pixel `(i, j)` reads
/// source pixel `(i + shift.0, j + shift.1)`, clamped to the source edge, so out-of-source
/// regions replicate the nearest wallpaper edge and never come out as opaque black.
#[must_use]
pub fn edge_extend_rgba(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    shift: (i64, i64),
    win: (u32, u32),
) -> Vec<u8> {
    let (sw, sh) = (i64::from(src_w.max(1)), i64::from(src_h.max(1)));
    let mut out = vec![0u8; win.0 as usize * win.1 as usize * 4];
    if src.len() < (sw * sh * 4) as usize {
        return out;
    }
    for (j, row) in out.chunks_exact_mut(win.0 as usize * 4).enumerate() {
        let sy = (j as i64 + shift.1).clamp(0, sh - 1) as usize;
        let src_row = &src[sy * sw as usize * 4..(sy + 1) * sw as usize * 4];
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let sx = (i as i64 + shift.0).clamp(0, sw - 1) as usize;
            px.copy_from_slice(&src_row[sx * 4..sx * 4 + 4]);
        }
    }
    out
}

/// Source index for destination index `i` when a `n`-long source sits at `origin` in the
/// destination. Positions outside the source reflect back across the nearest edge (period `2n`),
/// so a grown strip continues the existing content with no seam and no black.
#[must_use]
pub fn mirror_index(i: i64, origin: i64, n: i64) -> usize {
    if n <= 1 {
        return 0;
    }
    let u = (i - origin).rem_euclid(2 * n);
    let idx = if u < n { u } else { 2 * n - 1 - u };
    idx as usize
}

/// Mirror-extends a `src_w`x`src_h` ARGB snapshot over a `dst_w`x`dst_h` canvas. The snapshot's
/// top-left lands at `origin`; every other pixel is the reflected snapshot, so growing the window
/// right/bottom (origin 0) or left/top (origin = the snapshot's offset) keeps the join seamless.
#[must_use]
pub fn mirror_extend_argb(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: (u32, u32),
    origin: (i64, i64),
) -> Vec<u32> {
    let (sw, sh) = (i64::from(src_w), i64::from(src_h));
    let mut out = vec![0u32; dst.0 as usize * dst.1 as usize];
    if sw == 0 || sh == 0 || src.len() < (sw * sh) as usize {
        return out;
    }
    for (j, row) in out.chunks_exact_mut(dst.0 as usize).enumerate() {
        let sy = mirror_index(j as i64, origin.1, sh);
        let src_row = &src[sy * sw as usize..(sy + 1) * sw as usize];
        for (i, px) in row.iter_mut().enumerate() {
            *px = src_row[mirror_index(i as i64, origin.0, sw)];
        }
    }
    out
}

#[cfg(test)]
mod mirror_extend_tests {
    use super::*;

    /// 3x2 source: row 0 = 1,2,3 ; row 1 = 4,5,6.
    const SRC: [u32; 6] = [1, 2, 3, 4, 5, 6];

    #[test]
    fn grow_right_mirrors_the_right_edge_outward() {
        // 3x2 -> 5x2, origin 0: column 3 reflects column 2, column 4 reflects column 1.
        let out = mirror_extend_argb(&SRC, 3, 2, (5, 2), (0, 0));
        assert_eq!(out, vec![1, 2, 3, 3, 2, 4, 5, 6, 6, 5]);
    }

    #[test]
    fn grow_bottom_mirrors_the_bottom_edge_outward() {
        // 3x2 -> 3x4: row 2 repeats row 1, row 3 reflects row 0.
        let out = mirror_extend_argb(&SRC, 3, 2, (3, 4), (0, 0));
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn grow_left_places_source_at_offset_and_mirrors_leading_edge() {
        // Window grew 2px to the left: the old content starts at x=2 of a 5-wide canvas.
        let out = mirror_extend_argb(&SRC, 3, 2, (5, 2), (2, 0));
        assert_eq!(out, vec![2, 1, 1, 2, 3, 5, 4, 4, 5, 6]);
    }

    #[test]
    fn source_inside_destination_is_copied_unchanged() {
        let out = mirror_extend_argb(&SRC, 3, 2, (3, 2), (0, 0));
        assert_eq!(out, SRC.to_vec());
    }

    #[test]
    fn no_destination_pixel_is_left_zero_when_source_is_present() {
        let out = mirror_extend_argb(&SRC, 3, 2, (9, 7), (1, 3));
        assert!(out.iter().all(|px| *px != 0));
    }

    #[test]
    fn short_or_empty_source_yields_zeroed_canvas_without_panic() {
        assert_eq!(
            mirror_extend_argb(&[1, 2], 3, 2, (2, 2), (0, 0)),
            vec![0; 4]
        );
        assert_eq!(mirror_extend_argb(&[], 0, 0, (2, 2), (0, 0)), vec![0; 4]);
    }
}

#[cfg(test)]
mod edge_extend_tests {
    use super::*;

    #[test]
    fn anchor_is_intersection_for_partially_outside_rect() {
        assert_eq!(
            root_anchor_rect((-100, 50, 300, 200), (1350, 641)),
            (0, 50, 200, 200)
        );
        assert_eq!(
            root_anchor_rect((1200, 600, 300, 200), (1350, 641)),
            (1200, 600, 150, 41)
        );
    }

    #[test]
    fn anchor_is_full_rect_when_inside_root() {
        assert_eq!(
            root_anchor_rect((10, 20, 30, 40), (1350, 641)),
            (10, 20, 30, 40)
        );
    }

    #[test]
    fn anchor_clamps_fully_outside_rect_to_nearest_root_pixel() {
        assert_eq!(
            root_anchor_rect((-500, 10, 100, 100), (1350, 641)),
            (0, 10, 1, 1)
        );
        assert_eq!(
            root_anchor_rect((2000, 900, 100, 100), (1350, 641)),
            (1349, 640, 1, 1)
        );
        assert_eq!(
            root_anchor_rect((-50, -50, 10, 10), (1350, 641)),
            (0, 0, 1, 1)
        );
    }

    #[test]
    fn anchor_never_empty_for_zero_root() {
        assert_eq!(root_anchor_rect((5, 5, 10, 10), (0, 0)), (0, 0, 1, 1));
    }

    #[test]
    fn edge_extend_replicates_source_edges_outside_region() {
        // 2x2 source, pixel value = (x, y, 0, 255).
        let src: Vec<u8> = (0..4)
            .flat_map(|i: u8| [i % 2 + 10, i / 2 + 20, 0, 255])
            .collect();
        // Window 4x3 whose (0,0) is one pixel left/above the source origin.
        let out = edge_extend_rgba(&src, 2, 2, (-1, -1), (4, 3));
        let px = |x: usize, y: usize| &out[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(px(0, 0), &[10, 20, 0, 255]); // clamped to source (0,0)
        assert_eq!(px(1, 1), &[10, 20, 0, 255]); // source (0,0)
        assert_eq!(px(3, 2), &[11, 21, 0, 255]); // clamped to source (1,1)
        assert!(out.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn edge_extend_fully_outside_window_is_single_colour_not_black() {
        let src = vec![9u8, 8, 7, 255];
        let anchor = root_anchor_rect((-300, -300, 64, 64), (1350, 641));
        let shift = (-300 - i64::from(anchor.0), -300 - i64::from(anchor.1));
        let out = edge_extend_rgba(&src, 1, 1, shift, (64, 64));
        assert!(out.chunks_exact(4).all(|p| p == [9, 8, 7, 255]));
    }

    #[test]
    fn edge_extend_rejects_short_source_without_panicking() {
        let out = edge_extend_rgba(&[1, 2], 2, 2, (0, 0), (2, 2));
        assert_eq!(out.len(), 16);
    }
}
