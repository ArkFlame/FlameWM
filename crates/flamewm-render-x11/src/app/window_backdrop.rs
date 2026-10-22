//! Window-level pseudo-transparency: the wallpaper under a whole Normal window.
//!
//! Owns the window snapshot lifecycle (capture, move/resize policy, teardown). The snapshot
//! lives in its own slot so the popup mask (which keys off `surface_backdrop`) keeps its shape.
//! Paint falls back to this slot only when no popup backdrop is installed.

use flamewm_image_core::RgbaImage;

use super::X11App;
use crate::backdrop::{WindowBackdrop, blurred_argb_words, recapture_due, wallpaper_file};
use crate::xlib::*;
use crate::xlib_ext::XTranslateCoordinates;

/// Per-window state: the spec, the decoded wallpaper cache, and whether a capture is due.
#[derive(Debug)]
pub(crate) struct WindowBackdropState {
    pub(crate) spec: WindowBackdrop,
    natural: Option<RgbaImage>,
    pub(crate) stale: bool,
}

impl WindowBackdropState {
    pub(crate) fn new(spec: WindowBackdrop) -> Self {
        Self {
            spec,
            natural: None,
            stale: true,
        }
    }
}

impl X11App {
    /// Marks the snapshot out of date after the window moved or resized.
    pub(crate) fn mark_window_backdrop_stale(&mut self) {
        if let Some(state) = self.window_backdrop.as_mut() {
            state.stale = true;
        }
    }

    /// Recaptures the window snapshot when it is stale and the policy allows it. Capture
    /// failure is nonfatal: the slot is cleared and translucent fills show as straight colour.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread, before the scene is cleared for a paint.
    pub(crate) unsafe fn refresh_window_backdrop(&mut self) {
        if !self
            .window_backdrop
            .as_ref()
            .is_some_and(|state| state.stale)
        {
            return;
        }
        let snapshot_size = self.window_backdrop_pixmap.map(|b| (b.width, b.height));
        let queued = unsafe { XEventsQueued(self.display, crate::xlib::QUEUED_AFTER_READING) } > 0;
        if !recapture_due(snapshot_size, (self.width, self.height), queued) {
            return;
        }
        if let Some(state) = self.window_backdrop.as_mut() {
            state.stale = false;
        }
        if let Err(error) = unsafe { self.capture_window_backdrop() } {
            eprintln!("FLAMEWM_WINDOW_BACKDROP degraded reason={error}");
            self.clear_window_backdrop();
        }
    }

    /// Wallpaper under the window's root rect, blurred and uploaded as the window snapshot.
    unsafe fn capture_window_backdrop(&mut self) -> Result<(), String> {
        let (root_w, root_h) = (
            unsafe { XDisplayWidth(self.display, self.screen) }.max(0) as u32,
            unsafe { XDisplayHeight(self.display, self.screen) }.max(0) as u32,
        );
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let (mut rx, mut ry, mut child) = (0, 0, 0);
        unsafe {
            XTranslateCoordinates(
                self.display,
                self.window,
                root,
                0,
                0,
                &mut rx,
                &mut ry,
                &mut child,
            )
        };
        let rect = (rx, ry, self.width, self.height);
        let state = self
            .window_backdrop
            .as_mut()
            .ok_or("window backdrop: no spec")?;
        if state.natural.is_none() {
            let path = wallpaper_file(&state.spec.wallpaper, &state.spec.packaged_root);
            state.natural = Some(flamewm_image_core::wallpaper::decode_file(&path)?);
        }
        let fit = flamewm_image_core::wallpaper::parse(&state.spec.fit);
        let natural = state
            .natural
            .as_ref()
            .ok_or("window backdrop: wallpaper missing")?;
        let snapshot =
            flamewm_image_core::wallpaper::compose_region(natural, root_w, root_h, fit, rect)
                .ok_or("window backdrop: wallpaper region unavailable")?;
        let words = blurred_argb_words(
            snapshot.width,
            snapshot.height,
            snapshot.pixels,
            0,
            state.spec.blur_percent,
        );
        let (width, height) = (snapshot.width, snapshot.height);
        if words.len() != width as usize * height as usize {
            return Err("window backdrop: snapshot size mismatch".to_string());
        }
        let pixmap = unsafe { self.upload_argb_pixmap(&words, width, height)? };
        if let Some(old) =
            self.window_backdrop_pixmap
                .replace(super::surface_backdrop::SurfaceBackdrop {
                    pixmap,
                    width,
                    height,
                })
        {
            unsafe { XFreePixmap(self.display, old.pixmap) };
        }
        Ok(())
    }

    /// Frees the window snapshot, if any. Translucent fills then show as straight colour.
    pub(crate) fn clear_window_backdrop(&mut self) {
        if let Some(old) = self.window_backdrop_pixmap.take() {
            unsafe { XFreePixmap(self.display, old.pixmap) };
        }
    }
}
