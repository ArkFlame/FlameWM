use super::*;
use flamewm_api::repeat_log::RepeatLog;
use std::sync::{Mutex, PoisonError};

/// Last capture diagnostic printed; an unchanged reason stays silent until it changes.
static CAPTURE_LOG: Mutex<RepeatLog> = Mutex::new(RepeatLog::new());

fn capture_log(reason: &str) {
    if CAPTURE_LOG
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .should_emit(reason)
    {
        eprintln!("render.overlay.capture reason={reason}");
    }
}

/// Straight RGBA8 capture plus the server format it was decoded from.
#[derive(Debug, Clone)]
pub struct BackdropCapture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// XGetImage of any window or pixmap drawable (`rect` is drawable-local),
/// decoded to straight RGBA8 with the live server masks.
pub(crate) fn capture_drawable_rect(
    display: *mut Display,
    drawable: u64,
    rect: (i32, i32, u32, u32),
) -> Result<BackdropCapture, String> {
    use std::os::raw::{c_int, c_uint, c_ulong};
    let (x, y, width, height) = rect;
    if width == 0 || height == 0 {
        return Err("backdrop capture: empty rect".to_string());
    }
    if display.is_null() || drawable == 0 {
        return Err("backdrop capture: no display/root".to_string());
    }
    // XGetImage answers NULL with a BadMatch for any rect that leaves the drawable; reject that
    // here so the caller falls back quietly instead of raising an X error.
    let (mut root, mut gx, mut gy) = (0 as c_ulong, 0 as c_int, 0 as c_int);
    let (mut dw, mut dh, mut border, mut depth) =
        (0 as c_uint, 0 as c_uint, 0 as c_uint, 0 as c_uint);
    let known = unsafe {
        crate::xlib_ext::XGetGeometry(
            display,
            drawable as c_ulong,
            &mut root,
            &mut gx,
            &mut gy,
            &mut dw,
            &mut dh,
            &mut border,
            &mut depth,
        )
    } != 0;
    if known
        && (x < 0
            || y < 0
            || i64::from(x) + i64::from(width) > i64::from(dw)
            || i64::from(y) + i64::from(height) > i64::from(dh))
    {
        return Err(format!(
            "backdrop capture rect {rect:?} outside drawable {dw}x{dh}"
        ));
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
        capture_log("xgetimage-null");
        return Err("backdrop capture unsupported (XGetImage returned NULL)".to_string());
    }
    let head = unsafe { &*(image as *const XImageHead) };
    if head.depth != 24 && head.depth != 32 {
        unsafe { XDestroyImage(image) };
        capture_log(&format!("unsupported-depth depth={}", head.depth));
        return Err(format!("backdrop capture unsupported depth {}", head.depth));
    }
    if head.width <= 0 || head.height <= 0 || head.data.is_null() {
        unsafe { XDestroyImage(image) };
        capture_log("invalid-image");
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
            capture_log(&format!("unsupported-bpp bpp={bits_per_pixel}"));
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
                capture_log("short-row");
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
