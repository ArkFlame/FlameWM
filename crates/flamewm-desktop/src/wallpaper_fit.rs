//! Pure wallpaper placement: composes a natural-size decoded image onto a root-sized canvas.

use flamewm_api::background::WallpaperFit;
use flamewm_image_core::RgbaImage;
use flamewm_image_core::scale::{ScaleFilter, scale_rgba};

/// Opaque black: the desktop background colour from `project_background`. Used for letterbox
/// bars in `Fit` and for margins around a smaller image in `Center`.
const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

/// Parses the `wallpaperFit` wire value. Unknown or empty values keep the `Fill` default.
#[must_use]
pub fn parse(value: &str) -> WallpaperFit {
    match value {
        "fit" => WallpaperFit::Fit,
        "stretch" => WallpaperFit::Stretch,
        "center" => WallpaperFit::Center,
        "tile" => WallpaperFit::Tile,
        _ => WallpaperFit::Fill,
    }
}

/// Composes `natural` onto a `width` x `height` canvas. Returns `None` for zero-sized inputs or
/// dimension overflow.
#[must_use]
pub fn compose(
    natural: &RgbaImage,
    width: u32,
    height: u32,
    fit: WallpaperFit,
) -> Option<RgbaImage> {
    if width == 0 || height == 0 || natural.width == 0 || natural.height == 0 {
        return None;
    }
    let (tw, th) = (u64::from(width), u64::from(height));
    let (nw, nh) = (u64::from(natural.width), u64::from(natural.height));
    match fit {
        WallpaperFit::Stretch => resize(natural, width, height),
        WallpaperFit::Fill => {
            // Cover: the scaled image reaches both target edges and the overflow is cropped.
            let (sw, sh) = if tw * nh >= th * nw {
                (tw, (nh * tw).div_ceil(nw))
            } else {
                ((nw * th).div_ceil(nh), th)
            };
            let scaled = resize(natural, u32::try_from(sw).ok()?, u32::try_from(sh).ok()?)?;
            let mut canvas = solid(width, height, BACKGROUND)?;
            blit(&mut canvas, &scaled, centred(tw, sw), centred(th, sh));
            Some(canvas)
        }
        WallpaperFit::Fit => {
            // Contain: the scaled image reaches the nearer target edge; the rest is letterboxed.
            let (dw, dh) = if tw * nh <= th * nw {
                (tw, (nh * tw / nw).max(1))
            } else {
                ((nw * th / nh).max(1), th)
            };
            let fitted = resize(natural, u32::try_from(dw).ok()?, u32::try_from(dh).ok()?)?;
            let mut canvas = solid(width, height, BACKGROUND)?;
            blit(&mut canvas, &fitted, centred(tw, dw), centred(th, dh));
            Some(canvas)
        }
        WallpaperFit::Center => {
            let mut canvas = solid(width, height, BACKGROUND)?;
            blit(&mut canvas, natural, centred(tw, nw), centred(th, nh));
            Some(canvas)
        }
        WallpaperFit::Tile => {
            let mut canvas = solid(width, height, BACKGROUND)?;
            for y in 0..th {
                for x in 0..tw {
                    let source = (((y % nh) * nw + (x % nw)) * 4) as usize;
                    let target = ((y * tw + x) * 4) as usize;
                    canvas.pixels[target..target + 4]
                        .copy_from_slice(&natural.pixels[source..source + 4]);
                }
            }
            Some(canvas)
        }
    }
}

/// Offset that centres an `inner` span within an `outer` span. Negative when `inner` overflows.
fn centred(outer: u64, inner: u64) -> i64 {
    (outer as i64 - inner as i64) / 2
}

fn resize(source: &RgbaImage, width: u32, height: u32) -> Option<RgbaImage> {
    if source.width == width && source.height == height {
        return Some(source.clone());
    }
    scale_rgba(source, width, height, ScaleFilter::Bilinear)
}

fn solid(width: u32, height: u32, colour: [u8; 4]) -> Option<RgbaImage> {
    let count = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    Some(RgbaImage {
        width,
        height,
        pixels: colour.iter().copied().cycle().take(count).collect(),
    })
}

