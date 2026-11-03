//! Backend-neutral window outline rasteriser (`windowOutlineColor`, `windowOutlineOpacity`,
//! `outlines`).
//!
//! One shared contract: the X11 frame painter and the Wayland overlay element paint these exact
//! bytes, so the two backends cannot drift. The ring is 1px wide, antialiased by 4x4
//! supersampling, and follows the rounded rectangle of `width` x `height` with corner `radius`.
//! It occupies the outermost pixel row and column of that silhouette (inset 0): the inner edge of
//! the ring is the silhouette edge of the pixels it covers, so nothing is drawn outside the frame.
//!
//! Every buffer here is straight-alpha RGBA, the shared image contract. A consumer that needs
//! premultiplied bytes (the Wayland memory element) converts once, in its own buffer, and never
//! re-rasterises the arc.
//!
//! `radius <= 1` is a plain 1px rectangular border; a larger radius is clamped to
//! `min(width, height) / 2` so a tiny frame still yields a valid silhouette.

/// Supersampling samples per axis: 16 samples per pixel keeps a 24px arc smooth.
const SAMPLES: u32 = 4;

/// Opacity of one ring pixel: the configured outline opacity scaled by the 1px coverage.
///
/// Shared rounding, so the X11 frame compositor and the Wayland overlay produce identical alpha.
#[must_use]
pub const fn ring_alpha(opacity: u8, coverage: u8) -> u8 {
    ((opacity as u16 * coverage as u16 + 127) / 255) as u8
}

/// One strip of the ring: `(x, y, width, height, rgba)`, positioned from the frame origin.
pub type OutlineStrip = (u32, u32, u32, u32, Vec<u8>);

/// Whether a point is inside the rounded rectangle `w` x `h` with corner `radius`.
fn inside_rounded(px: f32, py: f32, w: f32, h: f32, radius: f32) -> bool {
    if px < 0.0 || py < 0.0 || px >= w || py >= h {
        return false;
    }
    if radius <= 0.0 {
        return true;
    }
    let dx = px - px.clamp(radius, w - radius);
    let dy = py - py.clamp(radius, h - radius);
    dx * dx + dy * dy <= radius * radius
}

/// Coverage (`0..=255`) of the 1px antialiased ring on the rounded-rect silhouette, at pixel
/// `(x, y)`. Exterior and interior are 0; a fully covered straight edge pixel is 255.
#[must_use]
pub fn ring_coverage(width: u32, height: u32, radius: u32, x: u32, y: u32) -> u8 {
    if width == 0 || height == 0 || x >= width || y >= height {
        return 0;
    }
    let radius = radius.min(width.min(height) / 2);
    if radius <= 1 {
        // A plain 1px rectangular border: every outermost pixel, nothing else.
        let border = x == 0 || y == 0 || x + 1 == width || y + 1 == height;
        return if border { 255 } else { 0 };
    }
    let (w, h) = (width as f32, height as f32);
    let (outer, inner) = (radius as f32, (radius - 1) as f32);
    let mut hits = 0;
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let px = x as f32 + (sx as f32 + 0.5) / SAMPLES as f32;
            let py = y as f32 + (sy as f32 + 0.5) / SAMPLES as f32;
            if inside_rounded(px, py, w, h, outer)
                && !inside_rounded(px - 1.0, py - 1.0, w - 2.0, h - 2.0, inner)
            {
                hits += 1;
            }
        }
    }
    ((hits * 255 + SAMPLES * SAMPLES / 2) / (SAMPLES * SAMPLES)) as u8
}

/// `width * height * 4` straight-alpha RGBA of the ring, zero where the coverage is 0.
#[must_use]
pub fn outline_rgba(width: u32, height: u32, radius: u32, rgb: u32, alpha: u8) -> Vec<u8> {
    let mut out = vec![0_u8; width as usize * height as usize * 4];
    if alpha == 0 {
        return out;
    }
    for y in 0..height {
        for x in 0..width {
            let coverage = ring_coverage(width, height, radius, x, y);
            if coverage == 0 {
                continue;
            }
            let at = ((y as usize * width as usize) + x as usize) * 4;
            out[at..at + 4].copy_from_slice(&[
                (rgb >> 16) as u8,
                (rgb >> 8) as u8,
                rgb as u8,
                ring_alpha(alpha, coverage),
            ]);
        }
    }
    out
}

