//! Rounded snap-target preview for a move grab. Same style as the desktop selection: a 2px solid
//! accent border on an 8px radius with a transparent fill. The ring is rasterised by the shared
//! outline owner and cached; frames only wrap the cached buffers, and a hide clears them.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::utils::{Point, Size, Transform};

use crate::model::Rect;
use crate::render::{Elem, premultiplied_argb_bytes};
use flamewm_render_core::outline;

/// Corner radius of `.desktop-selection` (`border-radius: 8px`).
pub(crate) const PREVIEW_RADIUS: u32 = 8;
/// Border width of `.desktop-selection` (`border: 2px solid`).
pub(crate) const PREVIEW_THICKNESS: u32 = 2;

struct Strip {
    offset: (i32, i32),
    size: (i32, i32),
    buffer: MemoryRenderBuffer,
}

/// Visible preview state: the landing frame. Only its origin changes during a translation.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shown {
    frame: Rect,
}

/// What the cached strips were rasterised for. Translation never changes it, so a moving ring
/// keeps its buffers (and their GL textures) and only its element location moves.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RingKey {
    w: i32,
    h: i32,
    rgb: u32,
    radius: u32,
}

pub struct SnapPreview {
    accent: u32,
    shown: Option<Shown>,
    /// Key of `strips`; `None` while no ring is cached.
    key: Option<RingKey>,
    strips: Vec<Strip>,
    /// When the ring was last rasterised; resize drags change the size on every pointer event, so
    /// rasterising is paced to one per frame interval.
    last_raster: Option<std::time::Instant>,
    /// Latest frame deferred by pacing; applied by `settle` on the next painted frame so the ring
    /// never stays behind the pointer once motion stops.
    pending: Option<Rect>,
    /// Raster passes performed; the tests assert that a translation-only drag costs none.
    #[cfg(test)]
    rasters: u32,
    /// Test-only clock override; `None` reads the real monotonic clock.
    #[cfg(test)]
    clock: Option<std::time::Instant>,
}

impl Default for SnapPreview {
    fn default() -> Self {
        Self {
            accent: flamewm_api::settings::DEFAULT_ACCENT_RGB,
            shown: None,
            key: None,
            strips: Vec::new(),
            last_raster: None,
            pending: None,
            #[cfg(test)]
            rasters: 0,
            #[cfg(test)]
            clock: None,
        }
    }
}

impl SnapPreview {
    /// Monotonic time used for pacing; tests inject it so they never sleep.
    fn now(&self) -> std::time::Instant {
        #[cfg(test)]
        {
            if let Some(now) = self.clock {
                return now;
            }
        }
        std::time::Instant::now()
    }

    /// Records the accent from the shared settings owner. Returns whether it changed; a visible
    /// preview is re-rasterised in place, so the next frame paints the new colour.
    pub fn set_accent(&mut self, rgb: u32) -> bool {
        if self.accent == rgb {
            return false;
        }
        self.accent = rgb;
        if let Some(shown) = self.shown {
            self.ensure_ring((shown.frame.w, shown.frame.h));
        }
        true
    }

    /// Shows the preview around `frame`. Re-rasterises only when the size or accent changed; a
    /// pure move reuses the cached strips. Returns whether the visible preview changed.
    pub fn show(&mut self, frame: Rect) -> bool {
        let before = (self.shown, self.key);
        // A size change while a ring is cached is paced to ~60 Hz: raw pointer motion arrives at
        // 1 kHz+, and each raster allocates strips and uploads new textures. The stale ring is
        // replaced by the next due motion or hidden on release.
        let paced = self.key.is_some_and(|k| (k.w, k.h) != (frame.w, frame.h))
            && self.last_raster.is_some_and(|t| {
                self.now().saturating_duration_since(t) < std::time::Duration::from_millis(16)
            });
        if paced {
            // Defer to the next frame (one raster per frame at most) and ask for that paint.
            self.pending = Some(frame);
            return true;
        }
        self.pending = None;
        self.ensure_ring((frame.w, frame.h));
        self.shown = Some(Shown { frame });
        before != (self.shown, self.key)
    }

