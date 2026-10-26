#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> i32 {
        self.x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
    }

    pub fn bottom(self) -> i32 {
        self.y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }

    pub fn clamp_inside(self, bounds: Self) -> Self {
        let width = self.width.min(bounds.width.max(1));
        let height = self.height.min(bounds.height.max(1));
        let max_x = bounds
            .right()
            .saturating_sub(i32::try_from(width).unwrap_or(i32::MAX));
        let max_y = bounds
            .bottom()
            .saturating_sub(i32::try_from(height).unwrap_or(i32::MAX));
        Self {
            x: self.x.clamp(bounds.x, max_x.max(bounds.x)),
            y: self.y.clamp(bounds.y, max_y.max(bounds.y)),
            width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_handles_negative_monitor_origin() {
        let bounds = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(
            Rect::new(-2500, -100, 800, 600).clamp_inside(bounds),
            Rect::new(-1920, 0, 800, 600)
        );
    }
}
