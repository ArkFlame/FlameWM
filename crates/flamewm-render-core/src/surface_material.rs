//! Liquid Glass surface material: pure geometry and alpha math shared by every painter.
//!
//! A drop shadow is a stack of 1px rings painted outside the surface rect. Ring `k` covers the
//! pixels `k..k+1` away from the edge, so rings never overlap the interior and a translucent
//! surface is not darkened from beneath.
//! ui-core keeps the same formulas for settings-side callers; keep the two in sync.

use crate::model::Rect;

/// Per-node material request. `shadow_peak == 0` or `shadow_spread == 0` paints no shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceMaterial {
    /// Shadow extent in px outside the surface edge.
    pub shadow_spread: u8,
    /// Alpha of the ring nearest the edge; falls to 0 at `shadow_spread`.
    pub shadow_peak: u8,
    /// Vertical shift of the shadow in px (positive moves it down).
    pub shadow_offset_y: f32,
}

/// Soft shadow alpha at `distance` px outside a shape. Peak `peak` at the edge, falling
/// quadratically to 0 at `spread` px. Same formula as ui-core `surface::shadow_alpha`.
#[must_use]
pub fn shadow_ring_alpha(distance: u32, spread: u32, peak: u8) -> u8 {
    if spread == 0 || distance >= spread {
        return 0;
    }
    let remaining = u64::from(spread - distance);
    let spread = u64::from(spread);
    ((u64::from(peak) * remaining * remaining + spread * spread / 2) / (spread * spread)) as u8
}

/// The shadow rings for a surface at `rect` with corner `radius`, outermost last. Each item is
/// the ring's outer rectangle, its outer corner radius and its alpha. Stroking the outer rect with
/// a 1px width gives exactly the band `k..k+1` px outside the surface.
pub fn shadow_rings(
    rect: Rect,
    radius: f32,
    spread: u32,
    peak: u8,
    offset_y: f32,
) -> impl Iterator<Item = (Rect, f32, u8)> {
    (0..spread).map(move |k| {
        let grow = (k + 1) as f32;
        let outer = Rect {
            x: rect.x - grow,
            y: rect.y + offset_y - grow,
            width: rect.width + 2.0 * grow,
            height: rect.height + 2.0 * grow,
        };
        (outer, radius + grow, shadow_ring_alpha(k, spread, peak))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_peaks_at_edge_and_is_zero_beyond_spread() {
        assert_eq!(shadow_ring_alpha(0, 16, 90), 90);
        assert_eq!(shadow_ring_alpha(16, 16, 90), 0);
        assert_eq!(shadow_ring_alpha(0, 0, 90), 0);
        assert!(shadow_ring_alpha(8, 16, 90) < shadow_ring_alpha(4, 16, 90));
    }

    #[test]
    fn rings_grow_outward_one_pixel_each_and_follow_the_offset() {
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        let rings: Vec<_> = shadow_rings(rect, 12.0, 3, 120, 4.0).collect();
        assert_eq!(rings.len(), 3);
        let (first, first_radius, first_alpha) = rings[0];
        assert_eq!(first.x, 9.0);
        assert_eq!(first.y, 20.0 + 4.0 - 1.0);
        assert_eq!(first.width, 102.0);
        assert_eq!(first_radius, 13.0);
        assert_eq!(first_alpha, 120);
        let (last, last_radius, last_alpha) = rings[2];
        assert_eq!(last.x, 7.0);
        assert_eq!(last_radius, 15.0);
        assert!(last_alpha < first_alpha);
    }
}