    /// Hides the preview and drops its buffers. Returns whether it was visible.
    pub fn hide(&mut self) -> bool {
        self.pending = None;
        if self.shown.take().is_none() {
            return false;
        }
        self.strips.clear();
        self.key = None;
        true
    }

    /// Applies the frame that pacing deferred. Called once per painted frame, so it costs at most
    /// one raster per frame and leaves the ring on the pointer's final position.
    pub fn settle(&mut self) {
        if let Some(frame) = self.pending.take() {
            self.ensure_ring((frame.w, frame.h));
            self.shown = Some(Shown { frame });
        }
    }

    /// Rasterises the ring only when its key (size, accent, radius) differs from the cache.
    fn ensure_ring(&mut self, (w, h): (i32, i32)) {
        let key = RingKey {
            w,
            h,
            rgb: self.accent,
            radius: PREVIEW_RADIUS,
        };
        if self.key == Some(key) {
            return;
        }
        self.strips = ring_strips(Rect { x: 0, y: 0, w, h }, self.accent);
        self.key = Some(key);
        self.last_raster = Some(self.now());
        #[cfg(test)]
        {
            self.rasters += 1;
        }
    }

    /// The cached ring as overlay elements at the shown frame. Empty while hidden.
    pub fn elements(&self, renderer: &mut GlowRenderer, scale: f64) -> Vec<Elem> {
        let Some(shown) = self.shown else {
            return Vec::new();
        };
        let frame = shown.frame;
        self.strips
            .iter()
            .filter_map(|strip| {
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    Point::from((
                        f64::from(frame.x.saturating_add(strip.offset.0)) * scale,
                        f64::from(frame.y.saturating_add(strip.offset.1)) * scale,
                    )),
                    &strip.buffer,
                    None,
                    None,
                    Some(Size::from(strip.size)),
                    Kind::Unspecified,
                )
                .ok()
                .map(Elem::Memory)
            })
            .collect()
    }
}