/// Copies `source` onto `canvas` at (`dx`, `dy`), clipping anything outside the canvas.
fn blit(canvas: &mut RgbaImage, source: &RgbaImage, dx: i64, dy: i64) {
    let x0 = dx.max(0);
    let y0 = dy.max(0);
    let x1 = (dx + i64::from(source.width)).min(i64::from(canvas.width));
    let y1 = (dy + i64::from(source.height)).min(i64::from(canvas.height));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let row_bytes = (x1 - x0) as usize * 4;
    for y in y0..y1 {
        let source_start = ((y - dy) as usize * source.width as usize + (x0 - dx) as usize) * 4;
        let target_start = (y as usize * canvas.width as usize + x0 as usize) * 4;
        canvas.pixels[target_start..target_start + row_bytes]
            .copy_from_slice(&source.pixels[source_start..source_start + row_bytes]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4x2 image: red encodes the column (0, 60, 120, 180), green encodes the row (0, 100).
    fn columns_image() -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                pixels.extend_from_slice(&[x * 60, y * 100, 0, 255]);
            }
        }
        RgbaImage {
            width: 4,
            height: 2,
            pixels,
        }
    }

    fn at(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * image.width + x) * 4) as usize;
        image.pixels[index..index + 4].try_into().expect("pixel")
    }

    #[test]
    fn parse_maps_wire_values_and_defaults_to_fill() {
        assert_eq!(parse("fill"), WallpaperFit::Fill);
        assert_eq!(parse("fit"), WallpaperFit::Fit);
        assert_eq!(parse("stretch"), WallpaperFit::Stretch);
        assert_eq!(parse("center"), WallpaperFit::Center);
        assert_eq!(parse("tile"), WallpaperFit::Tile);
        assert_eq!(parse(""), WallpaperFit::Fill);
        assert_eq!(parse("Tile"), WallpaperFit::Fill);
        assert_eq!(parse("zoom"), WallpaperFit::Fill);
    }

    #[test]
    fn fill_crops_the_overflow_evenly_without_stretching() {
        // 4x2 into 2x2 covers by height; the middle two columns survive unscaled.
        let image = compose(&columns_image(), 2, 2, WallpaperFit::Fill).expect("fill");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(at(&image, 0, 0), [60, 0, 0, 255]);
        assert_eq!(at(&image, 1, 0), [120, 0, 0, 255]);
        assert_eq!(at(&image, 0, 1), [60, 100, 0, 255]);
        assert_eq!(at(&image, 1, 1), [120, 100, 0, 255]);
    }

    #[test]
    fn fit_letterboxes_in_the_desktop_background_colour() {
        // 4x2 into 4x4 keeps the aspect ratio: one black bar above and below.
        let image = compose(&columns_image(), 4, 4, WallpaperFit::Fit).expect("fit");
        assert_eq!(at(&image, 0, 0), BACKGROUND);
        assert_eq!(at(&image, 3, 3), BACKGROUND);
        assert_eq!(at(&image, 2, 1), [120, 0, 0, 255]);
        assert_eq!(at(&image, 2, 2), [120, 100, 0, 255]);
    }

    #[test]
    fn stretch_fills_the_whole_target_and_differs_from_cover() {
        let stretched = compose(&columns_image(), 2, 2, WallpaperFit::Stretch).expect("stretch");
        let covered = compose(&columns_image(), 2, 2, WallpaperFit::Fill).expect("fill");
        assert_eq!((stretched.width, stretched.height), (2, 2));
        assert_ne!(stretched.pixels, covered.pixels);
    }

    #[test]
    fn center_keeps_natural_size_and_crops_when_larger() {
        let smaller = compose(&columns_image(), 6, 4, WallpaperFit::Center).expect("center");
        assert_eq!(at(&smaller, 0, 0), BACKGROUND);
        assert_eq!(at(&smaller, 0, 1), BACKGROUND);
        assert_eq!(at(&smaller, 2, 1), [60, 0, 0, 255]);
        assert_eq!(at(&smaller, 5, 1), BACKGROUND);

        let cropped = compose(&columns_image(), 2, 2, WallpaperFit::Center).expect("center crop");
        assert_eq!(at(&cropped, 0, 0), [60, 0, 0, 255]);
        assert_eq!(at(&cropped, 1, 1), [120, 100, 0, 255]);
    }

    #[test]
    fn tile_repeats_the_natural_image_from_the_origin() {
        let tiled = compose(&columns_image(), 6, 3, WallpaperFit::Tile).expect("tile");
        assert_eq!(at(&tiled, 0, 0), [0, 0, 0, 255]);
        assert_eq!(at(&tiled, 4, 0), [0, 0, 0, 255]);
        assert_eq!(at(&tiled, 5, 0), [60, 0, 0, 255]);
        assert_eq!(at(&tiled, 4, 1), [0, 100, 0, 255]);
        assert_eq!(at(&tiled, 5, 2), [60, 0, 0, 255]);
    }

    #[test]
    fn zero_sized_inputs_compose_to_nothing() {
        assert!(compose(&columns_image(), 0, 2, WallpaperFit::Fill).is_none());
        assert!(compose(&columns_image(), 2, 0, WallpaperFit::Tile).is_none());
    }
}
