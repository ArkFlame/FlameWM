//! Pure view geometry: zoom, fit, pan clamping and the visible rectangle. No I/O, no UI types.
//! Offsets are whole stage pixels; `zoom` is a multiplier of the source size.

pub const MIN_ZOOM: f64 = 0.02;
pub const MAX_ZOOM: f64 = 32.0;
/// Multiplicative step for `+`/`-` and wheel ticks.
pub const ZOOM_STEP: f64 = 1.25;

/// Displayed size of a `w`x`h` image at `zoom`. Rounded, never zero.
#[must_use]
pub fn display_size(w: u32, h: u32, zoom: f64) -> (u32, u32) {
    let scale = |v: u32| (f64::from(v) * zoom).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    (scale(w), scale(h))
}

/// Zoom that fits the whole image into the stage. Never enlarges: small images show at 100%.
#[must_use]
pub fn fit_zoom(w: u32, h: u32, stage: (u32, u32)) -> f64 {
    let zoom = (f64::from(stage.0) / f64::from(w.max(1)))
        .min(f64::from(stage.1) / f64::from(h.max(1)))
        .min(1.0);
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

#[must_use]
pub fn clamp_zoom(zoom: f64) -> f64 {
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// Where the displayed image sits and which part of it is visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Clamped offset of the displayed image's top-left corner relative to the stage.
    pub offset: (i64, i64),
    /// Top-left of the visible part, in displayed-image pixels.
    pub src: (u32, u32),
    /// Top-left of the visible part, in stage pixels.
    pub at: (u32, u32),
    /// Size of the visible part. Never larger than the stage.
    pub size: (u32, u32),
}

#[derive(Clone, Copy)]
struct Axis {
    offset: i64,
    src: i64,
    at: i64,
    len: i64,
}

fn axis(stage: u32, displayed: u32, offset: i64) -> Axis {
    let (s, d) = (i64::from(stage.max(1)), i64::from(displayed.max(1)));
    // Smaller than the stage: centred. Larger: the offset may only range over [s - d, 0].
    let offset = if d <= s { (s - d) / 2 } else { offset.clamp(s - d, 0) };
    let src = (-offset).max(0);
    let at = offset.max(0);
    Axis {
        offset,
        src,
        at,
        len: (s - at).min(d - src),
    }
}

/// Clamps `offset` for a `displayed` image in a `stage` and returns the visible rectangle.
#[must_use]
pub fn place(stage: (u32, u32), displayed: (u32, u32), offset: (i64, i64)) -> Placement {
    let x = axis(stage.0, displayed.0, offset.0);
    let y = axis(stage.1, displayed.1, offset.1);
    Placement {
        offset: (x.offset, y.offset),
        src: (x.src as u32, y.src as u32),
        at: (x.at as u32, y.at as u32),
        size: (x.len.max(1) as u32, y.len.max(1) as u32),
    }
}

/// New offset after zooming from `old` to `new` so the image point under stage `point` stays put.
#[must_use]
pub fn zoom_about(offset: (i64, i64), old: f64, new: f64, point: (f64, f64)) -> (i64, i64) {
    let ratio = new / old;
    let shift = |p: f64, o: i64| (p - (p - o as f64) * ratio).round() as i64;
    (shift(point.0, offset.0), shift(point.1, offset.1))
}

/// Next or previous index in a list of `len` items, wrapping at both ends. `len` must be non-zero.
#[must_use]
pub fn step_index(len: usize, index: usize, forward: bool) -> usize {
    if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_size_rounds_and_never_zero() {
        assert_eq!(display_size(1000, 500, 0.5), (500, 250));
        assert_eq!(display_size(3, 3, 0.02), (1, 1));
        assert_eq!(display_size(10, 4, 2.0), (20, 8));
    }

    #[test]
    fn fit_never_enlarges_and_fills_one_axis() {
        assert_eq!(fit_zoom(100, 100, (1000, 1000)), 1.0);
        assert_eq!(fit_zoom(4000, 2000, (1000, 1000)), 0.25);
        assert_eq!(fit_zoom(1000, 4000, (1000, 500)), 0.125);
    }

    #[test]
    fn small_image_is_centred_and_not_panned() {
        let p = place((800, 600), (200, 100), (5000, -5000));
        assert_eq!(p.offset, (300, 250));
        assert_eq!(p.at, (300, 250));
        assert_eq!(p.src, (0, 0));
        assert_eq!(p.size, (200, 100));
    }

    #[test]
    fn large_image_offset_is_clamped_to_its_edges() {
        let p = place((800, 600), (2000, 1500), (100, -10_000));
        assert_eq!(p.offset, (0, -900));
        assert_eq!(p.at, (0, 0));
        assert_eq!(p.src, (0, 900));
        assert_eq!(p.size, (800, 600));
        let p = place((800, 600), (2000, 1500), (-10_000, 50));
        assert_eq!(p.offset, (-1200, 0));
        assert_eq!(p.src, (1200, 0));
    }

    #[test]
    fn visible_rect_stays_inside_stage_when_image_is_offset() {
        let p = place((800, 600), (2000, 1500), (-100, -100));
        assert_eq!(p.src, (100, 100));
        assert_eq!(p.size, (800, 600));
        let p = place((800, 600), (900, 500), (300, 0));
        assert_eq!(p.offset, (0, 50));
        assert_eq!(p.at, (0, 50));
        assert_eq!(p.size, (800, 500));
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let point = (400.0, 300.0);
        let offset = (-100, -50);
        let new = zoom_about(offset, 1.0, 2.0, point);
        // Image point under the pointer: x (400 - -100) = 500 at 1x, 1000 at 2x -> offset 400 - 1000.
        // y (300 - -50) = 350 at 1x, 700 at 2x -> offset 300 - 700.
        assert_eq!(new, (-600, -400));
    }

    #[test]
    fn index_wraps_both_ways() {
        assert_eq!(step_index(3, 2, true), 0);
        assert_eq!(step_index(3, 0, false), 2);
        assert_eq!(step_index(3, 1, true), 2);
    }
}
