//! Shared deterministic image decode core (J01).
//!
//! Single owner for PPM/SVG/PNG/WebP/JPEG/GIF decoding into straight (non-premultiplied)
//! RGBA8. PPM and JPEG inputs are always opaque (alpha=255); SVG/PNG/WebP/GIF preserve alpha.
//! Size limits: max 4096x4096, max 64 MiB decoded.

use std::path::Path;

pub mod blur;
pub use blur::{
    frosted_backdrop, gaussian_blur_rgba, gaussian_blur_rgba_with, BlurError, BlurScratch, Rect,
};
pub mod gif_frame;
pub mod jpeg;
pub mod png;
pub mod ppm;
pub mod scale;
pub mod svg;
pub mod wallpaper;
pub mod webp;
pub mod xpm;

/// Semantic Breeze-style color scheme for SVG `ColorScheme-*` classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvgColorScheme {
    pub text: [u8; 4],
    pub background: [u8; 4],
    pub highlight: [u8; 4],
    pub negative_text: [u8; 4],
}

impl SvgColorScheme {
    /// Flame default: light text, dark surface, red highlight/negative.
    #[must_use]
    pub const fn flame_default() -> Self {
        Self {
            text: [0xf1, 0xf2, 0xf3, 255],
            background: [0x20, 0x23, 0x26, 255],
            highlight: [0xef, 0x40, 0x48, 255],
            negative_text: [0xda, 0x44, 0x53, 255],
        }
    }
}

/// Maximum decoded width or height in pixels.
pub const MAX_DIMENSION: u32 = 4096;
/// Maximum decoded pixel payload in bytes (64 MiB).
pub const MAX_PIXEL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageSourceFormat {
    Ppm,
    Svg,
    Png,
    Webp,
    Jpeg,
    Gif,
}

impl ImageSourceFormat {
    /// Identifies a supported raster format from its leading magic bytes.
    #[must_use]
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
            Some(Self::Png)
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }

    #[must_use]
    pub fn from_extension(path: &Path) -> Option<Self> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "ppm" => Some(Self::Ppm),
            "svg" => Some(Self::Svg),
            "png" => Some(Self::Png),
            "webp" => Some(Self::Webp),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            _ => None,
        }
    }
}

/// Straight (non-premultiplied) RGBA8, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// Builds from packed RGB8, forcing alpha=255 (opaque PPM contract).
    #[must_use]
    pub fn from_rgb8_opaque(width: u32, height: u32, rgb: &[u8]) -> Option<Self> {
        check_size_limits(width, height).ok()?;
        let pixel_count = width as usize * height as usize;
        if rgb.len() != pixel_count * 3 {
            return None;
        }
        let mut pixels = Vec::with_capacity(pixel_count * 4);
        pixels.extend(rgb.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]));
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Builds from straight RGBA8 pixels.
    #[must_use]
    pub fn from_rgba8(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        check_size_limits(width, height).ok()?;
        if pixels.len() != width as usize * height as usize * 4 {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    #[must_use]
    pub fn is_fully_opaque(&self) -> bool {
        self.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255)
    }

    #[must_use]
    pub fn alpha_at(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = (y as usize * self.width as usize + x as usize) * 4 + 3;
        self.pixels.get(index).copied()
    }
}

pub fn check_size_limits(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("image dimensions must be non-zero".to_string());
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(format!(
            "image dimensions {width}x{height} exceed maximum {MAX_DIMENSION}x{MAX_DIMENSION}"
        ));
    }
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    match bytes {
        Some(len) if len <= MAX_PIXEL_BYTES => Ok(()),
        _ => Err(format!(
            "image pixel payload exceeds maximum {MAX_PIXEL_BYTES} bytes"
        )),
    }
}

/// Decodes at natural size. SVG without intrinsic dimensions is rejected.
pub fn decode_bytes(bytes: &[u8], format: ImageSourceFormat) -> Result<RgbaImage, String> {
    match format {
        ImageSourceFormat::Ppm => ppm::decode(bytes),
        ImageSourceFormat::Svg => {
            let (width, height) = svg::natural_size(bytes)?;
            svg::render(bytes, width, height)
        }
        ImageSourceFormat::Png => png::decode(bytes),
        ImageSourceFormat::Webp => webp::decode(bytes),
        ImageSourceFormat::Jpeg => jpeg::decode(bytes),
        ImageSourceFormat::Gif => gif_frame::decode(bytes),
    }
}

