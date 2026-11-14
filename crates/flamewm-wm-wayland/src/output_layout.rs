//! Pure output policy for the Wayland backend: mode selection, fractional scale math, row layout
//! of several outputs, and pointer crossing between them. No smithay or DRM types live here, so
//! every rule is unit-tested without hardware.
//!
//! Only the mode policy is called from the live DRM path today. Scale, row layout and crossing
//! wait for per-output rendering and window ownership; they are kept here so that work reuses the
//! tested rules instead of re-deriving them.

use crate::model::Rect;
use std::collections::{BTreeMap, HashMap};

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

/// Scale for the output at snapshot `index`: its own `wayland-{index}` key when saved, else the
/// `wayland-0` key that configs saved before per-output keys carry. Old configs keep their scale.
pub(crate) fn output_scale_percent(scales: &BTreeMap<String, u16>, index: usize) -> u16 {
    let own = format!("wayland-{index}");
    if scales.contains_key(&own) {
        scale_percent_for(scales, &own)
    } else {
        scale_percent_for(scales, "wayland-0")
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

/// Physical size of a logical size at `percent` scale, the inverse of [`logical_size`]: rounded to
/// the nearest pixel like it, so `physical_size(logical_size(p))` returns `p` for every size the
/// fractional scale can express exactly once rounded.
pub(crate) fn physical_size(logical: (i32, i32), percent: u16) -> (i32, i32) {
    let percent = i64::from(percent.max(1));
    let scale = |value: i32| {
        let value = i64::from(value.max(0));
        ((value * percent + 50) / 100) as i32
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

/// Origin of an output added after `placed` (logical sizes, in row order): the end of the row, as
/// `arrange_row` would place it. With nothing placed this is the origin `(0, 0)`.
pub(crate) fn origin_after(placed: &[(i32, i32)], next: (i32, i32)) -> (i32, i32) {
    let mut sizes = placed.to_vec();
    sizes.push(next);
    arrange_row(&sizes)
        .last()
        .map_or((0, 0), |rect| (rect.x, rect.y))
}

/// Outputs the pointer is clamped to: the placed row when the backend drives several outputs, else
/// the primary output alone (single-output and nested sessions keep their old bounds).
pub(crate) fn pointer_layout<'a>(stored: &'a [Rect], primary: &'a [Rect]) -> &'a [Rect] {
    if stored.is_empty() { primary } else { stored }
}

/// Which output each surface sits on, keyed by the surface's protocol id. A surface with no entry
/// belongs to the primary output, so scale and placement keep the single-output path.
#[derive(Debug)]
pub(crate) struct SurfaceOutputs<K> {
    map: HashMap<K, String>,
}

impl<K> Default for SurfaceOutputs<K> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

impl<K: Eq + std::hash::Hash> SurfaceOutputs<K> {
    pub(crate) fn assign(&mut self, key: K, output: String) {
        self.map.insert(key, output);
    }

    pub(crate) fn remove(&mut self, key: &K) {
        self.map.remove(key);
    }

    pub(crate) fn output_of(&self, key: &K) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    /// Drops entries on outputs not in `live`, so those surfaces fall back to the primary output.
    pub(crate) fn retain_outputs(&mut self, live: &[String]) {
        self.map.retain(|_, output| live.contains(output));
    }
}

/// Name of the output holding most of `rect`, or `primary` when it overlaps none. `names[i]` names
/// `outputs[i]`; an empty row (single or nested output) is always `primary`.
pub(crate) fn output_name_for_rect<'a>(
    outputs: &[Rect],
    names: &'a [String],
    rect: Rect,
    primary: &'a str,
) -> &'a str {
    output_for_rect(outputs, rect)
        .and_then(|index| names.get(index))
        .map_or(primary, String::as_str)
}

/// Scale of the output a surface sits on. Unmapped surfaces, and outputs with no entry in
/// `scales`, take `primary_scale`, the scale the compositor's own output reports.
pub(crate) fn scale_for_output(
    scales: &[(String, f64)],
    output: Option<&str>,
    primary_scale: f64,
) -> f64 {
    output
        .and_then(|name| {
            scales
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, scale)| *scale)
        })
        .unwrap_or(primary_scale)
}

/// Index of the output whose area contains `(x, y)`, using the half-open rule of `Rect::contains`.
pub(crate) fn output_at(outputs: &[Rect], x: f64, y: f64) -> Option<usize> {
    outputs.iter().position(|rect| rect.contains(x, y))
}

