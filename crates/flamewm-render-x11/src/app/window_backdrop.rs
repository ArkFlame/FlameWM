//! Window-level pseudo-transparency: what lies behind a whole Normal window.
//!
//! There is no compositor, and the root is never read back: a root read holds other windows as
//! they were at that instant (they move, close or are still unpainted), and baking them in left
//! permanent ghosts and black marks behind the window.
//!
//! When the WM redirects its frames, the backdrop is live: the desktop and the frames stacked
//! below this window, composed on the server from their own pixmaps (see `live_backdrop`) and
//! refreshed when they change, at most every `LIVE_INTERVAL`. Otherwise the window blends over the
//! desktop layer, the blurred wallpaper canvas shared through `backdrop::wallpaper_canvas` (about
//! half a megabyte at 1080p, cached on disk across processes), and a move only crops it.

use super::X11App;
use super::live_backdrop::{LIVE_INTERVAL, LiveScene};
use crate::backdrop::{BlurredCanvas, WindowBackdrop, recapture_due};
use crate::xlib::*;
use crate::xlib_ext::XTranslateCoordinates;

/// Per-window state: the spec, the desktop canvas and the uploaded snapshot.
#[derive(Debug)]
pub(crate) struct WindowBackdropState {
    pub(crate) spec: WindowBackdrop,
    /// Blurred desktop layer under the whole root, built (or loaded) once per root size.
    canvas: Option<BlurredCanvas>,
    /// The window moved or resized since the snapshot was cropped.
    pub(crate) stale: bool,
    /// Last cropped snapshot and the root origin it was cropped at. A resized window mirror-extends
    /// it into the grown area until the settled crop replaces it.
    pub(crate) captured: Option<CapturedSnapshot>,
    /// Mirror-extended fill of `captured` for the last (size, origin); reused while it matches.
    pub(crate) mirror: Option<MirrorFill>,
    /// Bumped exactly when the window backdrop pixels change. A backbuffer painted at generation
    /// `g` holds this backdrop only while the generation is still `g`.
    pub(crate) generation: u64,
    /// Live backdrop of the windows behind; `None` until tried, `Some(None)` when unavailable.
    live: Option<Option<LiveScene>>,
}

/// Cropped ARGB snapshot as uploaded, with the root position of its top-left pixel.
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
            canvas: None,
            stale: true,
            captured: None,
            mirror: None,
            generation: 0,
            live: None,
        }
    }
}

impl X11App {
    /// The window moved or resized: the snapshot is recropped on the next paint.
    pub(crate) fn mark_window_backdrop_stale(&mut self) {
        if let Some(state) = self.window_backdrop.as_mut() {
            state.stale = true;
            state.generation += 1;
        }
    }

