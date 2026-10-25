//! Window-level pseudo-transparency: what lies under a whole Normal window.
//!
//! Owns the window snapshot lifecycle (capture, move/resize policy, teardown). The snapshot
//! lives in its own slot so the popup mask (which keys off `surface_backdrop`) keeps its shape.
//! Paint falls back to this slot only when no popup backdrop is installed.
//!
//! The root content under the window (other windows, desktop icons, wallpaper) is cached as one
//! root-sized image, read **once** while the window still cannot be on the root, and blurred once.
//! Moves and resizes then only crop that blurred copy at the new origin: the settle that ends a
//! move never reads the root, never hides the window and never touches its bounding shape, so a
//! moved window cannot disappear and reappear after it stops (A01). The settle path used to
//! shape-hide the whole ancestor chain, `XSync` and read the full root ~250 ms after the motion
//! stopped, which is exactly the visible hide.
//!
//! Reads are safe without hiding because the only moment a read can be clean without a compositor
//! is before the surface is on the root: no frame yet, presentation before `Mapped`. A moved
//! window keeps cropping the cached copy instead, which is what keeps other windows visible under
//! translucent chrome (A04). Explicit invalidation of that cache: root size change, wallpaper
//! change (the state is rebuilt with the new spec), or a change in the root's top-level stacking
//! order (a window appeared, vanished or was restacked) probed at most every
//! [`SCENE_PROBE_INTERVAL`]. Invalidation only takes effect at the next safe read moment; while the
//! surface stays on screen the cached copy keeps painting (no hide, no flicker).
//!
//! Limit: other windows moving behind a surface that stays on screen are not re-read (that would
//! need a compositor or hiding the visible window). The crop then shows them where they were when
//! the copy was read; no root event subscription is added here (event sources are owned by
//! `flamewm-reactor`).

use std::time::{Duration, Instant};

use flamewm_image_core::RgbaImage;

use super::X11App;
use crate::backdrop::{WindowBackdrop, blurred_argb_words, recapture_due, wallpaper_file};
use crate::xlib::*;
use crate::xlib_ext::{XQueryTree, XTranslateCoordinates};

/// Quiet period after the last move or resize. Each configure slides the deadline, so the loop
/// wakes once the window stops moving. The wake only recomposes the snapshot from the cached
/// clean root; it never reads the root and never hides the window (A01). It is also the minimum
/// spacing between two root reads.
pub(crate) const CAPTURE_QUIET: Duration = Duration::from_millis(250);

/// Minimum spacing between two probes of the root's top-level stacking order (A04 invalidation).
pub(crate) const SCENE_PROBE_INTERVAL: Duration = Duration::from_millis(500);

/// What the snapshot policy allows on this paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootCapture {
    /// Read the root now. Only reachable while the surface cannot be on the root yet, so the read
    /// is clean without hiding anything.
    ReadClean,
    /// Crop the clean root already cached at the new origin: no root read, no shape change.
    ReuseCache,
    /// No clean root is cached and the surface may be on screen: keep the wallpaper fallback and
    /// retry while the surface is still off screen. A visible window is never hidden.
    Defer,
}

/// The snapshot policy. `settled` is the wake that ends the quiet period after a move; it never
/// changes the answer, which is the A01 guarantee: a settle crops the cached root, it never reads
/// the root (the read used to hide the window and its ancestors) and never hides anything.
#[must_use]
pub(crate) fn root_capture_decision(cached: bool, settled: bool, clean: bool) -> RootCapture {
    let _ = settled;
    if cached {
        RootCapture::ReuseCache
    } else if clean {
        RootCapture::ReadClean
    } else {
        RootCapture::Defer
    }
}

/// Whether enough time has passed since the last root read for another one. `None` means no
/// read has happened yet.
#[must_use]
pub(crate) fn readback_spaced(since_last: Option<Duration>) -> bool {
    since_last.map_or(true, |elapsed| elapsed >= CAPTURE_QUIET)
}

