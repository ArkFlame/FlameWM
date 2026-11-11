// Render-owned X Shape (libXext) bounding-mask bridge for rounded popup/menu
// surfaces. Loaded dynamically like the XRender bridge: when libXext or the
// Shape symbols are unavailable, construction fails and callers log an
// explicit degraded mode (no silent transparency claim).

use std::os::raw::{c_int, c_ulong};

use crate::ffi::dynamic_library::DynamicLibrary;
use crate::xlib::{Display, Window};

pub(crate) const SHAPE_BOUNDING: c_int = 0;
pub(crate) const SHAPE_INPUT: c_int = 2;
pub(crate) const SHAPE_SET: c_int = 0;
pub(crate) const SHAPE_YX_SORTED: c_int = 1;
type ShapeRectanglesFn = unsafe extern "C" fn(
    *mut Display,
    c_ulong,
    c_int,
    c_int,
    c_int,
    *const XRectangle,
    c_int,
    c_int,
    c_int,
);

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct XRectangle {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

pub(crate) struct XShapeBridge {
    display: *mut Display,
    shape_rectangles: ShapeRectanglesFn,
    _library: Option<DynamicLibrary>,
}

impl XShapeBridge {
    /// # Safety
    ///
    /// `display` must be a live Xlib display that outlives the bridge.
    pub(crate) unsafe fn new(display: *mut Display) -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libXext.so.6", "libXext.so"])?;
        let shape_rectangles =
            unsafe { library.symbol::<ShapeRectanglesFn>(b"XShapeCombineRectangles\0") }?;
        Ok(Self {
            display,
            shape_rectangles,
            _library: Some(library),
        })
    }

    /// Transfer libXext's handle to the display lifetime owner. Shape has no
    /// client-side X resource to release, but its dynamic symbols may still be
    /// used by Xlib's close-display teardown path.
    pub(crate) fn into_deferred_libraries(mut self) -> Vec<DynamicLibrary> {
        let library = self._library.take();
        drop(self);
        library.into_iter().collect()
    }

    /// Apply a rounded-rect bounding mask built from row spans.
    /// `spans` are (y, x_start, x_end_exclusive) in window pixels.
    ///
    /// # Safety
    ///
    /// `window` must be a live X window on this bridge's display.
    pub(crate) unsafe fn apply_rounded_mask(
        &self,
        window: Window,
        spans: &[(u32, u32, u32)],
    ) -> Result<(), String> {
        unsafe { self.apply_mask_kind(window, SHAPE_BOUNDING, spans) }
    }

    /// Apply the rounded mask intersected with `clip` (window-local). An empty result is a valid
    /// bounding region and hides the window, so a slide that starts fully below the panel stays
    /// invisible.
    ///
    /// # Safety
    ///
    /// `window` must be a live X window on this bridge's display.
    pub(crate) unsafe fn apply_clipped_mask(
        &self,
        window: Window,
        spans: &[(u32, u32, u32)],
        clip: ClipRect,
    ) -> Result<(), String> {
        let rects = spans_to_rects(clip_spans_iter(spans, clip));
        // SAFETY: `window` is live per the caller contract.
        unsafe { self.set_rects(window, SHAPE_BOUNDING, &rects) };
        Ok(())
    }

    /// Apply an input-region mask. Pass-through surfaces call this with an
    /// empty span list, producing an empty XShape input region so pointer
    /// events fall through. Empty input is legal here (unlike bounding).
    ///
    /// # Safety
    ///
    /// `window` must be a live X window on this bridge's display.
    pub(crate) unsafe fn apply_input_mask(
        &self,
        window: Window,
        spans: &[(u32, u32, u32)],
    ) -> Result<(), String> {
        if spans.is_empty() {
            // SAFETY: `window` is live per the caller contract.
            unsafe { self.set_rects(window, SHAPE_INPUT, &[]) };
            return Ok(());
        }
        unsafe { self.apply_mask_kind(window, SHAPE_INPUT, spans) }
    }

    unsafe fn apply_mask_kind(
        &self,
        window: Window,
        kind: c_int,
        spans: &[(u32, u32, u32)],
    ) -> Result<(), String> {
        let rects = spans_to_rects(spans.iter().copied());
        if rects.is_empty() {
            return Err("rounded shape mask has no visible rows".to_string());
        }
        // SAFETY: `window` is live per the caller contract.
        unsafe { self.set_rects(window, kind, &rects) };
        Ok(())
    }

    /// # Safety
    ///
    /// `window` must be a live X window on this bridge's display.
    unsafe fn set_rects(&self, window: Window, kind: c_int, rects: &[XRectangle]) {
        // SAFETY: display is live per `new`; rects is a valid slice for the call.
        unsafe {
            (self.shape_rectangles)(
                self.display,
                window,
                kind,
                0,
                0,
                rects.as_ptr(),
                rects.len().min(c_int::MAX as usize) as c_int,
                SHAPE_SET,
                SHAPE_YX_SORTED,
            )
        };
    }
}

