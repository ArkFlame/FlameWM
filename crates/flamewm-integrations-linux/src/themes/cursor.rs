//! Pure-Rust Xcursor reader for the `left_ptr` preview. Read-only.

use std::fmt;
use std::fs;
use std::path::Path;

const MAGIC: &[u8; 4] = b"Xcur";
const IMAGE_TYPE: u32 = 0xfffd_0002;
const PREVIEW_SIZE: u32 = 24;
const MAX_DIM: u32 = 256;
const MAX_TOC: u32 = 4096;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const IMAGE_HEADER_MIN: usize = 36;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorPreview {
    pub nominal_size: u32,
    pub width: u32,
    pub height: u32,
    pub hotspot: (u32, u32),
    /// Straight RGBA, row-major, 4 bytes per pixel, alpha as stored in the file.
    pub rgba: Vec<u8>,
}

#[derive(Debug)]
pub enum CursorError {
    Io(std::io::Error),
    TooLarge,
    BadMagic,
    Truncated,
    Malformed(&'static str),
    NoImage,
}

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CursorError {}

/// Load `<theme_root>/cursors/left_ptr` and decode its preview frame.
pub fn load_left_ptr_preview(theme_root: &Path) -> Result<CursorPreview, CursorError> {
    let path = theme_root.join("cursors").join("left_ptr");
    let meta = fs::metadata(&path).map_err(CursorError::Io)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(CursorError::TooLarge);
    }
    let bytes = fs::read(&path).map_err(CursorError::Io)?;
    parse_xcursor_preview(&bytes)
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, CursorError> {
    let end = at.checked_add(4).ok_or(CursorError::Truncated)?;
    let raw: [u8; 4] = bytes
        .get(at..end)
        .and_then(|s| s.try_into().ok())
        .ok_or(CursorError::Truncated)?;
    Ok(u32::from_le_bytes(raw))
}

/// Decode the image frame whose nominal size is nearest 24 (ties prefer the smaller).
pub fn parse_xcursor_preview(bytes: &[u8]) -> Result<CursorPreview, CursorError> {
    if bytes.get(0..4) != Some(MAGIC.as_slice()) {
        return Err(CursorError::BadMagic);
    }
    let header_len = u32_at(bytes, 4)? as usize;
    let ntoc = u32_at(bytes, 12)?;
    if header_len < 16 {
        return Err(CursorError::Malformed("header length"));
    }
    if ntoc > MAX_TOC {
        return Err(CursorError::Malformed("toc count"));
    }
    let mut best: Option<(u32, u32, usize)> = None;
    for index in 0..ntoc as usize {
        let base = index
            .checked_mul(12)
            .and_then(|off| off.checked_add(header_len))
            .ok_or(CursorError::Truncated)?;
        if u32_at(bytes, base)? != IMAGE_TYPE {
            continue;
        }
        let size = u32_at(bytes, base + 4)?;
        let pos = u32_at(bytes, base + 8)? as usize;
        let distance = size.abs_diff(PREVIEW_SIZE);
        if best.is_none_or(|(d, s, _)| (distance, size) < (d, s)) {
            best = Some((distance, size, pos));
        }
    }
    let (_, nominal_size, pos) = best.ok_or(CursorError::NoImage)?;
    let chunk_len = u32_at(bytes, pos)? as usize;
    if u32_at(bytes, pos + 4)? != IMAGE_TYPE {
        return Err(CursorError::Malformed("chunk type"));
    }
    if chunk_len < IMAGE_HEADER_MIN {
        return Err(CursorError::Malformed("chunk length"));
    }
    let width = u32_at(bytes, pos + 16)?;
    let height = u32_at(bytes, pos + 20)?;
    let xhot = u32_at(bytes, pos + 24)?;
    let yhot = u32_at(bytes, pos + 28)?;
    if width == 0 || height == 0 || width > MAX_DIM || height > MAX_DIM {
        return Err(CursorError::Malformed("dimensions"));
    }
    if xhot >= width || yhot >= height {
        return Err(CursorError::Malformed("hotspot"));
    }
    let start = pos.checked_add(chunk_len).ok_or(CursorError::Truncated)?;
    let need = (width as usize) * (height as usize) * 4;
    let end = start.checked_add(need).ok_or(CursorError::Truncated)?;
    let pixels = bytes.get(start..end).ok_or(CursorError::Truncated)?;
    // File pixels are little-endian ARGB words, i.e. bytes B, G, R, A.
    let mut rgba = Vec::with_capacity(need);
    for px in pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    Ok(CursorPreview {
        nominal_size,
        width,
        height,
        hotspot: (xhot, yhot),
        rgba,
    })
}
