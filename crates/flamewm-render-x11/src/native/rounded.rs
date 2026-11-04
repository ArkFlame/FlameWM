//! Rounded-rect corner geometry shared by the XShape mask, the external fill path and the
//! XRender anti-aliased fills. One owner for the quarter-circle inset formula.

/// Corner radius clamped to half the smaller side.
pub(crate) fn clamp_radius(width: u32, height: u32, radius: u32) -> u32 {
    radius.min(width.min(height) / 2)
}

/// Horizontal offset from the corner edge to the circle at `distance_from_corner`.
fn corner_offset(radius: u32, distance_from_corner: f64) -> f64 {
    let r = f64::from(radius);
    let dy = r - distance_from_corner;
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    (r - dx).clamp(0.0, r)
}

/// Inset of one corner row at the pixel-row center: floor(r - dx).
fn corner_inset(radius: u32, corner_row: u32) -> u32 {
    if corner_row >= radius {
        return 0;
    }
    corner_offset(radius, f64::from(corner_row) + 0.5).floor() as u32
}

/// Horizontal inset of row `row` for a rounded rect of `width`x`height` with corner radius
/// `radius`: floor(r - dx) at pixel-row centers. Top and bottom corners fold symmetrically.
pub(crate) fn row_inset(width: u32, height: u32, radius: u32, row: u32) -> u32 {
    if radius <= 1 || width == 0 || height == 0 {
        return 0;
    }
    corner_inset(
        radius,
        row.min(height.saturating_sub(1).saturating_sub(row)),
    )
}

/// Horizontal inset of row `row` when only the bottom corners are rounded, for a mask that must
/// not cover any antialiased ring pixel. The frame-band client window has its top edge under the
/// titlebar, so its top arc is never visible. The row is sampled at the edge nearer the corner and
/// the boundary is rounded outward (ceil), so a partially covered ring pixel stays outside the
/// client instead of being clipped by it.
pub(crate) fn bottom_row_inset(width: u32, height: u32, radius: u32, row: u32) -> u32 {
    if radius <= 1 || width == 0 || height == 0 {
        return 0;
    }
    let corner_row = height.saturating_sub(1).saturating_sub(row);
    if corner_row >= radius {
        return 0;
    }
    corner_offset(radius, f64::from(corner_row)).ceil() as u32
}

/// Row spans (y, x_start, x_end_exclusive) for a rounded rect; empty when either side is zero.
pub(crate) fn spans(width: u32, height: u32, radius: u32) -> Vec<(u32, u32, u32)> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    (0..height)
        .map(|row| {
            let start = row_inset(width, height, radius, row).min(width);
            let end = width.saturating_sub(start).max(start);
            (row, start, end)
        })
        .collect()
}
