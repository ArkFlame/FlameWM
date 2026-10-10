//! Bounded WebP decode into the shared straight-alpha RGBA8 image contract.

use std::io::Cursor;

use crate::{RgbaImage, check_size_limits};

/// Decodes the first WebP frame at natural size, preserving alpha when present.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let mut decoder =
        image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    check_size_limits(width, height)?;
    let has_alpha = decoder.has_alpha();
    let source_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(if has_alpha { 4 } else { 3 }))
        .ok_or_else(|| "WebP dimensions overflow".to_string())?;
    let mut source = vec![0; source_len];
    decoder
        .read_image(&mut source)
        .map_err(|error| error.to_string())?;
    if has_alpha {
        RgbaImage::from_rgba8(width, height, source)
            .ok_or_else(|| "WebP dimensions overflow".to_string())
    } else {
        RgbaImage::from_rgb8_opaque(width, height, &source)
            .ok_or_else(|| "WebP dimensions overflow".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_packaged_wallpaper_fixture() {
        let image = decode(include_bytes!(
            "../../../assets/branding/flamewm-wallpaper.webp"
        ))
        .expect("packaged WebP wallpaper fixture");
        assert!(image.width > 0 && image.height > 0);
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize * 4
        );
        assert!(image.is_fully_opaque());
        assert!(
            image
                .pixels
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != [0, 0, 0])
        );
    }

    #[test]
    fn rejects_invalid_webp() {
        assert!(decode(b"not webp").is_err());
    }
}
