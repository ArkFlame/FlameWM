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
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

impl RuntimeDocument {
    pub fn set_position_px(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
        if !left.is_finite() || !top.is_finite() {
            return Err("runtime position must be finite".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        let entry = self.geometry_overrides.entry(index).or_default();
        entry.left = Some(left);
        entry.top = Some(top);
        Ok(())
    }

    pub fn set_size_px(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return Err("runtime size must be finite and non-negative".to_string());
        }
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        let entry = self.geometry_overrides.entry(index).or_default();
        entry.width = Some(width);
        entry.height = Some(height);
        Ok(())
    }

    pub fn clear_geometry_override(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .node_by_id(id)
            .ok_or_else(|| format!("no node with id '{id}'"))?;
        self.geometry_overrides.remove(&index);
        Ok(())
    }

    pub fn geometry_override(&self, index: u32) -> Option<GeometryOverride> {
        self.geometry_overrides.get(&index).copied()
    }
}
