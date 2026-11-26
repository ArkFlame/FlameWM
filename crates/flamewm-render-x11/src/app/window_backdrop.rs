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
use super::live_backdrop::{LIVE_INTERVAL, LiveFrame, LiveScene};
use crate::backdrop::{BlurredCanvas, WindowBackdrop, recapture_due};
use crate::xlib::*;
use crate::xlib_ext::XTranslateCoordinates;
use flamewm_render_core::RuntimeDocument;

/// An output rect in root coordinates (`x, y, width, height`).
type OutputRect = (i32, i32, u32, u32);

/// Per-window state: the spec, the desktop canvas and the uploaded snapshot.
#[derive(Debug)]
pub(crate) struct WindowBackdropState {
    pub(crate) spec: WindowBackdrop,
    /// Blurred desktop layer under the whole root, built (or loaded) once per root size and
    /// wallpaper output layout.
    canvas: Option<(Vec<OutputRect>, BlurredCanvas)>,
    /// Bumped whenever `canvas` is rebuilt; a snapshot cropped from an older canvas is not reused.
    canvas_epoch: u64,
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
    /// The canvas epoch the words were cropped from; `None` for a live composition.
    pub(crate) canvas_epoch: Option<u64>,
}

/// Scene size and snapshot origin (snapshot root position minus the window's root position)
/// that a mirrored fill was computed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MirrorKey {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) origin: (i64, i64),
}

/// Cached mirrored fill: the pixmap holds `mirror_extend_argb` of the snapshot.
///
/// `capacity` is the pixmap's pixel size and never shrinks, so a resize grows only the strips it
/// added instead of reallocating and re-uploading the whole window per frame. `filled` is the
/// top-left area already written for this origin; every pixel in it stays correct regardless of
/// the key, because the mirror of a pixel depends on its own coordinates and the origin only. A
/// shrink refills the key area and resets `filled`, so coverage is always one rectangle.
#[derive(Debug)]
pub(crate) struct MirrorFill {
    pub(crate) key: MirrorKey,
    pub(crate) capacity: (u32, u32),
    pub(crate) filled: (u32, u32),
    pub(crate) pixmap: Pixmap,
}

/// Pixmap size chosen for `size`: the window plus a quarter, so a drag that grows the window
/// reallocates a handful of times instead of on every frame.
#[must_use]
pub(crate) fn mirror_capacity(size: (u32, u32)) -> (u32, u32) {
    let grow = |v: u32| v.saturating_add((v / 4).max(64));
    (grow(size.0), grow(size.1))
}

/// Pixmap regions that a crop moved from `old_root` to `new_root` does not already hold, as
/// `(x, y, width, height)` in window pixels. `None` when the shift covers the whole window (or
/// none of it), where a full upload is simpler.
#[must_use]
pub(crate) fn shift_patches(
    old_root: (i32, i32),
    new_root: (i32, i32),
    size: (u32, u32),
) -> Option<Vec<(u32, u32, u32, u32)>> {
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    let dx = i64::from(new_root.0) - i64::from(old_root.0);
    let dy = i64::from(new_root.1) - i64::from(old_root.1);
    if (dx, dy) == (0, 0) || dx.abs() >= w || dy.abs() >= h {
        return None;
    }
    let mut patches = Vec::with_capacity(2);
    if dx > 0 {
        patches.push((0, 0, dx as u32, size.1));
    } else if dx < 0 {
        patches.push(((w + dx) as u32, 0, (-dx) as u32, size.1));
    }
    if dy > 0 {
        patches.push((0, 0, size.0, dy as u32));
    } else if dy < 0 {
        patches.push((0, (h + dy) as u32, size.0, (-dy) as u32));
    }
    Some(patches)
}

/// Copies one `(x, y, width, height)` sub-rect of `width`-wide words.
#[must_use]
pub(crate) fn crop_words(src: &[u32], src_width: u32, rect: (u32, u32, u32, u32)) -> Vec<u32> {
    let (x, y, w, h) = rect;
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for row in 0..h {
        let start = (y + row) as usize * src_width as usize + x as usize;
        out.extend_from_slice(&src[start..start + w as usize]);
    }
    out
}

