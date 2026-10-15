//! Rounded-rect corner geometry shared by the XShape mask, the external fill path and the
//! XRender anti-aliased fills. One owner for the quarter-circle inset formula.

/// Corner radius clamped to half the smaller side.
pub(crate) fn clamp_radius(width: u32, height: u32, radius: u32) -> u32 {
    radius.min(width.min(height) / 2)
}

/// Horizontal inset of row `row` for a rounded rect of `width`x`height` with corner radius
/// `radius`: floor(r - dx) at pixel-row centers. Top and bottom corners fold symmetrically.
pub(crate) fn row_inset(width: u32, height: u32, radius: u32, row: u32) -> u32 {
    if radius <= 1 || width == 0 || height == 0 {
        return 0;
    }
    let corner_row = row.min(height.saturating_sub(1).saturating_sub(row));
    if corner_row >= radius {
        return 0;
    }
    let r = f64::from(radius);
    let dy = r - (f64::from(corner_row) + 0.5);
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    (r - dx).floor().clamp(0.0, r) as u32
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
