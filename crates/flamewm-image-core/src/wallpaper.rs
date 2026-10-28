//! Shared wallpaper placement: decodes a wallpaper file at natural size and composes it onto a
//! root-sized canvas. Single owner for both the desktop wallpaper worker and the shell's
//! pseudo-transparency backdrop, so both see identical pixels.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

pub use flamewm_api::background::WallpaperFit;

use crate::scale::{ScaleFilter, area_tap, scale_rgba};
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
/// dimension overflow. Direct, fully covered scaling keeps the fast scale path. Cropped `Fill`
/// uses [`compose_region`] to avoid allocating an oversized offscreen scaled image.
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
            // A cropped Fill used to allocate a full oversized scaled layer *and* the
            // destination canvas. The existing pixel-identical region compositor
            // samples only the visible destination pixels into one canvas. Keep the
            // cheap borrowed blit for unscaled natural images and the normal direct
            // scale path when the layer exactly covers the destination.
            if fit == WallpaperFit::Fill
                && (sw, sh) != (width, height)
                && (sw, sh) != (natural.width, natural.height)
            {
                return compose_region(natural, width, height, fit, (0, 0, width, height));
            }
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
    let (width, height) = (rect.2, rect.3);
    // Validate before allocation, preserving the previous rejection semantics.
    let x0 = u32::try_from(rect.0).ok()?;
    let y0 = u32::try_from(rect.1).ok()?;
    if natural.width == 0
        || natural.height == 0
        || width == 0
        || height == 0
        || x0.checked_add(width)? > root_w
        || y0.checked_add(height)? > root_h
    {
        return None;
    }
    let length = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let mut pixels = Vec::with_capacity(length);
    emit_region(natural, root_w, root_h, fit, rect, |_, _, pixel| {
        pixels.extend_from_slice(pixel);
    })?;
    Some(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// Write the region directly into an existing RGBA8 canvas without allocating a
/// second region-sized pixel buffer. Used for per-output wallpaper composition:
/// it preserves the same area-filter and integer-rounded pixel output as
/// [`compose_region`], including Fill, Fit, Center, Stretch and Tile.
///
/// The destination rectangle must fit `canvas` and its packed RGBA8 storage.
/// Invalid dimensions return `None` before writing any pixels.
pub fn compose_region_into(
    natural: &RgbaImage,
    root_w: u32,
    root_h: u32,
    fit: WallpaperFit,
    rect: (i32, i32, u32, u32),
    canvas: &mut RgbaImage,
    dest: (u32, u32),
) -> Option<()> {
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
        || dest.0.checked_add(width)? > canvas.width
        || dest.1.checked_add(height)? > canvas.height
    {
        return None;
    }
    let stride = (canvas.width as usize).checked_mul(4)?;
    let required = stride.checked_mul(canvas.height as usize)?;
    if canvas.pixels.len() != required {
        return None;
    }
    emit_region(natural, root_w, root_h, fit, rect, |ix, iy, pixel| {
        let index = (dest.1 as usize + iy) * stride + (dest.0 as usize + ix) * 4;
        canvas.pixels[index..index + 4].copy_from_slice(pixel);
    })
}

/// The single source of the pixel-sampling logic for owned contiguous regions
/// and regions written directly into a larger output canvas.
fn emit_region(
    natural: &RgbaImage,
    root_w: u32,
    root_h: u32,
    fit: WallpaperFit,
    rect: (i32, i32, u32, u32),
    mut emit: impl FnMut(usize, usize, &[u8]),
) -> Option<()> {
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
    match layer(natural, root_w, root_h, fit)? {
        Layer::Tiled => {
            for (j, y) in (y0..y0 + height).enumerate() {
                for (i, x) in (x0..x0 + width).enumerate() {
                    let source = (((u64::from(y) % nh) * nw + u64::from(x) % nw) * 4) as usize;
                    emit(i, j, &natural.pixels[source..source + 4]);
                }
            }
        }
        Layer::Scaled {
            width: sw,
            height: sh,
            x,
            y,
        } => {
            // Keep the identical area_tap sequence and float accumulation order.
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
            for (j, row) in row_taps.iter().enumerate() {
                for (i, column) in column_taps.iter().enumerate() {
                    match (row, column) {
                        (Some(row), Some(column)) => {
                            emit(i, j, &area_pixel(natural, column, row));
                        }
                        _ => emit(i, j, &BACKGROUND),
                    }
                }
            }
        }
    }
    Some(())
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

/// Root-space region of the wallpaper laid out the way the desktop draws it: each output in
/// `outputs` (root coordinates `x, y, w, h`) gets its own copy fitted to its own size. An empty
/// `outputs` is one wallpaper over the whole root. Root pixels no output covers are [`BACKGROUND`].
/// `None` for an empty `rect` or a refused composition.
#[must_use]
pub fn compose_outputs_region(
    natural: &RgbaImage,
    root: (u32, u32),
    outputs: &[(i32, i32, u32, u32)],
    fit: WallpaperFit,
    rect: (i32, i32, u32, u32),
) -> Option<RgbaImage> {
    if outputs.is_empty() {
        return compose_region(natural, root.0, root.1, fit, rect);
    }
    let (rx, ry, rw, rh) = rect;
    if rw == 0 || rh == 0 {
        return None;
    }
    let mut canvas = solid(rw, rh, BACKGROUND)?;
    for &(ox, oy, ow, oh) in outputs {
        let x0 = rx.max(ox);
        let y0 = ry.max(oy);
        let x1 = (i64::from(rx) + i64::from(rw)).min(i64::from(ox) + i64::from(ow)) as i32;
        let y1 = (i64::from(ry) + i64::from(rh)).min(i64::from(oy) + i64::from(oh)) as i32;
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        compose_region_into(
            natural,
            ow,
            oh,
            fit,
            (x0 - ox, y0 - oy, w, h),
            &mut canvas,
            ((x0 - rx) as u32, (y0 - ry) as u32),
        )?;
    }
    Some(canvas)
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
            for (sum, &value) in acc.iter_mut().zip(&natural.pixels[index..index + 4]) {
                *sum += weight * f32::from(value);
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
    // `cycle().take(count).collect()` has no exact lower size hint, so a
    // multi-monitor background can over-allocate while growing the Vec.
    let mut pixels = vec![0u8; count];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&colour);
    }
    Some(RgbaImage {
        width,
        height,
        pixels,
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
    fn solid_canvas_reserves_once_and_preserves_all_channels() {
        let fill = [17, 61, 119, 203];
        for (width, height) in [(1, 1), (101, 137), (192, 108)] {
            let canvas = super::solid(width, height, fill).expect("canvas");
            assert_eq!(canvas.pixels.len(), width as usize * height as usize * 4);
            assert!(
                canvas
                    .pixels
                    .chunks_exact(4)
                    .all(|pixel| pixel == fill.as_slice())
            );
        }
    }

    #[test]
    fn cropped_fill_uses_only_the_visible_canvas_and_matches_reference() {
        // Both overdraw orientations, including dimensions that require resampling.
        // Every RGBA channel must match the previous scale-and-blit implementation.
        for (source, target) in [
            ((41, 13), (18, 24)),
            ((17, 43), (31, 12)),
            ((29, 19), (15, 13)),
            ((27, 9), (7, 19)),
        ] {
            let natural = natural(source.0, source.1);
            let previous = reference_compose(&natural, target.0, target.1, WallpaperFit::Fill)
                .expect("reference wallpaper");
            let composed = compose(&natural, target.0, target.1, WallpaperFit::Fill)
                .expect("composed wallpaper");
            assert_eq!(composed, previous, "source {source:?} target {target:?}");
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

#[cfg(test)]
mod output_layout_tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[(x * 37) as u8, (y * 91) as u8, 200, 255]);
            }
        }
        RgbaImage {
            width,
            height,
            pixels,
        }
    }

    fn at(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = (y * image.width + x) as usize * 4;
        [
            image.pixels[i],
            image.pixels[i + 1],
            image.pixels[i + 2],
            image.pixels[i + 3],
        ]
    }

    #[test]
    fn each_output_gets_its_own_fit_at_its_own_offset() {
        let natural = gradient(64, 32);
        let fit = parse("fill");
        // Two outputs of different sizes side by side: 8x4 at 0,0 and 6x5 at 8,0.
        let outputs = [(0, 0, 8u32, 4u32), (8, 0, 6, 5)];
        let root = compose_outputs_region(&natural, (14, 5), &outputs, fit, (0, 0, 14, 5))
            .expect("two outputs compose");
        let left = compose(&natural, 8, 4, fit).expect("left copy");
        let right = compose(&natural, 6, 5, fit).expect("right copy");
        for y in 0..4 {
            for x in 0..8 {
                assert_eq!(at(&root, x, y), at(&left, x, y), "left ({x},{y})");
            }
        }
        for y in 0..5 {
            for x in 0..6 {
                assert_eq!(at(&root, 8 + x, y), at(&right, x, y), "right ({x},{y})");
            }
        }
    }

    #[test]
    fn region_crossing_the_seam_matches_the_per_output_copies() {
        let natural = gradient(64, 32);
        let fit = parse("fill");
        let outputs = [(0, 0, 8u32, 4u32), (8, 0, 6, 5)];
        let whole =
            compose_outputs_region(&natural, (14, 5), &outputs, fit, (0, 0, 14, 5)).expect("whole");
        let crop = compose_outputs_region(&natural, (14, 5), &outputs, fit, (5, 1, 6, 3))
            .expect("seam crop");
        for y in 0..3 {
            for x in 0..6 {
                assert_eq!(at(&crop, x, y), at(&whole, 5 + x, 1 + y), "({x},{y})");
            }
        }
    }

    #[test]
    fn root_pixels_no_output_covers_are_black_and_empty_layout_is_one_root() {
        let natural = gradient(64, 32);
        let fit = parse("fill");
        let outputs = [(0, 0, 8u32, 4u32)];
        let partial =
            compose_outputs_region(&natural, (8, 6), &outputs, fit, (0, 0, 8, 6)).expect("partial");
        assert_eq!(at(&partial, 3, 5), BACKGROUND, "row below the output");
        let single =
            compose_outputs_region(&natural, (14, 5), &[], fit, (0, 0, 14, 5)).expect("one root");
        let direct = compose(&natural, 14, 5, fit).expect("direct");
        assert_eq!(single.pixels, direct.pixels);
        assert!(compose_outputs_region(&natural, (14, 5), &outputs, fit, (0, 0, 0, 3)).is_none());
    }
    #[test]
    fn strided_region_write_matches_owned_region_and_keeps_canvas_storage() {
        let image = gradient(7, 5);
        for fit in [
            WallpaperFit::Fill,
            WallpaperFit::Fit,
            WallpaperFit::Stretch,
            WallpaperFit::Center,
            WallpaperFit::Tile,
        ] {
            let region = compose_region(&image, 9, 7, fit, (2, 1, 5, 4)).expect("reference region");
            let mut output = solid(12, 10, BACKGROUND).expect("output");
            let ptr = output.pixels.as_ptr();
            let len = output.pixels.len();
            compose_region_into(&image, 9, 7, fit, (2, 1, 5, 4), &mut output, (3, 4))
                .expect("strided region");
            assert_eq!(output.pixels.as_ptr(), ptr, "no output reallocation");
            assert_eq!(output.pixels.len(), len);
            for row in 0..4usize {
                let actual = ((row + 4) * 12 + 3) * 4;
                let source = row * 5 * 4;
                assert_eq!(
                    &output.pixels[actual..actual + 20],
                    &region.pixels[source..source + 20],
                    "{fit:?} row {row}"
                );
            }
            let before = output.pixels.clone();
            assert!(
                compose_region_into(&image, 9, 7, fit, (2, 1, 5, 4), &mut output, (10, 9))
                    .is_none()
            );
            assert_eq!(output.pixels, before, "invalid destination is nonmutating");
        }
    }
}