/// Window-local clip rectangle as (x0, y0, x1_exclusive, y1_exclusive).
pub(crate) type ClipRect = (u32, u32, u32, u32);

fn spans_to_rects(spans: impl IntoIterator<Item = (u32, u32, u32)>) -> Vec<XRectangle> {
    // Adjacent rows with identical coverage form one XShape rectangle. A
    // full-height frame is one rectangle rather than hundreds of 1px calls;
    // curved corners retain their exact independently covered rows.
    let mut rects: Vec<XRectangle> = Vec::new();
    for (y, start, end) in spans {
        if end <= start {
            continue;
        }
        let x = start.min(i16::MAX as u32) as i16;
        let yy = y.min(i16::MAX as u32) as i16;
        let width = end.saturating_sub(start).min(u16::MAX as u32) as u16;
        if let Some(last) = rects.last_mut()
            && last.x == x
            && last.width == width
            && i32::from(last.y) + i32::from(last.height) == i32::from(yy)
            && last.height < u16::MAX
        {
            last.height += 1;
            continue;
        }
        rects.push(XRectangle {
            x,
            y: yy,
            width,
            height: 1,
        });
    }
    rects
}

/// Intersects row spans with `clip`; order is preserved so YX-sorted Shape requests stay valid.
fn clip_spans_iter(
    spans: &[(u32, u32, u32)],
    clip: ClipRect,
) -> impl Iterator<Item = (u32, u32, u32)> + '_ {
    let (x0, y0, x1, y1) = clip;
    spans
        .iter()
        .filter(move |(y, _, _)| *y >= y0 && *y < y1)
        .filter_map(move |&(y, start, end)| {
            let (start, end) = (start.max(x0), end.min(x1));
            (end > start).then_some((y, start, end))
        })
}

#[cfg(test)]
pub(crate) fn clip_spans(spans: &[(u32, u32, u32)], clip: ClipRect) -> Vec<(u32, u32, u32)> {
    clip_spans_iter(spans, clip).collect()
}

