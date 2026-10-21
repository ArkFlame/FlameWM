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

/// Shared desktop-selection material for one material, two consumers
/// (desktop selection renderer and snap-preview backdrop): translucent
/// accent fill plus strong accent border ring. Pure and headless-testable.
#[must_use]
pub fn selection_material(opacity_percent: u8) -> ((u8, u8, u8, u8), (u8, u8, u8, u8)) {
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
pub fn capture_drawable_rect(
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

/// Whether a root capture holds real content. An empty or single-colour capture (all black
/// included) means the root was not painted yet, so the caller must not install it.
#[must_use]
pub fn capture_is_ready(rgba: &[u8]) -> bool {
    let mut pixels = rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]);
    let Some(first) = pixels.next() else {
        return false;
    };
    pixels.any(|pixel| pixel != first)
}

thread_local! {
    /// Blur work buffers shared by every snapshot blur on the display thread.
    static BLUR_SCRATCH: std::cell::RefCell<flamewm_image_core::blur::BlurScratch> =
        std::cell::RefCell::default();
}

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
    if radius > 0.0 {
        let blurred = BLUR_SCRATCH.with(|scratch| {
            flamewm_image_core::blur::gaussian_blur_rgba_with(
                &mut scratch.borrow_mut(),
                &mut rgba,
                width,
                height,
                radius,
            )
        });
        if let Err(error) = blurred {
            eprintln!("flamewm-render-x11: backdrop blur skipped: {error:?}");
        }
    }
    rgba.chunks_exact(4)
        .map(|p| 0xff00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_and_black_captures_are_not_ready() {
        assert!(!capture_is_ready(&[]));
        assert!(!capture_is_ready(&[0, 0, 0, 255].repeat(16)));
        assert!(!capture_is_ready(&[0x40, 0x80, 0xc0, 255].repeat(16)));
    }

    #[test]
    fn one_differing_pixel_makes_capture_ready() {
        let mut pixels = [0x40, 0x80, 0xc0, 255].repeat(16);
        pixels[4 * 7] = 0x41;
        assert!(capture_is_ready(&pixels));
        assert!(capture_is_ready(&[0, 0, 0, 255, 0, 0, 1, 255]));
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
            for &(opacity, blur) in &[(100u8, 80u8), (80, 0), (40, 100), (60, 33)] {
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