/// What a requested backdrop change does to the installed one. `None` means the installed backdrop
/// already matches (or stays absent); `Some(next)` replaces it with `next`. Opaque requests are
/// dropped, as `UiDocument::with_window_backdrop` does at creation.
pub(crate) fn backdrop_replacement(
    installed: Option<&WindowBackdrop>,
    requested: Option<WindowBackdrop>,
) -> Option<Option<WindowBackdrop>> {
    let requested = requested.filter(WindowBackdrop::is_translucent);
    (installed != requested.as_ref()).then_some(requested)
}

/// Whether a composed snapshot repeats the retained one: same root position, size and pixels.
/// Only then may the upload be skipped; the generation stays put, so a region paint can reuse the
/// backbuffer.
pub(crate) fn snapshot_unchanged(
    old: &CapturedSnapshot,
    words: &[u32],
    size: (u32, u32),
    root: (i32, i32),
) -> bool {
    old.root == root && (old.width, old.height) == size && old.words == words
}

/// Loop wake-up for a recapture deferred behind queued events: due at once (the loop drains the
/// queue, then recaptures), or sooner if an earlier wake-up is already pending.
pub(crate) fn deferred_recapture_deadline(
    pending: Option<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Instant> {
    Some(pending.map_or(now, |at| at.min(now)))
}

impl WindowBackdropState {
    pub(crate) fn new(spec: WindowBackdrop) -> Self {
        Self {
            spec,
            canvas: None,
            canvas_epoch: 0,
            stale: true,
            captured: None,
            mirror: None,
            generation: 0,
            live: None,
        }
    }
}

impl X11App {
    /// Applies a backdrop the UI requested after window creation (see `RuntimeDocument::request_window_backdrop`).
    /// A different spec drops the snapshot, canvas and live scene, then repaints once; the next paint
    /// recaptures before the scene is cleared. An equal spec changes nothing and repaints nothing.
    pub(crate) fn apply_window_backdrop_request(&mut self, document: &mut RuntimeDocument) {
        let Some(requested) = document.take_window_backdrop() else {
            return;
        };
        let installed = self.window_backdrop.as_ref().map(|state| &state.spec);
        let Some(next) = backdrop_replacement(installed, requested) else {
            return;
        };
        self.clear_window_backdrop();
        match (self.window_backdrop.as_mut(), next) {
            (Some(state), Some(spec)) => {
                state.spec = spec;
                state.canvas = None;
                state.stale = true;
            }
            (None, Some(spec)) => {
                self.window_backdrop = Some(WindowBackdropState::new(spec));
            }
            (_, None) => self.window_backdrop = None,
        }
        self.mark_full();
    }

    /// The window moved or resized: the snapshot is recropped on the next paint.
    ///
    /// The generation is not bumped here: a recapture that lands the same pixels at the same size
    /// and root leaves the installed backdrop valid, so a retained backbuffer may still be reused.
    /// `capture_window_backdrop` bumps it exactly when the cropped pixels change.
    pub(crate) fn mark_window_backdrop_stale(&mut self) {
        if let Some(state) = self.window_backdrop.as_mut() {
            state.stale = true;
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
        let snapshot_size = self
            .window_backdrop_pixmap
            .as_ref()
            .map(|b| (b.width, b.height));
        let queued = unsafe { XEventsQueued(self.display, crate::xlib::QUEUED_AFTER_READING) } > 0;
        if !recapture_due(snapshot_size, (self.width, self.height), queued) {
            // Events are queued behind a resize. Nothing else wakes the loop once they drain, so
            // the stale snapshot would stay until an unrelated event repaints the window.
            let now = std::time::Instant::now();
            self.backdrop_deadline = deferred_recapture_deadline(self.backdrop_deadline, now);
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
        crate::app::display_extent(self.display, self.screen)
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
        let (words, canvas_epoch) = match live {
            // What lies behind the window is unchanged since the snapshot on it was uploaded.
            Some(LiveFrame::Unchanged) => return Ok(()),
            Some(LiveFrame::Pixels(words)) => (words, None),
            None => {
                let state = self
                    .window_backdrop
                    .as_mut()
                    .ok_or("window backdrop: no spec")?;
                // SAFETY: the window's display is live.
                let outputs = unsafe { crate::backdrop::published_wallpaper_outputs(self.display) };
                if state
                    .canvas
                    .as_ref()
                    .is_none_or(|(layout, canvas)| canvas.size() != root || *layout != outputs)
                {
                    let _compose_span = flamewm_profiler::start("render.backdrop.compose");
                    let canvas = crate::backdrop::wallpaper_canvas(&state.spec, root, &outputs)?;
                    state.canvas = Some((outputs, canvas));
                    state.canvas_epoch = state.canvas_epoch.wrapping_add(1);
                }
                let pixmap_fits = self
                    .window_backdrop_pixmap
                    .as_ref()
                    .is_some_and(|pixmap| (pixmap.width, pixmap.height) == (width, height));
                let state = self
                    .window_backdrop
                    .as_mut()
                    .ok_or("window backdrop: no spec")?;
                // Same root, size and canvas: the uploaded crop is still the desktop under the window.
                let epoch = state.canvas_epoch;
                if pixmap_fits
                    && state.captured.as_ref().is_some_and(|old| {
                        old.root == (rx, ry)
                            && (old.width, old.height) == (width, height)
                            && old.canvas_epoch == Some(epoch)
                    })
                {
                    return Ok(());
                }
                let (_, canvas) = state.canvas.as_ref().ok_or("window backdrop: no canvas")?;
                let _crop_span = flamewm_profiler::start("render.backdrop.crop");
                // A moved window shows the same canvas shifted: reuse the overlap of the last crop.
                let words = match state.captured.as_ref().filter(|old| {
                    old.canvas_epoch == Some(epoch) && (old.width, old.height) == (width, height)
                }) {
                    Some(old) => canvas.recrop(&old.words, old.root, (rx, ry), (width, height)),
                    None => canvas.crop((rx, ry), (width, height)),
                };
                (words, Some(epoch))
            }
        };
        if words.len() != width as usize * height as usize {
            return Err("window backdrop: snapshot size mismatch".to_string());
        }
        // Identical pixels at the same root and size: the uploaded snapshot already holds them.
        let unchanged = self
            .window_backdrop_pixmap
            .as_ref()
            .is_some_and(|old| (old.width, old.height) == (width, height))
            && self
                .window_backdrop
                .as_ref()
                .and_then(|state| state.captured.as_ref())
                .is_some_and(|old| snapshot_unchanged(old, &words, (width, height), (rx, ry)));
        if unchanged {
            return Ok(());
        }
        // A moved window re-crops the same canvas: upload only the strips the shift exposed
        // instead of the whole snapshot, since the retained pixmap already holds the overlap.
        if let Some(epoch) = canvas_epoch
            && unsafe { self.patch_moved_window_backdrop(&words, (rx, ry), epoch)? }
        {
            return Ok(());
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
                canvas_epoch,
            });
        }
        if let Some(pixmap) = upload?
            && let Some(old) =
                self.window_backdrop_pixmap
                    .replace(super::surface_backdrop::SurfaceBackdrop {
                        pixmap,
                        width,
                        height,
                        rgba: None,
                    })
        {
            unsafe { crate::native::xresource::free_pixmap_raw(self.display, old.pixmap) };
        }
        Ok(())
    }

    /// A same-size crop that moved on the canvas: uploads only the strips the shift exposed and
    /// keeps the retained pixmap for the overlap, so a drag repaints no full-window snapshot.
    ///
    /// Returns whether the retained snapshot was patched; `false` sends the caller through the
    /// full upload (a shrink, a jump of a whole window, or no matching retained pixmap).
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread, before the scene is cleared for a paint.
    pub(crate) unsafe fn patch_moved_window_backdrop(
        &mut self,
        words: &[u32],
        root: (i32, i32),
        epoch: u64,
    ) -> Result<bool, String> {
        let (width, height) = (self.width, self.height);
        if words.len() != width as usize * height as usize {
            return Ok(false);
        }
        let Some(pixmap) = self
            .window_backdrop_pixmap
            .as_ref()
            .filter(|backdrop| (backdrop.width, backdrop.height) == (width, height))
            .map(|backdrop| backdrop.pixmap)
        else {
            return Ok(false);
        };
        let Some(old_root) = self
            .window_backdrop
            .as_ref()
            .and_then(|state| state.captured.as_ref())
            .filter(|old| {
                old.canvas_epoch == Some(epoch) && (old.width, old.height) == (width, height)
            })
            .map(|old| old.root)
        else {
            return Ok(false);
        };
        let Some(patches) = shift_patches(old_root, root, (width, height)) else {
            return Ok(false);
        };
        let visual = self.backdrop_argb_visual()?;
        for (x, y, w, h) in &patches {
            let strip = crop_words(words, width, (*x, *y, *w, *h));
            unsafe {
                self.upload_argb32_words_at(
                    pixmap,
                    visual,
                    &strip,
                    *w,
                    *h,
                    (*x as i32, *y as i32),
                )?;
            }
        }
        self.drop_mirror_fill();
        if let Some(state) = self.window_backdrop.as_mut() {
            // The pixels at a moved root position differ, so the backbuffer no longer holds this
            // backdrop.
            state.generation += 1;
            state.captured = Some(CapturedSnapshot {
                words: words.to_vec(),
                width,
                height,
                root,
                canvas_epoch: Some(epoch),
            });
        }
        Ok(true)
    }

    /// What lies behind the window at root `rect`, composed live from the desktop and the frames
    /// below it; `None` keeps the desktop layer (no redirected frames, or no Composite/Damage).
    unsafe fn compose_live_backdrop(&mut self, rect: (i32, i32, u32, u32)) -> Option<LiveFrame> {
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
            self.request_window_backdrop_refresh();
        }
        consumed
    }

    /// The backdrop needs recomputing: repaint now, or when [`LIVE_INTERVAL`] has passed since the
    /// last live refresh. Painting earlier would only redraw the old backdrop.
    pub(crate) fn request_window_backdrop_refresh(&mut self) {
        let Some(state) = self.window_backdrop.as_mut() else {
            return;
        };
        state.stale = true;
        let due = state
            .live
            .as_ref()
            .and_then(|live| live.as_ref()?.last_refresh)
            .map(|at| at + LIVE_INTERVAL)
            .filter(|due| *due > std::time::Instant::now());
        match due {
            Some(due) => {
                self.backdrop_deadline = Some(self.backdrop_deadline.map_or(due, |at| at.min(due)));
            }
            None => self.mark_full(),
        }
    }

    /// Whether the window snapshot the backbuffer was last painted over is still the current one
    /// (same generation, same size). Only then may a region paint reuse the backbuffer's backdrop.
    pub(crate) fn window_backdrop_unchanged(&self) -> bool {
        self.window_backdrop_pixmap
            .as_ref()
            .is_some_and(|b| (b.width, b.height) == (self.width, self.height))
            && self.window_backdrop_painted.is_some()
            && self.window_backdrop_painted == self.window_backdrop.as_ref().map(|s| s.generation)
    }

    /// Records the generation of the snapshot the backbuffer now holds (called after a paint).
    pub(crate) fn record_window_backdrop_painted(&mut self) {
        self.window_backdrop_painted = self
            .window_backdrop_pixmap
            .as_ref()
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
    ///
    /// A grown fill keeps its pixmap, so the replaced entry may hold the same handle; only a
    /// different, dropped pixmap is freed.
    pub(crate) fn store_mirror_fill(&mut self, fill: MirrorFill) {
        let display = self.display;
        let new_pixmap = fill.pixmap;
        match self.window_backdrop.as_mut() {
            Some(state) => {
                if let Some(old) = state.mirror.replace(fill)
                    && old.pixmap != new_pixmap
                {
                    unsafe { crate::native::xresource::free_pixmap_raw(display, old.pixmap) };
                }
            }
            None => {
                unsafe { crate::native::xresource::free_pixmap_raw(display, new_pixmap) };
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

    fn snapshot(words: Vec<u32>, root: (i32, i32), epoch: Option<u64>) -> CapturedSnapshot {
        CapturedSnapshot {
            words,
            width: 2,
            height: 2,
            root,
            canvas_epoch: epoch,
        }
    }

    #[test]
    fn repeated_snapshot_is_unchanged_only_for_same_root_size_and_pixels() {
        let kept = snapshot(vec![1, 2, 3, 4], (10, 20), Some(1));
        assert!(snapshot_unchanged(&kept, &[1, 2, 3, 4], (2, 2), (10, 20)));
        assert!(!snapshot_unchanged(&kept, &[1, 2, 3, 5], (2, 2), (10, 20)));
        assert!(!snapshot_unchanged(&kept, &[1, 2, 3, 4], (2, 2), (11, 20)));
        assert!(!snapshot_unchanged(&kept, &[1, 2, 3, 4], (1, 4), (10, 20)));
    }

    #[test]
    fn a_move_recrops_the_same_canvas_at_the_new_origin() {
        let root = grey_canvas();
        assert_eq!(root.crop((0, 0), (2, 2)), [0, 1, 4, 5].map(grey).to_vec());
        assert_eq!(root.crop((1, 1), (2, 2)), [5, 6, 9, 10].map(grey).to_vec());
    }

    #[test]
    fn a_move_patches_only_the_strips_the_shift_exposed() {
        // Moved right and down: the new left column and top row are the only unknown pixels.
        assert_eq!(
            shift_patches((0, 0), (2, 3), (10, 8)),
            Some(vec![(0, 0, 2, 8), (0, 0, 10, 3)])
        );
        // Moved left and up: the new right column and bottom row.
        assert_eq!(
            shift_patches((5, 5), (3, 2), (10, 8)),
            Some(vec![(8, 0, 2, 8), (0, 5, 10, 3)])
        );
        // A whole-window jump or no move at all rebuilds.
        assert_eq!(shift_patches((0, 0), (10, 0), (10, 8)), None);
        assert_eq!(shift_patches((4, 4), (4, 4), (10, 8)), None);
    }

    #[test]
    fn a_patched_strip_carries_the_moved_crop_pixels() {
        let new = grey_canvas().crop((1, 0), (3, 2));
        let (x, y, w, h) = shift_patches((0, 0), (1, 0), (3, 2)).expect("overlap")[0];
        assert_eq!((x, y, w, h), (0, 0, 1, 2));
        assert_eq!(crop_words(&new, 3, (x, y, w, h)), [new[0], new[3]]);
        assert_eq!(crop_words(&new, 3, (2, 1, 1, 1)), [new[5]]);
    }

    #[test]
    fn mirror_capacity_covers_the_window_it_was_chosen_for() {
        for size in [(1, 1), (80, 40), (900, 700), (3840, 2160)] {
            let capacity = mirror_capacity(size);
            assert!(capacity.0 >= size.0 && capacity.1 >= size.1);
        }
    }

    #[test]
    fn crop_replicates_the_root_edge_off_root() {
        let crop = grey_canvas().crop((-1, 1), (3, 2));
        assert_eq!(crop, [4, 4, 5, 8, 8, 9].map(grey).to_vec());
    }
}

#[cfg(test)]
mod deferred_recapture_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_deferred_recapture_wakes_the_loop_at_once_when_nothing_is_pending() {
        let now = Instant::now();
        assert_eq!(deferred_recapture_deadline(None, now), Some(now));
    }

    #[test]
    fn a_deferred_recapture_keeps_an_earlier_pending_wake_up() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(5);
        assert_eq!(
            deferred_recapture_deadline(Some(earlier), now),
            Some(earlier)
        );
        let later = now + Duration::from_millis(40);
        assert_eq!(deferred_recapture_deadline(Some(later), now), Some(now));
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    fn spec(opacity_percent: u8, blur_percent: u8) -> WindowBackdrop {
        WindowBackdrop {
            wallpaper: "/w.png".into(),
            fit: "fill".into(),
            packaged_root: std::path::PathBuf::from("/share"),
            opacity_percent,
            blur_percent,
        }
    }

    #[test]
    fn an_equal_spec_is_not_a_replacement() {
        let installed = spec(80, 40);
        assert_eq!(
            backdrop_replacement(Some(&installed), Some(spec(80, 40))),
            None
        );
    }

    #[test]
    fn an_opaque_request_with_no_backdrop_is_not_a_replacement() {
        assert_eq!(backdrop_replacement(None, None), None);
        assert_eq!(backdrop_replacement(None, Some(spec(100, 40))), None);
    }

    #[test]
    fn a_changed_spec_replaces_once_and_removal_is_explicit() {
        let installed = spec(80, 40);
        let changed = spec(80, 60);
        assert_eq!(
            backdrop_replacement(Some(&installed), Some(changed.clone())),
            Some(Some(changed.clone()))
        );
        assert_eq!(backdrop_replacement(Some(&installed), None), Some(None));
        assert_eq!(
            backdrop_replacement(None, Some(installed.clone())),
            Some(Some(installed))
        );
    }
}
