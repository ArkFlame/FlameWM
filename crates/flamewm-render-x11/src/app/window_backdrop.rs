//! Window-level pseudo-transparency: the wallpaper under a whole Normal window.
//!
//! Owns the window snapshot lifecycle (capture, move/resize policy, teardown). The snapshot
//! lives in its own slot so the popup mask (which keys off `surface_backdrop`) keeps its shape.
//! Paint falls back to this slot only when no popup backdrop is installed.

use std::time::{Duration, Instant};

use flamewm_image_core::RgbaImage;

use super::X11App;
use crate::backdrop::{WindowBackdrop, blurred_argb_words, recapture_due, wallpaper_file};
use crate::xlib::*;
use crate::xlib_ext::XTranslateCoordinates;

/// Smallest interval between two window snapshot captures. A drag re-captures at most this
/// often and a trailing capture lands once the pointer stops (see `arm_backdrop_deadline`).
pub(crate) const CAPTURE_INTERVAL: Duration = Duration::from_millis(250);

/// Outcome of the capture throttle for one refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureThrottle {
    /// Capture now.
    Capture,
    /// Too soon after the last capture: stay stale and wake at this trailing instant.
    Defer(Instant),
}

/// Pure throttle: captures at most once per `interval`; a deferred capture is due exactly
/// `interval` after the last one.
#[must_use]
pub(crate) fn throttle_capture(last: Option<Instant>, now: Instant, interval: Duration) -> CaptureThrottle {
    match last {
        Some(last) if now.duration_since(last) < interval => CaptureThrottle::Defer(last + interval),
        _ => CaptureThrottle::Capture,
    }
}

/// Per-window state: the spec, the decoded wallpaper cache, and whether a capture is due.
#[derive(Debug)]
pub(crate) struct WindowBackdropState {
    pub(crate) spec: WindowBackdrop,
    natural: Option<RgbaImage>,
    /// Wallpaper composed over the whole root for (root w, root h, fit); a capture crops it.
    canvas: Option<(u32, u32, String, RgbaImage)>,
    pub(crate) stale: bool,
    last_capture: Option<Instant>,
}

impl WindowBackdropState {
    pub(crate) fn new(spec: WindowBackdrop) -> Self {
        Self {
            spec,
            natural: None,
            canvas: None,
            stale: true,
            last_capture: None,
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
        let now = Instant::now();
        let Some(state) = self.window_backdrop.as_mut() else {
            return;
        };
        if let CaptureThrottle::Defer(deadline) =
            throttle_capture(state.last_capture, now, CAPTURE_INTERVAL)
        {
            // Throttled: keep the snapshot stale and wake again at the trailing edge.
            self.backdrop_deadline = Some(deadline);
            return;
        }
        state.stale = false;
        state.last_capture = Some(now);
        self.backdrop_deadline = None;
        if let Err(error) = unsafe { self.capture_window_backdrop() } {
            eprintln!("FLAMEWM_WINDOW_BACKDROP degraded reason={error}");
            self.clear_window_backdrop();
        }
    }

    /// Wallpaper under the window's root rect, blurred and uploaded as the window snapshot.
    unsafe fn capture_window_backdrop(&mut self) -> Result<(), String> {
        let _capture_span = flamewm_profiler::start("render.backdrop.capture");
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
        // Compose only the root-visible part; the window may be dragged partly off the root.
        // Off-root pixels replicate the nearest wallpaper edge (never an opaque black slot).
        let anchor = crate::backdrop::root_anchor_rect(rect, (root_w, root_h));
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
        let compose_span = flamewm_profiler::start("render.backdrop.compose");
        // The composed root canvas depends only on the wallpaper, the root size and the fit, so it is
        // built once and each capture crops it (the per-pixel resample is the expensive part).
        let canvas_fresh = state
            .canvas
            .as_ref()
            .is_some_and(|(w, h, f, _)| (*w, *h, f) == (root_w, root_h, &state.spec.fit));
        if !canvas_fresh {
            let full = flamewm_image_core::wallpaper::compose_region(
                natural,
                root_w,
                root_h,
                fit,
                (0, 0, root_w, root_h),
            )
            .ok_or("window backdrop: wallpaper canvas unavailable")?;
            state.canvas = Some((root_w, root_h, state.spec.fit.clone(), full));
        }
        let snapshot = crop_rgba(
            &state.canvas.as_ref().expect("canvas built above").3,
            anchor,
        )
        .ok_or("window backdrop: wallpaper region unavailable")?;
        drop(compose_span);
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

    /// Frees the window snapshot, if any. Translucent fills then show as straight colour.
    pub(crate) fn clear_window_backdrop(&mut self) {
        if let Some(old) = self.window_backdrop_pixmap.take() {
            unsafe { XFreePixmap(self.display, old.pixmap) };
        }
    }
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

    const INTERVAL: Duration = Duration::from_millis(250);

    #[test]
    fn first_capture_is_never_throttled() {
        assert_eq!(
            throttle_capture(None, Instant::now(), INTERVAL),
            CaptureThrottle::Capture
        );
    }

    #[test]
    fn capture_inside_interval_defers_to_trailing_edge() {
        let last = Instant::now();
        let now = last + Duration::from_millis(100);
        assert_eq!(
            throttle_capture(Some(last), now, INTERVAL),
            CaptureThrottle::Defer(last + INTERVAL),
            "trailing deadline is last capture + interval, not now + interval"
        );
    }

    #[test]
    fn capture_at_or_after_interval_is_allowed() {
        let last = Instant::now();
        assert_eq!(
            throttle_capture(Some(last), last + INTERVAL, INTERVAL),
            CaptureThrottle::Capture
        );
        assert_eq!(
            throttle_capture(Some(last), last + INTERVAL * 3, INTERVAL),
            CaptureThrottle::Capture
        );
    }
}

