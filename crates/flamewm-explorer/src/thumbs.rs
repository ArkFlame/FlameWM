//! Row thumbnails for image, SVG, ICO and `.desktop` rows. Decoding and downscaling run on a
//! worker; the UI only reads the bounded LRU cache, keyed by path plus the entry's mtime and size,
//! so an edited file gets a fresh thumbnail.
//!
//! A row slot shows either a thumbnail or the embedded type icon (`row-*.svg`), never nothing:
//! every miss, failure, oversized source, corrupt file or fully transparent result is cached as a
//! failure so the UI keeps the type icon in the slot.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use flamewm_image_core::scale::{ScaleFilter, scale_rgba};
use flamewm_image_core::{
    ImageSourceFormat, RgbaImage, WrappedImageFormat, decode_bytes, decode_bytes_at,
    decode_icon_container_at, svg,
};

use crate::{Entry, appicon};

/// Edge length of a row thumbnail in pixels. Matches the `.row-ico` box in `ui/explorer/explorer.css`
/// (18 px) so the thumbnail and the type icon occupy the same slot and never shift the name cell.
pub const THUMB_SIZE: u32 = 18;
/// Larger files are never read for a thumbnail.
pub const MAX_SOURCE_BYTES: u64 = 25 * 1024 * 1024;
/// Images above this many pixels are skipped before decoding, bounding the RGBA decode buffer to about 100 MB.
pub const MAX_SOURCE_PIXELS: u64 = 25_000_000;
/// Decoded thumbnails kept in memory (about 330 KB at 18 px).
pub const CACHE_CAPACITY: usize = 256;
/// Alpha at or above this value counts as a visible pixel; anything less is an invisible thumbnail.
pub const MIN_VISIBLE_ALPHA: u8 = 8;

/// Source kind behind one row thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbFormat {
    /// A single-bitmap raster the shared decoder already owns.
    Raster(ImageSourceFormat),
    /// Scalable vector, rendered straight at the thumbnail size.
    Svg,
    /// Windows icon container, decoded to its largest entry.
    Ico,
    /// `.desktop` launcher: the app icon named by its own `Icon=` metadata.
    Desktop,
}

impl ThumbFormat {
    /// Formats whose bytes are an image the pipeline can decode directly.
    pub fn is_image(self) -> bool {
        !matches!(self, Self::Desktop)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

impl ThumbKey {
    /// Key for a row entry, or `None` when the entry is not a thumbnailable file.
    pub fn of(entry: &Entry) -> Option<Self> {
        (!entry.directory && thumb_format(&entry.path).is_some()).then(|| Self {
            path: entry.path.clone(),
            modified: entry.modified,
            size: entry.size,
        })
    }
}

/// Source kind for a path: raster (png/jpg/jpeg/gif/webp), SVG, ICO or `.desktop`.
/// PPM stays excluded: it is a development format, not a document a file manager previews.
pub fn thumb_format(path: &Path) -> Option<ThumbFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "svg" => Some(ThumbFormat::Svg),
        "ico" | "cur" => Some(ThumbFormat::Ico),
        "desktop" => Some(ThumbFormat::Desktop),
        "png" | "jpg" | "jpeg" | "gif" | "webp" => {
            ImageSourceFormat::from_extension(path).map(ThumbFormat::Raster)
        }
        _ => None,
    }
}

/// Raster-only view of [`thumb_format`], for callers that can only handle single-bitmap formats.
pub fn source_format(path: &Path) -> Option<ImageSourceFormat> {
    match thumb_format(path)? {
        ThumbFormat::Raster(format) => Some(format),
        _ => None,
    }
}

fn read_uint(bytes: &[u8], at: usize, len: usize, big_endian: bool) -> Option<u32> {
    let raw = bytes.get(at..at + len)?;
    let value = if big_endian {
        raw.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
    } else {
        raw.iter()
            .rev()
            .fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
    };
    Some(value)
}

