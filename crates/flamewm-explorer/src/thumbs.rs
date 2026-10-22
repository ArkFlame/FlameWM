//! Row thumbnails for image files. Decoding and downscaling run on a worker; the UI only reads the
//! bounded LRU cache, keyed by path plus the entry's mtime and size, so an edited file gets a fresh thumbnail.
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use flamewm_image_core::scale::{scale_rgba, ScaleFilter};
use flamewm_image_core::{decode_bytes, ImageSourceFormat, RgbaImage};

use crate::Entry;

/// Edge length of a row thumbnail in pixels; the image is fitted and centred on a transparent square.
pub const THUMB_SIZE: u32 = 24;
/// Larger files are never read for a thumbnail.
pub const MAX_SOURCE_BYTES: u64 = 25 * 1024 * 1024;
/// Images above this many pixels are skipped before decoding, bounding the RGBA decode buffer to about 100 MB.
pub const MAX_SOURCE_PIXELS: u64 = 25_000_000;
/// Decoded thumbnails kept in memory (about 600 KB at 24 px).
pub const CACHE_CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThumbKey {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub size: u64,
}

impl ThumbKey {
    /// Key for a row entry, or `None` when the entry is not a thumbnailable image file.
    pub fn of(entry: &Entry) -> Option<Self> {
        (!entry.directory && source_format(&entry.path).is_some()).then(|| Self {
            path: entry.path.clone(),
            modified: entry.modified,
            size: entry.size,
        })
    }
}

/// Raster formats that get thumbnails. SVG and PPM are excluded: they are not image-file rows in the Finder sense.
pub fn source_format(path: &Path) -> Option<ImageSourceFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
        .then(|| ImageSourceFormat::from_extension(path))
        .flatten()
}

