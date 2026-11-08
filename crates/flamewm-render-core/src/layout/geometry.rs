//! Length clamping and scrollbar thumb geometry for layout.

use super::*;

pub(super) fn clamp_length(value: f32, min: Length, max: Length, available: f32) -> f32 {
    let mut result = value.max(0.0);
    if let Some(minimum) = min.resolve(available) {
        result = result.max(minimum);
    }
    if let Some(maximum) = max.resolve(available) {
        result = result.min(maximum);
    }
    result
}

pub(super) fn thumb_rect(
    viewport: Rect,
    content_len: f32,
    viewport_len: f32,
    offset: f32,
    horizontal: bool,
) -> Rect {
    // C01: never panic on nonfinite/zero/negative/tiny geometry.
    if !viewport_len.is_finite()
        || !content_len.is_finite()
        || viewport_len <= 0.0
        || content_len <= 0.0
        || content_len <= viewport_len
    {
        return Rect::default();
    }
    if !viewport.x.is_finite()
        || !viewport.y.is_finite()
        || !viewport.width.is_finite()
        || !viewport.height.is_finite()
    {
        return Rect::default();
    }
    let track = if horizontal {
        viewport.width
    } else {
        viewport.height
    };
    if !track.is_finite() || track <= 0.0 {
        return Rect::default();
    }
    let raw = viewport_len / content_len * track;
    if !raw.is_finite() {
        return Rect::default();
    }
    let min_thumb = 12.0f32.min(track);
    let thumb_len = raw.clamp(min_thumb, track);
    if !thumb_len.is_finite() {
        return Rect::default();
    }
    let travel = (track - thumb_len).max(0.0);
    let max_offset = (content_len - viewport_len).max(1.0);
    if !max_offset.is_finite() || max_offset <= 0.0 {
        return Rect::default();
    }
    let offset = if offset.is_finite() { offset } else { 0.0 };
    let pos = (offset.clamp(0.0, max_offset) / max_offset) * travel;
    if !pos.is_finite() {
        return Rect::default();
    }
    if horizontal {
        Rect {
            x: viewport.x + pos,
            y: viewport.y + viewport.height - 8.0,
            width: thumb_len,
            height: 8.0,
        }
    } else {
        Rect {
            x: viewport.x + viewport.width - 8.0,
            y: viewport.y + pos,
            width: 8.0,
            height: thumb_len,
        }
    }
}
