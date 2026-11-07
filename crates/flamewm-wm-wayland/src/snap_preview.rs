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

/// Visible preview state: the frame it outlines and the accent it was rasterised with.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shown {
    frame: Rect,
    rgb: u32,
}

pub struct SnapPreview {
    accent: u32,
    shown: Option<Shown>,
    strips: Vec<Strip>,
}

impl Default for SnapPreview {
    fn default() -> Self {
        Self {
            accent: flamewm_api::settings::DEFAULT_ACCENT_RGB,
            shown: None,
            strips: Vec::new(),
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
            self.strips = ring_strips(shown.frame, rgb);
            self.shown = Some(Shown { rgb, ..shown });
        }
        true
    }

    /// Shows the preview around `frame`. Rasterises only when the frame or accent changed, so a
    /// steady drag costs no allocation. Returns whether the preview changed.
    pub fn show(&mut self, frame: Rect) -> bool {
        let next = Shown {
            frame,
            rgb: self.accent,
        };
        if self.shown == Some(next) {
            return false;
        }
        self.strips = ring_strips(frame, self.accent);
        self.shown = Some(next);
        true
    }

    /// Hides the preview and drops its buffers. Returns whether it was visible.
    pub fn hide(&mut self) -> bool {
        if self.shown.take().is_none() {
            return false;
        }
        self.strips.clear();
        true
    }

    /// The cached ring as overlay elements at the shown frame. Empty while hidden.
    pub fn elements(&self, renderer: &mut GlowRenderer) -> Vec<Elem> {
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
                        f64::from(frame.x.saturating_add(strip.offset.0)),
                        f64::from(frame.y.saturating_add(strip.offset.1)),
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
    fn show_rasterises_once_per_frame_and_accent() {
        let mut preview = SnapPreview::default();
        assert!(preview.show(FRAME));
        assert!(!preview.show(FRAME), "same frame is a cache hit");
        assert!(preview.show(Rect { x: 11, ..FRAME }));
        assert!(!preview.strips.is_empty());
    }

    #[test]
    fn hide_clears_buffers_and_reports_only_real_changes() {
        let mut preview = SnapPreview::default();
        assert!(!preview.hide(), "hiding a hidden preview is a no-op");
        preview.show(FRAME);
        assert!(preview.hide());
        assert!(preview.strips.is_empty());
        assert!(preview.shown.is_none());
    }

    #[test]
    fn accent_change_re_rasterises_a_visible_preview_only() {
        let mut preview = SnapPreview::default();
        let accent = preview.accent;
        assert!(!preview.set_accent(accent));
        assert!(preview.set_accent(0x00ef_4048));
        assert!(preview.shown.is_none());
        preview.show(FRAME);
        assert!(preview.set_accent(0x0011_2233));
        assert_eq!(preview.shown.map(|shown| shown.rgb), Some(0x0011_2233));
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
