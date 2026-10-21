//! Anti-aliased rounded-rect fill and stroke over the pinned `XRenderBackend`.
//!
//! Spans are emitted through the backend's public `fill_rounded_rect` with
//! radius 0, which is its plain solid-rectangle composite path. The pinned
//! `xrender.rs` therefore stays byte-identical to its accepted baseline.

use flamewm_render_core::{Color, Rect};

use crate::xrender::XRenderBackend;

/// Sub-scanlines per corner row. Boundary coverage is the mean over these
/// samples, so it is deterministic and costs O(radius) per corner, not per pixel.
const EDGE_SAMPLES: u32 = 4;

/// Horizontal run `(start_x, len, coverage)` within one row; coverage in (0, 1].
type Run = (u32, u32, f32);

/// Fills a device-pixel rect with a solid color through the pinned backend.
///
/// # Safety
///
/// Same contract as `XRenderBackend::fill_rounded_rect`.
unsafe fn fill_box(
    backend: &mut XRenderBackend,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color,
) -> Result<(), String> {
    let rect = Rect {
        x: x as f32,
        y: y as f32,
        width: width as f32,
        height: height as f32,
    };
    // SAFETY: the caller validated the rect and the backend contract is forwarded.
    unsafe { backend.fill_rounded_rect(rect, 0.0, color) }
}

/// Fills each run of device row `row` at origin `(x, y)`; `unsafe` as `fill_box`.
unsafe fn paint_runs(
    backend: &mut XRenderBackend,
    (x, y): (i32, i32),
    row: u32,
    runs: &[Run],
    color: Color,
) -> Result<(), String> {
    for &(start, len, coverage) in runs {
        unsafe {
            fill_box(
                backend,
                x + start as i32,
                y + row as i32,
                len,
                1,
                scaled(color, coverage),
            )?
        };
    }
    Ok(())
}

/// Per-sub-scanline left boundary offsets (device px, from the corner row's left)
/// of a rounded corner, or `None` when `row` is not a rounded corner row.
fn corner_edges(height: u32, radius: u32, row: u32) -> Option<[f64; EDGE_SAMPLES as usize]> {
    if radius <= 1 || height == 0 {
        return None;
    }
    let corner_row = row.min(height.saturating_sub(1).saturating_sub(row));
    if corner_row >= radius {
        return None;
    }
    let r = f64::from(radius);
    let mut edges = [0.0; EDGE_SAMPLES as usize];
    for (s, edge) in edges.iter_mut().enumerate() {
        let y = f64::from(corner_row) + (s as f64 + 0.5) / f64::from(EDGE_SAMPLES);
        let dy = r - y;
        let dx = (r * r - dy * dy).max(0.0).sqrt();
        *edge = r - dx;
    }
    Some(edges)
}

/// Analytic-by-sample coverage of left-side pixel column `col` on `row`.
fn left_alpha(height: u32, radius: u32, row: u32, col: u32) -> f32 {
    match corner_edges(height, radius, row) {
        None => 1.0,
        Some(edges) => {
            let sum: f64 = edges
                .iter()
                .map(|&edge| (f64::from(col) + 1.0 - edge).clamp(0.0, 1.0))
                .sum();
            (sum / f64::from(EDGE_SAMPLES)) as f32
        }
    }
}

/// `(lo, full)`: columns `lo..full` on the left are partially covered; columns
/// `>= full` are fully covered. Both are `(0, 0)` for non-corner rows.
fn left_run(height: u32, radius: u32, row: u32) -> (u32, u32) {
    match corner_edges(height, radius, row) {
        None => (0, 0),
        Some(edges) => {
            let min = edges.iter().copied().fold(f64::INFINITY, f64::min);
            let max = edges.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let cap = f64::from(radius);
            (
                min.floor().clamp(0.0, cap) as u32,
                max.ceil().clamp(0.0, cap) as u32,
            )
        }
    }
}

/// Appends a run, merging contiguous fully covered runs to keep requests low.
fn push_run(runs: &mut Vec<Run>, start: u32, len: u32, coverage: f32) {
    let coverage = coverage.clamp(0.0, 1.0);
    if len == 0 || coverage <= 0.0 {
        return;
    }
    if coverage >= 1.0 {
        if let Some(last) = runs.last_mut() {
            if last.2 >= 1.0 && last.0 + last.1 == start {
                last.1 += len;
                return;
            }
        }
    }
    runs.push((start, len, coverage));
}