/// Pure geometry shared with the `flamewm-ui-x11` span tests: row spans
/// (y, x_start, x_end_exclusive) for a rounded rect of `width`x`height`
/// with corner radius `radius` (already clamped to half the min side).
/// Coverage spans from `native::coverage` reuse this rasterization so the
/// Shape bounding mask and the painted pixels agree.
pub(crate) use crate::native::rounded::spans as rounded_mask_spans;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_mask_cuts_corner_pixels_on_all_four_corners() {
        let (w, h, r) = (100u32, 80u32, 12u32);
        let rects = spans_to_rects(rounded_mask_spans(w, h, r).iter().copied());
        let covered = |x: u32, y: u32| {
            rects.iter().any(|rc| {
                x >= rc.x as u32
                    && x < rc.x as u32 + rc.width as u32
                    && y >= rc.y as u32
                    && y < rc.y as u32 + rc.height as u32
            })
        };
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(
                !covered(x, y),
                "corner pixel ({x},{y}) must be outside the mask"
            );
        }
        // Edge midpoints stay inside: the bar itself is not clipped.
        for (x, y) in [(w / 2, 0), (0, h / 2), (w - 1, h / 2), (w / 2, h - 1)] {
            assert!(
                covered(x, y),
                "edge pixel ({x},{y}) must stay inside the mask"
            );
        }
    }

    #[test]
    fn coalesced_rectangles_keep_exact_rounded_coverage() {
        let spans = rounded_mask_spans(100, 80, 12);
        let rects = spans_to_rects(spans.iter().copied());
        assert!(rects.len() < spans.len() / 2);
        let covered = |x: u32, y: u32| {
            rects.iter().any(|r| {
                x >= r.x as u32
                    && x < r.x as u32 + r.width as u32
                    && y >= r.y as u32
                    && y < r.y as u32 + r.height as u32
            })
        };
        for y in 0..80 {
            for x in 0..100 {
                let original = spans
                    .iter()
                    .any(|&(row, start, end)| row == y && x >= start && x < end);
                assert_eq!(covered(x, y), original, "{x},{y}");
            }
        }
    }

    #[test]
    fn rounded_mask_geometry_matches_quarter_circle_cut() {
        // Floor of (r - dx) at pixel-row centers, matching the render-side
        // corner rasterization. 8x8 r=4: row 0 cuts 2px per side, rows 1..6
        // are full width under floor (row-1 grazing coverage rounds to 0),
        // row 7 mirrors row 0.
        assert_eq!(
            rounded_mask_spans(8, 8, 4),
            vec![
                (0, 2, 6),
                (1, 0, 8),
                (2, 0, 8),
                (3, 0, 8),
                (4, 0, 8),
                (5, 0, 8),
                (6, 0, 8),
                (7, 2, 6),
            ]
        );
    }

    #[test]
    fn streamed_clipped_mask_matches_owned_clip_pipeline() {
        for (width, height, radius) in [(2, 2, 0), (32, 8, 4), (480, 360, 14), (1, 400, 0)] {
            let rows = rounded_mask_spans(width, height, radius);
            for clip in [
                (0, 0, width, height),
                (0, 0, width / 2, height / 2),
                (1, 1, width, height),
                (0, height, width, height + 10),
                (width, 0, width + 10, height),
            ] {
                let legacy = spans_to_rects(clip_spans(&rows, clip));
                let streamed = spans_to_rects(clip_spans_iter(&rows, clip));
                let tuples = |items: Vec<XRectangle>| {
                    items
                        .into_iter()
                        .map(|r| (r.x, r.y, r.width, r.height))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    tuples(legacy),
                    tuples(streamed),
                    "{width}x{height} clip={clip:?}"
                );
            }
        }
    }

    #[test]
    fn clip_keeps_rows_inside_the_clip_rect() {
        let spans = rounded_mask_spans(8, 8, 4);
        // Keep rows 0..3 only: rows 0 and 1 and 2 survive with their own spans.
        assert_eq!(
            clip_spans(&spans, (0, 0, 8, 3)),
            vec![(0, 2, 6), (1, 0, 8), (2, 0, 8)]
        );
        // Keep the lower half-plane: rows 5..8, columns 2..8 of the cut shape.
        assert_eq!(
            clip_spans(&spans, (2, 5, 8, 8)),
            vec![(5, 2, 8), (6, 2, 8), (7, 2, 6)]
        );
    }

    #[test]
    fn clip_outside_the_shape_is_empty() {
        let spans = rounded_mask_spans(8, 8, 4);
        // A clip entirely below the popup (slide start) yields an empty bounding region.
        assert!(clip_spans(&spans, (0, 8, 8, 16)).is_empty());
        // A clip that removes all columns is empty too.
        assert!(clip_spans(&spans, (9, 0, 12, 8)).is_empty());
    }

    #[test]
    fn small_radius_degrades_to_full_rect_rows() {
        assert_eq!(rounded_mask_spans(4, 2, 0), vec![(0, 0, 4), (1, 0, 4)]);
        assert!(rounded_mask_spans(0, 8, 4).is_empty());
    }
}
