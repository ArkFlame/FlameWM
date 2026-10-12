//! Bounded baseline JPEG decode into the shared straight-alpha RGBA8 image contract.
//!
//! Progressive, arithmetic, lossless and CMYK/YCCK JPEGs are rejected with an
//! error. Output is always opaque.

use std::io::Cursor;

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};

use crate::{MAX_DIMENSION, RgbaImage, check_size_limits};

/// Decodes a baseline or extended-sequential JPEG at natural size.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    reject_unsupported_frame(bytes)?;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(MAX_DIMENSION as usize)
        .set_max_height(MAX_DIMENSION as usize);
    let mut decoder = JpegDecoder::new_with_options(Cursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|error| error.to_string())?;
    let info = decoder
        .info()
        .ok_or_else(|| "JPEG headers missing".to_string())?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    check_size_limits(width, height)?;
    if !matches!(info.components, 1 | 3) {
        return Err(format!(
            "unsupported JPEG component count {}",
            info.components
        ));
    }
    let rgb = decoder.decode().map_err(|error| error.to_string())?;
    RgbaImage::from_rgb8_opaque(width, height, &rgb)
        .ok_or_else(|| "JPEG pixel buffer does not match its dimensions".to_string())
}

/// Walks the segment list up to the first frame header and accepts only
/// baseline (SOF0) or extended sequential Huffman (SOF1) frames.
fn reject_unsupported_frame(bytes: &[u8]) -> Result<(), String> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err("JPEG SOI marker missing".to_string());
    }
    let mut pos = 2;
    loop {
        while bytes.get(pos) == Some(&0xff) && bytes.get(pos + 1) == Some(&0xff) {
            pos += 1;
        }
        if bytes.get(pos) != Some(&0xff) {
            return Err("malformed JPEG marker stream".to_string());
        }
        let marker = *bytes
            .get(pos + 1)
            .ok_or_else(|| "truncated JPEG marker".to_string())?;
        match marker {
            0xc0 | 0xc1 => return Ok(()),
            0xc2..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                return Err(format!("unsupported JPEG frame type 0xff{marker:02x}"));
            }
            0xd9 | 0xda => return Err("JPEG has no baseline frame header".to_string()),
            0x01 | 0xd0..=0xd8 => pos += 2,
            _ => {
                let length = bytes
                    .get(pos + 2..pos + 4)
                    .map(|length| usize::from(u16::from_be_bytes([length[0], length[1]])))
                    .ok_or_else(|| "truncated JPEG segment".to_string())?;
                if length < 2 {
                    return Err("malformed JPEG segment length".to_string());
                }
                pos += 2 + length;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE: &[u8] = include_bytes!("../tests/fixtures/baseline-16x16.jpg");
    const GRAY: &[u8] = include_bytes!("../tests/fixtures/gray-16x16.jpg");
    const PROGRESSIVE: &[u8] = include_bytes!("../tests/fixtures/progressive-16x16.jpg");
    const CMYK: &[u8] = include_bytes!("../tests/fixtures/cmyk-16x16.jpg");
    const WIDE: &[u8] = include_bytes!("../tests/fixtures/wide-4097x1.jpg");

    fn assert_near(actual: [u8; 4], expected: [u8; 3]) {
        for channel in 0..3 {
            let delta = i32::from(actual[channel]) - i32::from(expected[channel]);
            assert!(delta.abs() <= 8, "channel {channel}: {actual:?} vs {expected:?}");
        }
        assert_eq!(actual[3], 255);
    }

    #[test]
    fn decodes_baseline_fixture_with_dims_and_colour() {
        let image = decode(BASELINE).expect("baseline JPEG");
        assert_eq!((image.width, image.height), (16, 16));
        assert!(image.is_fully_opaque());
        assert_near([image.pixels[0], image.pixels[1], image.pixels[2], image.pixels[3]], [200, 30, 40]);
        let last = image.pixels.len() - 4;
        assert_near([image.pixels[last], image.pixels[last + 1], image.pixels[last + 2], image.pixels[last + 3]], [200, 30, 40]);
    }

    #[test]
    fn decodes_grayscale_fixture_as_opaque_rgb() {
        let image = decode(GRAY).expect("grayscale JPEG");
        assert_eq!((image.width, image.height), (16, 16));
        assert_near([image.pixels[0], image.pixels[1], image.pixels[2], image.pixels[3]], [128, 128, 128]);
    }

    #[test]
    fn rejects_truncated_and_corrupt_jpeg_without_panicking() {
        for cut in [0, 1, 2, 3, 10, 100, BASELINE.len() / 2, BASELINE.len() - 2] {
            let _ = decode(&BASELINE[..cut]);
        }
        assert!(decode(&BASELINE[..BASELINE.len() / 2]).is_err());
        assert!(decode(b"\xff\xd8\xff").is_err());
        assert!(decode(b"not jpeg").is_err());
    }

    #[test]
    fn rejects_progressive_and_cmyk_jpeg() {
        assert!(decode(PROGRESSIVE).is_err());
        assert!(decode(CMYK).is_err());
    }

    #[test]
    fn rejects_oversize_dimensions() {
        assert!(decode(WIDE).is_err());
    }
}