/// Runs covering one row of a filled `width x height` rounded rect.
fn fill_row_runs(width: u32, height: u32, radius: u32, row: u32) -> Vec<Run> {
    let mut runs = Vec::new();
    if corner_edges(height, radius, row).is_none() {
        push_run(&mut runs, 0, width, 1.0);
        return runs;
    }
    let (lo, full) = left_run(height, radius, row);
    for col in lo..full {
        push_run(&mut runs, col, 1, left_alpha(height, radius, row, col));
    }
    push_run(&mut runs, full, width.saturating_sub(full * 2), 1.0);
    for col in (lo..full).rev() {
        push_run(
            &mut runs,
            width - 1 - col,
            1,
            left_alpha(height, radius, row, col),
        );
    }
    runs
}

/// Runs covering one row of a stroked rounded rect: outer boundary minus the
/// inner rounded boundary, both anti-aliased. Interior (between inner edges) is empty.
fn stroke_row_runs(width: u32, height: u32, radius: u32, thickness: u32, row: u32) -> Vec<Run> {
    let inner_present = row >= thickness
        && row < height.saturating_sub(thickness)
        && width > thickness * 2
        && height > thickness * 2;
    if !inner_present {
        return fill_row_runs(width, height, radius, row);
    }
    let inner_h = height - thickness * 2;
    let inner_r = radius.saturating_sub(thickness);
    let inner_row = row - thickness;
    let (_, full) = left_run(height, radius, row);
    let (_, inner_full) = left_run(inner_h, inner_r, inner_row);
    let ring_hi = full.max(thickness + inner_full);
    let ring_alpha = |col: u32| {
        let outer = left_alpha(height, radius, row, col);
        let inner = if col < thickness {
            0.0
        } else {
            left_alpha(inner_h, inner_r, inner_row, col - thickness)
        };
        (outer - inner).clamp(0.0, 1.0)
    };
    let mut runs = Vec::new();
    for col in 0..ring_hi {
        push_run(&mut runs, col, 1, ring_alpha(col));
    }
    for col in (0..ring_hi).rev() {
        push_run(&mut runs, width - 1 - col, 1, ring_alpha(col));
    }
    runs
}

/// Scales the straight-alpha fill by boundary coverage; full coverage is unchanged.
fn scaled(color: Color, coverage: f32) -> Color {
    Color {
        a: (f32::from(color.a) * coverage.clamp(0.0, 1.0)).round() as u8,
        ..color
    }
}

/// Anti-aliased replacement for `XRenderBackend::fill_rounded_rect`.
///
/// # Safety
///
/// Same contract as `XRenderBackend::fill_rounded_rect`.
pub(crate) unsafe fn fill_rounded_rect(
    backend: &mut XRenderBackend,
    rect: Rect,
    radius: f32,
    color: Color,
) -> Result<(), String> {
    let (x, y) = (rect.x.round() as i32, rect.y.round() as i32);
    let (width, height) = (rect.width.round() as u32, rect.height.round() as u32);
    if width == 0 || height == 0 || color.a == 0 {
        return Ok(());
    }
    let radius =
        crate::native::rounded::clamp_radius(width, height, radius.round().max(0.0) as u32);
    if radius <= 1 {
        // SAFETY: validated inputs above.
        return unsafe { fill_box(backend, x, y, width, height, color) };
    }

    // Paint each destination pixel at most once. With PictOpOver, overlapping
    // translucent rectangles compound alpha and create bright vertical/horizontal
    // strips. Keep the large middle band as one request, then rasterize only the
    // rounded corner rows.
    let middle_height = height.saturating_sub(radius * 2);
    if middle_height > 0 {
        // SAFETY: validated inputs above.
        unsafe { fill_box(backend, x, y + radius as i32, width, middle_height, color)? };
    }
    for row in 0..radius {
        let bottom_row = height - 1 - row;
        let runs = fill_row_runs(width, height, radius, row);
        // SAFETY: validated inputs above.
        unsafe { paint_runs(backend, (x, y), row, &runs, color)? };
        if bottom_row != row {
            let runs = fill_row_runs(width, height, radius, bottom_row);
            // SAFETY: validated inputs above.
            unsafe { paint_runs(backend, (x, y), bottom_row, &runs, color)? };
        }
    }
    Ok(())
}

