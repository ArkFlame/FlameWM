//! Window-level pseudo-transparency: what lies under a whole Normal window.
//!
//! Owns the window snapshot lifecycle (capture, move/resize policy, teardown). The snapshot
//! lives in its own slot so the popup mask (which keys off `surface_backdrop`) keeps its shape.
//! Paint falls back to this slot only when no popup backdrop is installed.
//!
//! The root content under the window (other windows, desktop icons, wallpaper) is read back
//! with the window hidden and cached as one root-sized image. Moves and resizes then crop the
//! cache at the new position at once; the readback itself runs only after the window settles.

use std::time::{Duration, Instant};

use flamewm_image_core::RgbaImage;

use super::X11App;
use crate::backdrop::{WindowBackdrop, blurred_argb_words, recapture_due, wallpaper_file};
use crate::xlib::*;
use crate::xlib_ext::{XQueryTree, XTranslateCoordinates};

/// Quiet period after the last move or resize before the root under the window is read again.
/// Each configure slides the deadline, so a drag reads the root once when it stops.
pub(crate) const CAPTURE_QUIET: Duration = Duration::from_millis(250);

/// Whether the root cache must be read again: none cached yet or it no longer matches the root
/// size, or the window has settled after a move/resize that left the cache stale.
#[must_use]
pub(crate) fn desktop_readback_due(cached: bool, settled: bool, stale: bool) -> bool {
    !cached || (settled && stale)
}

/// Per-window state: the spec, the decoded wallpaper cache, and whether a capture is due.
#[derive(Debug)]
pub(crate) struct WindowBackdropState {
    pub(crate) spec: WindowBackdrop,
    natural: Option<RgbaImage>,
    /// Wallpaper composed over the whole root for (root w, root h, fit); used when no root
    /// readback is cached.
    canvas: Option<(u32, u32, String, RgbaImage)>,
    /// Root pixels under the window as last read with the window hidden (root-sized).
    desktop: Option<RgbaImage>,
    /// The root changed since `desktop` was read (a move or resize happened).
    pub(crate) desktop_stale: bool,
    pub(crate) stale: bool,
    /// Last blurred snapshot and the root origin it was captured at. A resized window mirror-extends
    /// it into the grown area until the settled capture replaces it.
    pub(crate) captured: Option<CapturedSnapshot>,
    /// Mirror-extended fill of `captured` for the last (size, origin); reused while it matches.
    pub(crate) mirror: Option<MirrorFill>,
}

/// Blurred ARGB snapshot as uploaded, with the root position of its top-left pixel.
#[derive(Debug)]
pub(crate) struct CapturedSnapshot {
    pub(crate) words: Vec<u32>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) root: (i32, i32),
}

/// Scene size and snapshot origin (snapshot root position minus the window's root position)
/// that a mirrored fill was computed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MirrorKey {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) origin: (i64, i64),
}

/// Cached mirrored fill: the pixmap is `key`-sized and holds `mirror_extend_argb` of the snapshot.
#[derive(Debug)]
pub(crate) struct MirrorFill {
    pub(crate) key: MirrorKey,
    pub(crate) pixmap: Pixmap,
}

impl WindowBackdropState {
    pub(crate) fn new(spec: WindowBackdrop) -> Self {
        Self {
            spec,
            natural: None,
            canvas: None,
            desktop: None,
            desktop_stale: true,
            stale: true,
            captured: None,
            mirror: None,
        }
    }
}

impl X11App {
    /// Marks the snapshot out of date after the window moved or resized and restarts the quiet
    /// period. The snapshot is recomposed at once from the cached root; only the root readback
    /// waits for the period to pass.
    pub(crate) fn mark_window_backdrop_stale(&mut self) {
        if let Some(state) = self.window_backdrop.as_mut() {
            state.stale = true;
            state.desktop_stale = true;
            self.backdrop_deadline = Some(Instant::now() + CAPTURE_QUIET);
        }
    }

    /// Recaptures the window snapshot when it is stale and the policy allows it. Capture
    /// failure is nonfatal: the slot is cleared and translucent fills show as straight colour.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread, before the scene is cleared for a paint.
    pub(crate) unsafe fn refresh_window_backdrop(&mut self) {
        let Some(state) = self.window_backdrop.as_ref() else {
            return;
        };
        let root = unsafe { self.root_size() };
        let cached = state
            .desktop
            .as_ref()
            .is_some_and(|desktop| (desktop.width, desktop.height) == root);
        let readback = desktop_readback_due(
            cached,
            self.backdrop_deadline.is_none(),
            state.desktop_stale,
        );
        if !state.stale && !readback {
            return;
        }
        let snapshot_size = self.window_backdrop_pixmap.map(|b| (b.width, b.height));
        let queued = unsafe { XEventsQueued(self.display, crate::xlib::QUEUED_AFTER_READING) } > 0;
        if !recapture_due(snapshot_size, (self.width, self.height), queued) {
            return;
        }
        let Some(state) = self.window_backdrop.as_mut() else {
            return;
        };
        state.stale = false;
        if let Err(error) = unsafe { self.capture_window_backdrop(readback) } {
            eprintln!("FLAMEWM_WINDOW_BACKDROP degraded reason={error}");
            self.clear_window_backdrop();
        }
    }