/// Whether a probed top-level stack means the clean root no longer describes the desktop: a
/// window above or below this one appeared, vanished or was restacked. `known` is the stack the
/// cached root was read against.
#[must_use]
pub(crate) fn scene_changed(known: &[Window], probed: &[Window]) -> bool {
    known != probed
}

/// Per-window state: the spec, the decoded wallpaper cache, and whether a capture is due.
#[derive(Debug)]
pub(crate) struct WindowBackdropState {
    pub(crate) spec: WindowBackdrop,
    natural: Option<RgbaImage>,
    /// Root pixels as last read while the window could not be on the root (root-sized).
    desktop: Option<RgbaImage>,
    /// The root changed since `desktop` was read (a move or resize of this window, or a change in
    /// the root's top-level stack). Only a safe read clears it.
    pub(crate) desktop_stale: bool,
    /// Root read blurred once (root width, root height, ARGB words), cropped per move.
    blurred_root: Option<(u32, u32, Vec<u32>)>,
    /// When the root was last read (either variant); spaces reads by `CAPTURE_QUIET`.
    pub(crate) last_readback: Option<Instant>,
    pub(crate) stale: bool,
    /// Top-level children of the root (stacking order) the cached root was read against.
    pub(crate) scene_children: Option<Vec<Window>>,
    /// When the root stack was last probed (A04 invalidation).
    pub(crate) scene_probe_at: Option<Instant>,
    /// When a read was last refused because the surface may be on the root; throttles the retry so
    /// a deferred surface does not probe the server on every paint.
    pub(crate) deferred_at: Option<Instant>,
    /// A deferred read was already reported for this cache, so the log stays one line.
    pub(crate) deferred_logged: bool,
    /// Last blurred snapshot and the root origin it was captured at. A resized window mirror-extends
    /// it into the grown area until the settled capture replaces it.
    pub(crate) captured: Option<CapturedSnapshot>,
    /// Mirror-extended fill of `captured` for the last (size, origin); reused while it matches.
    pub(crate) mirror: Option<MirrorFill>,
    /// Bumped exactly when the window backdrop pixels change: a new or different snapshot, its
    /// removal, a root stack change, or a move or resize. A backbuffer painted at generation `g`
    /// holds this backdrop only while the generation is still `g`.
    pub(crate) generation: u64,
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
            desktop: None,
            desktop_stale: true,
            blurred_root: None,
            last_readback: None,
            stale: true,
            scene_children: None,
            scene_probe_at: None,
            deferred_at: None,
            deferred_logged: false,
            captured: None,
            mirror: None,
            generation: 0,
        }
    }
}

impl X11App {
    /// Marks the snapshot out of date after the window moved or resized and restarts the quiet
    /// period. The snapshot is recomposed at once from the cached root; the root itself is not
    /// read again (a settle must not hide the window, A01).
    pub(crate) fn mark_window_backdrop_stale(&mut self) {
        if let Some(state) = self.window_backdrop.as_mut() {
            state.stale = true;
            state.desktop_stale = true;
            state.generation += 1;
            self.backdrop_deadline = Some(Instant::now() + CAPTURE_QUIET);
        }
    }

