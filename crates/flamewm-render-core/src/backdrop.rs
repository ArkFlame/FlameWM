//! Window-level pseudo-transparency description.
//!
//! Pure data so every backend can name a backdrop without pulling in a renderer. The X11
//! renderer re-exports this type at its historical path; a Wayland renderer consumes the same
//! fields.

/// Window-level pseudo-transparency for a Normal window (for example the explorer): the
/// wallpaper under the whole window, blurred per `blur_percent`, is laid under the window's
/// translucent fills. Wallpaper pixels come from the shared wallpaper owner, never a root read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowBackdrop {
    /// `wallpaper` setting. Empty selects the packaged default wallpaper.
    pub wallpaper: String,
    /// `wallpaperFit` setting text, parsed by `flamewm_image_core::wallpaper::parse`.
    pub fit: String,
    /// Packaged asset root that the default wallpaper resolves against.
    pub packaged_root: std::path::PathBuf,
    /// Surface opacity percent. 100 is opaque and installs no backdrop.
    pub opacity_percent: u8,
    /// Blur percent applied to the snapshot (`blur_radius_px` policy).
    pub blur_percent: u8,
}

impl WindowBackdrop {
    /// Whether translucent fills need a backdrop at all.
    #[must_use]
    pub fn is_translucent(&self) -> bool {
        self.opacity_percent < 100
    }
}

/// Outgoing file-drag request: written by the UI, taken by the event loop after the event
/// that requested it. `Some(paths)` starts a file drag with absolute `paths`. Shared so a
/// document is not tied to one event source.
pub type FileDragSlot = std::rc::Rc<std::cell::RefCell<Option<Vec<std::path::PathBuf>>>>;