/// Decodes and scales to an exact target size with the given filter.
pub fn decode_bytes_at(
    bytes: &[u8],
    format: ImageSourceFormat,
    width: u32,
    height: u32,
    filter: scale::ScaleFilter,
) -> Result<RgbaImage, String> {
    check_size_limits(width, height)?;
    match format {
        ImageSourceFormat::Ppm => {
            let natural = ppm::decode(bytes)?;
            scale::scale_rgba(&natural, width, height, filter)
                .ok_or_else(|| "requested dimensions overflow".to_string())
        }
        ImageSourceFormat::Svg => svg::render(bytes, width, height),
        ImageSourceFormat::Png
        | ImageSourceFormat::Webp
        | ImageSourceFormat::Jpeg
        | ImageSourceFormat::Gif => {
            let natural = decode_bytes(bytes, format)?;
            if natural.width == width && natural.height == height {
                return Ok(natural);
            }
            scale::scale_rgba(&natural, width, height, filter)
                .ok_or_else(|| "requested dimensions overflow".to_string())
        }
    }
}

/// Converts tiny-skia premultiplied pixel bytes to straight RGBA8.
///
/// tiny-skia pixmaps are RGBA-ordered (data[0]->R, data[1]->G, data[2]->B)
/// with premultiplied color channels. Fully transparent pixels stay
/// (0,0,0,0); no color-key substitution.
#[must_use]
pub fn unpremultiply_swapped_to_rgba(premultiplied: &[u8]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(premultiplied.len());
    for rgba in premultiplied.chunks_exact(4) {
        let a = u16::from(rgba[3]);
        if a == 0 {
            pixels.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let straight = |c: u8| ((u16::from(c) * 255 + a / 2) / a).min(255) as u8;
        pixels.extend_from_slice(&[straight(rgba[0]), straight(rgba[1]), straight(rgba[2]), a as u8]);
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_reject_oversized_and_empty() {
        assert!(check_size_limits(0, 8).is_err());
        assert!(check_size_limits(4097, 8).is_err());
        assert!(check_size_limits(4096, 4096).is_ok());
        assert!(check_size_limits(8192, 8192).is_err());
        assert!(check_size_limits(64, 64).is_ok());
    }

    #[test]
    fn opaque_constructor_forces_full_alpha() {
        let image = RgbaImage::from_rgb8_opaque(1, 1, &[0x11, 0x22, 0x33]).unwrap();
        assert_eq!(image.pixels, vec![0x11, 0x22, 0x33, 255]);
        assert!(image.is_fully_opaque());
    }

    #[test]
    fn unknown_extension_resolves_to_none() {
        for (name, expected) in [
            ("icon.bmp", None),
            ("icon.JPG", Some(ImageSourceFormat::Jpeg)),
            ("anim.gif", Some(ImageSourceFormat::Gif)),
            ("icon.SVG", Some(ImageSourceFormat::Svg)),
            ("wallpaper.WEBP", Some(ImageSourceFormat::Webp)),
        ] {
            assert_eq!(ImageSourceFormat::from_extension(Path::new(name)), expected);
        }
    }

    #[test]
    fn magic_bytes_select_format_regardless_of_extension() {
        assert_eq!(
            ImageSourceFormat::from_magic(&[0xff, 0xd8, 0xff, 0xe0]),
            Some(ImageSourceFormat::Jpeg)
        );
        assert_eq!(
            ImageSourceFormat::from_magic(b"GIF89a\0\0"),
            Some(ImageSourceFormat::Gif)
        );
        assert_eq!(ImageSourceFormat::from_magic(b"BM\0\0"), None);
        assert_eq!(ImageSourceFormat::from_magic(b"\xff\xd8"), None);
    }

    #[test]
    fn unpremultiply_handles_transparent_and_opaque() {
        assert_eq!(
            unpremultiply_swapped_to_rgba(&[10, 20, 30, 0]),
            vec![0, 0, 0, 0]
        );
        assert_eq!(
            unpremultiply_swapped_to_rgba(&[10, 20, 30, 255]),
            vec![10, 20, 30, 255]
        );
    }
}
