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
    /// Raster passes performed; the tests assert that a translation-only drag costs none.
    #[cfg(test)]
    rasters: u32,
}

impl Default for SnapPreview {
    fn default() -> Self {
        Self {
            accent: flamewm_api::settings::DEFAULT_ACCENT_RGB,
            shown: None,
            key: None,
            strips: Vec::new(),
            #[cfg(test)]
            rasters: 0,
        }
    }
}

impl SnapPreview {
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
        self.ensure_ring((frame.w, frame.h));
        self.shown = Some(Shown { frame });
        before != (self.shown, self.key)
    }

    /// Hides the preview and drops its buffers. Returns whether it was visible.
    pub fn hide(&mut self) -> bool {
        if self.shown.take().is_none() {
            return false;
        }
        self.strips.clear();
        self.key = None;
        true
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
        self.strips = ring_strips(
            Rect {
                x: 0,
                y: 0,
                w,
                h,
            },
            self.accent,
        );
        self.key = Some(key);
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
        assert!(preview.show(Rect { x: 11, ..FRAME }), "a move is a visible change");
        assert_eq!(preview.rasters, 1, "a move with the same size re-rasterises nothing");
        assert!(!preview.strips.is_empty());
        assert!(preview.show(Rect { w: 301, ..FRAME }));
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
        assert_eq!(preview.rasters, 1, "100 translations: one raster, zero buffer rebuilds");
        assert_eq!(preview.strips.len(), strips_before);
        assert_eq!(preview.shown.map(|s| s.frame.x), Some(FRAME.x + 100));
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