/// The same image as [`outline_rgba`] cut into the bounded perimeter strips a caller uploads:
/// top band, bottom band, then the left and right bands between them. `width * height * 4` bytes
/// are never allocated for a large frame.
#[must_use]
pub fn outline_strips(
    width: u32,
    height: u32,
    radius: u32,
    rgb: u32,
    alpha: u8,
) -> Vec<OutlineStrip> {
    if alpha == 0 || width == 0 || height == 0 {
        return Vec::new();
    }
    let radius = radius.min(width.min(height) / 2);
    // Rows and columns the ring can reach: the corner arcs span `radius` pixels plus the 1px band.
    let band = if radius <= 1 { 1 } else { radius + 1 };
    let top_h = band.min(height);
    let bottom_h = band.min(height - top_h);
    let mid_y = top_h;
    let mid_h = height - top_h - bottom_h;
    let left_w = band.min(width);
    let right_w = band.min(width - left_w);
    let mut strips = Vec::new();
    for (x, y, w, h) in [
        (0, 0, width, top_h),
        (0, height - bottom_h, width, bottom_h),
        (0, mid_y, left_w, mid_h),
        (width - right_w, mid_y, right_w, mid_h),
    ] {
        if w > 0 && h > 0 {
            let rgba = strip_rgba(width, height, radius, rgb, alpha, (x, y, w, h));
            strips.push((x, y, w, h, rgba));
        }
    }
    strips
}