    /// Recomputes the window snapshot for this paint. The settle path only crops the cached clean
    /// root at the new origin; the root is read only while the surface cannot be on the root yet.
    /// Capture failure is nonfatal: the slot is cleared and translucent fills show as straight
    /// colour.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread, before the scene is cleared for a paint.
    pub(crate) unsafe fn refresh_window_backdrop(&mut self) {
        if self.window_backdrop.is_none() {
            return;
        }
        // A04 invalidation: a top-level window that appeared, vanished or was restacked means the
        // cached clean root no longer describes the desktop. The flag only takes effect at the
        // next safe read; it never triggers a hide.
        if unsafe { self.probe_root_scene() } {
            if let Some(state) = self.window_backdrop.as_mut() {
                state.desktop_stale = true;
                state.stale = true;
                state.generation += 1;
            }
        }
        let root = unsafe { self.root_size() };
        let (cached, deferred_recent, last_readback, stale) = {
            let Some(state) = self.window_backdrop.as_ref() else {
                return;
            };
            (
                state
                    .blurred_root
                    .as_ref()
                    .is_some_and(|(width, height, _)| (*width, *height) == root),
                state
                    .deferred_at
                    .is_some_and(|at| at.elapsed() < SCENE_PROBE_INTERVAL),
                state.last_readback,
                state.stale,
            )
        };
        if !cached && deferred_recent {
            // A read was already refused this interval: do not probe the server on every paint.
            return;
        }
        // The only read that needs no hide is the one taken while the surface is still off root.
        let clean = if cached {
            false
        } else {
            unsafe { self.root_read_is_clean() }
        };
        let mut decision = root_capture_decision(cached, self.backdrop_deadline.is_none(), clean);
        if decision == RootCapture::ReadClean
            && !readback_spaced(last_readback.map(|at| at.elapsed()))
        {
            // Too soon after the last read: keep the cached root and wake when the spacing ends.
            decision = RootCapture::ReuseCache;
            self.backdrop_deadline = last_readback.map(|at| at + CAPTURE_QUIET);
        }
        match decision {
            RootCapture::Defer => {
                // No clean root and the surface may be on screen. On the first appearance only,
                // take one hidden-root capture so the initial backdrop contains the windows
                // behind the surface; after that, use the wallpaper fallback and never hide a
                // visible window again (A01). The one-shot hide lasts one get_image round trip.
                let mut captured = false;
                if last_readback.is_none() {
                    if let Ok(desktop) = unsafe { self.read_root_behind_window(root) } {
                        let scene = unsafe { self.root_children() };
                        let blur_percent = self
                            .window_backdrop
                            .as_ref()
                            .map(|state| state.spec.blur_percent)
                            .unwrap_or(0);
                        // The raw copy is dropped below, so its pixels move into the blur.
                        let words = blurred_argb_words(
                            desktop.width,
                            desktop.height,
                            desktop.pixels,
                            0,
                            blur_percent,
                        );
                        if let Some(state) = self.window_backdrop.as_mut() {
                            // The blurred copy is the crop source; the raw root copy is redundant.
                            state.desktop = None;
                            state.blurred_root = Some((root.0, root.1, words));
                            state.desktop_stale = false;
                            state.last_readback = Some(Instant::now());
                            state.scene_children = Some(scene);
                            state.scene_probe_at = Some(Instant::now());
                            state.stale = false;
                            state.deferred_logged = true;
                            state.deferred_at = Some(Instant::now());
                        }
                        captured = true;
                    }
                }
                if let Some(state) = self.window_backdrop.as_mut() {
                    if !state.deferred_logged {
                        eprintln!(
                            "FLAMEWM_WINDOW_BACKDROP deferred reason=surface-on-root detail=wallpaper-fallback"
                        );
                    }
                    state.deferred_logged = true;
                    state.deferred_at = Some(Instant::now());
                    if !captured {
                        if !stale {
                            return;
                        }
                        state.stale = false;
                    }
                }
                if let Err(error) = unsafe { self.capture_window_backdrop(false) } {
                    eprintln!("FLAMEWM_WINDOW_BACKDROP degraded reason={error}");
                    self.clear_window_backdrop();
                }
                return;
            }
            RootCapture::ReuseCache if !stale => return,
            RootCapture::ReuseCache | RootCapture::ReadClean => {}
        }
        let snapshot_size = self.window_backdrop_pixmap.map(|b| (b.width, b.height));
        let queued = unsafe { XEventsQueued(self.display, crate::xlib::QUEUED_AFTER_READING) } > 0;
        if !recapture_due(snapshot_size, (self.width, self.height), queued) {
            return;
        }
        let readback = decision == RootCapture::ReadClean;
        let Some(state) = self.window_backdrop.as_mut() else {
            return;
        };
        state.stale = false;
        if let Err(error) = unsafe { self.capture_window_backdrop(readback) } {
            eprintln!("FLAMEWM_WINDOW_BACKDROP degraded reason={error}");
            self.clear_window_backdrop();
        } else if readback {
            // Keep the blurred root only: the raw copy is no longer needed for crops.
            if let Some(state) = self.window_backdrop.as_mut() {
                state.desktop = None;
            }
        }
    }

