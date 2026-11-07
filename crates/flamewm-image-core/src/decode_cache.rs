//! On-disk cache of decoded wallpaper rasters under `$XDG_CACHE_HOME/flamewm/decoded`.
//!
//! An entry is keyed by source path, byte length and mtime, so an edited or replaced file never
//! serves stale pixels. Layout: 8-byte magic, `u32` LE width, `u32` LE height, then straight RGBA8.
//! Every cache failure (missing dir, short or corrupt entry, unwritable cache) falls back to a
//! plain decode, so the cache can only remove work, never change the result.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::RgbaImage;

const MAGIC: &[u8; 8] = b"FWRGBA01";
const HEADER_LEN: usize = 16;

/// Decodes `path` to natural-size straight RGBA8, reusing a cached raster when its key matches.
pub fn decode_file_cached(path: &Path) -> Result<RgbaImage, String> {
    let entry = entry_for(path);
    if let Some(image) = entry.as_deref().and_then(read_entry) {
        return Ok(image);
    }
    let image = crate::wallpaper::decode_file_uncached(path)?;
    if let Some(entry) = entry.as_deref() {
        write_entry(entry, &image);
    }
    Ok(image)
}

fn entry_for(path: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    feed(path.to_string_lossy().as_bytes());
    feed(&meta.len().to_le_bytes());
    feed(&mtime.to_le_bytes());
    Some(cache_dir()?.join(format!("{hash:016x}.rgba")))
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("flamewm").join("decoded"))
}

fn read_entry(entry: &Path) -> Option<RgbaImage> {
    let mut bytes = fs::read(entry).ok()?;
    if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    let expected = usize::try_from(u64::from(width) * u64::from(height) * 4).ok()?;
    if bytes.len() != HEADER_LEN + expected {
        return None;
    }
    let pixels = bytes.split_off(HEADER_LEN);
    RgbaImage::from_rgba8(width, height, pixels)
}

fn write_entry(entry: &Path, image: &RgbaImage) {
    let Some(dir) = entry.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut bytes = Vec::with_capacity(HEADER_LEN + image.pixels.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&image.width.to_le_bytes());
    bytes.extend_from_slice(&image.height.to_le_bytes());
    bytes.extend_from_slice(&image.pixels);
    // Write to a per-process temp name, then rename, so readers never see a partial entry.
    let temp = entry.with_extension(format!("rgba.{}.tmp", std::process::id()));
    if fs::write(&temp, &bytes).is_err() {
        return;
    }
    if fs::rename(&temp, entry).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_round_trips_and_rejects_corrupt_length() {
        let dir = std::env::temp_dir().join(format!("fw-decode-cache-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("entry.rgba");
        let image = RgbaImage::from_rgba8(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        write_entry(&entry, &image);
        let back = read_entry(&entry).expect("valid entry reads back");
        assert_eq!((back.width, back.height), (2, 1));
        assert_eq!(back.pixels, image.pixels);
        let mut bytes = fs::read(&entry).unwrap();
        bytes.pop();
        fs::write(&entry, &bytes).unwrap();
        assert!(read_entry(&entry).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
