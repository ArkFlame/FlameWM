/// Viewport used when no span rects are given.
pub const DEFAULT_VIEWPORT: (u32, u32) = (1280, 720);

/// Multiplies the base ribbon speed (145 px/s at screen scale 1) for the default motion.
pub const RIBBON_DEFAULT_SPEED_FACTOR: f64 = 1.35;

/// Axis-aligned rectangle in global output coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x as i64
            && y >= self.y as i64
            && x < self.x as i64 + self.w as i64
            && y < self.y as i64 + self.h as i64
    }
}

/// Ribbons configuration. Values outside the documented ranges are clamped by `Ribbons::new`.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Number of ribbons, clamped to `1..=12`.
    pub num: usize,
    /// Ribbon width in px at 1280x720, clamped to `2.0..=28.0`.
    pub width_px: f64,
    /// Output rects in global coordinates. Empty means a single 1280x720 viewport.
    pub span_rects: Vec<Rect>,
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            num: 6,
            width_px: 11.0,
            span_rects: Vec::new(),
            seed: 1,
        }
    }
}

impl Config {
    /// Clamps `num` to `1..=12` and `width_px` to `2.0..=28.0`; non-finite widths become 11.0.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.num = self.num.clamp(1, 12);
        self.width_px = if self.width_px.is_finite() {
            self.width_px.clamp(2.0, 28.0)
        } else {
            11.0
        };
        self
    }
}

/// Union of all rects, or the default viewport when there are none.
pub(crate) fn union_bounds(rects: &[Rect]) -> Rect {
    let Some(first) = rects.first() else {
        return Rect {
            x: 0,
            y: 0,
            w: DEFAULT_VIEWPORT.0,
            h: DEFAULT_VIEWPORT.1,
        };
    };
    let (mut x0, mut y0) = (first.x as i64, first.y as i64);
    let (mut x1, mut y1) = (x0 + first.w as i64, y0 + first.h as i64);
    for r in &rects[1..] {
        x0 = x0.min(r.x as i64);
        y0 = y0.min(r.y as i64);
        x1 = x1.max(r.x as i64 + r.w as i64);
        y1 = y1.max(r.y as i64 + r.h as i64);
    }
    Rect {
        x: x0 as i32,
        y: y0 as i32,
        w: (x1 - x0).max(0) as u32,
        h: (y1 - y0).max(0) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Rect, union_bounds};

    #[test]
    fn config_clamps_to_documented_ranges() {
        let c = Config {
            num: 99,
            width_px: 100.0,
            ..Config::default()
        }
        .clamped();
        assert_eq!(c.num, 12);
        assert_eq!(c.width_px, 28.0);
        let c = Config {
            num: 0,
            width_px: 0.1,
            ..Config::default()
        }
        .clamped();
        assert_eq!((c.num, c.width_px), (1, 2.0));
        assert_eq!(
            Config {
                width_px: f64::NAN,
                ..Config::default()
            }
            .clamped()
            .width_px,
            11.0
        );
    }

    #[test]
    fn union_bounds_spans_all_rects() {
        let rects = [
            Rect { x: 0, y: 0, w: 1920, h: 1080 },
            Rect { x: 1920, y: -200, w: 1280, h: 1024 },
        ];
        assert_eq!(
            union_bounds(&rects),
            Rect { x: 0, y: -200, w: 3200, h: 1280 }
        );
        assert_eq!(union_bounds(&[]).w, 1280);
    }
}