fn read_uint(bytes: &[u8], at: usize, len: usize, big_endian: bool) -> Option<u32> {
    let raw = bytes.get(at..at + len)?;
    let value = if big_endian {
        raw.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
    } else {
        raw.iter().rev().fold(0u32, |acc, b| (acc << 8) | u32::from(*b))
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
        b"VP8 " if bytes.get(23..26)? == [0x9d, 0x01, 0x2a] => {
            Some((read_uint(bytes, 26, 2, false)? & 0x3fff, read_uint(bytes, 28, 2, false)? & 0x3fff))
        }
        b"VP8L" if *bytes.get(20)? == 0x2f => {
            let bits = read_uint(bytes, 21, 4, false)?;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" => Some((read_uint(bytes, 24, 3, false)? + 1, read_uint(bytes, 27, 3, false)? + 1)),
        _ => None,
    }
}

/// Canvas size from the container header, read without decoding. `None` when the header is not recognised.
pub fn header_dimensions(bytes: &[u8], format: ImageSourceFormat) -> Option<(u32, u32)> {
    match format {
        ImageSourceFormat::Png if bytes.get(12..16)? == b"IHDR" => {
            Some((read_uint(bytes, 16, 4, true)?, read_uint(bytes, 20, 4, true)?))
        }
        ImageSourceFormat::Gif => Some((read_uint(bytes, 6, 2, false)?, read_uint(bytes, 8, 2, false)?)),
        ImageSourceFormat::Jpeg => jpeg_dimensions(bytes),
        ImageSourceFormat::Webp => webp_dimensions(bytes),
        _ => None,
    }
}

/// Size of the image after fitting it inside an `edge` square. Images that already fit are never upscaled.
pub fn fit_dimensions(width: u32, height: u32, edge: u32) -> (u32, u32) {
    if width <= edge && height <= edge {
        return (width, height);
    }
    let scaled = |num: u32, den: u32| ((u64::from(num) * u64::from(edge)) / u64::from(den)).max(1) as u32;
    if width >= height { (edge, scaled(height, width)) } else { (scaled(width, height), edge) }
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
    let (left, top) = ((THUMB_SIZE - image.width) / 2, (THUMB_SIZE - image.height) / 2);
    let row_bytes = image.width as usize * 4;
    for y in 0..image.height as usize {
        let src = y * row_bytes;
        let dst = ((top as usize + y) * edge + left as usize) * 4;
        canvas[dst..dst + row_bytes].copy_from_slice(&image.pixels[src..src + row_bytes]);
    }
    canvas
}

/// Reads and decodes one image into a thumbnail canvas. Runs on the worker thread; `None` means no thumbnail.
pub fn render_thumbnail(key: &ThumbKey) -> Option<Vec<u8>> {
    let format = source_format(&key.path)?;
    let meta = fs::metadata(&key.path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let bytes = fs::read(&key.path).ok()?;
    let (width, height) = header_dimensions(&bytes, format)?;
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return None;
    }
    let natural = decode_bytes(&bytes, format).ok()?;
    Some(fit_canvas(&natural))
}

/// Result of a cache lookup on the UI thread.
pub enum Lookup {
    Missing,
    Failed,
    /// Monotonic id (new per decode, so a re-decoded file rebinds its row image) and the 24x24 RGBA canvas.
    Ready(u64, Arc<Vec<u8>>),
}

/// Bounded LRU of decoded thumbnails plus the set of keys whose decode is queued.
#[derive(Default)]
pub struct Thumbnails {
    entries: HashMap<ThumbKey, (u64, Option<Arc<Vec<u8>>>)>,
    order: VecDeque<ThumbKey>,
    pending: HashSet<ThumbKey>,
    next_id: u64,
}

impl Thumbnails {
    /// Looks up `key` and marks it most recently used.
    pub fn lookup(&mut self, key: &ThumbKey) -> Lookup {
        let Some((id, pixels)) = self.entries.get(key).cloned() else { return Lookup::Missing };
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            if let Some(touched) = self.order.remove(pos) {
                self.order.push_back(touched);
            }
        }
        match pixels {
            Some(pixels) => Lookup::Ready(id, pixels),
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
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
        }
        self.entries.insert(key.clone(), (self.next_id, pixels.map(Arc::new)));
        self.order.push_back(key);
        while self.order.len() > CACHE_CAPACITY {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn key(name: &str, secs: u64) -> ThumbKey {
        ThumbKey { path: PathBuf::from(name), modified: Some(UNIX_EPOCH + std::time::Duration::from_secs(secs)), size: 10 }
    }

    /// 1x1 opaque red PNG, generated once with PIL and embedded so the test needs no fixture file.
    const PNG_1X1: [u8; 70] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
        0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
        0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
        0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn only_raster_extensions_are_thumbnailed() {
        for name in ["a.PNG", "b.jpg", "c.jpeg", "d.gif", "e.webp"] {
            assert!(source_format(Path::new(name)).is_some(), "{name}");
        }
        for name in ["f.bmp", "g.svg", "h.txt", "noext"] {
            assert!(source_format(Path::new(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn png_header_gives_dimensions_without_decoding() {
        assert_eq!(header_dimensions(&PNG_1X1, ImageSourceFormat::Png), Some((1, 1)));
        let mut bogus = PNG_1X1;
        bogus[12..16].copy_from_slice(b"JUNK");
        assert_eq!(header_dimensions(&bogus, ImageSourceFormat::Png), None);
    }

    #[test]
    fn jpeg_scan_skips_app_segments_to_reach_sof() {
        // SOI, APP0 of length 4 (two payload bytes), then SOF0 with height 300 and width 640.
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0x2C, 0x02, 0x80];
        assert_eq!(header_dimensions(&jpeg, ImageSourceFormat::Jpeg), Some((640, 300)));
    }

    #[test]
    fn gif_and_webp_headers_report_canvas_size() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x40, 0x01, 0x20, 0x00]);
        assert_eq!(header_dimensions(&gif, ImageSourceFormat::Gif), Some((320, 32)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        // Payload: 4 flag bytes and 4 reserved bytes, then canvas width-1 and height-1 as 24-bit little-endian.
        webp.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0x1f, 0x00, 0x00, 0x09, 0x00, 0x00]);
        assert_eq!(header_dimensions(&webp, ImageSourceFormat::Webp), Some((32, 10)));
    }

    #[test]
    fn fit_never_upscales_and_keeps_aspect() {
        assert_eq!(fit_dimensions(10, 5, 24), (10, 5));
        assert_eq!(fit_dimensions(480, 240, 24), (24, 12));
        assert_eq!(fit_dimensions(240, 480, 24), (12, 24));
    }

    #[test]
    fn decoded_png_is_centred_on_transparent_square() {
        let dir = std::env::temp_dir().join(format!("flamewm-thumb-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("red.png");
        fs::write(&path, PNG_1X1).unwrap();
        let entry = Entry { path: path.clone(), name: "red.png".into(), directory: false, symlink: false, size: PNG_1X1.len() as u64, modified: None };
        let thumb = render_thumbnail(&ThumbKey::of(&entry).unwrap()).expect("decodes");
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(thumb.len(), 24 * 24 * 4);
        // A 1x1 image is not upscaled: exactly one opaque pixel at the centre, everything else transparent.
        let opaque: Vec<usize> = thumb.chunks_exact(4).enumerate().filter(|(_, p)| p[3] == 255).map(|(i, _)| i).collect();
        // (24 - 1) / 2 = 11 on both axes.
        assert_eq!(opaque, vec![11 * 24 + 11]);
        assert_eq!(&thumb[(11 * 24 + 11) * 4..(11 * 24 + 11) * 4 + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn cache_is_bounded_lru_and_caches_failures() {
        let mut cache = Thumbnails::default();
        assert!(cache.begin(&key("a", 1)));
        assert!(!cache.begin(&key("a", 1)), "second request for a queued key is ignored");
        cache.complete(key("a", 1), None);
        assert!(matches!(cache.lookup(&key("a", 1)), Lookup::Failed));
        for i in 0..CACHE_CAPACITY - 1 {
            cache.complete(key(&format!("f{i}"), 1), Some(vec![0; 4]));
        }
        assert_eq!(cache.order.len(), CACHE_CAPACITY);
        // Touching "a" moves it to the back, so the insert that overflows the cache evicts "f0" instead.
        assert!(matches!(cache.lookup(&key("a", 1)), Lookup::Failed));
        cache.complete(key("new", 1), Some(vec![0; 4]));
        assert_eq!(cache.order.len(), CACHE_CAPACITY);
        assert!(matches!(cache.lookup(&key("f0", 1)), Lookup::Missing));
        assert!(matches!(cache.lookup(&key("a", 1)), Lookup::Failed));
        assert!(matches!(cache.lookup(&key("new", 1)), Lookup::Ready(..)));
    }

    #[test]
    fn changed_mtime_is_a_different_key() {
        assert_ne!(key("a", 1), key("a", 2));
    }
}
