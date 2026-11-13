//! Alt+Tab preview producer. While the switcher is open, the render path draws each queued window
//! offscreen at its thumbnail size and hands the pixels to the switcher cache (`control/switcher.rs`).
//! The GPU does the downscale, so the readback is thumbnail-sized only.
//!
//! Precedent: `.vendor/vela-main` `shell/src/windowcapture.cpp:251-385` (capture starts only when the
//! Alt-Tab UI asks, and the stored thumbnail is downscaled). The per-frame budget follows the X11 path
//! (`SWITCHER_CAPTURES_PER_TURN`, docs/backlog/alttab.md section 7). Screencopy precedent:
//! `screencopy.rs` (`PendingCopy`, `render::offscreen_pixels`).

use super::*;
use flamewm_api::switcher::SwitcherThumb;

/// Previews rendered per frame. One offscreen pass per frame keeps a switcher with many windows
/// from stalling the frame loop; the rest wait for the next frame.
const CAPTURES_PER_FRAME: usize = 1;

/// Largest preview box in pixels. Matches the X11 path (`THUMB_MAX_W` / `THUMB_MAX_H`).
const THUMB_MAX_W: u32 = 160;
const THUMB_MAX_H: u32 = 100;

/// Renders queued switcher previews, at most `CAPTURES_PER_FRAME` per call. Does nothing when no
/// switcher is open, so an idle compositor does no extra GPU work.
pub fn serve_switcher_thumbs(state: &mut State, renderer: &mut GlowRenderer) {
    let Some(service) = state.control.clone() else {
        return;
    };
    let refreshes_waiting = service.promote_switcher_refreshes(std::time::Instant::now());
    for _ in 0..CAPTURES_PER_FRAME {
        let Some(id) = service.next_switcher_capture() else {
            break;
        };
        match capture_window(state, renderer, id) {
            Ok(Some(thumb)) => service.store_switcher_thumb(id, thumb),
            Ok(None) => {}
            Err(error) => eprintln!("flamewm-wayland: switcher preview failed: {error}"),
        }
    }
    if refreshes_waiting || service.switcher_captures_pending() {
        state.request_redraw();
    }
}

/// Renders one window's surface tree scaled into its thumbnail box. `None` when the window is gone,
/// has no usable size, or has no drawable surface yet, so the switcher keeps the title row.
fn capture_window(
    state: &State,
    renderer: &mut GlowRenderer,
    id: WindowId,
) -> Result<Option<SwitcherThumb>, Box<dyn std::error::Error>> {
    // A hidden window keeps the frame it had when last visible (control/switcher.rs ThumbCache).
    if state.model.is_hidden(id) {
        return Ok(None);
    }
    let Some((rect_w, rect_h)) = state.model.get(id).map(|m| (m.rect.w, m.rect.h)) else {
        return Ok(None);
    };
    let (Ok(src_w), Ok(src_h)) = (u32::try_from(rect_w), u32::try_from(rect_h)) else {
        return Ok(None);
    };
    let Some((width, height)) = thumb_fit(src_w, src_h) else {
        return Ok(None);
    };
    let Some((_, top)) = state.toplevels.iter().find(|(known, _)| *known == id) else {
        return Ok(None);
    };
    let scale = f64::from(width) / f64::from(src_w);
    let elements = surface_elements(renderer, top.wl_surface(), 0, 0, scale);
    if elements.is_empty() {
        return Ok(None);
    }
    let size = Size::from((width as i32, height as i32));
    let rgba = thumb_pixels(renderer, &elements, size)?;
    Ok(Some(SwitcherThumb {
        width,
        height,
        generation: 0,
        rgba,
    }))
}

/// Reads back `elements` drawn into a `size` texture. The damage tracker is sized to the thumbnail,
/// not the output: `OutputDamageTracker::from_output` sets the GL viewport to the output mode, which
/// squeezes a thumbnail-sized draw into a corner of the texture (the preview came back background only).
/// Precedent for the sized tracker: `render/dock_backdrop.rs`.
fn thumb_pixels(
    renderer: &mut GlowRenderer,
    elements: &[Elem],
    size: Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let buffer_size: Size<i32, Buffer> = Size::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
    {
        let mut target = renderer.bind(&mut texture)?;
        tracker.render_output(renderer, &mut target, 0, elements, CLEAR)?;
    }
    let target = renderer.bind(&mut texture)?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)?;
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

/// Aspect-fit a source size into the preview box. Never upscales: a source smaller than the box
/// keeps its size. `None` for an empty source.
fn thumb_fit(src_w: u32, src_h: u32) -> Option<(u32, u32)> {
    if src_w == 0 || src_h == 0 {
        return None;
    }
    if src_w <= THUMB_MAX_W && src_h <= THUMB_MAX_H {
        return Some((src_w, src_h));
    }
    let scale_w = u64::from(THUMB_MAX_W) * u64::from(src_h);
    let scale_h = u64::from(THUMB_MAX_H) * u64::from(src_w);
    if scale_w <= scale_h {
        let h = (u64::from(src_h) * u64::from(THUMB_MAX_W) / u64::from(src_w)).max(1);
        Some((THUMB_MAX_W, u32::try_from(h).unwrap_or(THUMB_MAX_H)))
    } else {
        let w = (u64::from(src_w) * u64::from(THUMB_MAX_H) / u64::from(src_h)).max(1);
        Some((u32::try_from(w).unwrap_or(THUMB_MAX_W), THUMB_MAX_H))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_fit_never_upscales_and_keeps_aspect() {
        assert_eq!(thumb_fit(0, 10), None);
        assert_eq!(thumb_fit(100, 50), Some((100, 50)));
        assert_eq!(thumb_fit(640, 400), Some((160, 100)));
        assert_eq!(thumb_fit(300, 200), Some((150, 100)));
        assert_eq!(thumb_fit(1000, 100), Some((160, 16)));
    }
}