/// JPEG start-of-frame scan: returns the dimensions from the first SOFn marker (SOF4/8/12 are not frames).
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
        } else if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            i += 2;
        } else if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = read_uint(bytes, i + 5, 2, true)?;
            let width = read_uint(bytes, i + 7, 2, true)?;
            return Some((width, height));
        } else {
            i += 2 + read_uint(bytes, i + 2, 2, true)? as usize;
        }
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8 " if bytes.get(23..26)? == [0x9d, 0x01, 0x2a] => Some((
            read_uint(bytes, 26, 2, false)? & 0x3fff,
            read_uint(bytes, 28, 2, false)? & 0x3fff,
        )),
        b"VP8L" if *bytes.get(20)? == 0x2f => {
            let bits = read_uint(bytes, 21, 4, false)?;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => Some((
            read_uint(bytes, 24, 3, false)? + 1,
            read_uint(bytes, 27, 3, false)? + 1,
        )),
        _ => None,
    }
}

/// Canvas size from the container header, read without decoding. `None` when the header is not recognised.
pub fn header_dimensions(bytes: &[u8], format: ImageSourceFormat) -> Option<(u32, u32)> {
    match format {
        ImageSourceFormat::Png if bytes.get(12..16)? == b"IHDR" => Some((
            read_uint(bytes, 16, 4, true)?,
            read_uint(bytes, 20, 4, true)?,
        )),
        ImageSourceFormat::Gif => Some((
            read_uint(bytes, 6, 2, false)?,
            read_uint(bytes, 8, 2, false)?,
        )),
        ImageSourceFormat::Jpeg => jpeg_dimensions(bytes),
        ImageSourceFormat::Webp => webp_dimensions(bytes),
        _ => None,
    }
}

/// Header probe for every thumbnail format. Runs before any decoding so an oversized or malformed
/// source is rejected without allocating pixels.
pub fn probe_dimensions(bytes: &[u8], format: ThumbFormat) -> Option<(u32, u32)> {
    match format {
        ThumbFormat::Raster(format) => header_dimensions(bytes, format),
        ThumbFormat::Svg => svg::natural_size(bytes).ok(),
        ThumbFormat::Ico => WrappedImageFormat::Ico.dimensions(bytes),
        ThumbFormat::Desktop => None,
    }
}

/// Size of the image after fitting it inside an `edge` square. Images that already fit are never upscaled.
pub fn fit_dimensions(width: u32, height: u32, edge: u32) -> (u32, u32) {
    if width <= edge && height <= edge {
        return (width, height);
    }
    let scaled =
        |num: u32, den: u32| ((u64::from(num) * u64::from(edge)) / u64::from(den)).max(1) as u32;
    if width >= height {
        (edge, scaled(height, width))
    } else {
        (scaled(width, height), edge)
    }
}

/// Fits `source` into a `THUMB_SIZE` square of straight RGBA8, centred, with transparent margins.
pub fn fit_canvas(source: &RgbaImage) -> Vec<u8> {
    let (width, height) = fit_dimensions(source.width, source.height, THUMB_SIZE);
    let scaled = if (width, height) == (source.width, source.height) {
        None
    } else {
        scale_rgba(source, width, height, ScaleFilter::Area)
    };
    let image = scaled.as_ref().unwrap_or(source);
    let edge = THUMB_SIZE as usize;
    let mut canvas = vec![0u8; edge * edge * 4];
    let (left, top) = (
        (THUMB_SIZE - image.width) / 2,
        (THUMB_SIZE - image.height) / 2,
    );
    let row_bytes = image.width as usize * 4;
    for y in 0..image.height as usize {
        let src = y * row_bytes;
        let dst = ((top as usize + y) * edge + left as usize) * 4;
        canvas[dst..dst + row_bytes].copy_from_slice(&image.pixels[src..src + row_bytes]);
    }
    canvas
}

/// True when at least one pixel would actually be painted. A fully transparent canvas (an image
/// that is nothing but alpha, or a broken encoder) is treated as "no thumbnail" so the row keeps
/// its type icon instead of showing an empty slot.
pub fn visible_canvas(canvas: &[u8]) -> bool {
    canvas
        .chunks_exact(4)
        .any(|pixel| pixel[3] >= MIN_VISIBLE_ALPHA)
}

