//! Renderer-neutral decoded-image validity/tint contract from the native Flame UI backend.

use crate::Color;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImageDescriptor {
    pub width: u32,
    pub height: u32,
    pub intrinsic_width: u32,
    pub intrinsic_height: u32,
    pub byte_count: usize,
    pub tint: Option<Color>,
}

impl ImageDescriptor {
    #[must_use]
    pub const fn valid(self) -> bool {
        self.width != 0
            && self.height != 0
            && self.intrinsic_width != 0
            && self.intrinsic_height != 0
            && self.byte_count != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_intrinsic_size_is_not_drawable_image_metadata() {
        let image = ImageDescriptor {
            width: 32,
            height: 32,
            intrinsic_width: 0,
            intrinsic_height: 32,
            byte_count: 4096,
            tint: None,
        };
        assert!(!image.valid());
    }
}