/// `(x, y)` relative to the origin of the output that contains it. A point on no output is returned
/// unchanged, so a single-output session keeps its pointer as it is.
pub(crate) fn relative_to_output(outputs: &[Rect], x: f64, y: f64) -> (f64, f64) {
    output_at(outputs, x, y).map_or((x, y), |index| {
        (
            x - f64::from(outputs[index].x),
            y - f64::from(outputs[index].y),
        )
    })
}

/// Whether a pointer hit lookup may run at `(x, y)`. With a placed row the point must lie on one of
/// its outputs. An empty row (single or nested output) imposes no gate, so that lookup is unchanged.
pub(crate) fn hit_on_layout(stored: &[Rect], x: f64, y: f64) -> bool {
    stored.is_empty() || output_at(stored, x, y).is_some()
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

/// Output a new layer surface maps on: the one it requested when that output is live, else the output
/// under the pointer. `live` lists every driven output, primary included.
pub(crate) fn choose_layer_output<'a>(
    requested: Option<&'a str>,
    live: &[String],
    pointer: &'a str,
) -> &'a str {
    requested
        .filter(|name| live.iter().any(|output| output.as_str() == *name))
        .unwrap_or(pointer)
}

/// Entries homed on `output`, in order. These are the surfaces that leave a removed output.
pub(crate) fn homed_on<T: Clone>(homes: &[(T, String)], output: &str) -> Vec<T> {
    homes
        .iter()
        .filter(|(_, home)| home == output)
        .map(|(surface, _)| surface.clone())
        .collect()
}

