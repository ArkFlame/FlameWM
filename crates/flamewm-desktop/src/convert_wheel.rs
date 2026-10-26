//! Shared radial geometry and cursor-sector highlight for the Convert wheel.
//! UI hit testing and the hover raster use *exactly the same* annulus predicate.
//! The wheel is otherwise a standard surface-coloured FlameWM popup.
use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

pub const WHEEL_SIZE: u32 = 328;
const CENTER: f32 = 164.0;
const INNER_RADIUS: f32 = 54.0;
const OUTER_RADIUS: f32 = 160.0;
const SECTOR_STEP: f32 = PI / 4.0;
const HALF_ARC: f32 = PI / 8.0 - 0.045;

/// Returns the clockwise sector from 12 o'clock. The hub and transparent
/// exterior have no selection, and all eight slices share one predicate.
pub fn sector_at(x: f32, y: f32) -> Option<usize> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let dx = x - CENTER;
    let dy = y - CENTER;
    let radius_sq = dx * dx + dy * dy;
    if !(INNER_RADIUS * INNER_RADIUS..=OUTER_RADIUS * OUTER_RADIUS).contains(&radius_sq) {
        return None;
    }
    let angle = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    let sector = ((angle + SECTOR_STEP / 2.0) / SECTOR_STEP).floor() as usize % 8;
    let center = sector as f32 * SECTOR_STEP;
    let distance = (angle - center + PI).rem_euclid(TAU) - PI;
    (distance.abs() <= HALF_ARC).then_some(sector)
}

const NO_SECTOR: u8 = u8::MAX;

/// Per pixel, the sector hit by each of the four antialiasing subsamples (or `NO_SECTOR`).
/// Retaining 4 bytes per pixel instead of eight 8-bit masks halves the idle footprint; the
/// alpha of a sector is recovered exactly as `hits * 23` (= `hits * 92 / 4`).
static SECTOR_SAMPLES: OnceLock<Vec<[u8; 4]>> = OnceLock::new();

fn sector_samples() -> &'static [[u8; 4]] {
    SECTOR_SAMPLES.get_or_init(|| {
        let mut samples = Vec::with_capacity((WHEEL_SIZE * WHEEL_SIZE) as usize);
        for y in 0..WHEEL_SIZE {
            for x in 0..WHEEL_SIZE {
                // Four subsamples give antialiased rounded arc edges.
                samples.push(
                    [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].map(|(ox, oy)| {
                        sector_at(x as f32 + ox, y as f32 + oy).map_or(NO_SECTOR, |s| s as u8)
                    }),
                );
            }
        }
        samples
    })
}

/// Only the hovered fraction is painted: no permanent triangles or dividers.
/// RGB is the live Settings accent; straight alpha has antialiased edges.
pub fn highlight_pixels(sector: usize, accent: [u8; 3]) -> Vec<u8> {
    let mut pixels = vec![0; (WHEEL_SIZE * WHEEL_SIZE * 4) as usize];
    if sector >= 8 {
        return pixels;
    }
    for (out, hits) in pixels.chunks_exact_mut(4).zip(sector_samples()) {
        let count = hits
            .iter()
            .filter(|hit| usize::from(**hit) == sector)
            .count();
        if count != 0 {
            out[..3].copy_from_slice(&accent);
            out[3] = (count * 23) as u8;
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn center_edges_and_octants() {
        assert_eq!(sector_at(164.0, 164.0), None);
        assert_eq!(sector_at(164.0, 0.0), None);
        assert_eq!(sector_at(164.0, 64.0), Some(0));
        assert_eq!(sector_at(235.0, 93.0), Some(1));
        assert_eq!(sector_at(264.0, 164.0), Some(2));
        assert_eq!(sector_at(235.0, 235.0), Some(3));
        assert_eq!(sector_at(164.0, 264.0), Some(4));
        assert_eq!(sector_at(93.0, 235.0), Some(5));
        assert_eq!(sector_at(64.0, 164.0), Some(6));
        assert_eq!(sector_at(93.0, 93.0), Some(7));
    }
    #[test]
    fn highlight_only_paints_selected_wedge() {
        let pixels = highlight_pixels(2, [12, 56, 78]);
        let pixel = |x: usize, y: usize| -> &[u8] { &pixels[(y * 328 + x) * 4..][..4] };
        assert_eq!(pixel(264, 164), &[12, 56, 78, 92]);
        assert_eq!(pixel(64, 164)[3], 0);
        assert_eq!(pixel(164, 164)[3], 0);
    }
}
