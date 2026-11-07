//! Pure output policy for the Wayland backend: mode selection, fractional scale math, row layout
//! of several outputs, and pointer crossing between them. No smithay or DRM types live here, so
//! every rule is unit-tested without hardware.
//!
//! Only the mode policy is called from the live DRM path today. Scale, row layout and crossing
//! wait for per-output rendering and window ownership; they are kept here so that work reuses the
//! tested rules instead of re-deriving them.
#![allow(dead_code)]

use crate::model::Rect;
use std::collections::BTreeMap;

/// Smallest accepted per-output scale, in percent.
pub(crate) const SCALE_MIN_PERCENT: u16 = 100;
/// Largest accepted per-output scale, in percent.
pub(crate) const SCALE_MAX_PERCENT: u16 = 200;
/// Scales move in steps of this many percent, matching the settings validation.
pub(crate) const SCALE_STEP_PERCENT: u16 = 5;

/// One connector mode as the policy sees it. `refresh_mhz` is millihertz, like `Mode::refresh`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModeCandidate {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
    pub preferred: bool,
}

/// Picks the connector mode to scan out at: the preferred resolution (the EDID preferred mode, or
/// the largest resolution when none is flagged), at its highest refresh rate. Ties keep the
/// preferred-flagged mode, then the earlier entry. Returns `None` only for an empty list.
pub(crate) fn select_mode(modes: &[ModeCandidate]) -> Option<usize> {
    let resolution = modes
        .iter()
        .find(|mode| mode.preferred)
        .or_else(|| {
            modes.iter().max_by_key(|mode| {
                (
                    i64::from(mode.width) * i64::from(mode.height),
                    mode.refresh_mhz,
                )
            })
        })
        .map(|mode| (mode.width, mode.height))?;
    modes
        .iter()
        .enumerate()
        .filter(|(_, mode)| (mode.width, mode.height) == resolution)
        .max_by(|(ai, a), (bi, b)| {
            a.refresh_mhz
                .cmp(&b.refresh_mhz)
                .then(a.preferred.cmp(&b.preferred))
                .then(bi.cmp(ai))
        })
        .map(|(index, _)| index)
}

/// Whether `percent` is an accepted scale: 100..=200 in 5% steps.
pub(crate) fn is_valid_scale_percent(percent: u16) -> bool {
    (SCALE_MIN_PERCENT..=SCALE_MAX_PERCENT).contains(&percent)
        && (percent - SCALE_MIN_PERCENT) % SCALE_STEP_PERCENT == 0
}

/// The configured scale for `output`, or 100 when it has none or the stored value is invalid.
pub(crate) fn scale_percent_for(scales: &BTreeMap<String, u16>, output: &str) -> u16 {
    match scales.get(output) {
        Some(&percent) if is_valid_scale_percent(percent) => percent,
        _ => SCALE_MIN_PERCENT,
    }
}

/// Logical size of a physical mode at `percent` scale, rounded to the nearest pixel, never below 1.
pub(crate) fn logical_size(physical: (i32, i32), percent: u16) -> (i32, i32) {
    let percent = i64::from(percent.max(1));
    let scale = |value: i32| {
        let value = i64::from(value.max(0));
        ((value * 100 + percent / 2) / percent).max(1) as i32
    };
    (scale(physical.0), scale(physical.1))
}

/// Physical size of a logical size at `percent` scale, the inverse of [`logical_size`].
pub(crate) fn physical_size(logical: (i32, i32), percent: u16) -> (i32, i32) {
    let percent = i64::from(percent.max(1));
    let scale = |value: i32| {
        let value = i64::from(value.max(0));
        (value * percent / 100) as i32
    };
    (scale(logical.0), scale(logical.1))
}

