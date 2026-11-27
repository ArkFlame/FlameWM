//! Pure-Rust Xcursor reader for the `left_ptr` preview. Read-only.

use std::fmt;
use std::path::Path;

use super::read_regular_capped;

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

/// Bundled `left_ptr` preview for themes with no usable cursor file.
///
/// Smithay's anvil uses the same shape: a bundled cursor image when the configured theme cannot be
/// loaded (`.vendor/smithay-master/anvil/src/cursor.rs:9,26-40`). A machine with a good theme never
/// faults these bytes into RAM; the `OnceLock` makes a theme-less machine parse them once.
static BUNDLED_PREVIEW: std::sync::OnceLock<Option<CursorPreview>> = std::sync::OnceLock::new();
static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn bundled_preview() -> Option<CursorPreview> {
    BUNDLED_PREVIEW
        .get_or_init(|| {
            parse_xcursor_preview(
                &include_bytes!("../../../../assets/cursors/cursors/left_ptr")[..],
            )
            .ok()
        })
        .clone()
}

/// Load `<theme_root>/cursors/left_ptr` and decode its preview frame.
///
/// Missing, oversized or corrupt cursors never panic: the failure is reported once per process and
/// a bundled preview is shown instead, so a theme row is never blank. The root/escape and size gates
/// still run first, so a refused file is never decoded.
pub fn load_left_ptr_preview(theme_root: &Path) -> Result<CursorPreview, CursorError> {
    let preview = (|| {
        let path = resolve_in_theme_root(theme_root, &theme_root.join("cursors").join("left_ptr"))
            .map_err(CursorError::Io)?;
        let bytes = read_regular_capped(&path, MAX_FILE_BYTES)
            .map_err(CursorError::Io)?
            .ok_or(CursorError::TooLarge)?;
        parse_xcursor_preview(&bytes)
    })();
    match preview {
        Ok(preview) => Ok(preview),
        Err(error) => {
            if !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "flamewm: cursor theme {} left_ptr preview unavailable: {error}; using bundled left_ptr fallback; blast_radius=works:cursor-theme,system-cursor|degraded:theme-preview",
                    theme_root.display()
                );
            }
            bundled_preview().ok_or(error)
        }
    }
}

/// Canonicalizes `path` and accepts it only when it stays under the canonical
/// `theme_root`, so in-theme symlinks preview and escaping ones are refused.
fn resolve_in_theme_root(theme_root: &Path, path: &Path) -> std::io::Result<std::path::PathBuf> {
    let root = std::fs::canonicalize(theme_root)?;
    let resolved = std::fs::canonicalize(path)?;
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "left_ptr resolves outside theme root",
        ))
    }
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
    for px in pixels.as_chunks::<4>().0 {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::{bundled_preview, load_left_ptr_preview, parse_xcursor_preview};

    fn one_pixel_xcursor() -> Vec<u8> {
        let mut v = b"Xcur".to_vec();
        for word in [
            16u32,
            0x0001_0000,
            1,
            0xfffd_0002,
            24,
            28,
            36,
            0xfffd_0002,
            24,
            1,
            1,
            1,
            0,
            0,
            0,
        ] {
            v.extend_from_slice(&word.to_le_bytes());
        }
        v.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]); // B, G, R, A
        v
    }

    #[test]
    fn valid_frame_decodes_to_straight_rgba() {
        let preview = parse_xcursor_preview(&one_pixel_xcursor()).expect("valid one-pixel Xcursor");
        assert_eq!(
            (preview.width, preview.height, preview.nominal_size),
            (1, 1, 24)
        );
        assert_eq!(preview.rgba, vec![0xff, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn corrupt_or_truncated_xcursor_yields_no_preview() {
        let valid = one_pixel_xcursor();
        for bytes in [
            &valid[..40],
            b"Xcur".as_slice(),
            b"Xcur\x00\x00\x00\x00".as_slice(),
            b"nope".as_slice(),
        ] {
            assert!(
                parse_xcursor_preview(bytes).is_err(),
                "must not decode {bytes:?}"
            );
        }
    }

    #[test]
    fn oversized_left_ptr_is_refused_before_decoding() {
        let root =
            std::env::temp_dir().join(format!("flamewm-cursor-oversize-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("cursors")).expect("cursors dir");
        let file = fs::File::create(root.join("cursors").join("left_ptr")).expect("left_ptr");
        file.set_len(16 * 1024 * 1024 + 1).expect("sparse oversize");
        let expected = bundled_preview();
        assert!(expected.is_some(), "bundled preview must decode");
        let result = load_left_ptr_preview(&root);
        assert!(
            result.is_ok(),
            "oversized left_ptr must fall back, not panic"
        );
        if let (Some(expected), Ok(preview)) = (expected, result) {
            assert_eq!(
                preview, expected,
                "oversized file must be refused; bundled shown"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_left_ptr_uses_bundled_fallback_without_panic() {
        let root =
            std::env::temp_dir().join(format!("flamewm-cursor-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("cursors")).expect("cursors dir");
        let result = load_left_ptr_preview(&root);
        assert!(
            result.is_ok(),
            "missing left_ptr must use the bundled fallback, not error"
        );
        if let Ok(preview) = result {
            assert!(!preview.rgba.is_empty(), "bundled preview is decoded");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn left_ptr_symlink_escaping_theme_root_is_refused() {
        let base =
            std::env::temp_dir().join(format!("flamewm-cursor-escape-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("theme");
        fs::create_dir_all(root.join("cursors")).expect("cursors dir");
        fs::write(base.join("outside"), one_pixel_xcursor()).expect("outside file");
        symlink(base.join("outside"), root.join("cursors").join("left_ptr")).expect("symlink");
        let expected = bundled_preview();
        assert!(expected.is_some(), "bundled preview must decode");
        let result = load_left_ptr_preview(&root);
        assert!(
            result.is_ok(),
            "escaping symlink must fall back, not decode"
        );
        if let (Some(expected), Ok(preview)) = (expected, result) {
            assert_eq!(
                preview, expected,
                "escaping file must be refused; bundled shown"
            );
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn left_ptr_symlink_inside_theme_root_previews() {
        let root =
            std::env::temp_dir().join(format!("flamewm-cursor-inroot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("cursors")).expect("cursors dir");
        fs::write(root.join("cursors").join("default"), one_pixel_xcursor()).expect("default");
        symlink("default", root.join("cursors").join("left_ptr")).expect("symlink");
        let preview = load_left_ptr_preview(&root).expect("in-root symlink previews");
        assert_eq!((preview.width, preview.height), (1, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bundled_left_ptr_preview_is_usable() {
        let Some(preview) = bundled_preview() else {
            panic!("the bundled left_ptr preview must decode")
        };
        assert!(
            preview.width > 0 && preview.height > 0,
            "bundled preview has a frame"
        );
        assert_eq!(
            preview.rgba.len(),
            (preview.width * preview.height * 4) as usize,
            "bundled preview is straight RGBA"
        );
        assert!(
            preview.rgba.iter().any(|&channel| channel != 0),
            "bundled preview has visible pixels"
        );
    }
}
