use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

impl RuntimeDocument {
    pub fn set_position_px(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
        if !left.is_finite() || !top.is_finite() {
            return Err("runtime position must be finite".to_string());
        }
        let index = self.require_node(id)?;
        let entry = self.geometry_overrides.entry(index).or_default();
        if entry.left == Some(left) && entry.top == Some(top) {
            return Ok(());
        }
        entry.left = Some(left);
        entry.top = Some(top);
        // Isolated nodes patch the retained layout in place (no layout revision bump); any other
        // node advances the layout revision and forces a full compute.
        self.touch_geometry(index);
        Ok(())
    }

    pub fn set_size_px(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return Err("runtime size must be finite and non-negative".to_string());
        }
        let index = self.require_node(id)?;
        let entry = self.geometry_overrides.entry(index).or_default();
        if entry.width == Some(width) && entry.height == Some(height) {
            return Ok(());
        }
        entry.width = Some(width);
        entry.height = Some(height);
        // Geometry dimensions are part of the cached layout as well. Do not
        // invalidate on repeated size writes from a pointer-motion projector.
        self.touch_geometry(index);
        Ok(())
    }

    fn touch_geometry(&mut self, index: u32) {
        if self.geometry_is_isolated(index) {
            self.geometry_revision = self.geometry_revision.wrapping_add(1);
            self.touch_node(index, DirtyReason::Geometry, false);
        } else {
            self.touch();
        }
    }

    pub fn geometry_override(&self, index: u32) -> Option<GeometryOverride> {
        self.geometry_overrides.get(&index).copied()
    }
}

/// Offset change that brings `node` inside `view` along one axis: 0 when it is already visible.
/// A node taller than the view aligns its start edge, as browsers do for `scrollIntoView`.
pub fn reveal_delta_axis(view_start: f32, view_len: f32, node_start: f32, node_len: f32) -> f32 {
    let view_end = view_start + view_len;
    if node_len > view_len || node_start < view_start {
        node_start - view_start
    } else if node_start + node_len > view_end {
        node_start + node_len - view_end
    } else {
        0.0
    }
}

/// Scroll delta `(dx, dy)` that moves `node` into `view`, both in the same coordinate space.
/// Scrolling by the delta moves the node's on-screen rectangle by `-delta`.
pub fn reveal_delta(view: Rect, node: Rect) -> (f32, f32) {
    (
        reveal_delta_axis(view.x, view.width, node.x, node.width),
        reveal_delta_axis(view.y, view.height, node.y, node.height),
    )
}

#[cfg(test)]
mod reveal_tests {
    use super::{Rect, reveal_delta, reveal_delta_axis};

    #[test]
    fn visible_node_needs_no_scroll() {
        assert_eq!(reveal_delta_axis(0.0, 100.0, 10.0, 20.0), 0.0);
        assert_eq!(
            reveal_delta_axis(0.0, 100.0, 80.0, 20.0),
            0.0,
            "bottom edge touches"
        );
    }

    #[test]
    fn node_below_scrolls_down_just_enough() {
        // Node spans 90..110 in a 0..100 view: 10px more scroll puts its bottom on the edge.
        assert_eq!(reveal_delta_axis(0.0, 100.0, 90.0, 20.0), 10.0);
    }

    #[test]
    fn node_above_scrolls_up_by_its_gap() {
        assert_eq!(reveal_delta_axis(50.0, 100.0, 30.0, 20.0), -20.0);
    }

    #[test]
    fn node_taller_than_view_aligns_its_start_edge() {
        assert_eq!(reveal_delta_axis(0.0, 50.0, 70.0, 80.0), 70.0);
        assert_eq!(reveal_delta_axis(0.0, 50.0, -10.0, 80.0), -10.0);
    }

    #[test]
    fn both_axes_are_independent() {
        let view = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(
            reveal_delta(view, Rect::new(120.0, 95.0, 10.0, 10.0)),
            (30.0, 5.0)
        );
    }
}
