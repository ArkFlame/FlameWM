//! ICO/CUR icon-container decode (A06).
//!
//! A Windows icon file is an `ICONDIR` index of one or more images. Each entry is either an
//! embedded PNG or a bottom-up Windows DIB (a BMP body without the file header) followed by a
//! 1-bit AND mask. [`decode`] walks the entries from the largest area down and returns straight
//! (non-premultiplied) RGBA8, so the explorer's bounded thumbnail pipeline can treat an `.ico`
//! like any other raster. Every read is bounds-checked: a truncated or bogus file is an error,
//! never a panic.

use crate::{RgbaImage, png};

/// Entries beyond this count are ignored: real icon files hold a handful of sizes.
pub const MAX_ENTRIES: usize = 64;
/// Largest accepted icon edge. Guards the DIB stride arithmetic before any allocation.
pub const MAX_ENTRY_EDGE: u32 = 4096;

const ICONDIR_BYTES: usize = 6;
const ICONDIRENTRY_BYTES: usize = 16;

/// One image inside an ICO container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IcoEntry {
    /// Pixel width; a stored `0` means 256.
    pub width: u32,
    /// Pixel height; a stored `0` means 256.
    pub height: u32,
    pub bits_per_pixel: u16,
    /// Byte offset of the image payload inside the container.
    pub offset: usize,
    /// Payload length in bytes.
    pub length: usize,
}

impl IcoEntry {
    fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Magic probe: `00 00 01 00` (icon) or `00 00 02 00` (cursor) followed by at least one entry.
#[must_use]
pub fn is_icon(bytes: &[u8]) -> bool {
    matches!(u16_at(bytes, 2), Some(1 | 2))
        && u16_at(bytes, 0) == Some(0)
        && u16_at(bytes, 4).is_some_and(|count| count > 0)
}

/// Index of every entry the header declares, bounded by [`MAX_ENTRIES`]. Empty when the header
/// is not an `ICONDIR`.
#[must_use]
pub fn entries(bytes: &[u8]) -> Vec<IcoEntry> {
    if !is_icon(bytes) {
        return Vec::new();
    }
    let count = u16_at(bytes, 4).unwrap_or(0) as usize;
    (0..count)
        .take(MAX_ENTRIES)
        .filter_map(|index| entry_at(bytes, ICONDIR_BYTES + index * ICONDIRENTRY_BYTES))
        .collect()
}

fn entry_at(bytes: &[u8], at: usize) -> Option<IcoEntry> {
    let edge = |stored: u8| if stored == 0 { 256 } else { u32::from(stored) };
    let width = edge(*bytes.get(at)?);
    let height = edge(*bytes.get(at + 1)?);
    let bits_per_pixel = u16_at(bytes, at + 6)?;
    let length = u32_at(bytes, at + 8)? as usize;
    let offset = u32_at(bytes, at + 12)? as usize;
    // Reject entries whose payload would run past the container: a truncated file must never be
    // sliced into a short buffer that a decoder then reads as pixels.
    if length == 0 || offset.checked_add(length)? > bytes.len() {
        return None;
    }
    Some(IcoEntry {
        width,
        height,
        bits_per_pixel,
        offset,
        length,
    })
}

/// Canvas size of the largest entry, read without decoding.
#[must_use]
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    entries(bytes)
        .into_iter()
        .max_by_key(|entry| entry.area())
        .map(|entry| (entry.width, entry.height))
}

/// Decodes the largest entry that yields pixels. Errors describe the last failure.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let mut candidates = entries(bytes);
    if candidates.is_empty() {
        return Err("not an ICO icon (no readable directory entry)".to_string());
    }
    candidates.sort_by_key(|entry| std::cmp::Reverse(entry.area()));
    let mut first_error: Option<String> = None;
    for entry in candidates {
        let Some(payload) = bytes.get(entry.offset..entry.offset + entry.length) else {
            continue;
        };
        match decode_entry(entry, payload) {
            Ok(image) => return Ok(image),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "icon holds no decodable image".to_string()))
}

fn decode_entry(entry: IcoEntry, payload: &[u8]) -> Result<RgbaImage, String> {
    if entry.width == 0
        || entry.height == 0
        || entry.width > MAX_ENTRY_EDGE
        || entry.height > MAX_ENTRY_EDGE
    {
        return Err(format!(
            "icon entry {}x{} is out of range",
            entry.width, entry.height
        ));
    }
    if payload.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return png::decode(payload);
    }
    decode_dib(payload)
}