/// Places outputs in one row, left to right in the given order, all at `y = 0`.
pub(crate) fn arrange_row(logical_sizes: &[(i32, i32)]) -> Vec<Rect> {
    let mut x = 0;
    logical_sizes
        .iter()
        .map(|&(w, h)| {
            let rect = Rect::new(x, 0, w, h);
            x += w;
            rect
        })
        .collect()
}

/// Index of the output whose area contains `(x, y)`, using the half-open rule of `Rect::contains`.
pub(crate) fn output_at(outputs: &[Rect], x: f64, y: f64) -> Option<usize> {
    outputs.iter().position(|rect| rect.contains(x, y))
}

/// Output holding most of `rect`, the owner a window belongs to. `None` when it overlaps none.
pub(crate) fn output_for_rect(outputs: &[Rect], rect: Rect) -> Option<usize> {
    outputs
        .iter()
        .enumerate()
        .map(|(index, output)| (index, overlap_area(*output, rect)))
        .filter(|(_, area)| *area > 0)
        .max_by(|(ai, a), (bi, b)| a.cmp(b).then(bi.cmp(ai)))
        .map(|(index, _)| index)
}

fn overlap_area(a: Rect, b: Rect) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0 || h <= 0 {
        0
    } else {
        i64::from(w) * i64::from(h)
    }
}

