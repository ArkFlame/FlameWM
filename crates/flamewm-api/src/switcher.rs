//! Alt+Tab switcher thumbnail as the WM publishes it to the shell.

/// One window preview: straight RGBA8, row-major, `rgba.len() == width * height * 4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherThumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl SwitcherThumb {
    /// True when the pixel buffer matches the declared size.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        u64::from(self.width) * u64::from(self.height) * 4 == self.rgba.len() as u64
    }
}
