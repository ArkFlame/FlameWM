//! Shared wallpaper placement: decodes a wallpaper file at natural size and composes it onto a
//! root-sized canvas. Single owner for both the desktop wallpaper worker and the shell's
//! pseudo-transparency backdrop, so both see identical pixels.

use std::fs;
use std::path::Path;

use flamewm_api::background::WallpaperFit;

use crate::scale::{ScaleFilter, scale_rgba};
use crate::{ImageSourceFormat, RgbaImage};

/// Opaque black: the desktop background colour from `project_background`. Used for letterbox
/// bars in `Fit` and for margins around a smaller image in `Center`.
pub const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

/// Packaged default wallpaper, relative to the packaged root; used when the setting is empty.
pub const DEFAULT_WALLPAPER_PATH: &str = "assets/wallpapers/fitz-roy-dawn.webp";

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

/// Decodes the wallpaper file at `path` to straight RGBA8 at natural size.
pub fn decode_file(path: &Path) -> Result<RgbaImage, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let format = ImageSourceFormat::from_extension(path)
        .ok_or_else(|| format!("unsupported wallpaper format: {}", path.display()))?;
    crate::decode_bytes(&bytes, format)
        .map_err(|message| format!("decode {}: {message}", path.display()))
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
    scale_rgba(source, width, height, ScaleFilter::Area)
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