/// Perimeter strips of the 2px rounded ring for `frame`. Empty for a degenerate frame.
fn ring_strips(frame: Rect, rgb: u32) -> Vec<Strip> {
    if frame.w <= 0 || frame.h <= 0 || frame.w > 32_767 || frame.h > 32_767 {
        return Vec::new();
    }
    outline::outline_strips(
        frame.w as u32,
        frame.h as u32,
        PREVIEW_RADIUS,
        PREVIEW_THICKNESS,
        rgb,
        flamewm_ui_core::style::PREVIEW_OUTLINE_ALPHA,
    )
    .into_iter()
    .map(|(x, y, w, h, rgba)| Strip {
        offset: (x as i32, y as i32),
        size: (w as i32, h as i32),
        buffer: MemoryRenderBuffer::from_slice(
            &premultiplied_argb_bytes(&rgba),
            Fourcc::Argb8888,
            (w as i32, h as i32),
            1,
            Transform::Normal,
            None,
        ),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect {
        x: 10,
        y: 20,
        w: 300,
        h: 200,
    };

    #[test]
    fn show_rasterises_once_per_size_and_accent() {
        let mut preview = SnapPreview::default();
        assert!(preview.show(FRAME));
        assert!(!preview.show(FRAME), "same frame is a cache hit");
        assert!(
            preview.show(Rect { x: 11, ..FRAME }),
            "a move is a visible change"
        );
        assert_eq!(
            preview.rasters, 1,
            "a move with the same size re-rasterises nothing"
        );
        assert!(!preview.strips.is_empty());
        assert!(
            preview.show(Rect { w: 301, ..FRAME }),
            "a paced resize still requests a paint"
        );
        assert_eq!(preview.rasters, 1, "but defers the raster");
        preview.settle();
        assert_eq!(preview.rasters, 2, "the next frame settles it");
        assert_eq!(preview.rasters, 2, "a resize re-rasterises once");
    }

    #[test]
    fn hundred_translation_motions_rasterise_once() {
        let mut preview = SnapPreview::default();
        preview.show(FRAME);
        let strips_before = preview.strips.len();
        for step in 1..=100 {
            let moved = Rect {
                x: FRAME.x + step,
                y: FRAME.y + step / 2,
                ..FRAME
            };
            assert!(preview.show(moved), "step {step} moves the visible ring");
        }
        assert_eq!(
            preview.rasters, 1,
            "100 translations: one raster, zero buffer rebuilds"
        );
        assert_eq!(preview.strips.len(), strips_before);
        assert_eq!(preview.shown.map(|s| s.frame.x), Some(FRAME.x + 100));
    }

    #[test]
    fn one_second_of_outline_resize_stays_within_raster_budget() {
        let mut preview = SnapPreview::default();
        let base = std::time::Instant::now();
        preview.clock = Some(base);
        preview.show(FRAME);
        let rasters_before = preview.rasters;
        let at = |ms: u64| Some(base + std::time::Duration::from_millis(ms));
        let frame_for = |i: i32| Rect {
            w: FRAME.w + i + 1,
            h: FRAME.h + i + 1,
            ..FRAME
        };
        // 1000 resize events at 1 ms spacing = one simulated second; settle once per 16 ms.
        for i in 0..1000i32 {
            preview.clock = at(i as u64);
            preview.show(frame_for(i));
            if i % 16 == 15 {
                preview.settle();
            }
        }
        let rasters = preview.rasters - rasters_before;
        assert!(
            rasters <= 70,
            "one second of outline resize: {rasters} rasters (budget 70)"
        );
        // Final settle after the last event: the ring must land on the last frame.
        preview.clock = at(1008);
        preview.settle();
        let last = frame_for(999);
        assert_eq!(
            preview
                .shown
                .map(|s| (s.frame.x, s.frame.y, s.frame.w, s.frame.h)),
            Some((last.x, last.y, last.w, last.h)),
            "no stale trailing edge after the final event ({rasters} rasters)"
        );
        assert_eq!(
            preview.key.map(|k| (k.w, k.h)),
            Some((last.w, last.h)),
            "cached ring matches the last frame ({rasters} rasters)"
        );
    }

    #[test]
    fn hide_clears_buffers_and_reports_only_real_changes() {
        let mut preview = SnapPreview::default();
        assert!(!preview.hide(), "hiding a hidden preview is a no-op");
        preview.show(FRAME);
        assert!(preview.hide());
        assert!(preview.strips.is_empty());
        assert!(preview.shown.is_none());
        assert!(preview.key.is_none());
    }

    #[test]
    fn accent_change_re_rasterises_a_visible_preview_only() {
        let mut preview = SnapPreview::default();
        let accent = preview.accent;
        assert!(!preview.set_accent(accent));
        assert!(preview.set_accent(0x00ef_4048));
        assert!(preview.shown.is_none());
        preview.show(FRAME);
        let rasters = preview.rasters;
        assert!(preview.set_accent(0x0011_2233));
        assert_eq!(preview.rasters, rasters + 1);
        assert_eq!(preview.key.map(|k| k.rgb), Some(0x0011_2233));
        assert!(!preview.strips.is_empty());
    }

    #[test]
    fn ring_is_rounded_and_degenerate_frames_draw_nothing() {
        // Corner pixel sits outside the 8px arc; the mid-edge pixel is on the 2px band.
        let rgba = outline::outline_rgba(
            300,
            200,
            PREVIEW_RADIUS,
            PREVIEW_THICKNESS,
            0x003d_aee9,
            255,
        );
        let alpha = |x: usize, y: usize| rgba[(y * 300 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "rounded corner is transparent");
        assert_eq!(alpha(150, 0), 255, "top edge is the accent border");
        assert_eq!(alpha(150, 100), 0, "fill stays transparent");
        assert!(ring_strips(Rect { w: 0, ..FRAME }, 0).is_empty());
    }
}
