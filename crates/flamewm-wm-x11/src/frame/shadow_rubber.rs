//! Pure recomposition of a frame shadow over the live desktop rubber band.
//!
//! The rubber is part of the desktop, and the shadow sits above the desktop, so the rubber is
//! blended into the raw root snapshot first and the shadow darkens the result, pixel by pixel.
//! Nothing here touches X: the WM owns the pixmap and the damage it asks for.

use crate::frame::shadow::{shadow_keep_from_table, shadow_keep_table};
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

/// Four disjoint pixel bands containing every visible part of a centred frame shadow: the top and
/// bottom bands are full width, and the side bands cover only the middle rows beside the frame.
/// The frame masks its interior, so retaining or repainting those hidden pixels is wasteful.
#[must_use]
pub fn visible_shadow_regions(area: Rect, frame: Rect) -> [Option<Rect>; 4] {
    visible_shadow_regions_rounded(area, frame, 0)
}

/// Ring bands for a frame with corner radius `radius`. The rounded notches lie in rows within
/// `radius` of the frame's top or bottom edge, so the full-width top and bottom bands hold them.
#[must_use]
pub fn visible_shadow_regions_rounded(area: Rect, frame: Rect, radius: u32) -> [Option<Rect>; 4] {
    let r = radius.min(frame.width.min(frame.height) / 2) as i32;
    let top = frame.y + r;
    let bottom = frame.bottom() - r;
    let band = |x0: i32, y0: i32, x1: i32, y1: i32| {
        intersect(
            area,
            Rect::new(x0, y0, (x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32),
        )
    };
    [
        band(area.x, area.y, area.right(), top),
        band(area.x, bottom, area.right(), area.bottom()),
        band(area.x, top, frame.x, bottom),
        band(frame.right(), top, area.right(), bottom),
    ]
}

/// Only retain the raw desktop pixels that can actually appear in a shadow.
/// Each row is tightly packed BGRX; the original full frame is released after presentation.
#[derive(PartialEq, Eq)]
pub struct ShadowBands {
    radius: u32,
    strips: [Option<ShadowStrip>; 4],
}

#[derive(PartialEq, Eq)]
struct ShadowStrip {
    region: Rect,
    pixels: Vec<u8>,
}

impl ShadowBands {
    #[must_use]
    pub fn capture(raw: &[u8], area: Rect, frame: Rect) -> Self {
        let stride = area.width as usize * 4;
        let strips = visible_shadow_regions(area, frame).map(|region| {
            region.map(|region| {
                let mut pixels =
                    Vec::with_capacity(region.width as usize * region.height as usize * 4);
                for y in region.y..region.bottom() {
                    let start = (y - area.y) as usize * stride + (region.x - area.x) as usize * 4;
                    pixels.extend_from_slice(&raw[start..start + region.width as usize * 4]);
                }
                ShadowStrip { region, pixels }
            })
        });
        Self { radius: 0, strips }
    }

    #[must_use]
    pub fn capture_rounded(raw: &[u8], area: Rect, frame: Rect, radius: u32) -> Self {
        let stride = area.width as usize * 4;
        let strips = visible_shadow_regions_rounded(area, frame, radius).map(|region| {
            region.map(|region| {
                let mut pixels =
                    Vec::with_capacity(region.width as usize * region.height as usize * 4);
                for y in region.y..region.bottom() {
                    let start = (y - area.y) as usize * stride + (region.x - area.x) as usize * 4;
                    pixels.extend_from_slice(&raw[start..start + region.width as usize * 4]);
                }
                ShadowStrip { region, pixels }
            })
        });
        Self { radius, strips }
    }

    fn pixel(&self, x: i32, y: i32) -> [u8; 3] {
        for strip in self.strips.iter().flatten() {
            let rect = strip.region;
            if x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom() {
                let at = ((y - rect.y) as usize * rect.width as usize + (x - rect.x) as usize) * 4;
                return [strip.pixels[at], strip.pixels[at + 1], strip.pixels[at + 2]];
            }
        }
        unreachable!("only visible shadow regions may read compacted raw pixels")
    }

    #[cfg(test)]
    fn stored_bytes(&self) -> usize {
        self.strips
            .iter()
            .flatten()
            .map(|strip| strip.pixels.len())
            .sum()
    }
}

/// Split damage into the visible ring bands. A rectangle which crosses the hidden
/// frame interior never causes a full-frame pixel upload to Xorg.
#[must_use]
pub fn visible_damage_regions(
    area: Rect,
    frame: Rect,
    old: Option<&RootRubber>,
    new: Option<&RootRubber>,
) -> [Option<Rect>; 4] {
    let Some(changed) = damage_region(area, old, new) else {
        return [None; 4];
    };
    visible_shadow_regions(area, frame).map(|region| region.and_then(|r| intersect(r, changed)))
}

/// Existing damage geometry, extended only for the small rounded-corner notches.
#[must_use]
pub fn visible_motion_damage_regions_rounded(
    area: Rect,
    frame: Rect,
    radius: u32,
    old: Option<&RootRubber>,
    new: Option<&RootRubber>,
) -> [Option<Rect>; 8] {
    if radius == 0 {
        return visible_motion_damage_regions(area, frame, old, new);
    }
    // The old two-edge optimization assumes a rectangular shadow, so applying it
    // to the newly exposed corner could miss damage after the selection shrinks.
    // Keep the conservative damage bound for the small curved region.
    let changed = damage_region(area, old, new);
    let mut result = [None; 8];
    if let Some(changed) = changed {
        for (i, region) in visible_shadow_regions_rounded(area, frame, radius)
            .into_iter()
            .enumerate()
        {
            result[i] = region.and_then(|r| intersect(r, changed));
        }
    }
    result
}

/// For a selection rectangle resized from the same top-left corner, the existing fill and
/// top/left border do not change. Only its trailing right/bottom edges need compositing. This
/// keeps mouse motion from uploading the entire accumulated selection area to Xorg.
///
/// Fallback to the conservative damage bound for moved rectangles or changed colours. The four
/// slots are the two non-overlapping edge bands intersected with the two visible shadow strips.
#[must_use]
pub fn visible_motion_damage_regions(
    area: Rect,
    frame: Rect,
    old: Option<&RootRubber>,
    new: Option<&RootRubber>,
) -> [Option<Rect>; 8] {
    if let (Some(previous), Some(current)) = (old, new) {
        if previous.x == current.x
            && previous.y == current.y
            && previous.fill == current.fill
            && previous.border == current.border
        {
            let previous_rect = band_rect(previous);
            let current_rect = band_rect(current);
            let right = previous_rect.right().max(current_rect.right());
            let bottom = previous_rect.bottom().max(current_rect.bottom());
            let width_changed = previous.width != current.width;
            let height_changed = previous.height != current.height;
            let x_edge = previous_rect
                .right()
                .min(current_rect.right())
                .saturating_sub(ROOT_RUBBER_BORDER_PX)
                .max(previous.x);
            let y_edge = previous_rect
                .bottom()
                .min(current_rect.bottom())
                .saturating_sub(ROOT_RUBBER_BORDER_PX)
                .max(previous.y);
            // The vertical edge also covers the bottom-right corner; the horizontal edge stops
            // there to avoid sending the same pixels twice.
            let edges = [
                width_changed.then(|| {
                    Rect::new(
                        x_edge,
                        previous.y,
                        right.saturating_sub(x_edge) as u32,
                        bottom.saturating_sub(previous.y) as u32,
                    )
                }),
                height_changed.then(|| {
                    Rect::new(
                        previous.x,
                        y_edge,
                        (if width_changed { x_edge } else { right }).saturating_sub(previous.x)
                            as u32,
                        bottom.saturating_sub(y_edge) as u32,
                    )
                }),
            ];
            let visible = visible_shadow_regions(area, frame);
            let mut result = [None; 8];
            for (i, edge) in edges.iter().enumerate() {
                for (j, strip) in visible.iter().enumerate() {
                    result[i * 4 + j] = edge.and_then(|e| strip.and_then(|s| intersect(e, s)));
                }
            }
            return result;
        }
    }
    let conservative = visible_damage_regions(area, frame, old, new);
    let mut result = [None; 8];
    result[..4].copy_from_slice(&conservative);
    result
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
    radius: u32,
    focused: bool,
    rubber: Option<&RootRubber>,
) -> Vec<u8> {
    let stride = area.width as usize * 4;
    compose_pixels(region, frame, radius, focused, rubber, |x, y| {
        let at = (y - area.y) as usize * stride + (x - area.x) as usize * 4;
        [raw[at], raw[at + 1], raw[at + 2]]
    })
}

/// Pixel-equivalent to `compose_region` for every visible shadow pixel, but reads only the
/// compact retained strips. Called exclusively with regions from `visible_damage_regions`.
#[must_use]
pub fn compose_visible_region(
    raw: &ShadowBands,
    region: Rect,
    frame: Rect,
    focused: bool,
    rubber: Option<&RootRubber>,
) -> Vec<u8> {
    let mut out = Vec::new();
    compose_visible_region_into(raw, region, frame, focused, rubber, &mut out);
    out
}

/// Same pixels as `compose_visible_region`, but retain the allocation between damage regions.
/// The X11 WM can upload one region and refill this buffer for the next without reallocating.
pub fn compose_visible_region_into(
    raw: &ShadowBands,
    region: Rect,
    frame: Rect,
    focused: bool,
    rubber: Option<&RootRubber>,
    out: &mut Vec<u8>,
) {
    // Every production damage region is clipped to exactly one retained strip. Resolve that
    // strip once instead of checking both strip bounds for every pixel in the hot loop.
    if let Some(strip) = raw.strips.iter().flatten().find(|strip| {
        region.x >= strip.region.x
            && region.y >= strip.region.y
            && region.right() <= strip.region.right()
            && region.bottom() <= strip.region.bottom()
    }) {
        let stride = strip.region.width as usize * 4;
        compose_pixels_into(
            region,
            frame,
            raw.radius,
            focused,
            rubber,
            |x, y| {
                let at = (y - strip.region.y) as usize * stride + (x - strip.region.x) as usize * 4;
                [strip.pixels[at], strip.pixels[at + 1], strip.pixels[at + 2]]
            },
            out,
        );
    } else {
        // Preserve the original lookup for any future caller that supplies a region spanning
        // multiple strips; the current WM never takes this fallback.
        compose_pixels_into(region, frame, raw.radius, focused, rubber, |x, y| raw.pixel(x, y), out);
    }
}

fn compose_pixels(
    region: Rect,
    frame: Rect,
    radius: u32,
    focused: bool,
    rubber: Option<&RootRubber>,
    raw_pixel: impl FnMut(i32, i32) -> [u8; 3],
) -> Vec<u8> {
    let mut out = Vec::new();
    compose_pixels_into(region, frame, radius, focused, rubber, raw_pixel, &mut out);
    out
}

fn compose_pixels_into(
    region: Rect,
    frame: Rect,
    radius: u32,
    focused: bool,
    rubber: Option<&RootRubber>,
    mut raw_pixel: impl FnMut(i32, i32) -> [u8; 3],
    out: &mut Vec<u8>,
) {
    out.clear();
    out.reserve(region.width as usize * region.height as usize * 4);
    let keep_table = shadow_keep_table(focused);
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            let [b, g, r] = raw_pixel(x, y);
            let mut px = [b, g, r, 0];
            if let Some(color) = rubber.and_then(|rubber| rubber_coverage(rubber, x, y)) {
                blend_bgr(&mut px[..3], color);
            }
            let keep = shadow_keep_from_table(&keep_table, frame, radius, focused, x, y);
            for channel in &mut px[..3] {
                *channel = ((u32::from(*channel) * keep) >> 8) as u8;
            }
            out.extend_from_slice(&px);
        }
    }
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
    fn compact_shadow_retains_only_visible_pixels() {
        let area = shadow_rect(FRAME);
        let full = snapshot(area);
        let bands = ShadowBands::capture(&full, area, FRAME);
        // The retained bytes are exactly the centred ring: the shadow rect minus the frame interior.
        let ring_pixels = area.width as usize * area.height as usize
            - FRAME.width as usize * FRAME.height as usize;
        assert_eq!(bands.stored_bytes(), ring_pixels * 4);
        for region in visible_shadow_regions(area, FRAME).into_iter().flatten() {
            for y in region.y..region.bottom() {
                for x in region.x..region.right() {
                    let at =
                        ((y - area.y) as usize * area.width as usize + (x - area.x) as usize) * 4;
                    assert_eq!(bands.pixel(x, y), [full[at], full[at + 1], full[at + 2]]);
                }
            }
        }
    }

    #[test]
    fn compact_composition_is_pixel_identical_on_visible_strips() {
        let area = shadow_rect(FRAME);
        let raw = snapshot(area);
        let compact = ShadowBands::capture(&raw, area, FRAME);
        let cases = [
            (None, None),
            (None, Some(rubber(240, 200, 30, 20))),
            (
                Some(rubber(290, 230, 100, 100)),
                Some(rubber(105, 115, 300, 160)),
            ),
            (Some(rubber(100, 150, 120, 100)), None),
        ];
        for (old, new) in cases {
            for region in visible_damage_regions(area, FRAME, old.as_ref(), new.as_ref())
                .into_iter()
                .flatten()
            {
                for focused in [false, true] {
                    assert_eq!(
                        compose_visible_region(&compact, region, FRAME, focused, new.as_ref()),
                        compose_region(&raw, area, region, FRAME, 0, focused, new.as_ref()),
                        "region={region:?}, focused={focused}",
                    );
                }
            }
        }
    }

    #[test]
    fn reusable_composition_matches_owned_output_after_grow_shrink_and_style_change() {
        let area = shadow_rect(FRAME);
        let raw = snapshot(area);
        let compact = ShadowBands::capture(&raw, area, FRAME);
        let strips: Vec<_> = visible_shadow_regions(area, FRAME)
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(strips.len(), 4);
        let narrow = Rect::new(strips[0].x, strips[0].y, 1, 1);
        let broad = strips[1];
        let mut reusable = Vec::new();
        for (region, focused, overlay) in [
            (narrow, false, None),
            (broad, true, Some(rubber(100, 200, 400, 300))),
            (narrow, true, None),
            (broad, false, None),
        ] {
            compose_visible_region_into(
                &compact,
                region,
                FRAME,
                focused,
                overlay.as_ref(),
                &mut reusable,
            );
            assert_eq!(
                reusable,
                compose_region(&raw, area, region, FRAME, 0, focused, overlay.as_ref()),
            );
            assert_eq!(
                reusable.len(),
                region.width as usize * region.height as usize * 4
            );
        }
        compose_visible_region_into(
            &compact,
            Rect::new(0, 0, 0, 0),
            FRAME,
            false,
            None,
            &mut reusable,
        );
        assert!(
            reusable.is_empty(),
            "no stale bytes after a zero-sized update"
        );
    }

    #[test]
    fn damage_never_repaints_the_occluded_frame_interior() {
        let area = shadow_rect(FRAME);
        let all = rubber(area.x, area.y, area.width, area.height);
        let regions = visible_damage_regions(area, FRAME, None, Some(&all));
        let affected: u64 = regions
            .iter()
            .flatten()
            .map(|r| u64::from(r.width) * u64::from(r.height))
            .sum();
        // Exactly the centred ring: the shadow rect minus the frame interior.
        let ring = u64::from(area.width) * u64::from(area.height)
            - u64::from(FRAME.width) * u64::from(FRAME.height);
        assert_eq!(affected, ring);
        for region in regions.into_iter().flatten() {
            for y in region.y..region.bottom() {
                for x in region.x..region.right() {
                    let inside = x >= FRAME.x && x < FRAME.right() && y >= FRAME.y && y < FRAME.bottom();
                    assert!(!inside, "damage must not repaint the occluded frame interior");
                }
            }
        }
    }

    #[test]
    fn stationary_origin_resize_only_damages_changed_pixels() {
        let frame = Rect::new(-15, -12, 37, 29);
        let area = shadow_rect(frame);
        for origin_x in [-20, 0, 18, 27] {
            for origin_y in [-18, 0, 11, 23] {
                for old_width in [1, 2, 3, 9, 30, 55] {
                    for new_width in [1, 2, 6, 10, 28, 65] {
                        for (old_height, new_height) in [(1, 2), (12, 7), (7, 32), (24, 24)] {
                            let before = rubber(origin_x, origin_y, old_width, old_height);
                            let after = rubber(origin_x, origin_y, new_width, new_height);
                            let dirty = visible_motion_damage_regions(
                                area,
                                frame,
                                Some(&before),
                                Some(&after),
                            );
                            for visible in visible_shadow_regions(area, frame).into_iter().flatten()
                            {
                                for y in visible.y..visible.bottom() {
                                    for x in visible.x..visible.right() {
                                        if rubber_coverage(&before, x, y)
                                            != rubber_coverage(&after, x, y)
                                        {
                                            assert!(
                                                dirty.iter().flatten().any(|rect| {
                                                    x >= rect.x
                                                        && x < rect.right()
                                                        && y >= rect.y
                                                        && y < rect.bottom()
                                                }),
                                                "uncovered change at ({x}, {y}), old={old_width}x{old_height}, new={new_width}x{new_height}",
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn changed_colours_or_origin_preserve_the_conservative_fallback() {
        let area = shadow_rect(FRAME);
        let first = rubber(200, 200, 55, 55);
        let moved = rubber(201, 200, 55, 55);
        let recoloured = RootRubber {
            fill: [1, 2, 3, 4],
            ..first
        };
        for next in [moved, recoloured] {
            let old = visible_damage_regions(area, FRAME, Some(&first), Some(&next));
            let new = visible_motion_damage_regions(area, FRAME, Some(&first), Some(&next));
            assert_eq!(new, [old[0], old[1], old[2], old[3], None, None, None, None]);
        }
    }

    #[test]
    fn growing_selection_does_not_recompose_its_unchanged_interior() {
        let area = shadow_rect(FRAME);
        let start = rubber(200, 150, 150, 130);
        let end = rubber(200, 150, 151, 131);
        let old = visible_damage_regions(area, FRAME, Some(&start), Some(&end));
        let optimized = visible_motion_damage_regions(area, FRAME, Some(&start), Some(&end));
        let pixels = |regions: &[Option<Rect>]| -> u64 {
            regions
                .iter()
                .flatten()
                .map(|r| u64::from(r.width) * u64::from(r.height))
                .sum()
        };
        assert!(pixels(&optimized) < pixels(&old));
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
        assert_eq!(compose_region(&raw, area, area, FRAME, 0, true, None), plain);
    }

    #[test]
    fn rubber_is_blended_then_shaded_only_inside_its_region() {
        let area = shadow_rect(FRAME);
        let raw = snapshot(area);
        let band = rubber(240, 200, 30, 20);
        let mut plain = raw.clone();
        shade_shadow(FRAME, false, &mut plain);
        let region = damage_region(area, None, Some(&band)).expect("touches shadow");
        let composed = compose_region(&raw, area, region, FRAME, 0, false, Some(&band));
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
        let with = compose_region(&raw, area, region, FRAME, 0, true, Some(&band));
        let restored = compose_region(&raw, area, region, FRAME, 0, true, None);
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

/// Compose micro-benchmark: run with
/// `cargo test -p flamewm-wm-x11 --offline --release compose_bench -- --ignored --nocapture`.
/// Prints min-of-N wall time per full ring compose (all four strips, rounded, focused and not).
#[cfg(test)]
mod compose_bench {
    use super::*;
    use crate::frame::shadow::{SPREAD, shadow_rect};
    use std::hint::black_box;
    use std::time::Instant;

    const FRAME: Rect = Rect {
        x: 100,
        y: 100,
        width: 800,
        height: 600,
    };
    const RADIUS: u32 = 10;
    const RUNS: usize = 15;
    const ITERS: usize = 4;

    #[test]
    #[ignore = "timing harness, run explicitly"]
    fn compose_bench_min_of_n() {
        let area = shadow_rect(FRAME);
        let raw: Vec<u8> = (0..area.width as usize * area.height as usize)
            .flat_map(|i| [(i % 251) as u8, (i % 113) as u8, (i % 67) as u8, 0])
            .collect();
        let compact = ShadowBands::capture_rounded(&raw, area, FRAME, RADIUS);
        let strips: Vec<Rect> = visible_shadow_regions_rounded(area, FRAME, RADIUS)
            .into_iter()
            .flatten()
            .collect();
        let overlay = RootRubber {
            x: FRAME.x + 50,
            y: FRAME.y + 50,
            width: 300,
            height: 200,
            fill: [0x20, 0x40, 0x80, 0x80],
            border: [0xff, 0x00, 0x00, 0xff],
        };
        let mut out = Vec::new();
        let mut best = [u128::MAX; 2];
        for run in 0..RUNS {
            for (slot, rubber) in [None, Some(&overlay)].into_iter().enumerate() {
                let started = Instant::now();
                for _ in 0..ITERS {
                    for focused in [false, true] {
                        for region in &strips {
                            compose_visible_region_into(
                                &compact, *region, FRAME, focused, rubber, &mut out,
                            );
                            black_box(&out);
                        }
                    }
                }
                let nanos = started.elapsed().as_nanos() / ITERS as u128;
                if run > 0 {
                    best[slot] = best[slot].min(nanos);
                }
            }
        }
        let px: usize = strips.iter().map(|r| r.width as usize * r.height as usize).sum();
        println!("compose_bench spread={SPREAD} radius={RADIUS} strips_px={px} min_ns_plain={} min_ns_rubber={}", best[0], best[1]);
    }
}