/// Pointer position on the layout. A point on any output stays put. A point in a gap or past an
/// outer edge moves to the nearest pixel of the nearest output, so the pointer never disappears.
/// An empty layout leaves the point unchanged.
pub(crate) fn clamp_point_to_layout(x: f64, y: f64, outputs: &[Rect]) -> (f64, f64) {
    if output_at(outputs, x, y).is_some() {
        return (x, y);
    }
    let nearest = outputs
        .iter()
        .filter(|rect| rect.w > 0 && rect.h > 0)
        .map(|rect| {
            let min_x = f64::from(rect.x);
            let min_y = f64::from(rect.y);
            let max_x = f64::from(rect.x + rect.w - 1);
            let max_y = f64::from(rect.y + rect.h - 1);
            let cx = x.clamp(min_x, max_x);
            let cy = y.clamp(min_y, max_y);
            ((cx, cy), (cx - x).powi(2) + (cy - y).powi(2))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    nearest.map_or((x, y), |(point, _)| point)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: i32, height: i32, refresh_mhz: i32, preferred: bool) -> ModeCandidate {
        ModeCandidate {
            width,
            height,
            refresh_mhz,
            preferred,
        }
    }

    #[test]
    fn highest_refresh_at_the_preferred_resolution_wins() {
        let modes = [
            mode(1920, 1080, 60_000, true),
            mode(2560, 1440, 144_000, false),
            mode(1920, 1080, 144_000, false),
            mode(1280, 720, 240_000, false),
        ];
        assert_eq!(select_mode(&modes), Some(2));
    }

    #[test]
    fn largest_resolution_is_the_fallback_without_a_preferred_flag() {
        let modes = [
            mode(1024, 768, 75_000, false),
            mode(1600, 900, 60_000, false),
            mode(1600, 900, 75_000, false),
        ];
        assert_eq!(select_mode(&modes), Some(2));
    }

    #[test]
    fn ties_keep_the_preferred_then_the_earlier_mode() {
        let preferred_second = [mode(800, 600, 60_000, false), mode(800, 600, 60_000, true)];
        assert_eq!(select_mode(&preferred_second), Some(1));
        let equal = [mode(800, 600, 60_000, false), mode(800, 600, 60_000, false)];
        assert_eq!(select_mode(&equal), Some(0));
    }

    #[test]
    fn empty_mode_list_selects_nothing() {
        assert_eq!(select_mode(&[]), None);
    }

    #[test]
    fn scale_must_be_100_to_200_in_5_percent_steps() {
        for valid in [100, 105, 125, 150, 175, 200] {
            assert!(is_valid_scale_percent(valid), "{valid}");
        }
        for invalid in [0, 99, 101, 102, 112, 201, 300] {
            assert!(!is_valid_scale_percent(invalid), "{invalid}");
        }
    }

    #[test]
    fn invalid_or_missing_scale_falls_back_to_one() {
        let mut scales = BTreeMap::new();
        scales.insert("DP-1".to_owned(), 150);
        scales.insert("HDMI-A-1".to_owned(), 137);
        assert_eq!(scale_percent_for(&scales, "DP-1"), 150);
        assert_eq!(scale_percent_for(&scales, "HDMI-A-1"), 100);
        assert_eq!(scale_percent_for(&scales, "eDP-1"), 100);
    }

    #[test]
    fn logical_size_rounds_to_the_nearest_pixel() {
        assert_eq!(logical_size((3840, 2160), 150), (2560, 1440));
        assert_eq!(logical_size((1920, 1080), 125), (1536, 864));
        assert_eq!(logical_size((1920, 1080), 100), (1920, 1080));
        // 1366 / 1.25 = 1092.8, rounds up to 1093.
        assert_eq!(logical_size((1366, 768), 125), (1093, 614));
        assert_eq!(logical_size((0, 0), 200), (1, 1));
    }

    #[test]
    fn physical_size_inverts_exact_scales() {
        assert_eq!(physical_size((2560, 1440), 150), (3840, 2160));
        assert_eq!(physical_size((1536, 864), 125), (1920, 1080));
        assert_eq!(physical_size((1920, 1080), 100), (1920, 1080));
    }

    #[test]
    fn outputs_are_placed_in_a_row() {
        let rects = arrange_row(&[(1920, 1080), (2560, 1440), (1280, 1024)]);
        assert_eq!(rects[0], Rect::new(0, 0, 1920, 1080));
        assert_eq!(rects[1], Rect::new(1920, 0, 2560, 1440));
        assert_eq!(rects[2], Rect::new(4480, 0, 1280, 1024));
        assert!(arrange_row(&[]).is_empty());
    }

    #[test]
    fn point_on_an_output_is_not_moved_across_the_seam() {
        let rects = arrange_row(&[(100, 100), (100, 50)]);
        assert_eq!(clamp_point_to_layout(99.0, 10.0, &rects), (99.0, 10.0));
        assert_eq!(clamp_point_to_layout(100.0, 10.0, &rects), (100.0, 10.0));
        assert_eq!(output_at(&rects, 100.0, 10.0), Some(1));
        assert_eq!(output_at(&rects, 199.0, 60.0), None);
    }

    #[test]
    fn pointer_past_the_edge_of_the_smaller_output_lands_on_its_last_pixel() {
        let rects = arrange_row(&[(100, 100), (100, 50)]);
        // Below the short second output: nearest pixel is its bottom row.
        assert_eq!(clamp_point_to_layout(150.0, 80.0, &rects), (150.0, 49.0));
        // Off the far right of the layout: last column of the nearest output.
        assert_eq!(clamp_point_to_layout(400.0, 20.0, &rects), (199.0, 20.0));
        // Negative coordinates: the origin pixel of the first output.
        assert_eq!(clamp_point_to_layout(-5.0, -5.0, &rects), (0.0, 0.0));
    }

    #[test]
    fn empty_layout_leaves_the_pointer_unchanged() {
        assert_eq!(clamp_point_to_layout(-3.0, 9.0, &[]), (-3.0, 9.0));
    }

    #[test]
    fn window_belongs_to_the_output_holding_most_of_it() {
        let rects = arrange_row(&[(100, 100), (100, 100)]);
        assert_eq!(output_for_rect(&rects, Rect::new(10, 10, 50, 50)), Some(0));
        assert_eq!(output_for_rect(&rects, Rect::new(80, 10, 60, 50)), Some(1));
        assert_eq!(output_for_rect(&rects, Rect::new(300, 0, 10, 10)), None);
        assert_eq!(output_for_rect(&[], Rect::new(0, 0, 1, 1)), None);
    }
}
