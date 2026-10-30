//! Rect conversions between frame, root, and core coordinate types.
use super::*;

pub(super) fn from_root_rect(rect: RootRect) -> Rect {
    Rect::new(rect.x, rect.y, rect.w.max(0) as u32, rect.h.max(0) as u32)
}

/// Work-area rect as a root rect; a zero-size area becomes 1px so the controller never sees empty bounds.
pub(super) fn to_root_rect(rect: Rect) -> RootRect {
    RootRect::new(
        rect.x,
        rect.y,
        rect.width.max(1) as i32,
        rect.height.max(1) as i32,
    )
}

pub(super) fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

pub(super) fn clamp_u16(value: u32) -> u16 {
    value.clamp(1, u32::from(u16::MAX)) as u16
}

pub(super) fn to_core_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(
        rect.x,
        rect.y,
        i32::try_from(rect.width).unwrap_or(i32::MAX),
        i32::try_from(rect.height).unwrap_or(i32::MAX),
    )
}

pub(super) fn from_core_rect(rect: flamewm_api::Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y,
        u32::try_from(rect.width).unwrap_or(1).max(1),
        u32::try_from(rect.height).unwrap_or(1).max(1),
    )
}

pub(super) fn rect_overlap_area(a: Rect, b: Rect) -> u64 {
    let width = a.right().min(b.right()).saturating_sub(a.x.max(b.x));
    let height = a.bottom().min(b.bottom()).saturating_sub(a.y.max(b.y));
    u64::try_from(width)
        .unwrap_or(0)
        .saturating_mul(u64::try_from(height).unwrap_or(0))
}