/// Ring pixels of one `(x, y, w, h)` region, indexed from the frame origin.
fn strip_rgba(
    width: u32,
    height: u32,
    radius: u32,
    rgb: u32,
    alpha: u8,
    (x0, y0, w, h): (u32, u32, u32, u32),
) -> Vec<u8> {
    let mut out = vec![0_u8; w as usize * h as usize * 4];
    for y in 0..h {
        for x in 0..w {
            let coverage = ring_coverage(width, height, radius, x0 + x, y0 + y);
            if coverage == 0 {
                continue;
            }
            let at = ((y as usize * w as usize) + x as usize) * 4;
            out[at..at + 4].copy_from_slice(&[
                (rgb >> 16) as u8,
                (rgb >> 8) as u8,
                rgb as u8,
                ring_alpha(alpha, coverage),
            ]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reconstruct(width: u32, height: u32, radius: u32, rgb: u32, alpha: u8) -> Vec<u8> {
        let mut full = vec![0_u8; width as usize * height as usize * 4];
        for (x, y, w, h, rgba) in outline_strips(width, height, radius, rgb, alpha) {
            for row in 0..h {
                let src = (row as usize * w as usize) * 4;
                let dst = ((y + row) as usize * width as usize + x as usize) * 4;
                full[dst..dst + w as usize * 4].copy_from_slice(&rgba[src..src + w as usize * 4]);
            }
        }
        full
    }

    #[test]
    fn strips_reconstruct_the_full_image() {
        for (w, h, r) in [
            (120, 80, 24),
            (200, 200, 12),
            (64, 40, 0),
            (40, 64, 1),
            (30, 30, 99),
        ] {
            for alpha in [127_u8, 255] {
                let full = outline_rgba(w, h, r, 0x00ab_cdef, alpha);
                assert_eq!(
                    reconstruct(w, h, r, 0x00ab_cdef, alpha),
                    full,
                    "{w}x{h} r{r}"
                );
            }
        }
    }

    #[test]
    fn strips_are_perimeter_only() {
        let (w, h, r) = (1920_u32, 1080_u32, 24_u32);
        let pixels: u64 = outline_strips(w, h, r, 0x00ff_ffff, 127)
            .iter()
            .map(|(_, _, sw, sh, _)| u64::from(*sw) * u64::from(*sh))
            .sum();
        assert!(
            pixels < u64::from(w) * u64::from(h) / 4,
            "perimeter strips must stay far below a full-window buffer, got {pixels}"
        );
    }

    #[test]
    fn ring_is_mirror_symmetric_on_both_axes() {
        let (w, h, r) = (90_u32, 50_u32, 16_u32);
        for y in 0..h {
            for x in 0..w {
                let coverage = ring_coverage(w, h, r, x, y);
                assert_eq!(
                    coverage,
                    ring_coverage(w, h, r, w - 1 - x, y),
                    "x {x} y {y}"
                );
                assert_eq!(
                    coverage,
                    ring_coverage(w, h, r, x, h - 1 - y),
                    "x {x} y {y}"
                );
            }
        }
    }

    #[test]
    fn straight_edges_are_exactly_the_alpha() {
        let (w, h, r, alpha) = (120_u32, 80_u32, 24_u32, 127_u8);
        let full = outline_rgba(w, h, r, 0x0011_2233, alpha);
        let pixel = |x: u32, y: u32| {
            let at = (y as usize * w as usize + x as usize) * 4;
            &full[at..at + 4]
        };
        assert_eq!(pixel(60, 0), [0x11, 0x22, 0x33, alpha]);
        assert_eq!(pixel(60, h - 1), [0x11, 0x22, 0x33, alpha]);
        assert_eq!(pixel(0, 40), [0x11, 0x22, 0x33, alpha]);
        assert_eq!(pixel(w - 1, 40), [0x11, 0x22, 0x33, alpha]);
        assert_eq!(pixel(60, 40), [0, 0, 0, 0], "interior stays empty");
    }

    #[test]
    fn radius_zero_and_one_are_rectangular_borders() {
        let (w, h) = (40_u32, 24_u32);
        assert_eq!(
            outline_rgba(w, h, 0, 0x00ff_ffff, 255),
            outline_rgba(w, h, 1, 0x00ff_ffff, 255)
        );
        for y in 0..h {
            for x in 0..w {
                let border = x == 0 || y == 0 || x + 1 == w || y + 1 == h;
                let expected = if border { 255 } else { 0 };
                assert_eq!(
                    ring_coverage(w, h, 0, x, y),
                    expected,
                    "radius 0 at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn corner_arc_has_an_outside_pixel_and_a_partial_pixel() {
        let (w, h, r) = (120_u32, 80_u32, 24_u32);
        assert_eq!(ring_coverage(w, h, r, 0, 0), 0, "corner is outside the arc");
        let partial: Vec<u8> = (0..r)
            .flat_map(|y| (0..r).map(move |x| (x, y)))
            .map(|(x, y)| ring_coverage(w, h, r, x, y))
            .filter(|coverage| *coverage > 0 && *coverage < 255)
            .collect();
        assert!(
            partial.len() > 3,
            "the arc must be antialiased, got {} partial pixels",
            partial.len()
        );
    }

    #[test]
    fn alpha_zero_renders_nothing() {
        let (w, h, r) = (120_u32, 80_u32, 24_u32);
        assert!(
            outline_rgba(w, h, r, 0x00ff_ffff, 0)
                .iter()
                .all(|byte| *byte == 0)
        );
        assert!(outline_strips(w, h, r, 0x00ff_ffff, 0).is_empty());
    }

    #[test]
    fn radius_is_clamped_on_tiny_rects() {
        for (w, h) in [(1_u32, 1_u32), (2, 7), (3, 3), (9, 4)] {
            for radius in [0_u32, 1, 2, 50, u32::MAX] {
                let clamped = radius.min(w.min(h) / 2);
                for y in 0..h {
                    for x in 0..w {
                        assert_eq!(
                            ring_coverage(w, h, radius, x, y),
                            ring_coverage(w, h, clamped, x, y),
                            "{w}x{h} r{radius} at {x},{y}"
                        );
                    }
                }
                assert_eq!(
                    outline_rgba(w, h, radius, 0x00ff_ffff, 255).len(),
                    (w * h * 4) as usize
                );
            }
        }
    }

    #[test]
    fn no_pixel_outside_the_image_is_addressed() {
        for (w, h, r) in [
            (1_u32, 9_u32, 3_u32),
            (9, 1, 3),
            (5, 5, 5),
            (80, 3, 2),
            (3, 80, 2),
        ] {
            for (x, y, sw, sh, rgba) in outline_strips(w, h, r, 0x00ff_ffff, 255) {
                assert!(sw > 0 && sh > 0);
                assert!(
                    x + sw <= w && y + sh <= h,
                    "{w}x{h} strip at {x},{y} {sw}x{sh}"
                );
                assert_eq!(rgba.len(), (sw * sh * 4) as usize);
            }
            assert_eq!(
                reconstruct(w, h, r, 0x00ff_ffff, 255),
                outline_rgba(w, h, r, 0x00ff_ffff, 255)
            );
        }
    }
}