    /// Root (screen) size in device pixels.
    unsafe fn root_size(&self) -> (u32, u32) {
        (
            unsafe { XDisplayWidth(self.display, self.screen) }.max(0) as u32,
            unsafe { XDisplayHeight(self.display, self.screen) }.max(0) as u32,
        )
    }

    /// Root pixels under the window's rect, blurred and uploaded as the window snapshot. `readback`
    /// re-reads the whole root with the window hidden first; otherwise the cached root is cropped.
    unsafe fn capture_window_backdrop(&mut self, readback: bool) -> Result<(), String> {
        let _capture_span = flamewm_profiler::start("render.backdrop.capture");
        let (root_w, root_h) = unsafe { self.root_size() };
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
        // Compose only the root-visible part; the window may be dragged partly off the root.
        // Off-root pixels replicate the nearest root edge (never an opaque black slot).
        let anchor = crate::backdrop::root_anchor_rect(rect, (root_w, root_h));
        if readback {
            // A failed readback leaves no cache: this capture falls back to the wallpaper and the
            // next refresh retries.
            let _readback_span = flamewm_profiler::start("render.backdrop.readback");
            let desktop = unsafe { self.read_root_behind_window((root_w, root_h)) }.ok();
            if let Some(state) = self.window_backdrop.as_mut() {
                state.desktop = desktop;
                state.desktop_stale = false;
            }
        }
        let state = self
            .window_backdrop
            .as_mut()
            .ok_or("window backdrop: no spec")?;
        let snapshot = match state.desktop.as_ref() {
            Some(desktop) => crop_rgba(desktop, anchor)
                .ok_or("window backdrop: root region unavailable")?,
            None => wallpaper_region(state, (root_w, root_h), anchor)?,
        };
        let shift = (
            i64::from(rx) - i64::from(anchor.0),
            i64::from(ry) - i64::from(anchor.1),
        );
        let (width, height) = (self.width, self.height);
        let pixels = crate::backdrop::edge_extend_rgba(
            &snapshot.pixels,
            snapshot.width,
            snapshot.height,
            shift,
            (width, height),
        );
        let blur_span = flamewm_profiler::start("render.backdrop.blur");
        let words = blurred_argb_words(width, height, pixels, 0, state.spec.blur_percent);
        drop(blur_span);
        if words.len() != width as usize * height as usize {
            return Err("window backdrop: snapshot size mismatch".to_string());
        }
        if let Some(state) = self.window_backdrop.as_mut() {
            state.captured = Some(CapturedSnapshot {
                words: words.clone(),
                width,
                height,
                root: (rx, ry),
            });
        }
        // The cached mirror was made from the previous snapshot.
        self.drop_mirror_fill();
        let upload_span = flamewm_profiler::start("render.backdrop.upload");
        let pixmap = unsafe { self.upload_argb_pixmap(&words, width, height)? };
        drop(upload_span);
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

    /// Root pixels with this window hidden, so the readback holds what lies behind it (other
    /// windows, desktop icons, wallpaper). The bounding shape is emptied and then restored, which
    /// sends the WM no UnmapNotify. Without a shape bridge the window could not be hidden.
    unsafe fn read_root_behind_window(&mut self, root: (u32, u32)) -> Result<RgbaImage, String> {
        let bridge = self
            .xshape
            .as_ref()
            .ok_or("window backdrop: no shape bridge to hide the window")?;
        // The WM frame is an ancestor that paints chrome over the client area, so hide the whole chain.
        let root_window = unsafe { XRootWindow(self.display, self.screen) };
        let mut saved: Vec<(Window, Vec<crate::xshape::XRectangle>)> = Vec::new();
        for window in unsafe { self.window_and_ancestors(root_window) } {
            let rects = unsafe { bridge.bounding_rects(window) };
            if rects.is_empty() {
                for (hidden, rects) in &saved {
                    unsafe { bridge.set_bounding_rects(*hidden, rects) };
                }
                return Err("window backdrop: window shape unavailable".to_string());
            }
            unsafe { bridge.set_bounding_rects(window, &[]) };
            saved.push((window, rects));
        }
        unsafe { XSync(self.display, 0) };
        let capture = crate::backdrop::capture_drawable_rect(
            self.display,
            root_window as u64,
            (0, 0, root.0, root.1),
        );
        for (window, rects) in &saved {
            unsafe { bridge.set_bounding_rects(*window, rects) };
        }
        unsafe { XFlush(self.display) };
        let capture = capture?;
        Ok(RgbaImage {
            width: capture.width,
            height: capture.height,
            pixels: capture.pixels,
        })
    }

    /// The window followed by its ancestors below the root (the WM frame chain).
    unsafe fn window_and_ancestors(&self, root: Window) -> Vec<Window> {
        let mut chain = vec![self.window];
        let mut current = self.window;
        loop {
            let (mut queried_root, mut parent, mut children, mut count) = (0, 0, std::ptr::null_mut(), 0);
            unsafe {
                XQueryTree(self.display, current, &mut queried_root, &mut parent, &mut children, &mut count)
            };
            if !children.is_null() {
                unsafe { XFree(children.cast()) };
            }
            if parent == 0 || parent == root {
                return chain;
            }
            chain.push(parent);
            current = parent;
        }
    }

    /// Frees the window snapshot, if any. Translucent fills then show as straight colour.
    pub(crate) fn clear_window_backdrop(&mut self) {
        if let Some(old) = self.window_backdrop_pixmap.take() {
            unsafe { XFreePixmap(self.display, old.pixmap) };
        }
        if let Some(state) = self.window_backdrop.as_mut() {
            state.desktop = None;
        }
        self.drop_mirror_fill();
    }

    /// Retains `fill` as the cached mirror, freeing the pixmap it replaces.
    pub(crate) fn store_mirror_fill(&mut self, fill: MirrorFill) {
        let display = self.display;
        match self.window_backdrop.as_mut() {
            Some(state) => {
                if let Some(old) = state.mirror.replace(fill) {
                    unsafe { XFreePixmap(display, old.pixmap) };
                }
            }
            None => {
                unsafe { XFreePixmap(display, fill.pixmap) };
            }
        }
    }

    /// Frees the cached mirrored fill, if any.
    pub(crate) fn drop_mirror_fill(&mut self) {
        let display = self.display;
        if let Some(old) = self.window_backdrop.as_mut().and_then(|s| s.mirror.take()) {
            unsafe { XFreePixmap(display, old.pixmap) };
        }
    }
}

/// Wallpaper region under `rect` for when no root readback is cached. The composed root canvas
/// depends only on the wallpaper, the root size and the fit, so it is built once and each
/// capture crops it.
fn wallpaper_region(
    state: &mut WindowBackdropState,
    root: (u32, u32),
    rect: (i32, i32, u32, u32),
) -> Result<RgbaImage, String> {
    let _compose_span = flamewm_profiler::start("render.backdrop.compose");
    if state.natural.is_none() {
        let path = wallpaper_file(&state.spec.wallpaper, &state.spec.packaged_root);
        state.natural = Some(flamewm_image_core::wallpaper::decode_file(&path)?);
    }
    let fit = flamewm_image_core::wallpaper::parse(&state.spec.fit);
    let natural = state
        .natural
        .as_ref()
        .ok_or("window backdrop: wallpaper missing")?;
    let canvas_fresh = state
        .canvas
        .as_ref()
        .is_some_and(|(w, h, f, _)| (*w, *h, f) == (root.0, root.1, &state.spec.fit));
    if !canvas_fresh {
        let full = flamewm_image_core::wallpaper::compose_region(
            natural,
            root.0,
            root.1,
            fit,
            (0, 0, root.0, root.1),
        )
        .ok_or("window backdrop: wallpaper canvas unavailable")?;
        state.canvas = Some((root.0, root.1, state.spec.fit.clone(), full));
    }
    let canvas = &state.canvas.as_ref().expect("canvas built above").3;
    crop_rgba(canvas, rect).ok_or_else(|| "window backdrop: wallpaper region unavailable".to_string())
}

/// Crop `rect` out of a root-sized RGBA image; `None` when the rect leaves the image.
fn crop_rgba(image: &RgbaImage, rect: (i32, i32, u32, u32)) -> Option<RgbaImage> {
    let (x, y, w, h) = rect;
    let (x, y) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
    if x.checked_add(w)? > image.width || y.checked_add(h)? > image.height {
        return None;
    }
    let stride = image.width as usize * 4;
    let row = w as usize * 4;
    let mut pixels = Vec::with_capacity(row * h as usize);
    for line in y as usize..(y + h) as usize {
        let start = line * stride + x as usize * 4;
        pixels.extend_from_slice(&image.pixels[start..start + row]);
    }
    Some(RgbaImage { width: w, height: h, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_read_first_then_only_after_the_window_settles() {
        assert!(desktop_readback_due(false, false, false), "no cache yet: read even while moving");
        assert!(!desktop_readback_due(true, false, true), "moving: crop the cached root");
        assert!(!desktop_readback_due(true, true, false), "settled, nothing moved: keep cache");
        assert!(desktop_readback_due(true, true, true), "settled after a move: read once");
    }
}

