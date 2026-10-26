//! Pure recomposition of a frame shadow over the live desktop rubber band.
//!
//! The rubber is part of the desktop, and the shadow sits above the desktop, so the rubber is
//! blended into the raw root snapshot first and the shadow darkens the result, pixel by pixel.
//! Nothing here touches X: the WM owns the pixmap and the damage it asks for.

use crate::frame::shadow::keep_at;
use crate::geometry::Rect;
use flamewm_api::root_rubber::{ROOT_RUBBER_BORDER_PX, RootRubber};

fn band_rect(rubber: &RootRubber) -> Rect {
    Rect::new(rubber.x, rubber.y, rubber.width, rubber.height)
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = a.right().max(b.right());
    let y1 = a.bottom().max(b.bottom());
    Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
}

/// Straight RGBA colour the rubber paints at root pixel `(x, y)`, or `None` outside the band.
/// The outermost [`ROOT_RUBBER_BORDER_PX`] pixels take the border colour, the rest the fill.
#[must_use]
pub fn rubber_coverage(rubber: &RootRubber, x: i32, y: i32) -> Option<[u8; 4]> {
    let band = band_rect(rubber);
    if x < band.x || y < band.y || x >= band.right() || y >= band.bottom() {
        return None;
    }
    let edge = (x - band.x)
        .min(y - band.y)
        .min(band.right() - 1 - x)
        .min(band.bottom() - 1 - y);
    Some(if edge < ROOT_RUBBER_BORDER_PX {
        rubber.border
    } else {
        rubber.fill
    })
}

/// Source-over blend of straight RGBA `src` onto one BGR pixel, in place.
pub fn blend_bgr(bgr: &mut [u8], src: [u8; 4]) {
    let alpha = u32::from(src[3]);
    if alpha == 0 {
        return;
    }
    let inverse = 255 - alpha;
    let mix = |source: u8, dest: u8| {
        ((u32::from(source) * alpha + u32::from(dest) * inverse + 127) / 255) as u8
    };
    bgr[0] = mix(src[2], bgr[0]);
    bgr[1] = mix(src[1], bgr[1]);
    bgr[2] = mix(src[0], bgr[2]);
}

/// Part of `area` (a shadow's root rect) that the rubber changed from `old` to `new` touches:
/// the bounding box of both bands clipped to `area`. `None` when neither band reaches it.
#[must_use]
pub fn damage_region(
    area: Rect,
    old: Option<&RootRubber>,
    new: Option<&RootRubber>,
) -> Option<Rect> {
    [old, new]
        .into_iter()
        .flatten()
        .filter_map(|rubber| intersect(band_rect(rubber), area))
        .reduce(union)
}