    /// Root (screen) size in device pixels.
    unsafe fn root_size(&self) -> (u32, u32) {
        (
            unsafe { XDisplayWidth(self.display, self.screen) }.max(0) as u32,
            unsafe { XDisplayHeight(self.display, self.screen) }.max(0) as u32,
        )
    }

    /// Whether a root read right now is guaranteed to hold nothing of this surface, so it needs no
    /// shape hide, no unmap and no `XSync`. Two proofs, both read-only:
    ///
    /// * the surface has not been mapped yet (presentation is still before `Mapped`) and the WM has
    ///   not reparented it into a frame, so the server cannot draw it into the root;
    /// * the surface and every ancestor paint nothing right now (empty bounding shape, e.g. while a
    ///   slide clip hides it).
    ///
    /// Every other state refuses the read instead of hiding a visible window (A01).
    unsafe fn root_read_is_clean(&self) -> bool {
        if !matches!(
            self.presentation,
            super::PresentationState::Mapped | super::PresentationState::Presented
        ) && unsafe { self.window_parent_is_root() }
        {
            return true;
        }
        unsafe { self.surface_paints_nothing() }
    }

    /// Whether this window is still a direct child of the root (no WM frame yet).
    unsafe fn window_parent_is_root(&self) -> bool {
        let root_window = unsafe { XRootWindow(self.display, self.screen) };
        let (mut queried_root, mut parent, mut children, mut count) =
            (0, 0, std::ptr::null_mut(), 0);
        unsafe {
            XQueryTree(
                self.display,
                self.window,
                &mut queried_root,
                &mut parent,
                &mut children,
                &mut count,
            )
        };
        if !children.is_null() {
            unsafe { XFree(children.cast()) };
        }
        parent == root_window
    }

    /// Whether this window and its ancestors below the root have an empty bounding shape, so the
    /// server paints none of them. The shape is only read, never changed.
    unsafe fn surface_paints_nothing(&self) -> bool {
        let Some(bridge) = self.xshape.as_ref() else {
            return false;
        };
        let root_window = unsafe { XRootWindow(self.display, self.screen) };
        let mut window = self.window;
        loop {
            if !unsafe { bridge.bounding_rects(window) }.is_empty() {
                return false;
            }
            let (mut queried_root, mut parent, mut children, mut count) =
                (0, 0, std::ptr::null_mut(), 0);
            unsafe {
                XQueryTree(
                    self.display,
                    window,
                    &mut queried_root,
                    &mut parent,
                    &mut children,
                    &mut count,
                )
            };
            if !children.is_null() {
                unsafe { XFree(children.cast()) };
            }
            if parent == 0 || parent == root_window {
                return true;
            }
            window = parent;
        }
    }

    /// Probes the root's top-level stacking order (A04 invalidation) and reports whether it differs
    /// from the stack the cached root was read against. Throttled to [`SCENE_PROBE_INTERVAL`] and
    /// skipped until a clean root exists.
    unsafe fn probe_root_scene(&mut self) -> bool {
        let Some(state) = self.window_backdrop.as_ref() else {
            return false;
        };
        // Probing needs a clean root to compare against: the readback leaves `blurred_root`, not
        // `desktop` (which is dropped after the read), so the test must use `blurred_root`.
        if state.blurred_root.is_none()
            || state
                .scene_probe_at
                .is_some_and(|at| at.elapsed() < SCENE_PROBE_INTERVAL)
        {
            return false;
        }
        let probed = unsafe { self.root_children() };
        let Some(state) = self.window_backdrop.as_mut() else {
            return false;
        };
        state.scene_probe_at = Some(Instant::now());
        let changed = state
            .scene_children
            .as_deref()
            .is_some_and(|known| scene_changed(known, &probed));
        if changed || state.scene_children.is_none() {
            state.scene_children = Some(probed);
        }
        changed
    }

