//! Shared wallpaper placement: decodes a wallpaper file at natural size and composes it onto a
//! root-sized canvas. Single owner for both the desktop wallpaper worker and the shell's
//! pseudo-transparency backdrop, so both see identical pixels.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use flamewm_api::background::WallpaperFit;

use crate::scale::{ScaleFilter, scale_rgba};
use crate::{ImageSourceFormat, RgbaImage, check_size_limits};

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
/// dimension overflow. Full-canvas composition keeps the scale-then-blit form (no per-pixel taps),
/// so it costs the same as the original `compose`; [`compose_region`] must match it pixel for pixel.
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
    match layer(natural, width, height, fit)? {
        Layer::Tiled => compose_region(natural, width, height, fit, (0, 0, width, height)),
        Layer::Scaled {
            width: sw,
            height: sh,
            x,
            y,
        } => {
            let scaled = if (sw, sh) == (natural.width, natural.height) {
                Cow::Borrowed(natural)
            } else {
                Cow::Owned(scale_rgba(natural, sw, sh, ScaleFilter::Area)?)
            };
            // A layer covering the whole canvas is the result itself: no fill and no blit.
            if (x, y) == (0, 0) && (sw, sh) == (width, height) {
                return Some(scaled.into_owned());
            }
            let mut canvas = solid(width, height, BACKGROUND)?;
            blit(&mut canvas, &scaled, x, y);
            Some(canvas)
        }
    }
}

