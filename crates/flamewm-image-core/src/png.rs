//! PNG decode via tiny-skia. Alpha is preserved.

use crate::{RgbaImage, check_size_limits, unpremultiply_swapped_in_place};

/// Decodes PNG at natural size, preserving alpha.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    // tiny-skia allocates from IHDR before returning, so gate the pixel budget on the header.
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        check_size_limits(width, height)?;
    }
    let source = resvg::tiny_skia::Pixmap::decode_png(bytes).map_err(|error| error.to_string())?;
    if source.width() == 0 || source.height() == 0 {
        return Err("PNG dimensions must be non-zero".to_string());
    }
    let width = source.width();
    let height = source.height();
    let mut pixels = source.take();
    unpremultiply_swapped_in_place(&mut pixels);
    RgbaImage::from_rgba8(width, height, pixels)
        .ok_or_else(|| "PNG dimensions overflow".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_fixture_with_alpha() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/branding/flamewm-wordmark.png");
        let bytes = std::fs::read(path).expect("PNG fixture");
        let image = decode(&bytes).unwrap();
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize * 4
        );
        assert!(
            image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 0 && pixel[..3] != [0, 0, 0])
        );
    }

    #[test]
    fn rejects_invalid_png() {
        assert!(decode(b"not png").is_err());
    }

    #[test]
    fn rejects_header_pixel_bomb_before_tiny_skia_allocates() {
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&20_000u32.to_be_bytes());
        png.extend_from_slice(&[0; 5]);
        let error = decode(&png).expect_err("oversized PNG is rejected");
        assert!(error.contains("pixel decode budget"), "{error}");
    }
}
