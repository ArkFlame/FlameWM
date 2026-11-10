//! Alt+Tab switcher thumbnail as the WM publishes it to the shell.

/// One window preview: straight RGBA8, row-major, `rgba.len() == width * height * 4`.
///
/// `generation` names the preview content on the WM side. The WM sends pixels only when the
/// content changed; an unchanged preview is a reference (`rgba` empty, `generation` set) and the
/// shell reuses the pixels it already holds for that generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherThumb {
    pub width: u32,
    pub height: u32,
    /// Content identity; `0` means untracked and always carries pixels (legacy shape).
    pub generation: u64,
    pub rgba: Vec<u8>,
}

impl SwitcherThumb {
    /// True when the preview carries no pixels and names a generation the shell must already hold.
    #[must_use]
    pub fn is_reference(&self) -> bool {
        self.generation != 0 && self.rgba.is_empty()
    }

    /// True when the pixel buffer matches the declared size, or the preview is a reference.
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        self.is_reference()
            || u64::from(self.width) * u64::from(self.height) * 4 == self.rgba.len() as u64
    }
}