    /// The root's children in stacking order (bottom first).
    unsafe fn root_children(&self) -> Vec<Window> {
        let root_window = unsafe { XRootWindow(self.display, self.screen) };
        let (mut queried_root, mut parent, mut children, mut count) =
            (0, 0, std::ptr::null_mut(), 0);
        unsafe {
            XQueryTree(
                self.display,
                root_window,
                &mut queried_root,
                &mut parent,
                &mut children,
                &mut count,
            )
        };
        if children.is_null() {
            return Vec::new();
        }
        let out = unsafe { std::slice::from_raw_parts(children, count.max(0) as usize) }.to_vec();
        unsafe { XFree(children.cast()) };
        out
    }

    /// Root pixels under the window's rect, blurred and uploaded as the window snapshot. `readback`
    /// reads the whole root first (only allowed while the surface cannot be on the root); otherwise
    /// the cached root is cropped.
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
            // next refresh retries. The read only runs while the surface cannot be on the root, so
            // no shape hide, no unmap and no XSync is needed and nothing is hidden.
            let _readback_span = flamewm_profiler::start("render.backdrop.readback");
            let desktop = unsafe { self.read_root_clean((root_w, root_h)) }.ok();
            let scene = desktop.as_ref().map(|_| unsafe { self.root_children() });
            if let Some(state) = self.window_backdrop.as_mut() {
                // The root is blurred once per read, so a move only crops the blurred root.
                let blur_percent = state.spec.blur_percent;
                state.blurred_root = desktop.as_ref().map(|root| {
                    let pixels = root.pixels.clone();
                    let words =
                        blurred_argb_words(root.width, root.height, pixels, 0, blur_percent);
                    (root.width, root.height, words)
                });
                state.desktop = desktop;
                state.desktop_stale = false;
                state.last_readback = Some(Instant::now());
                // A04 invalidation baseline: the root stack this clean copy was read against.
                state.scene_children = scene;
                state.scene_probe_at = Some(Instant::now());
            }
        }
        let (width, height) = (self.width, self.height);
        let state = self
            .window_backdrop
            .as_mut()
            .ok_or("window backdrop: no spec")?;
        let blurred_fits = state
            .blurred_root
            .as_ref()
            .is_some_and(|(w, h, _)| (*w, *h) == (root_w, root_h));
        let words = if blurred_fits {
            let (_, _, blurred) = state.blurred_root.as_ref().expect("checked above");
            let blur_span = flamewm_profiler::start("render.backdrop.crop");
            let words = crop_words_clamped(blurred, (root_w, root_h), (rx, ry), (width, height));
            drop(blur_span);
            words
        } else {
            let snapshot = match state.desktop.as_ref() {
                Some(desktop) => {
                    crop_rgba(desktop, anchor).ok_or("window backdrop: root region unavailable")?
                }
                None => wallpaper_region(state, (root_w, root_h), anchor)?,
            };
            let shift = (
                i64::from(rx) - i64::from(anchor.0),
                i64::from(ry) - i64::from(anchor.1),
            );
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
            words
        };
        if words.len() != width as usize * height as usize {
            return Err("window backdrop: snapshot size mismatch".to_string());
        }
        // The cached mirror was made from the previous snapshot.
        self.drop_mirror_fill();
        // Same-size snapshots re-upload into the retained pixmap instead of a create/free pair.
        let upload_span = flamewm_profiler::start("render.backdrop.upload");
        let reuse = self
            .window_backdrop_pixmap
            .as_ref()
            .filter(|old| (old.width, old.height) == (width, height))
            .map(|old| old.pixmap);
        // `None` means the retained pixmap was refreshed in place.
        let upload = match reuse {
            Some(pixmap) => unsafe { self.upload_argb_into_pixmap(pixmap, &words, width, height) }
                .map(|()| None),
            None => unsafe { self.upload_argb_pixmap(&words, width, height) }.map(Some),
        };
        drop(upload_span);
        if let Some(state) = self.window_backdrop.as_mut() {
            // The pixels change only when the snapshot, its origin or its size does.
            let changed = state.captured.as_ref().is_none_or(|old| {
                old.words != words || (old.width, old.height) != (width, height) || old.root != (rx, ry)
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

    /// One-shot hidden-root capture used only on a surface's first appearance. Hides the window
    /// and its WM-frame ancestors for one `get_image` round trip so the snapshot contains the
    /// windows behind the surface, then restores the shapes. Never called during moves or settles.
    unsafe fn read_root_behind_window(&self, root: (u32, u32)) -> Result<RgbaImage, String> {
        let bridge = self
            .xshape
            .as_ref()
            .ok_or("window backdrop: no shape bridge to hide the window")?;
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
            let (mut queried_root, mut parent, mut children, mut count) =
                (0, 0, std::ptr::null_mut(), 0);
            unsafe {
                XQueryTree(
                    self.display,
                    current,
                    &mut queried_root,
                    &mut parent,
                    &mut children,
                    &mut count,
                )
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

    /// Root pixels for the whole root, read once per surface while nothing of it can be on the
    /// root. No shape change, no unmap and no `XSync` happen here: the caller only reaches this
    /// while [`Self::root_read_is_clean`] holds, so there is nothing to hide and nothing to
    /// restore (A01). Every later capture crops the blurred copy of this read.
    unsafe fn read_root_clean(&self, root: (u32, u32)) -> Result<RgbaImage, String> {
        let root_window = unsafe { XRootWindow(self.display, self.screen) };
        let capture = crate::backdrop::capture_drawable_rect(
            self.display,
            root_window as u64,
            (0, 0, root.0, root.1),
        )?;
        Ok(RgbaImage {
            width: capture.width,
            height: capture.height,
            pixels: capture.pixels,
        })
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
        if let Some(state) = self.window_backdrop.as_mut() {
            state.desktop = None;
            state.blurred_root = None;
            // A new read needs a new invalidation baseline.
            state.scene_children = None;
            state.deferred_at = None;
            // The snapshot is gone: the next one is a change even if its pixels match.
            state.captured = None;
            state.generation += 1;
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

/// Wallpaper region under `rect` for when no root readback is cached. The decoded image is kept
/// in `state`; only `rect` is composed (pixel-identical to cropping the full root canvas), so a
/// capture never builds the whole root.
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
    flamewm_image_core::wallpaper::compose_region(natural, root.0, root.1, fit, rect)
        .ok_or_else(|| "window backdrop: wallpaper region unavailable".to_string())
}

/// Crops a root-sized ARGB word image at `origin` to `size`. Pixels off the root replicate the
/// nearest root edge, as `edge_extend_rgba` does for the uncached path.
fn crop_words_clamped(
    words: &[u32],
    root: (u32, u32),
    origin: (i32, i32),
    size: (u32, u32),
) -> Vec<u32> {
    if root.0 == 0 || root.1 == 0 {
        return Vec::new();
    }
    let (rw, rh) = (i64::from(root.0), i64::from(root.1));
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let sy = (i64::from(origin.1) + y as i64).clamp(0, rh - 1) as usize;
        let row = &words[sy * root.0 as usize..][..root.0 as usize];
        for x in 0..w {
            let sx = (i64::from(origin.0) + x as i64).clamp(0, rw - 1) as usize;
            out.push(row[sx]);
        }
    }
    out
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
    Some(RgbaImage {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_cache_is_read_only_while_the_surface_cannot_be_on_the_root() {
        assert_eq!(
            root_capture_decision(false, false, true),
            RootCapture::ReadClean,
            "surface off root, nothing cached: one clean read"
        );
        assert_eq!(
            root_capture_decision(false, false, false),
            RootCapture::Defer,
            "surface on root, nothing cached: wallpaper, never a hide"
        );
        assert_eq!(
            root_capture_decision(false, true, false),
            RootCapture::Defer,
            "a settle never un-defers a read"
        );
    }

    /// A01 regression guard: the wake that ends the quiet period after a move never reads the root
    /// (the read used to shape-hide the window and its ancestors for a full-root `XGetImage`).
    #[test]
    fn settle_path_never_reads_the_root_and_never_hides_a_visible_window() {
        for cached in [false, true] {
            for clean in [false, true] {
                let settled = root_capture_decision(cached, true, clean);
                let unsettled = root_capture_decision(cached, false, clean);
                assert_eq!(
                    settled, unsettled,
                    "settling must not change the capture policy (cached={cached} clean={clean})"
                );
                if !clean {
                    assert_ne!(
                        settled,
                        RootCapture::ReadClean,
                        "a read needs a surface that cannot be on the root"
                    );
                }
                if cached {
                    assert_eq!(
                        settled,
                        RootCapture::ReuseCache,
                        "the cached clean root is reused at the settle"
                    );
                }
            }
        }
    }

    /// A01: after a move the cached copy is reused, and the crop follows the new origin without a
    /// second read (`blurred_root` is only ever written by the capture).
    #[test]
    fn a_move_crops_the_cached_snapshot_and_never_re_reads_the_root() {
        // 4x3 root, pixel value = y*4 + x.
        let root: Vec<u32> = (0..12).collect();
        let first = crop_words_clamped(&root, (4, 3), (0, 0), (2, 2));
        let moved = crop_words_clamped(&root, (4, 3), (1, 1), (2, 2));
        assert_eq!(first, vec![0, 1, 4, 5]);
        assert_eq!(moved, vec![5, 6, 9, 10], "same cache, new origin");
        assert_eq!(
            root_capture_decision(true, true, false),
            RootCapture::ReuseCache,
            "a settled move reuses the cached copy instead of reading the root"
        );
    }

    #[test]
    fn a_changed_root_stack_invalidates_the_cached_copy() {
        let known = [3, 7, 9];
        assert!(
            !scene_changed(&known, &[3, 7, 9]),
            "same stack: still valid"
        );
        assert!(scene_changed(&known, &[9, 7, 3]), "restacked");
        assert!(scene_changed(&known, &[3, 7]), "window vanished");
        assert!(scene_changed(&known, &[3, 7, 9, 11]), "window appeared");
    }

    #[test]
    fn blurred_root_crop_replicates_the_root_edge_off_root() {
        // 4x3 root, pixel value = y*4 + x.
        let root: Vec<u32> = (0..12).collect();
        let crop = crop_words_clamped(&root, (4, 3), (-1, 1), (3, 2));
        // Row y=1 and y=2 starting at x=-1: x=-1 clamps to column 0.
        assert_eq!(crop, vec![4, 4, 5, 8, 8, 9]);
        assert!(crop_words_clamped(&root, (0, 0), (0, 0), (2, 2)).is_empty());
    }

    #[test]
    fn root_reads_are_spaced_by_the_quiet_period() {
        assert!(readback_spaced(None), "no read yet: always allowed");
        assert!(
            !readback_spaced(Some(Duration::from_millis(100))),
            "too soon: keep cache"
        );
        assert!(!readback_spaced(Some(Duration::from_millis(249))));
        assert!(
            readback_spaced(Some(CAPTURE_QUIET)),
            "exactly one period later: read"
        );
    }
}
