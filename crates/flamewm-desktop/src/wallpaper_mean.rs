//! Mean colour of the desktop wallpaper, sampled on a 16x16 grid of cell centres. The desktop
//! computes it once per applied wallpaper from the buffer the loader already decoded, and publishes
//! it as the in-memory `wallpaperMeanRgb` derived setting. Glass ink reads it as the backdrop.

use crate::entry::wallpaper_loader::{WallpaperPresentation, WallpaperRaster};

/// Samples per axis. 16x16 keeps the cost to 256 pixel reads per wallpaper change.
pub(crate) const GRID: usize = 16;

/// Packed `0x00rrggbb` mean of the presented wallpaper. The default watermark is black.
#[must_use]
pub(crate) fn presentation_mean(presentation: &WallpaperPresentation) -> u32 {
    match presentation {
        WallpaperPresentation::Solid([r, g, b]) => {
            (u32::from(*r) << 16) | (u32::from(*g) << 8) | u32::from(*b)
        }
        WallpaperPresentation::DefaultBlackWatermark => 0,
        WallpaperPresentation::Image(raster) => raster_mean(raster),
    }
}

/// Mean of `GRID * GRID` centre samples of a straight RGBA8 raster. A malformed buffer yields black.
#[must_use]
pub(crate) fn raster_mean(raster: &WallpaperRaster) -> u32 {
    let (width, height) = (raster.width as usize, raster.height as usize);
    let pixels = raster.pixels.as_slice();
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return 0;
    }
    let mut sums = [0_u64; 3];
    for gy in 0..GRID {
        let y = (gy * 2 + 1) * height / (GRID * 2);
        for gx in 0..GRID {
            let x = (gx * 2 + 1) * width / (GRID * 2);
            let index = (y * width + x) * 4;
            for (channel, sum) in sums.iter_mut().enumerate() {
                *sum += u64::from(pixels[index + channel]);
            }
        }
    }
    let count = (GRID * GRID) as u64;
    let channel = |sum: u64| ((sum + count / 2) / count) as u32;
    (channel(sums[0]) << 16) | (channel(sums[1]) << 8) | channel(sums[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn raster(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> WallpaperRaster {
        let mut bytes = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&pixel(x, y));
            }
        }
        WallpaperRaster {
            source: "fixture".to_owned(),
            width,
            height,
            pixels: Arc::new(bytes),
        }
    }

    #[test]
    fn solid_wallpaper_mean_is_its_colour() {
        assert_eq!(
            presentation_mean(&WallpaperPresentation::Solid([0x12, 0x34, 0x56])),
            0x0012_3456
        );
        assert_eq!(
            presentation_mean(&WallpaperPresentation::DefaultBlackWatermark),
            0
        );
    }

    #[test]
    fn uniform_image_mean_is_its_colour() {
        let image = raster(64, 48, |_, _| [0xff, 0x80, 0x00, 0xff]);
        assert_eq!(raster_mean(&image), 0x00ff_8000);
    }

    #[test]
    fn half_black_half_white_image_averages_to_mid_grey() {
        let image = raster(64, 64, |x, _| {
            let v = if x < 32 { 0 } else { 255 };
            [v, v, v, 255]
        });
        assert_eq!(raster_mean(&image), 0x0080_8080);
    }

    #[test]
    fn short_buffer_yields_black_instead_of_panicking() {
        let mut image = raster(8, 8, |_, _| [255, 255, 255, 255]);
        Arc::get_mut(&mut image.pixels).unwrap().truncate(10);
        assert_eq!(raster_mean(&image), 0);
    }
}