/// Decodes image bytes of a known thumbnail format into a fitted canvas. `None` means "no thumbnail".
pub fn thumbnail_from_bytes(bytes: &[u8], format: ThumbFormat) -> Option<Vec<u8>> {
    let (width, height) = probe_dimensions(bytes, format)?;
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return None;
    }
    let canvas = match format {
        ThumbFormat::Raster(format) => fit_canvas(&decode_bytes(bytes, format).ok()?),
        // Vectors render straight at the fitted size: no full-size raster is ever allocated.
        ThumbFormat::Svg | ThumbFormat::Ico => {
            let (fit_w, fit_h) = fit_dimensions(width, height, THUMB_SIZE);
            let decoded = match format {
                ThumbFormat::Svg => decode_bytes_at(
                    bytes,
                    ImageSourceFormat::Svg,
                    fit_w,
                    fit_h,
                    ScaleFilter::Area,
                ),
                _ => decode_icon_container_at(
                    bytes,
                    WrappedImageFormat::Ico,
                    fit_w,
                    fit_h,
                    ScaleFilter::Area,
                ),
            };
            fit_canvas(&decoded.ok()?)
        }
        ThumbFormat::Desktop => return None,
    };
    visible_canvas(&canvas).then_some(canvas)
}

/// Reads a source file, refusing anything that is not a regular file within the byte cap.
fn read_source(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
        return None;
    }
    fs::read(path).ok()
}

/// Reads and decodes one image into a thumbnail canvas. Runs on the worker thread; `None` means no thumbnail.
pub fn render_thumbnail(key: &ThumbKey) -> Option<Vec<u8>> {
    let format = thumb_format(&key.path)?;
    if format == ThumbFormat::Desktop {
        // The .desktop file itself is data, not a picture: its Icon= names the artwork to decode.
        let icon = appicon::icon_file(&key.path)?;
        let icon_format = thumb_format(&icon).filter(|format| format.is_image())?;
        return thumbnail_from_bytes(&read_source(&icon)?, icon_format);
    }
    thumbnail_from_bytes(&read_source(&key.path)?, format)
}

/// Result of a cache lookup on the UI thread.
pub enum Lookup {
    Missing,
    Failed,
    /// Monotonic id (new per decode, so a re-decoded file rebinds its row image) and the RGBA canvas.
    Ready(u64, Arc<Vec<u8>>),
}

/// One cached decode: its monotonic id, the pixels (`None` caches a failure), and the last-use tick.
struct Slot {
    id: u64,
    pixels: Option<Arc<Vec<u8>>>,
    used: u64,
}

/// Bounded LRU of decoded thumbnails plus the set of keys whose decode is queued.
/// A lookup only bumps a tick (O(1)); the oldest tick is scanned for only when an insert overflows.
#[derive(Default)]
pub struct Thumbnails {
    entries: HashMap<ThumbKey, Slot>,
    pending: HashSet<ThumbKey>,
    next_id: u64,
    clock: u64,
}

impl Thumbnails {
    /// Looks up `key` and marks it most recently used.
    pub fn lookup(&mut self, key: &ThumbKey) -> Lookup {
        self.clock += 1;
        let Some(slot) = self.entries.get_mut(key) else {
            return Lookup::Missing;
        };
        slot.used = self.clock;
        match &slot.pixels {
            Some(pixels) => Lookup::Ready(slot.id, Arc::clone(pixels)),
            None => Lookup::Failed,
        }
    }

    /// Marks `key` as queued. Returns `true` only for the first request, so each file decodes once.
    pub fn begin(&mut self, key: &ThumbKey) -> bool {
        self.pending.insert(key.clone())
    }

    /// Stores a finished decode (`None` caches the failure) and evicts the least recently used entries.
    pub fn complete(&mut self, key: ThumbKey, pixels: Option<Vec<u8>>) {
        self.pending.remove(&key);
        self.next_id += 1;
        self.clock += 1;
        let slot = Slot {
            id: self.next_id,
            pixels: pixels.map(Arc::new),
            used: self.clock,
        };
        self.entries.insert(key, slot);
        while self.entries.len() > CACHE_CAPACITY {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, slot)| slot.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests;
