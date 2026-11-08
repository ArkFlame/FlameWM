//! On-disk cache of decoded wallpaper rasters under `$XDG_CACHE_HOME/flamewm/decoded`.
//!
//! An entry is keyed by source path, byte length and mtime, so an edited or replaced file never
//! serves stale pixels. File name: `<path hash>-<key hash>.rgba`, so every entry for one source
//! path shares a prefix. Layout: 8-byte magic, `u32` LE width, `u32` LE height, then straight RGBA8.
//! Every cache failure (missing dir, short or corrupt entry, unwritable cache) falls back to a
//! plain decode, so the cache can only remove work, never change the result.
//!
//! Bounds: a write removes older entries for the same source path, then evicts least recently
//! used entries (mtime; a hit refreshes it) until the directory is at most [`MAX_CACHE_BYTES`].
//! The entry just written is never evicted.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::RgbaImage;

const MAGIC: &[u8; 8] = b"FWRGBA01";
const HEADER_LEN: usize = 16;
/// Total size budget for the decoded-wallpaper cache directory.
pub const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

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

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn entry_for(path: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let path_bytes = path.to_string_lossy();
    let path_hash = fnv1a(0xcbf2_9ce4_8422_2325, path_bytes.as_bytes());
    let key = fnv1a(
        fnv1a(path_hash, &meta.len().to_le_bytes()),
        &mtime.to_le_bytes(),
    );
    Some(cache_dir()?.join(format!("{path_hash:016x}-{key:016x}.rgba")))
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
    let image = RgbaImage::from_rgba8(width, height, pixels)?;
    touch(entry);
    Some(image)
}

/// Marks `entry` as recently used for LRU eviction. Failure only leaves the old order in place.
fn touch(entry: &Path) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(entry) {
        let _ = file.set_modified(SystemTime::now());
    }
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
        return;
    }
    prune(dir, entry, MAX_CACHE_BYTES);
}

/// Removes `<source path>` siblings of `keep` (same name prefix, older key) and then evicts the
/// least recently used files until the directory holds at most `max_bytes`. `keep` is never
/// removed. Best effort: unreadable entries are skipped.
fn prune(dir: &Path, keep: &Path, max_bytes: u64) {
    let Some(keep_name) = keep
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let owner = keep_name
        .split_once('-')
        .map(|(owner, _)| format!("{owner}-"));
    let Ok(read) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for item in read.flatten() {
        let Ok(meta) = item.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        let stale = name != keep_name
            && name.ends_with(".rgba")
            && owner
                .as_deref()
                .is_some_and(|owner| name.starts_with(owner));
        if stale {
            let _ = fs::remove_file(&path);
            continue;
        }
        let modified = meta.modified().unwrap_or(UNIX_EPOCH);
        files.push((modified, meta.len(), path));
    }
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    if total <= max_bytes {
        return;
    }
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if path
            .file_name()
            .is_some_and(|name| name == keep_name.as_str())
        {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fw-decode-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `name` with `len` bytes and an mtime `age_secs` before now (larger = older).
    fn put(dir: &Path, name: &str, len: usize, age_secs: u64) {
        let path = dir.join(name);
        fs::write(&path, vec![0u8; len]).unwrap();
        let when = SystemTime::now() - std::time::Duration::from_secs(age_secs);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn entry_round_trips_and_rejects_corrupt_length() {
        let dir = scratch("roundtrip");
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

    #[test]
    fn write_removes_older_entries_for_the_same_source_path() {
        let dir = scratch("stale");
        put(&dir, "aaaa-1111.rgba", 10, 30);
        put(&dir, "aaaa-2222.rgba", 10, 20);
        put(&dir, "bbbb-3333.rgba", 10, 10);
        prune(&dir, &dir.join("aaaa-2222.rgba"), MAX_CACHE_BYTES);
        assert_eq!(names(&dir), ["aaaa-2222.rgba", "bbbb-3333.rgba"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_evicts_least_recently_used_first() {
        let dir = scratch("budget");
        put(&dir, "aaaa-1111.rgba", 40, 300);
        put(&dir, "bbbb-2222.rgba", 40, 200);
        put(&dir, "cccc-3333.rgba", 40, 100);
        put(&dir, "dddd-4444.rgba", 40, 0);
        prune(&dir, &dir.join("dddd-4444.rgba"), 100);
        assert_eq!(names(&dir), ["cccc-3333.rgba", "dddd-4444.rgba"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn written_entry_is_never_evicted_even_when_it_alone_exceeds_budget() {
        let dir = scratch("keep");
        put(&dir, "aaaa-1111.rgba", 10, 500);
        put(&dir, "bbbb-2222.rgba", 200, 1000);
        prune(&dir, &dir.join("bbbb-2222.rgba"), 100);
        assert_eq!(names(&dir), ["bbbb-2222.rgba"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