    /// Recrops the window snapshot for this paint when the window moved or resized. Failure is
    /// nonfatal: the slot is cleared and translucent fills show as straight colour.
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
        let last_live = self
            .window_backdrop
            .as_ref()
            .and_then(|state| state.live.as_ref()?.as_ref()?.last_refresh);
        if let Some(due) = last_live
            .map(|at| at + LIVE_INTERVAL)
            .filter(|due| *due > std::time::Instant::now())
        {
            // A live refresh ran moments ago: keep the current backdrop and wake when it is due.
            self.backdrop_deadline = Some(self.backdrop_deadline.map_or(due, |at| at.min(due)));
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

    /// Root (screen) size in device pixels.
    unsafe fn root_size(&self) -> (u32, u32) {
        (
            unsafe { XDisplayWidth(self.display, self.screen) }.max(0) as u32,
            unsafe { XDisplayHeight(self.display, self.screen) }.max(0) as u32,
        )
    }

    /// Crops the desktop canvas under the window's rect and uploads it as the window snapshot.
    unsafe fn capture_window_backdrop(&mut self) -> Result<(), String> {
        let _capture_span = flamewm_profiler::start("render.backdrop.capture");
        let root = unsafe { self.root_size() };
        let root_window = unsafe { XRootWindow(self.display, self.screen) };
        let (mut rx, mut ry, mut child) = (0, 0, 0);
        unsafe {
            XTranslateCoordinates(
                self.display,
                self.window,
                root_window,
                0,
                0,
                &mut rx,
                &mut ry,
                &mut child,
            )
        };
        let (width, height) = (self.width, self.height);
        let live = unsafe { self.compose_live_backdrop((rx, ry, width, height)) };
        let words = match live {
            Some(words) => words,
            None => {
                let state = self
                    .window_backdrop
                    .as_mut()
                    .ok_or("window backdrop: no spec")?;
                if state
                    .canvas
                    .as_ref()
                    .is_none_or(|canvas| canvas.size() != root)
                {
                    let _compose_span = flamewm_profiler::start("render.backdrop.compose");
                    state.canvas = Some(crate::backdrop::wallpaper_canvas(&state.spec, root)?);
                }
                let pixmap_fits = self
                    .window_backdrop_pixmap
                    .is_some_and(|pixmap| (pixmap.width, pixmap.height) == (width, height));
                let state = self
                    .window_backdrop
                    .as_mut()
                    .ok_or("window backdrop: no spec")?;
                if pixmap_fits
                    && state.captured.as_ref().is_some_and(|old| {
                        old.root == (rx, ry) && (old.width, old.height) == (width, height)
                    })
                {
                    return Ok(());
                }
                let canvas = state.canvas.as_ref().ok_or("window backdrop: no canvas")?;
                let _crop_span = flamewm_profiler::start("render.backdrop.crop");
                canvas.crop((rx, ry), (width, height))
            }
        };
        if words.len() != width as usize * height as usize {
            return Err("window backdrop: snapshot size mismatch".to_string());
        }
        self.drop_mirror_fill();
        let upload_span = flamewm_profiler::start("render.backdrop.upload");
        let reuse = self
            .window_backdrop_pixmap
            .as_ref()
            .filter(|old| (old.width, old.height) == (width, height))
            .map(|old| old.pixmap);
        let upload = match reuse {
            Some(pixmap) => unsafe { self.upload_argb_into_pixmap(pixmap, &words, width, height) }
                .map(|()| None),
            None => unsafe { self.upload_argb_pixmap(&words, width, height) }.map(Some),
        };
        drop(upload_span);
        if let Some(state) = self.window_backdrop.as_mut() {
            let changed = state.captured.as_ref().is_none_or(|old| {
                old.words != words
                    || (old.width, old.height) != (width, height)
                    || old.root != (rx, ry)
            });
            if changed {
                state.generation += 1;
            }
            state.captured = Some(CapturedSnapshot {
                words,
                width,
                height,
                root: (rx, ry),
            });
        }
        if let Some(pixmap) = upload? {
            if let Some(old) =
                self.window_backdrop_pixmap
                    .replace(super::surface_backdrop::SurfaceBackdrop {
                        pixmap,
                        width,
                        height,
                    })
            {
                unsafe { crate::native::xresource::free_pixmap_raw(self.display, old.pixmap) };
            }
        }
        Ok(())
    }

    /// What lies behind the window at root `rect`, composed live from the desktop and the frames
    /// below it; `None` keeps the desktop layer (no redirected frames, or no Composite/Damage).
    unsafe fn compose_live_backdrop(&mut self, rect: (i32, i32, u32, u32)) -> Option<Vec<u32>> {
        let (display, screen, window) = (self.display, self.screen, self.window);
        let state = self.window_backdrop.as_mut()?;
        let blur_percent = state.spec.blur_percent;
        let live = state
            .live
            .get_or_insert_with(|| unsafe { LiveScene::new(display, screen) })
            .as_mut()?;
        let _span = flamewm_profiler::start("render.backdrop.live");
        unsafe { live.compose(display, screen, window, rect, blur_percent) }
    }

    /// Routes a live-backdrop event (root stacking or geometry, or damage on a frame behind).
    /// Returns whether the event was consumed. A relevant change refreshes the backdrop now, or
    /// once [`LIVE_INTERVAL`] has passed since the last refresh.
    pub(crate) fn note_live_backdrop_event(&mut self, event: &XEvent) -> bool {
        let Some(live) = self
            .window_backdrop
            .as_ref()
            .and_then(|state| state.live.as_ref()?.as_ref())
        else {
            return false;
        };
        let (consumed, relevant) = live.classify(event);
        if relevant {
            let due = live
                .last_refresh
                .map_or_else(std::time::Instant::now, |at| at + LIVE_INTERVAL);
            if let Some(state) = self.window_backdrop.as_mut() {
                state.stale = true;
            }
            if due <= std::time::Instant::now() {
                self.mark_full();
            } else {
                self.backdrop_deadline = Some(self.backdrop_deadline.map_or(due, |at| at.min(due)));
            }
        }
        consumed
    }

    /// Whether the window snapshot the backbuffer was last painted over is still the current one
    /// (same generation, same size). Only then may a region paint reuse the backbuffer's backdrop.
    pub(crate) fn window_backdrop_unchanged(&self) -> bool {
        self.window_backdrop_pixmap
            .is_some_and(|b| (b.width, b.height) == (self.width, self.height))
            && self.window_backdrop_painted.is_some()
            && self.window_backdrop_painted == self.window_backdrop.as_ref().map(|s| s.generation)
    }

    /// Records the generation of the snapshot the backbuffer now holds (called after a paint).
    pub(crate) fn record_window_backdrop_painted(&mut self) {
        self.window_backdrop_painted = self
            .window_backdrop_pixmap
            .and(self.window_backdrop.as_ref())
            .map(|state| state.generation);
    }

    /// Frees the window snapshot, if any. Translucent fills then show as straight colour.
    pub(crate) fn clear_window_backdrop(&mut self) {
        if let Some(old) = self.window_backdrop_pixmap.take() {
            unsafe { crate::native::xresource::free_pixmap_raw(self.display, old.pixmap) };
        }
        let display = self.display;
        if let Some(state) = self.window_backdrop.as_mut() {
            state.captured = None;
            state.generation += 1;
            if let Some(Some(mut live)) = state.live.take() {
                // SAFETY: the app's display is live until after Drop releases its resources.
                unsafe { live.release(display) };
            }
        }
        self.drop_mirror_fill();
    }

    /// Retains `fill` as the cached mirror, freeing the pixmap it replaces.
    pub(crate) fn store_mirror_fill(&mut self, fill: MirrorFill) {
        let display = self.display;
        match self.window_backdrop.as_mut() {
            Some(state) => {
                if let Some(old) = state.mirror.replace(fill) {
                    unsafe { crate::native::xresource::free_pixmap_raw(display, old.pixmap) };
                }
            }
            None => {
                unsafe { crate::native::xresource::free_pixmap_raw(display, fill.pixmap) };
            }
        }
    }

    /// Frees the cached mirrored fill, if any.
    pub(crate) fn drop_mirror_fill(&mut self) {
        let display = self.display;
        if let Some(old) = self.window_backdrop.as_mut().and_then(|s| s.mirror.take()) {
            unsafe { crate::native::xresource::free_pixmap_raw(display, old.pixmap) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(value: u32) -> u32 {
        0xff00_0000 | value << 16 | value << 8 | value
    }

    /// Unblurred 4x3 canvas whose grey value is `y * 4 + x`.
    fn grey_canvas() -> BlurredCanvas {
        let pixels = (0..12u8)
            .flat_map(|value| [value, value, value, 0xff])
            .collect();
        BlurredCanvas::new(4, 3, pixels, 0, 0)
    }

    #[test]
    fn a_move_recrops_the_same_canvas_at_the_new_origin() {
        let root = grey_canvas();
        assert_eq!(root.crop((0, 0), (2, 2)), [0, 1, 4, 5].map(grey).to_vec());
        assert_eq!(root.crop((1, 1), (2, 2)), [5, 6, 9, 10].map(grey).to_vec());
    }

    #[test]
    fn crop_replicates_the_root_edge_off_root() {
        let crop = grey_canvas().crop((-1, 1), (3, 2));
        assert_eq!(crop, [4, 4, 5, 8, 8, 9].map(grey).to_vec());
    }
}
