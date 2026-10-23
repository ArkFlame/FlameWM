//! Shared deterministic image decode core (J01).
//!
//! Single owner for PPM/SVG/PNG/WebP/JPEG/GIF decoding into straight (non-premultiplied)
//! RGBA8. PPM and JPEG inputs are always opaque (alpha=255); SVG/PNG/WebP/GIF preserve alpha.
//! Decoded dimensions are limited only by the source format, platform address
//! space, and available memory.

use std::path::Path;

pub mod blur;
pub use blur::{
    BlurError, BlurScratch, Rect, frosted_backdrop, gaussian_blur_rgba, gaussian_blur_rgba_with,
};
pub mod gif_frame;
pub mod ico;
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

/// Image formats that are *containers* of other images rather than one bitmap stream.
///
/// `ImageSourceFormat` stays a closed set of single-bitmap decoders (its variants are matched
/// exhaustively by other crates), so the multi-image icon container is probed and decoded through
/// this sibling type and [`decode_icon_container_at`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WrappedImageFormat {
    /// Windows ICO/CUR icon container: an index of PNG or DIB images.
    Ico,
}

impl WrappedImageFormat {
    /// Identifies a container format from its leading magic bytes.
    #[must_use]
    pub fn from_magic(bytes: &[u8]) -> Option<Self> {
        ico::is_icon(bytes).then_some(Self::Ico)
    }

    /// Identifies a container format from the file extension only.
    #[must_use]
    pub fn from_extension(path: &Path) -> Option<Self> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        matches!(extension.as_str(), "ico" | "cur").then_some(Self::Ico)
    }

    /// Canvas size of the container's largest image, read without decoding pixels.
    #[must_use]
    pub fn dimensions(self, bytes: &[u8]) -> Option<(u32, u32)> {
        match self {
            Self::Ico => ico::dimensions(bytes),
        }
    }
}

/// Decodes a wrapped container image and scales it to an exact target size (SVG-style aspect
/// handling is not applied: the caller already fitted the box).
pub fn decode_icon_container_at(
    bytes: &[u8],
    format: WrappedImageFormat,
    width: u32,
    height: u32,
    filter: scale::ScaleFilter,
) -> Result<RgbaImage, String> {
    check_size_limits(width, height)?;
    match format {
        WrappedImageFormat::Ico => {
            let natural = ico::decode(bytes)?;
            if natural.width == width && natural.height == height {
                return Ok(natural);
            }
            scale::scale_rgba(&natural, width, height, filter)
                .ok_or_else(|| "requested dimensions overflow".to_string())
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
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|_| ())
        .ok_or_else(|| format!("image dimensions {width}x{height} overflow addressable memory"))
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

/// Converts tiny-skia premultiplied pixel bytes to straight RGBA8 in place.
///
/// tiny-skia pixmaps are RGBA-ordered (data[0]->R, data[1]->G, data[2]->B)
/// with premultiplied color channels. Fully transparent pixels stay
/// (0,0,0,0); no color-key substitution. Working in place avoids a second
/// full-size buffer for every decoded PNG and SVG raster.
pub fn unpremultiply_swapped_in_place(pixels: &mut [u8]) {
    for rgba in pixels.chunks_exact_mut(4) {
        let a = u16::from(rgba[3]);
        if a == 0 {
            rgba.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let straight = |c: u8| ((u16::from(c) * 255 + a / 2) / a).min(255) as u8;
        rgba[0] = straight(rgba[0]);
        rgba[1] = straight(rgba[1]);
        rgba[2] = straight(rgba[2]);
    }
}

/// Expands `count` packed RGB pixels at the front of `pixels` to RGBA with alpha 255.
/// Walks backwards: each destination pixel starts at or after its source, and later
/// destinations never overwrite source bytes that are still unread.
pub(crate) fn expand_rgb_to_rgba_in_place(pixels: &mut [u8], count: usize) {
    for index in (0..count).rev() {
        let (src, dst) = (index * 3, index * 4);
        let rgb = [pixels[src], pixels[src + 1], pixels[src + 2]];
        pixels[dst..dst + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_rgb_in_place_with_opaque_alpha() {
        let mut pixels = vec![0u8; 4 * 3];
        pixels[..9].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        expand_rgb_to_rgba_in_place(&mut pixels, 3);
        assert_eq!(pixels, [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255]);
    }

    #[test]
    fn dimensions_have_no_arbitrary_cap_but_reject_zero_and_overflow() {
        assert!(check_size_limits(0, 8).is_err());
        assert!(check_size_limits(4097, 8).is_ok());
        assert!(check_size_limits(4096, 4096).is_ok());
        assert!(check_size_limits(8192, 8192).is_ok());
        assert!(check_size_limits(u32::MAX, u32::MAX).is_err());
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
    fn icon_container_probe_covers_magic_and_extension() {
        // ICONDIR with one 16x16 entry, no pixels: enough for the header probe.
        let header = [
            0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0, 1, 0, 0, 0, 22, 0, 0, 0, 0,
        ];
        assert_eq!(
            WrappedImageFormat::from_magic(&header),
            Some(WrappedImageFormat::Ico)
        );
        assert_eq!(
            WrappedImageFormat::from_extension(Path::new("app.ICO")),
            Some(WrappedImageFormat::Ico)
        );
        assert_eq!(
            WrappedImageFormat::from_extension(Path::new("app.png")),
            None
        );
        assert_eq!(WrappedImageFormat::from_magic(b"PNG\r\n"), None);
        assert_eq!(WrappedImageFormat::Ico.dimensions(&header), Some((16, 16)));
        // A container with no readable entry still has a header but no dimensions.
        assert_eq!(
            WrappedImageFormat::Ico.dimensions(&[0, 0, 1, 0, 0, 0]),
            None
        );
    }

    #[test]
    fn unpremultiply_handles_transparent_and_opaque() {
        let mut transparent = [10, 20, 30, 0];
        unpremultiply_swapped_in_place(&mut transparent);
        assert_eq!(transparent, [0, 0, 0, 0]);
        let mut opaque = [10, 20, 30, 255];
        unpremultiply_swapped_in_place(&mut opaque);
        assert_eq!(opaque, [10, 20, 30, 255]);
    }
}