/// Composes only the `rect` (x, y, width, height) of the `root_w` x `root_h` canvas that
/// [`compose`] would produce, without allocating the rest of the canvas. Pixel-identical to
/// cropping `compose` to `rect`. `None` for zero-sized inputs, a zero-sized or out-of-root `rect`,
/// or dimension overflow.
#[must_use]
pub fn compose_region(
    natural: &RgbaImage,
    root_w: u32,
    root_h: u32,
    fit: WallpaperFit,
    rect: (i32, i32, u32, u32),
) -> Option<RgbaImage> {
    if root_w == 0 || root_h == 0 || natural.width == 0 || natural.height == 0 {
        return None;
    }
    let (rx, ry, width, height) = rect;
    let x0 = u32::try_from(rx).ok()?;
    let y0 = u32::try_from(ry).ok()?;
    if width == 0
        || height == 0
        || x0.checked_add(width)? > root_w
        || y0.checked_add(height)? > root_h
    {
        return None;
    }
    let (nw, nh) = (u64::from(natural.width), u64::from(natural.height));
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    match layer(natural, root_w, root_h, fit)? {
        Layer::Tiled => {
            for y in y0..y0 + height {
                for x in x0..x0 + width {
                    let source = (((u64::from(y) % nh) * nw + u64::from(x) % nw) * 4) as usize;
                    pixels.extend_from_slice(&natural.pixels[source..source + 4]);
                }
            }
        }
        Layer::Scaled {
            width: sw,
            height: sh,
            x,
            y,
        } => {
            // Per canvas row/column: the area taps into the scaled layer, or `None` outside it.
            let taps = |start: u32, len: u32, origin: i64, src: u64, dst: u64| {
                (start..start + len)
                    .map(|c| {
                        let s = i64::from(c) - origin;
                        (0..dst as i64)
                            .contains(&s)
                            .then(|| area_tap(src as usize, dst as usize, s as usize))
                    })
                    .collect::<Vec<_>>()
            };
            let column_taps = taps(x0, width, x, nw, u64::from(sw));
            let row_taps = taps(y0, height, y, nh, u64::from(sh));
            for row in &row_taps {
                for column in &column_taps {
                    match (row, column) {
                        (Some(row), Some(column)) => {
                            pixels.extend_from_slice(&area_pixel(natural, column, row));
                        }
                        _ => pixels.extend_from_slice(&BACKGROUND),
                    }
                }
            }
        }
    }
    Some(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// How the natural image maps onto the root canvas for one fit.
#[derive(Clone, Copy)]
enum Layer {
    /// Natural area-resampled to `width` x `height` (1:1 when equal), placed at (`x`, `y`);
    /// canvas pixels outside it are `BACKGROUND`.
    Scaled {
        width: u32,
        height: u32,
        x: i64,
        y: i64,
    },
    /// Natural repeated across the whole canvas.
    Tiled,
}

fn layer(natural: &RgbaImage, width: u32, height: u32, fit: WallpaperFit) -> Option<Layer> {
    let (tw, th) = (u64::from(width), u64::from(height));
    let (nw, nh) = (u64::from(natural.width), u64::from(natural.height));
    let scaled = |sw: u64, sh: u64, x: i64, y: i64| {
        let (w, h) = (u32::try_from(sw).ok()?, u32::try_from(sh).ok()?);
        // A resampled intermediate must fit the image limits, as the `scale_rgba` path it replaces.
        if (w, h) != (natural.width, natural.height) {
            check_size_limits(w, h).ok()?;
        }
        Some(Layer::Scaled {
            width: w,
            height: h,
            x,
            y,
        })
    };
    match fit {
        WallpaperFit::Stretch => scaled(tw, th, 0, 0),
        WallpaperFit::Fill => {
            // Cover: the scaled image reaches both target edges and the overflow is cropped.
            let (sw, sh) = if tw * nh >= th * nw {
                (tw, (nh * tw).div_ceil(nw))
            } else {
                ((nw * th).div_ceil(nh), th)
            };
            scaled(sw, sh, centred(tw, sw), centred(th, sh))
        }
        WallpaperFit::Fit => {
            // Contain: the scaled image reaches the nearer target edge; the rest is letterboxed.
            let (dw, dh) = if tw * nh <= th * nw {
                (tw, (nh * tw / nw).max(1))
            } else {
                ((nw * th / nh).max(1), th)
            };
            scaled(dw, dh, centred(tw, dw), centred(th, dh))
        }
        WallpaperFit::Center => scaled(nw, nh, centred(tw, nw), centred(th, nh)),
        WallpaperFit::Tile => Some(Layer::Tiled),
    }
}

/// Area-filter taps for destination index `destination` of a `src_len` -> `dst_len` axis: the
/// first covered source index and the normalised coverage weights for the consecutive indices
/// from there. Per-index form of the `Area` filter in `scale.rs`, so a sub-range of the scaled
/// layer needs only its own taps.
fn area_tap(src_len: usize, dst_len: usize, destination: usize) -> (usize, Vec<f32>) {
    let ratio = src_len as f64 / dst_len as f64;
    let start = destination as f64 * ratio;
    let end = (destination + 1) as f64 * ratio;
    let first = start.floor() as usize;
    let last = (end.ceil() as usize).min(src_len);
    let mut weights: Vec<f32> = (first..last)
        .map(|index| {
            let lo = start.max(index as f64);
            let hi = end.min((index + 1) as f64);
            (hi - lo).max(0.0) as f32
        })
        .collect();
    let total: f32 = weights.iter().sum();
    if total > 0.0 {
        for weight in &mut weights {
            *weight /= total;
        }
    }
    (first, weights)
}

/// One output pixel of the area-filtered natural image, accumulated in the same order and
/// precision as `scale::scale_rgba` with `ScaleFilter::Area`.
fn area_pixel(natural: &RgbaImage, column: &(usize, Vec<f32>), row: &(usize, Vec<f32>)) -> [u8; 4] {
    let src_w = natural.width as usize;
    let mut acc = [0f32; 4];
    for (row_offset, row_weight) in row.1.iter().enumerate() {
        let base = (row.0 + row_offset) * src_w;
        for (col_offset, col_weight) in column.1.iter().enumerate() {
            let weight = row_weight * col_weight;
            let index = (base + column.0 + col_offset) * 4;
            for channel in 0..4 {
                acc[channel] += weight * f32::from(natural.pixels[index + channel]);
            }
        }
    }
    acc.map(|value| value.round().clamp(0.0, 255.0) as u8)
}

/// Offset that centres an `inner` span within an `outer` span. Negative when `inner` overflows.
fn centred(outer: u64, inner: u64) -> i64 {
    (outer as i64 - inner as i64) / 2
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
mod region_tests {
    use super::*;
    use crate::scale::{ScaleFilter, scale_rgba};

    const FITS: [WallpaperFit; 5] = [
        WallpaperFit::Fill,
        WallpaperFit::Fit,
        WallpaperFit::Stretch,
        WallpaperFit::Center,
        WallpaperFit::Tile,
    ];

    /// Pre-region `compose`, kept verbatim as the golden oracle.
    fn reference_compose(
        natural: &RgbaImage,
        width: u32,
        height: u32,
        fit: WallpaperFit,
    ) -> Option<RgbaImage> {
        fn resize(source: &RgbaImage, width: u32, height: u32) -> Option<RgbaImage> {
            if source.width == width && source.height == height {
                return Some(source.clone());
            }
            scale_rgba(source, width, height, ScaleFilter::Area)
        }
        fn solid(width: u32, height: u32, colour: [u8; 4]) -> RgbaImage {
            RgbaImage {
                width,
                height,
                pixels: colour
                    .iter()
                    .copied()
                    .cycle()
                    .take(width as usize * height as usize * 4)
                    .collect(),
            }
        }
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
                let s = ((y - dy) as usize * source.width as usize + (x0 - dx) as usize) * 4;
                let t = (y as usize * canvas.width as usize + x0 as usize) * 4;
                canvas.pixels[t..t + row_bytes].copy_from_slice(&source.pixels[s..s + row_bytes]);
            }
        }
        let (tw, th) = (u64::from(width), u64::from(height));
        let (nw, nh) = (u64::from(natural.width), u64::from(natural.height));
        match fit {
            WallpaperFit::Stretch => resize(natural, width, height),
            WallpaperFit::Fill => {
                let (sw, sh) = if tw * nh >= th * nw {
                    (tw, (nh * tw).div_ceil(nw))
                } else {
                    ((nw * th).div_ceil(nh), th)
                };
                let scaled = resize(natural, sw as u32, sh as u32)?;
                let mut canvas = solid(width, height, BACKGROUND);
                blit(&mut canvas, &scaled, centred(tw, sw), centred(th, sh));
                Some(canvas)
            }
            WallpaperFit::Fit => {
                let (dw, dh) = if tw * nh <= th * nw {
                    (tw, (nh * tw / nw).max(1))
                } else {
                    ((nw * th / nh).max(1), th)
                };
                let fitted = resize(natural, dw as u32, dh as u32)?;
                let mut canvas = solid(width, height, BACKGROUND);
                blit(&mut canvas, &fitted, centred(tw, dw), centred(th, dh));
                Some(canvas)
            }
            WallpaperFit::Center => {
                let mut canvas = solid(width, height, BACKGROUND);
                blit(&mut canvas, natural, centred(tw, nw), centred(th, nh));
                Some(canvas)
            }
            WallpaperFit::Tile => {
                let mut canvas = solid(width, height, BACKGROUND);
                for y in 0..th {
                    for x in 0..tw {
                        let s = (((y % nh) * nw + (x % nw)) * 4) as usize;
                        let t = ((y * tw + x) * 4) as usize;
                        canvas.pixels[t..t + 4].copy_from_slice(&natural.pixels[s..s + 4]);
                    }
                }
                Some(canvas)
            }
        }
    }

    fn crop(canvas: &RgbaImage, rect: (i32, i32, u32, u32)) -> RgbaImage {
        let (x0, y0, w, h) = (
            rect.0 as usize,
            rect.1 as usize,
            rect.2 as usize,
            rect.3 as usize,
        );
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in y0..y0 + h {
            let start = (y * canvas.width as usize + x0) * 4;
            pixels.extend_from_slice(&canvas.pixels[start..start + w * 4]);
        }
        RgbaImage {
            width: rect.2,
            height: rect.3,
            pixels,
        }
    }

    /// Non-uniform, partly transparent source so every channel and rounding path is exercised.
    fn natural(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let v = x
                    .wrapping_mul(2_654_435_761)
                    .wrapping_add(y.wrapping_mul(40_503));
                pixels.extend_from_slice(&[
                    (v & 0xff) as u8,
                    ((v >> 8) & 0xff) as u8,
                    ((v >> 16) ^ (x * y)) as u8,
                    if (x + y) % 5 == 0 { 0x80 } else { 0xff },
                ]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    /// Rects per root: bottom bar, offset margin, full, 1px corner, odd size, origin 1px.
    fn rects(root: (u32, u32)) -> Vec<(i32, i32, u32, u32)> {
        let (w, h) = (root.0 as i32, root.1 as i32);
        vec![
            (0, h - 44, root.0, 44),
            (13, 7, root.0 - 26, root.1 / 3 + 1),
            (0, 0, root.0, root.1),
            (w - 1, h - 1, 1, 1),
            (1, 2, 3, 5),
            (0, 0, 1, 1),
        ]
    }

    #[test]
    fn compose_region_matches_cropped_compose_for_every_fit_root_rect_and_natural() {
        // Debug-profile cost is per output pixel (~1 us each), so roots stay small: every fit, all
        // three roots and all rects are still covered for each natural. Full-size 1080p/4K timing
        // lives in `bench_compose_vs_reference_full_root` (release).
        let roots = [(320, 180), (500, 167), (97, 333)];
        let cases = [
            ((64, 64), roots.to_vec()),
            ((700, 300), roots.to_vec()),
            ((800, 450), roots.to_vec()),
            ((1920, 1080), vec![(97, 333)]),
        ];
        for ((nw, nh), roots) in cases {
            let source = natural(nw, nh);
            for root in roots {
                for fit in FITS {
                    let golden = reference_compose(&source, root.0, root.1, fit);
                    assert_eq!(
                        compose(&source, root.0, root.1, fit),
                        golden,
                        "compose {nw}x{nh} root {root:?} {fit:?}"
                    );
                    for rect in rects(root) {
                        let got = compose_region(&source, root.0, root.1, fit, rect);
                        assert_eq!(
                            got,
                            golden.as_ref().map(|canvas| crop(canvas, rect)),
                            "{nw}x{nh} root {root:?} {fit:?} rect {rect:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn compose_region_rejects_rects_outside_root_like_the_crop_did() {
        let source = natural(64, 64);
        for fit in FITS {
            for rect in [
                (0, 100, 160, 44),
                (-1, 0, 10, 10),
                (0, 0, 0, 5),
                (0, 0, 5, 0),
                (150, 0, 11, 1),
            ] {
                assert!(
                    compose_region(&source, 160, 120, fit, rect).is_none(),
                    "{fit:?} {rect:?}"
                );
            }
        }
    }

    /// Release timing sanity, not part of the default run:
    /// `cargo test --release -p flamewm-image-core bench_compose_vs_reference -- --ignored --nocapture`.
    /// Prints the median of 9 runs per fit for `compose` and for `reference_compose` (the verbatim
    /// pre-region `compose`, i.e. the HEAD baseline).
    #[test]
    #[ignore = "release timing sanity; run with --release -- --ignored --nocapture"]
    fn bench_compose_vs_reference_full_root() {
        fn median_ms(mut run: impl FnMut()) -> f64 {
            let mut samples: Vec<f64> = (0..9)
                .map(|_| {
                    let start = std::time::Instant::now();
                    run();
                    start.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            samples.sort_by(f64::total_cmp);
            samples[4]
        }
        // (label, natural size, root size): same-size identity, 4K downscaled to 1080p, 4K root.
        for (label, (nw, nh), (w, h)) in [
            ("1080p nat=root", (1920, 1080), (1920, 1080)),
            ("4k->1080p", (3840, 2160), (1920, 1080)),
            ("4k nat=root", (3840, 2160), (3840, 2160)),
        ] {
            let source = natural(nw, nh);
            for fit in FITS {
                let ours = median_ms(|| {
                    std::hint::black_box(compose(&source, w, h, fit));
                });
                let baseline = median_ms(|| {
                    std::hint::black_box(reference_compose(&source, w, h, fit));
                });
                println!("{label} {fit:?} compose {ours:.1} ms baseline {baseline:.1} ms");
            }
        }
    }
}