/// BGRX pixels of `region` (root space, inside `area`) for a shadow of `frame`: the raw desktop
/// snapshot `raw` (BGRX of `area`) with `rubber` blended over it, then shaded. With no rubber this
/// is exactly the plain shading of the snapshot, which is how a rubber that leaves restores pixels.
#[must_use]
pub fn compose_region(
    raw: &[u8],
    area: Rect,
    region: Rect,
    frame: Rect,
    focused: bool,
    rubber: Option<&RootRubber>,
) -> Vec<u8> {
    debug_assert!(
        intersect(area, region) == Some(region),
        "region outside shadow area"
    );
    let stride = area.width as usize * 4;
    let mut out = Vec::with_capacity(region.width as usize * region.height as usize * 4);
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            let at = (y - area.y) as usize * stride + (x - area.x) as usize * 4;
            let mut px = [raw[at], raw[at + 1], raw[at + 2], 0];
            if let Some(color) = rubber.and_then(|rubber| rubber_coverage(rubber, x, y)) {
                blend_bgr(&mut px[..3], color);
            }
            let keep = keep_at(frame, focused, x, y);
            for channel in &mut px[..3] {
                *channel = ((u32::from(*channel) * keep) >> 8) as u8;
            }
            out.extend_from_slice(&px);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::shadow::{shade_shadow, shadow_rect};

    const FRAME: Rect = Rect {
        x: 100,
        y: 100,
        width: 200,
        height: 150,
    };

    fn rubber(x: i32, y: i32, width: u32, height: u32) -> RootRubber {
        RootRubber {
            x,
            y,
            width,
            height,
            fill: [0x20, 0x40, 0x80, 0x80],
            border: [0xff, 0x00, 0x00, 0xff],
        }
    }

    fn snapshot(area: Rect) -> Vec<u8> {
        (0..area.width as usize * area.height as usize)
            .flat_map(|i| [(i % 251) as u8, (i % 113) as u8, (i % 67) as u8, 0])
            .collect()
    }

    #[test]
    fn coverage_is_border_ring_then_fill_then_nothing() {
        let band = rubber(10, 20, 8, 6);
        assert_eq!(rubber_coverage(&band, 9, 20), None);
        assert_eq!(rubber_coverage(&band, 18, 20), None);
        assert_eq!(rubber_coverage(&band, 10, 20), Some(band.border));
        assert_eq!(rubber_coverage(&band, 11, 21), Some(band.border));
        assert_eq!(rubber_coverage(&band, 12, 22), Some(band.fill));
        assert_eq!(rubber_coverage(&band, 15, 25), Some(band.border));
    }

    #[test]
    fn blend_is_source_over() {
        let mut keep = [10u8, 20, 30];
        blend_bgr(&mut keep, [0, 0, 0, 0]);
        assert_eq!(keep, [10, 20, 30]);
        let mut opaque = [10u8, 20, 30];
        blend_bgr(&mut opaque, [0x40, 0x50, 0x60, 0xff]);
        assert_eq!(opaque, [0x60, 0x50, 0x40]);
        let mut half = [0u8, 0, 0];
        blend_bgr(&mut half, [255, 255, 255, 128]);
        assert_eq!(half, [128, 128, 128]);
    }

    #[test]
    fn damage_is_clipped_to_the_shadow_and_covers_both_positions() {
        let area = shadow_rect(FRAME);
        let far = rubber(2000, 2000, 50, 50);
        assert_eq!(damage_region(area, Some(&far), None), None);
        assert_eq!(damage_region(area, None, None), None);
        let old = rubber(150, 150, 40, 40);
        let new = rubber(250, 200, 40, 40);
        let region = damage_region(area, Some(&old), Some(&new)).expect("touches shadow");
        assert!(intersect(area, region) == Some(region));
        assert!(region.x <= 150 && region.right() >= 290);
        assert!(region.y <= 150 && region.bottom() >= 240);
        let clipped = damage_region(area, Some(&rubber(0, 0, 400, 400)), None).expect("overlap");
        assert_eq!(clipped, area);
    }

    #[test]
    fn no_rubber_composes_exactly_the_plain_shade() {
        let area = shadow_rect(FRAME);
        let raw = snapshot(area);
        let mut plain = raw.clone();
        shade_shadow(FRAME, true, &mut plain);
        assert_eq!(compose_region(&raw, area, area, FRAME, true, None), plain);
    }

    #[test]
    fn rubber_is_blended_then_shaded_only_inside_its_region() {
        let area = shadow_rect(FRAME);
        let raw = snapshot(area);
        let band = rubber(240, 200, 30, 20);
        let mut plain = raw.clone();
        shade_shadow(FRAME, false, &mut plain);
        let region = damage_region(area, None, Some(&band)).expect("touches shadow");
        let composed = compose_region(&raw, area, region, FRAME, false, Some(&band));
        let stride = area.width as usize * 4;
        let mut differs = 0usize;
        for (row, y) in (region.y..region.bottom()).enumerate() {
            for (col, x) in (region.x..region.right()).enumerate() {
                let at = (row * region.width as usize + col) * 4;
                let plain_at = (y - area.y) as usize * stride + (x - area.x) as usize * 4;
                if rubber_coverage(&band, x, y).is_some() {
                    differs += usize::from(composed[at..at + 3] != plain[plain_at..plain_at + 3]);
                } else {
                    assert_eq!(
                        composed[at..at + 3],
                        plain[plain_at..plain_at + 3],
                        "({x},{y})"
                    );
                }
            }
        }
        assert!(differs > 0, "rubber must change some shadow pixels");
    }

    #[test]
    fn leaving_the_rubber_restores_the_plain_shade() {
        let area = shadow_rect(FRAME);
        let raw = snapshot(area);
        let band = rubber(240, 200, 30, 20);
        let region = damage_region(area, None, Some(&band)).expect("touches shadow");
        let with = compose_region(&raw, area, region, FRAME, true, Some(&band));
        let restored = compose_region(&raw, area, region, FRAME, true, None);
        assert_ne!(with, restored);
        let mut plain = raw.clone();
        shade_shadow(FRAME, true, &mut plain);
        let stride = area.width as usize * 4;
        for (row, y) in (region.y..region.bottom()).enumerate() {
            let src = (y - area.y) as usize * stride + (region.x - area.x) as usize * 4;
            let len = region.width as usize * 4;
            let dst = row * len;
            assert_eq!(restored[dst..dst + len], plain[src..src + len]);
        }
    }
}