/// Anti-aliased rounded-rect stroke for the XRender backend.
///
/// # Safety
///
/// `backend` must hold live Xlib/XRender handles for its display.
pub(crate) unsafe fn stroke_rounded_rect(
    backend: &mut XRenderBackend,
    rect: Rect,
    radius: f32,
    stroke_width: f32,
    color: Color,
) -> Result<(), String> {
    let (x, y) = (rect.x.round() as i32, rect.y.round() as i32);
    let (width, height) = (rect.width.round() as u32, rect.height.round() as u32);
    if width == 0 || height == 0 || color.a == 0 {
        return Ok(());
    }
    let thickness = stroke_width.round().clamp(1.0, 8.0) as u32;
    let thickness = thickness.min(width / 2).min(height / 2).max(1);
    let radius =
        crate::native::rounded::clamp_radius(width, height, radius.round().max(0.0) as u32);

    for row in 0..height {
        let runs = stroke_row_runs(width, height, radius, thickness, row);
        // SAFETY: validated inputs above.
        unsafe { paint_runs(backend, (x, y), row, &runs, color)? };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_runs(width: u32, height: u32, radius: u32) -> Vec<Vec<Run>> {
        (0..height)
            .map(|row| fill_row_runs(width, height, radius, row))
            .collect()
    }

    #[test]
    fn floating_panel_round_fill_and_stroke_have_partial_edge_coverage() {
        let filled = all_runs(280, 44, 15);
        let stroked: Vec<_> = (0..44)
            .map(|row| stroke_row_runs(280, 44, 15, 1, row))
            .collect();
        for rows in [&filled, &stroked] {
            assert!(
                rows.iter().flatten().any(|run| run.2 > 0.0 && run.2 < 1.0),
                "antialiased corners must retain fractional coverage"
            );
        }
        assert_eq!(filled[22], vec![(0, 280, 1.0)]);
        assert!(
            filled[0]
                .iter()
                .all(|(x, w, _)| x.saturating_add(*w) <= 280)
        );
        assert!(stroked[22].iter().all(|(_, len, _)| *len <= 2));
    }

    #[test]
    fn corner_boundary_is_partial_and_bounded() {
        let rows = all_runs(22, 22, 9);
        let mut partial = 0;
        for runs in &rows {
            for &(_, len, coverage) in runs {
                assert!(len > 0);
                assert!(coverage > 0.0 && coverage <= 1.0, "coverage {coverage}");
                if coverage < 1.0 {
                    partial += 1;
                }
            }
        }
        assert!(
            partial > 0,
            "22x22 r=9 knob must have anti-aliased edge pixels"
        );
    }

    #[test]
    fn left_edge_coverage_is_monotonic_toward_center() {
        // Moving from the top row toward the middle, the arc recedes, so each
        // left-edge column can only gain coverage.
        for col in 0..9 {
            let mut previous = 0.0_f32;
            for row in 0..9 {
                let alpha = left_alpha(22, 9, row, col);
                assert!(alpha + 1e-6 >= previous, "col {col} row {row}");
                previous = alpha;
            }
        }
    }

    #[test]
    fn corners_are_symmetric_across_quadrants() {
        let rows = all_runs(22, 22, 9);
        for runs in rows.iter().take(9) {
            // Left/right mirror: each run has a counterpart at the mirrored x.
            for &(start, len, coverage) in runs {
                assert!(
                    runs.contains(&(22 - start - len, len, coverage)),
                    "mirror of {start}+{len}"
                );
            }
        }
        for row in 0..11 {
            assert_eq!(rows[row], rows[21 - row], "top/bottom mirror at row {row}");
        }
    }

    #[test]
    fn interior_and_square_rows_are_unchanged() {
        // Middle rows and radius <= 1 stay one full-width opaque run.
        assert_eq!(fill_row_runs(22, 22, 9, 11), vec![(0, 22, 1.0)]);
        assert_eq!(fill_row_runs(22, 22, 0, 0), vec![(0, 22, 1.0)]);
        assert_eq!(fill_row_runs(22, 22, 1, 0), vec![(0, 22, 1.0)]);
        // A corner row keeps a fully covered interior run.
        let top = fill_row_runs(22, 22, 9, 0);
        assert!(
            top.iter()
                .any(|&(start, len, c)| c == 1.0 && start <= 11 && start + len >= 11)
        );
    }

    #[test]
    fn pill_radius_clamps_to_half_extent() {
        // The caller clamps radius to min(width, height) / 2, so a 999px pill is a circle.
        let rows = all_runs(40, 22, 11);
        let top_coverage: f32 = rows[0].iter().map(|&(_, len, c)| len as f32 * c).sum();
        assert!(top_coverage < 40.0 && top_coverage > 0.0);
    }

    #[test]
    fn stroke_ring_is_anti_aliased_and_interior_empty() {
        let middle = stroke_row_runs(22, 22, 9, 1, 11);
        assert_eq!(middle, vec![(0, 1, 1.0), (21, 1, 1.0)]);
        let corner = stroke_row_runs(22, 22, 9, 1, 0);
        assert!(corner.iter().any(|&(_, _, c)| c < 1.0));
        assert!(corner.iter().all(|&(_, _, c)| c > 0.0 && c <= 1.0));
    }

    #[test]
    fn scaled_color_keeps_full_coverage_and_scales_alpha() {
        let color = Color {
            r: 1,
            g: 2,
            b: 3,
            a: 200,
        };
        assert_eq!(scaled(color, 1.0), color);
        assert_eq!(scaled(color, 0.5).a, 100);
    }
}
