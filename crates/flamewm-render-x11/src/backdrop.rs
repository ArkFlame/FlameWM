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

/// Blur runs on a snapshot shrunk by this factor once the radius is large: the blur cost falls with
/// the pixel count, and the soft result is upscaled bilinearly. Below twice the factor the blur is
/// already cheap, so it stays at full resolution.
const BLUR_DOWNSCALE: u32 = 4;

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
    if radius >= 2.0 * BLUR_DOWNSCALE as f32 {
        let (mut small, small_w, small_h) = box_downscale(&rgba, width, height, BLUR_DOWNSCALE);
        blur_in_place_slice(&mut small, small_w, small_h, radius / BLUR_DOWNSCALE as f32);
        return bilinear_argb_words(&small, small_w, small_h, width, height);
    }
    if radius > 0.0 {
        blur_in_place_slice(&mut rgba, width, height, radius);
    }
    rgba.chunks_exact(4)
        .map(|p| 0xff00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
        .collect()
}

/// Gaussian blur through the shared scratch buffers. A failure keeps the input pixels.
fn blur_in_place_slice(pixels: &mut [u8], width: u32, height: u32, radius: f32) {
    let blurred = BLUR_SCRATCH.with(|scratch| {
        flamewm_image_core::blur::gaussian_blur_rgba_with(
            &mut scratch.borrow_mut(),
            pixels,
            width,
            height,
            radius,
        )
    });
    if let Err(error) = blurred {
        eprintln!("flamewm-render-x11: backdrop blur skipped: {error:?}");
    }
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

/// Bilinear upscale of a small RGBA image to opaque ARGB32 words for a `width` x `height` target.
/// Weights are 8-bit fixed point and the taps are computed once per column and per row.
fn bilinear_argb_words(small: &[u8], small_w: u32, small_h: u32, width: u32, height: u32) -> Vec<u32> {
    // (lower index, upper index, weight of the upper tap out of 256) for each output coordinate.
    let taps = |len: u32, out: u32| -> Vec<(usize, usize, u32)> {
        let scale = len as f32 / out as f32;
        (0..out)
            .map(|i| {
                let f = ((i as f32 + 0.5) * scale - 0.5).clamp(0.0, (len - 1) as f32);
                let lo = f.floor();
                let hi = (lo as u32 + 1).min(len - 1);
                (lo as usize, hi as usize, ((f - lo) * 256.0).round() as u32)
            })
            .collect()
    };
    let (cols, rows) = (taps(small_w, width), taps(small_h, height));
    let stride = small_w as usize * 4;
    let mut words = Vec::with_capacity((width * height) as usize);
    for &(r0, r1, wy) in &rows {
        let (top_row, bottom_row) = (&small[r0 * stride..], &small[r1 * stride..]);
        for &(c0, c1, wx) in &cols {
            let (a, b) = (c0 * 4, c1 * 4);
            let mut word = 0xff00_0000u32;
            for channel in 0..3usize {
                let top = u32::from(top_row[a + channel]) * (256 - wx) + u32::from(top_row[b + channel]) * wx;
                let bottom = u32::from(bottom_row[a + channel]) * (256 - wx) + u32::from(bottom_row[b + channel]) * wx;
                let value = (top * (256 - wy) + bottom * wy + (1 << 15)) >> 16;
                word |= value << (16 - 8 * channel);
            }
            words.push(word);
        }
    }
    words
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
        assert!(channel(0) < channel(16) && channel(16) < channel(33), "dark side ramps up");
        assert!(channel(33) < channel(50) && channel(50) < channel(65), "light side ramps up");
        assert!((64..192).contains(&channel(33)), "edge is soft: {}", channel(33));
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
                (0..3).map(|c| (i32::from(p[c]) - got[c] as i32).unsigned_abs()).sum::<u32>()
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
    let (rx, ry, w, h) = (i64::from(rect.0), i64::from(rect.1), i64::from(rect.2), i64::from(rect.3));
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
        assert_eq!(mirror_extend_argb(&[1, 2], 3, 2, (2, 2), (0, 0)), vec![0; 4]);
        assert_eq!(mirror_extend_argb(&[], 0, 0, (2, 2), (0, 0)), vec![0; 4]);
    }
}

#[cfg(test)]
mod edge_extend_tests {
    use super::*;

    #[test]
    fn anchor_is_intersection_for_partially_outside_rect() {
        assert_eq!(root_anchor_rect((-100, 50, 300, 200), (1350, 641)), (0, 50, 200, 200));
        assert_eq!(root_anchor_rect((1200, 600, 300, 200), (1350, 641)), (1200, 600, 150, 41));
    }

    #[test]
    fn anchor_is_full_rect_when_inside_root() {
        assert_eq!(root_anchor_rect((10, 20, 30, 40), (1350, 641)), (10, 20, 30, 40));
    }

    #[test]
    fn anchor_clamps_fully_outside_rect_to_nearest_root_pixel() {
        assert_eq!(root_anchor_rect((-500, 10, 100, 100), (1350, 641)), (0, 10, 1, 1));
        assert_eq!(root_anchor_rect((2000, 900, 100, 100), (1350, 641)), (1349, 640, 1, 1));
        assert_eq!(root_anchor_rect((-50, -50, 10, 10), (1350, 641)), (0, 0, 1, 1));
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
