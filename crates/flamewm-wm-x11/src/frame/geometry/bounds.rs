use super::*;

/// Minimum visible extent on one axis: a quarter of the window, rounded up.
pub(super) fn min_visible(extent: i32) -> i32 {
    extent.max(0).saturating_add(3) / 4
}

/// Keeps `rect` in `bounds` with at least a quarter of each axis visible; up to 3/4 may overflow.
#[must_use]
pub fn keep_visible(rect: RootRect, bounds: RootRect) -> RootRect {
    let rect = flamewm_window_core::keep_visible(
        flamewm_api::Rect::new(rect.x, rect.y, rect.w, rect.h),
        flamewm_api::Rect::new(bounds.x, bounds.y, bounds.w, bounds.h),
    );
    RootRect::new(rect.x, rect.y, rect.width, rect.height)
}

/// Bounding box of the outputs, or `fallback` when there are none.
pub(super) fn outputs_bounds(outputs: &[RootRect], fallback: RootRect) -> RootRect {
    let Some((first, rest)) = outputs.split_first() else {
        return fallback;
    };
    let (mut x0, mut y0) = (first.x, first.y);
    let (mut x1, mut y1) = (
        first.x.saturating_add(first.w),
        first.y.saturating_add(first.h),
    );
    for o in rest {
        x0 = x0.min(o.x);
        y0 = y0.min(o.y);
        x1 = x1.max(o.x.saturating_add(o.w));
        y1 = y1.max(o.y.saturating_add(o.h));
    }
    RootRect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// True when `r` keeps a quarter of its width and height visible on the single output `o`.
pub(super) fn visible_on(r: RootRect, o: RootRect) -> bool {
    let vw =
        r.x.saturating_add(r.w)
            .min(o.x.saturating_add(o.w))
            .saturating_sub(r.x.max(o.x));
    let vh =
        r.y.saturating_add(r.h)
            .min(o.y.saturating_add(o.h))
            .saturating_sub(r.y.max(o.y));
    vw >= min_visible(r.w) && vh >= min_visible(r.h)
}

/// Squared gap distance between `r` and `o` (0 when they touch or overlap).
pub(super) fn gap_sq(r: RootRect, o: RootRect) -> i64 {
    let (r_right, o_right) = (
        i64::from(r.x) + i64::from(r.w),
        i64::from(o.x) + i64::from(o.w),
    );
    let (r_bottom, o_bottom) = (
        i64::from(r.y) + i64::from(r.h),
        i64::from(o.y) + i64::from(o.h),
    );
    let dx = (i64::from(o.x) - r_right)
        .max(i64::from(r.x) - o_right)
        .max(0);
    let dy = (i64::from(o.y) - r_bottom)
        .max(i64::from(r.y) - o_bottom)
        .max(0);
    dx * dx + dy * dy
}

/// Keeps `rect` visible on the outputs. Bounds are the outputs' bounding box; a rect parked in a
/// gap of a non-rectangular union moves minimally onto the nearest output.
#[must_use]
pub fn keep_on_outputs(rect: RootRect, outputs: &[RootRect], fallback: RootRect) -> RootRect {
    let bounded = keep_visible(rect, outputs_bounds(outputs, fallback));
    let Some(nearest) = outputs.iter().copied().min_by_key(|o| gap_sq(bounded, *o)) else {
        return bounded;
    };
    if outputs.iter().any(|o| visible_on(bounded, *o)) {
        bounded
    } else {
        keep_visible(bounded, nearest)
    }
}

/// Brings a restored floating rect fully inside `work_area`: the size is capped to the work area,
/// then the rect slides back in from the edge it crosses. Restores reappear where they were saved,
/// so they take this stricter rule instead of the quarter-visible drag rule of [`keep_on_outputs`].
#[must_use]
pub fn fit_restore_rect(rect: RootRect, work_area: RootRect) -> RootRect {
    let w = rect.w.min(work_area.w).max(1);
    let h = rect.h.min(work_area.h).max(1);
    let fit = |start: i32, len: i32, lo: i32, span: i32| {
        let hi = lo.saturating_add(span.saturating_sub(len)).max(lo);
        start.clamp(lo, hi)
    };
    RootRect::new(
        fit(rect.x, w, work_area.x, work_area.w),
        fit(rect.y, h, work_area.y, work_area.h),
        w,
        h,
    )
}

/// Valid `[lo, hi]` for a left edge at `x` while the right edge `right` is fixed, against `[b0, b1)`.
/// Visible width must stay >= a quarter of the width (ceil); derived from `4 * visible >= width`.
pub(super) fn left_edge_range(right: i32, b0: i32, b1: i32) -> (i32, i32) {
    let (r, b0, b1) = (i64::from(right), i64::from(b0), i64::from(b1));
    let m = r.min(b1);
    let outside_min = r - 4 * (m - b0);
    let inside_max = (4 * m - r).div_euclid(3);
    let hi = (b0 - 1).max(inside_max).min(r - 1);
    let lo = outside_min.min(b0).min(hi);
    (to_i32(lo), to_i32(hi))
}

/// Valid `[lo, hi]` for a right edge at `x` while the left edge `left` is fixed, against `[b0, b1)`.
pub(super) fn right_edge_range(left: i32, b0: i32, b1: i32) -> (i32, i32) {
    let (l, b0, b1) = (i64::from(left), i64::from(b0), i64::from(b1));
    let a = l.max(b0);
    let inside_min = (4 * a - l + 2).div_euclid(3);
    let outside_max = l + 4 * (b1 - a);
    let lo = inside_min.min(b1 + 1).max(l + 1);
    let hi = b1.max(outside_max).max(lo);
    (to_i32(lo), to_i32(hi))
}

pub(super) fn to_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Keeps the titlebar band reachable against each panel reserve of the output holding `work`.
///
/// - Bottom: the band stays above the work area's bottom edge. A quarter of a window shorter than
///   176 px is under the 44 px panel, so `keep_on_outputs` alone let its whole titlebar sit under
///   the panel with no grab handle left. Applied on every output, reserve or not.
/// - Top, left, right: only when the work area is inset from that output's edge (a top, side or
///   stacked panel). The top band stays fully below the reserve; on a side, at least a quarter of
///   the frame width stays inside the work area. Without a reserve a window may still be dragged
///   past that edge, as `plan_move` documents.
///
/// The output is the one with the largest overlap with `work`, never the outputs' bounding box,
/// so a secondary monitor's work area is not mistaken for a reserve against its neighbour.
pub(super) fn keep_titlebar_in_work_area(
    frame: RootRect,
    work: RootRect,
    outputs: &[RootRect],
    titlebar_h: i32,
) -> RootRect {
    if titlebar_h <= 0 {
        return frame;
    }
    let (mut x, mut y) = (frame.x, frame.y);
    let max_y = work.y.saturating_add(work.h).saturating_sub(titlebar_h);
    y = y.min(max_y.max(work.y));
    let home = outputs
        .iter()
        .copied()
        .max_by_key(|output| overlap_area(*output, work))
        .filter(|output| overlap_area(*output, work) > 0);
    if let Some(output) = home {
        let visible = min_visible(frame.w);
        let work_right = work.x.saturating_add(work.w);
        let out_right = output.x.saturating_add(output.w);
        if work.y > output.y {
            y = y.max(work.y);
        }
        if work.x > output.x {
            x = x.max(work.x.saturating_add(visible).saturating_sub(frame.w));
        }
        if work_right < out_right {
            x = x.min(work_right.saturating_sub(visible));
        }
    }
    RootRect::new(x, y, frame.w, frame.h)
}

/// Intersection area of two rects, 0 when disjoint.
pub(super) fn overlap_area(a: RootRect, b: RootRect) -> i64 {
    let w =
        i64::from(a.x.saturating_add(a.w).min(b.x.saturating_add(b.w))) - i64::from(a.x.max(b.x));
    let h =
        i64::from(a.y.saturating_add(a.h).min(b.y.saturating_add(b.h))) - i64::from(a.y.max(b.y));
    if w <= 0 || h <= 0 { 0 } else { w * h }
}

/// Clamp a span of `len` at `start` so it fits inside `[lo, lo + extent)`.
pub(super) fn clamp_span(start: i32, len: i32, lo: i32, extent: i32) -> i32 {
    start.clamp(lo, lo.saturating_add(extent.saturating_sub(len).max(0)))
}
