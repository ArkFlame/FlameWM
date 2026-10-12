//! Bounded GIF decode of the first frame into the shared straight-alpha RGBA8
//! image contract. The frame is placed on a transparent logical-screen canvas.

use crate::{RgbaImage, check_size_limits};

/// Decodes the first GIF frame at logical-screen size, preserving alpha.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(bytes)
        .map_err(|error| error.to_string())?;
    let (screen_width, screen_height) = (usize::from(decoder.width()), usize::from(decoder.height()));
    check_size_limits(screen_width as u32, screen_height as u32)?;
    let frame = decoder
        .read_next_frame()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "GIF has no frames".to_string())?;
    let (left, top) = (usize::from(frame.left), usize::from(frame.top));
    let frame_width = usize::from(frame.width);
    let columns = frame_width.min(screen_width.saturating_sub(left));
    let rows = usize::from(frame.height).min(screen_height.saturating_sub(top));
    let mut pixels = vec![0; screen_width * screen_height * 4];
    for row in 0..rows {
        let source = frame
            .buffer
            .get(row * frame_width * 4..row * frame_width * 4 + columns * 4)
            .ok_or_else(|| "GIF frame buffer is shorter than its dimensions".to_string())?;
        let start = ((top + row) * screen_width + left) * 4;
        pixels[start..start + columns * 4].copy_from_slice(source);
    }
    RgbaImage::from_rgba8(screen_width as u32, screen_height as u32, pixels)
        .ok_or_else(|| "GIF dimensions overflow".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_first_frame_fixture_with_colour() {
        let image = decode(include_bytes!("../tests/fixtures/first-frame-4x4.gif"))
            .expect("GIF fixture");
        assert_eq!((image.width, image.height), (4, 4));
        assert_eq!(&image.pixels[..4], &[255, 0, 0, 255]);
        assert!(image.is_fully_opaque());
    }

    #[test]
    fn rejects_invalid_and_truncated_gif() {
        assert!(decode(b"not gif").is_err());
        assert!(decode(&include_bytes!("../tests/fixtures/first-frame-4x4.gif")[..20]).is_err());
    }
}
