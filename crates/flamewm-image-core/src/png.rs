//! PNG decode via tiny-skia. Alpha is preserved.

use crate::{RgbaImage, unpremultiply_swapped_in_place};

/// Decodes PNG at natural size, preserving alpha.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
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
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/flamewm.png");
        let bytes = std::fs::read(path).expect("PNG fixture");
        let image = decode(&bytes).unwrap();
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize * 4
        );
        assert!(
            image
                .pixels
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[..3] != [0, 0, 0])
        );
    }

    #[test]
    fn rejects_invalid_png() {
        assert!(decode(b"not png").is_err());
    }
}