/// Decodes one bottom-up Windows DIB entry and applies the AND mask.
fn decode_dib(payload: &[u8]) -> Result<RgbaImage, String> {
    let header_size = u32_at(payload, 0).ok_or("icon entry header is truncated")? as usize;
    if header_size < 40 || header_size > payload.len() {
        return Err("icon entry has an unsupported bitmap header".to_string());
    }
    let stored_width = i32_at(payload, 4).ok_or("icon entry width is truncated")?;
    let stored_height = i32_at(payload, 8).ok_or("icon entry height is truncated")?;
    let bits_per_pixel = u16_at(payload, 14).ok_or("icon entry depth is truncated")?;
    // BI_RGB only: RLE and bitfield entries are rare in icons and need a separate code path.
    if u32_at(payload, 16).unwrap_or(u32::MAX) != 0 {
        return Err("icon entry uses a compressed or bitfield bitmap".to_string());
    }
    if stored_width <= 0 || stored_height == 0 {
        return Err("icon entry has invalid dimensions".to_string());
    }
    // Icons store the XOR bitmap height doubled: the XOR image plus the AND mask.
    let width = stored_width as u32;
    let height = stored_height.unsigned_abs() as u32 / 2;
    if width == 0 || height == 0 || width > MAX_ENTRY_EDGE || height > MAX_ENTRY_EDGE {
        return Err(format!("icon entry {width}x{height} is out of range"));
    }
    let (width_usize, height_usize) = (width as usize, height as usize);
    let row_bytes = (width_usize * bits_per_pixel as usize).div_ceil(32) * 4;
    let mask_row_bytes = width_usize.div_ceil(32) * 4;
    let palette_len = match bits_per_pixel {
        32 | 24 => 0,
        8 | 4 | 1 => {
            let declared = u32_at(payload, 32).unwrap_or(0) as usize;
            let count = if declared == 0 {
                1usize << bits_per_pixel
            } else {
                declared
            }
            .min(256);
            // Some encoders leave `biClrUsed` at zero and store only the colours they use, so the
            // palette table is clamped to the bytes the payload can spare beside the pixels.
            // The XOR bitmap and the AND mask follow the palette, so only the bytes beyond both
            // can belong to the colour table.
            let needed = row_bytes
                .saturating_add(mask_row_bytes)
                .saturating_mul(height_usize);
            let available = payload
                .len()
                .saturating_sub(header_size)
                .saturating_sub(needed);
            (count * 4).min(available / 4 * 4)
        }
        _ => return Err(format!("icon entry depth {bits_per_pixel} is unsupported")),
    };
    let pixels_at = header_size
        .checked_add(palette_len)
        .ok_or("icon entry palette overflows")?;
    let palette = payload.get(header_size..pixels_at).unwrap_or(&[]);
    let body = payload
        .get(pixels_at..)
        .ok_or("icon entry pixel data is truncated")?;
    if row_bytes
        .checked_mul(height_usize)
        .is_none_or(|need| need > body.len())
    {
        return Err("icon entry pixel data is truncated".to_string());
    }
    let mask_start = row_bytes * height_usize;
    let mask = body
        .get(mask_start..)
        .filter(|mask| mask.len() >= mask_row_bytes * height_usize);

    let mut pixels = vec![
        0u8;
        width_usize
            .checked_mul(height_usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or("icon entry overflows memory")?
    ];
    let mut any_alpha = false;
    for row in 0..height_usize {
        // DIB rows are stored bottom-up.
        let source_row = (height_usize - 1 - row) * row_bytes;
        let target_row = row * width_usize * 4;
        for column in 0..width_usize {
            let target = target_row + column * 4;
            let (red, green, blue, alpha) = match bits_per_pixel {
                32 => {
                    let at = source_row + column * 4;
                    (body[at + 2], body[at + 1], body[at], body[at + 3])
                }
                24 => {
                    let at = source_row + column * 3;
                    (body[at + 2], body[at + 1], body[at], 255)
                }
                depth => {
                    let index = bit_index(body, source_row, column, depth).unwrap_or(0) as usize;
                    let at = (index * 4).min(palette.len().saturating_sub(4));
                    let entry = palette.get(at..at + 4).unwrap_or(&[0, 0, 0, 255]);
                    (entry[2], entry[1], entry[0], 255)
                }
            };
            any_alpha |= alpha != 0;
            pixels[target] = red;
            pixels[target + 1] = green;
            pixels[target + 2] = blue;
            pixels[target + 3] = alpha;
        }
    }
    // A 32-bit entry whose alpha channel is entirely zero (what some editors write) still carries
    // its transparency in the AND mask, exactly like the sub-32-bit entries.
    if let Some(mask) = mask.filter(|_| bits_per_pixel != 32 || !any_alpha) {
        for row in 0..height_usize {
            let source_row = (height_usize - 1 - row) * mask_row_bytes;
            for column in 0..width_usize {
                let transparent = mask[source_row + column / 8] >> (7 - column % 8) & 1 == 1;
                pixels[(row * width_usize + column) * 4 + 3] = if transparent { 0 } else { 255 };
            }
        }
    }
    RgbaImage::from_rgba8(width, height, pixels)
        .ok_or_else(|| "icon entry dimensions overflow".to_string())
}

/// Palette index of one pixel in a 1/4/8-bit row.
fn bit_index(body: &[u8], row_start: usize, column: usize, depth: u16) -> Option<u8> {
    match depth {
        8 => body.get(row_start + column).copied(),
        4 => {
            let byte = *body.get(row_start + column / 2)?;
            Some(if column % 2 == 0 {
                byte >> 4
            } else {
                byte & 0x0f
            })
        }
        1 => {
            let byte = *body.get(row_start + column / 8)?;
            Some(byte >> (7 - column % 8) & 1)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a one-entry ICO around `payload`. Width/height of `0` mean 256 in the directory.
    fn ico(payload: &[u8], width: u8, height: u8, depth: u16) -> Vec<u8> {
        let mut out = vec![0, 0, 1, 0, 1, 0];
        out.extend_from_slice(&[width, height, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&depth.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// A bottom-up `BI_RGB` DIB with an AND mask. `pixels` are tightly packed BGRA rows, top row
    /// first, and `mask` tightly packed 1-bit rows; the helper pads each row to the DIB stride.
    fn dib(
        width: u32,
        height: u32,
        depth: u16,
        palette: &[u8],
        pixels: &[u8],
        mask: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(width as i32).to_le_bytes());
        out.extend_from_slice(&((height * 2) as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&depth.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(palette);
        let (width, height, depth) = (width as usize, height as usize, depth as usize);
        let (tight_row, row) = (width * depth / 8, (width * depth).div_ceil(32) * 4);
        let (tight_mask, mask_row) = (width.div_ceil(8), width.div_ceil(32) * 4);
        // ICO rows are bottom-up, so the last stored row is the first visual row.
        for visual_row in (0..height).rev() {
            let row_bytes = pixels
                .get(visual_row * tight_row..(visual_row + 1) * tight_row)
                .unwrap_or(&[]);
            out.extend_from_slice(row_bytes);
            out.resize(out.len() + row - row_bytes.len(), 0);
        }
        for visual_row in (0..height).rev() {
            let row_bytes = mask
                .get(visual_row * tight_mask..(visual_row + 1) * tight_mask)
                .unwrap_or(&[]);
            out.extend_from_slice(row_bytes);
            out.resize(out.len() + mask_row - row_bytes.len(), 0);
        }
        out
    }

    #[test]
    fn magic_probe_and_dimensions_read_the_directory() {
        let payload = dib(1, 1, 32, &[], &[0, 0, 255, 255], &[0, 0, 0, 0]);
        let file = ico(&payload, 1, 1, 32);
        assert!(is_icon(&file));
        assert_eq!(dimensions(&file), Some((1, 1)));
        assert!(!is_icon(b"BM\0\0"));
        assert!(!is_icon(&[]));
        // A stored zero edge means 256 pixels, not zero.
        let mut large = file.clone();
        large[6] = 0;
        large[7] = 0;
        assert_eq!(dimensions(&large), Some((256, 256)));
        assert!(entries(&file).len() == 1);
    }

    #[test]
    fn thirty_two_bit_entry_decodes_straight_rgba() {
        // Two pixels on one row: opaque red and half-transparent blue, BGRA order.
        let pixels = [0, 0, 255, 255, 255, 0, 0, 128];
        let file = ico(&dib(2, 1, 32, &[], &pixels, &[0, 0, 0, 0]), 2, 1, 32);
        let image = decode(&file).expect("decodes");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(&image.pixels[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image.pixels[4..8], &[0, 0, 255, 128]);
    }

    #[test]
    fn twenty_four_bit_entry_takes_alpha_from_the_and_mask() {
        // Row 0: two pixels (blue, green); the mask marks the second pixel transparent.
        let pixels = [255, 0, 0, 0, 255, 0];
        let mask = [0b0100_0000, 0, 0, 0];
        let file = ico(&dib(2, 1, 24, &[], &pixels, &mask), 2, 1, 24);
        let image = decode(&file).expect("decodes");
        assert_eq!(&image.pixels[0..4], &[0, 0, 255, 255]);
        assert_eq!(&image.pixels[4..8], &[0, 255, 0, 0]);
    }

    #[test]
    fn palette_entry_reads_indexed_pixels() {
        let palette = [0, 0, 255, 255, 0, 255, 0, 255];
        let file = ico(
            &dib(2, 1, 8, &palette, &[1, 0, 0, 0], &[0, 0, 0, 0]),
            2,
            1,
            8,
        );
        let image = decode(&file).expect("decodes");
        assert_eq!(&image.pixels[0..4], &[0, 255, 0, 255]);
        assert_eq!(&image.pixels[4..8], &[255, 0, 0, 255]);
    }

    #[test]
    fn embedded_png_entry_decodes_with_the_png_decoder() {
        const PNG_1X1: [u8; 70] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let file = ico(&PNG_1X1, 1, 1, 32);
        let image = decode(&file).expect("decodes");
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(&image.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn largest_entry_wins_and_a_broken_entry_falls_through() {
        // Entry 0 is a valid 1x1 PNG; entry 1 claims 4x4 but its payload is not a bitmap.
        const PNG_1X1: [u8; 70] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let broken = [0u8; 8];
        let mut file = vec![0, 0, 1, 0, 2, 0];
        let entry = |width: u8, height: u8, length: usize, offset: usize| {
            let mut bytes = vec![width, height, 0, 0];
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&32u16.to_le_bytes());
            bytes.extend_from_slice(&(length as u32).to_le_bytes());
            bytes.extend_from_slice(&(offset as u32).to_le_bytes());
            bytes
        };
        let first = entry(1, 1, PNG_1X1.len(), 6 + 32);
        let second = entry(4, 4, broken.len(), 6 + 32 + PNG_1X1.len());
        file.extend_from_slice(&first);
        file.extend_from_slice(&second);
        file.extend_from_slice(&PNG_1X1);
        file.extend_from_slice(&broken);
        assert_eq!(dimensions(&file), Some((4, 4)));
        // The 4x4 entry cannot decode, so the 1x1 PNG is used instead of failing outright.
        let image = decode(&file).expect("falls back to the readable entry");
        assert_eq!((image.width, image.height), (1, 1));
    }

    #[test]
    fn truncated_and_bogus_files_error_instead_of_panicking() {
        let payload = dib(1, 1, 32, &[], &[0, 0, 255, 255], &[0, 0, 0, 0]);
        let file = ico(&payload, 1, 1, 32);
        for cut in [0, 4, 8, 20, 22, 30, 40, file.len() - 1] {
            let sliced = &file[..cut.min(file.len())];
            assert!(decode(sliced).is_err(), "cut at {cut}");
        }
        // A directory entry that claims more bytes than the file holds is dropped, never sliced.
        let mut lying = file.clone();
        lying[14..18].copy_from_slice(&9999u32.to_le_bytes());
        assert!(entries(&lying).is_empty());
        assert!(decode(&lying).is_err());
        // An unsupported depth is an error, not a panic.
        assert!(
            decode(&ico(
                &dib(1, 1, 16, &[], &[0, 0, 0, 0], &[0, 0, 0, 0]),
                1,
                1,
                16
            ))
            .is_err()
        );
    }

    #[test]
    fn pixel_data_shorter_than_the_header_promises_is_an_error() {
        let mut payload = dib(4, 4, 32, &[], &[0; 4 * 16], &[0; 4 * 4]);
        payload.truncate(payload.len() - 20);
        assert!(decode(&ico(&payload, 4, 4, 32)).is_err());
    }
}
