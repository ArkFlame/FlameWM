//! Bounded WebP decode into the shared straight-alpha RGBA8 image contract.

use std::io::Cursor;

use crate::{RgbaImage, check_size_limits, expand_rgb_to_rgba_in_place};

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
    if has_alpha {
        let mut pixels = vec![0; source_len];
        decoder
            .read_image(&mut pixels)
            .map_err(|error| error.to_string())?;
        RgbaImage::from_rgba8(width, height, pixels)
            .ok_or_else(|| "WebP dimensions overflow".to_string())
    } else {
        // Decode RGB into the front of the final RGBA buffer, then expand in place
        // so no separate RGB temporary is kept alongside the output.
        let mut pixels = vec![0; source_len / 3 * 4];
        decoder
            .read_image(&mut pixels[..source_len])
            .map_err(|error| error.to_string())?;
        expand_rgb_to_rgba_in_place(&mut pixels, source_len / 3);
        RgbaImage::from_rgba8(width, height, pixels)
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