/// Whether a surface moving from `previous` to `next` changes output, so `wl_surface.leave` is due.
pub(crate) fn changes_output(previous: &str, next: &str) -> bool {
    previous != next
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

/// Lifts an output-local layer zone (`non_exclusive_zone` of that output's map) to layout
/// coordinates on `output`, clipped to the output.
pub(crate) fn zone_in_layout(output: Rect, zone: Rect) -> Rect {
    let left = (output.x + zone.x).max(output.x);
    let top = (output.y + zone.y).max(output.y);
    let right = (output.x + zone.x + zone.w).min(output.x + output.w);
    let bottom = (output.y + zone.y + zone.h).min(output.y + output.h);
    Rect::new(left, top, (right - left).max(0), (bottom - top).max(0))
}

/// Where a window homed on a removed output lands on `work`: the same offset from the removed
/// output's origin, clamped so it stays inside `work`. A window larger than `work` keeps its size.
pub(crate) fn migrate_rect(rect: Rect, removed: Rect, work: Rect) -> Rect {
    let x = work.x + (rect.x - removed.x).clamp(0, (work.w - rect.w).max(0));
    let y = work.y + (rect.y - removed.y).clamp(0, (work.h - rect.h).max(0));
    Rect::new(x, y, rect.w, rect.h)
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn zone_lifts_to_the_output_origin_and_stays_on_it() {
        let output = Rect::new(1000, 0, 1280, 800);
        // A 32 px top panel on the second output is output-local.
        assert_eq!(
            zone_in_layout(output, Rect::new(0, 32, 1280, 768)),
            Rect::new(1000, 32, 1280, 768)
        );
        // An overhanging zone is clipped to its output.
        assert_eq!(
            zone_in_layout(output, Rect::new(0, 0, 2000, 800)),
            Rect::new(1000, 0, 1280, 800)
        );
        assert_eq!(zone_in_layout(output, Rect::new(0, 0, 0, 0)).w, 0);
    }

    #[test]
    fn migration_keeps_the_relative_position_inside_the_remaining_work_area() {
        let removed = Rect::new(1000, 0, 1280, 800);
        let work = Rect::new(0, 0, 1280, 760);
        assert_eq!(
            migrate_rect(Rect::new(1100, 50, 400, 300), removed, work),
            Rect::new(100, 50, 400, 300)
        );
        assert_eq!(
            migrate_rect(Rect::new(2200, 700, 400, 300), removed, work),
            Rect::new(880, 460, 400, 300)
        );
        assert_eq!(
            migrate_rect(Rect::new(900, -20, 400, 300), removed, work),
            Rect::new(0, 0, 400, 300)
        );
        assert_eq!(
            migrate_rect(Rect::new(1100, 50, 2000, 900), removed, work).x,
            0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_reads_its_own_key_then_the_legacy_wayland_0_key() {
        let mut scales = BTreeMap::new();
        scales.insert("wayland-0".to_owned(), 150);
        // Old config: only wayland-0, so the primary and every secondary keep 150.
        assert_eq!(output_scale_percent(&scales, 0), 150);
        assert_eq!(output_scale_percent(&scales, 2), 150);
        scales.insert("wayland-1".to_owned(), 125);
        assert_eq!(output_scale_percent(&scales, 1), 125);
        assert_eq!(output_scale_percent(&scales, 2), 150);
        // An own key that is invalid is 100, not the legacy value.
        scales.insert("wayland-1".to_owned(), 137);
        assert_eq!(output_scale_percent(&scales, 1), 100);
        assert_eq!(output_scale_percent(&BTreeMap::new(), 3), 100);
    }

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
    fn single_output_stays_at_origin_and_later_outputs_follow_the_row() {
        assert_eq!(origin_after(&[], (1920, 1080)), (0, 0));
        assert_eq!(origin_after(&[(1920, 1080)], (2560, 1440)), (1920, 0));
        let rects = arrange_row(&[(1920, 1080), (2560, 1440), (1280, 1024)]);
        assert_eq!(
            origin_after(&[(1920, 1080), (2560, 1440)], (1280, 1024)),
            (rects[2].x, rects[2].y)
        );
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
    fn physical_size_round_trips_non_exact_scales() {
        // 1919 / 1.25 = 1535.2 -> 1535; 1535 * 1.25 = 1918.75 must come back as 1919, not 1918.
        let logical = logical_size((1919, 1079), 125);
        assert_eq!(logical, (1535, 863));
        assert_eq!(physical_size(logical, 125), (1919, 1079));
        let logical = logical_size((1366, 768), 125);
        assert_eq!(physical_size(logical, 125), (1366, 768));
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
    fn hit_lookup_resolves_against_the_placed_row() {
        let row = arrange_row(&[(100, 100), (100, 50)]);
        assert_eq!(output_at(&row, 150.0, 20.0), Some(1));
        assert!(hit_on_layout(&row, 150.0, 20.0));
        // Below the short second output: a gap, no scene owner.
        assert!(!hit_on_layout(&row, 150.0, 80.0));
        // Single output (no stored row): no gate, even for points off the primary output.
        assert!(hit_on_layout(&[], -40.0, 9000.0));
    }

    #[test]
    fn surface_scale_follows_its_output_and_falls_back_to_primary() {
        let scales = vec![("DP-1".to_owned(), 1.5), ("eDP-1".to_owned(), 1.0)];
        let mut map: SurfaceOutputs<u32> = SurfaceOutputs::default();
        map.assign(1, "DP-1".to_owned());
        assert_eq!(map.output_of(&1), Some("DP-1"));
        assert_eq!(scale_for_output(&scales, map.output_of(&1), 1.0), 1.5);
        assert_eq!(
            scale_for_output(&scales, map.output_of(&2), 1.0),
            1.0,
            "unmapped takes primary"
        );
        assert_eq!(
            scale_for_output(&scales, Some("gone"), 1.25),
            1.25,
            "unknown output takes primary"
        );
        map.remove(&1);
        assert_eq!(map.output_of(&1), None);
        assert_eq!(
            scale_for_output(&scales, map.output_of(&1), 2.0),
            2.0,
            "cleared takes primary"
        );
    }

    #[test]
    fn dragged_window_takes_the_output_with_most_overlap_and_primary_when_none() {
        let row = arrange_row(&[(100, 100), (100, 100)]);
        let names = ["DP-1".to_owned(), "DP-2".to_owned()];
        // Straddles the seam: 30 px on DP-1, 70 px on DP-2, so the window moves to DP-2.
        assert_eq!(
            output_name_for_rect(&row, &names, Rect::new(70, 0, 100, 40), "DP-1"),
            "DP-2"
        );
        // Still on the primary side where it overlaps more.
        assert_eq!(
            output_name_for_rect(&row, &names, Rect::new(40, 0, 100, 40), "DP-1"),
            "DP-1"
        );
        assert_eq!(
            output_name_for_rect(&row, &names, Rect::new(500, 0, 10, 10), "DP-1"),
            "DP-1",
            "no overlap takes primary"
        );
        // Nested session: no row, always primary.
        assert_eq!(
            output_name_for_rect(&[], &[], Rect::new(0, 0, 10, 10), "winit"),
            "winit"
        );
    }

    #[test]
    fn removed_output_resets_its_surfaces_to_the_primary_scale() {
        let scales = vec![("DP-1".to_owned(), 1.5), ("DP-2".to_owned(), 2.0)];
        let mut map: SurfaceOutputs<u32> = SurfaceOutputs::default();
        map.assign(1, "DP-2".to_owned()); // a window on the second output
        map.assign(2, "DP-1".to_owned()); // a layer surface on the first
        assert_eq!(scale_for_output(&scales, map.output_of(&1), 1.0), 2.0);
        map.retain_outputs(&["DP-1".to_owned(), "eDP-1".to_owned()]);
        assert_eq!(map.output_of(&1), None, "gone output is dropped");
        assert_eq!(map.output_of(&2), Some("DP-1"), "live output is kept");
        assert_eq!(scale_for_output(&scales, map.output_of(&1), 1.0), 1.0);
    }

    #[test]
    fn layer_surface_on_a_secondary_output_keeps_that_outputs_scale() {
        let scales = vec![("DP-1".to_owned(), 1.0), ("DP-2".to_owned(), 1.5)];
        let mut map: SurfaceOutputs<u32> = SurfaceOutputs::default();
        map.assign(7, "DP-2".to_owned()); // dock requested on DP-2
        assert_eq!(scale_for_output(&scales, map.output_of(&7), 1.0), 1.5);
    }

    #[test]
    fn pointer_layout_prefers_the_row_and_falls_back_to_the_primary_output() {
        let primary = [Rect::new(0, 0, 1920, 1080)];
        let row = arrange_row(&[(1920, 1080), (1280, 1024)]);
        assert_eq!(pointer_layout(&[], &primary), &primary[..]);
        assert_eq!(pointer_layout(&row, &primary), &row[..]);
        // Single output: an in-bounds point is unchanged; past the edge it lands on the last pixel.
        let single = pointer_layout(&[], &primary);
        assert_eq!(clamp_point_to_layout(10.5, 20.0, single), (10.5, 20.0));
        assert_eq!(clamp_point_to_layout(2500.0, 20.0, single), (1919.0, 20.0));
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

    #[test]
    fn equal_overlap_goes_to_the_lower_index_output() {
        let rects = [Rect::new(0, 0, 100, 100), Rect::new(100, 0, 100, 100)];
        assert_eq!(output_for_rect(&rects, Rect::new(90, 0, 20, 10)), Some(0));
    }

    #[test]
    fn layer_maps_on_its_requested_output_when_that_output_is_live() {
        let live = ["DP-1".to_owned(), "DP-2".to_owned()];
        assert_eq!(choose_layer_output(Some("DP-2"), &live, "DP-1"), "DP-2");
    }

    #[test]
    fn layer_falls_back_to_the_pointer_output_when_request_is_absent_or_gone() {
        let live = ["DP-1".to_owned(), "DP-2".to_owned()];
        assert_eq!(choose_layer_output(None, &live, "DP-2"), "DP-2");
        assert_eq!(choose_layer_output(Some("HDMI-9"), &live, "DP-2"), "DP-2");
        // Nested or single output: the primary is the only live name and the pointer sits on it.
        assert_eq!(choose_layer_output(None, &live[..1], "DP-1"), "DP-1");
    }

    #[test]
    fn removed_output_surfaces_are_the_ones_homed_on_it() {
        let homes = [
            (1_u32, "DP-1".to_owned()),
            (2, "DP-2".to_owned()),
            (3, "DP-2".to_owned()),
        ];
        assert_eq!(homed_on(&homes, "DP-2"), vec![2, 3]);
        assert!(homed_on(&homes, "HDMI-9").is_empty());
    }

    #[test]
    fn leave_is_due_only_when_the_output_actually_changes() {
        assert!(!changes_output("DP-1", "DP-1"));
        assert!(changes_output("DP-1", "DP-2"));
    }

    #[test]
    fn pointer_maps_into_the_output_under_it() {
        let row = [Rect::new(0, 0, 800, 600), Rect::new(800, 0, 800, 600)];
        assert_eq!(relative_to_output(&row, 1000.0, 100.0), (200.0, 100.0));
        assert_eq!(relative_to_output(&row, 10.0, 20.0), (10.0, 20.0));
        // Off every output: unchanged, so the single-output path keeps its coordinates.
        assert_eq!(relative_to_output(&row, -5.0, 20.0), (-5.0, 20.0));
        assert_eq!(relative_to_output(&[], 3.0, 4.0), (3.0, 4.0));
    }
}
