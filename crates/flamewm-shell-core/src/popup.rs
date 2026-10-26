//! Shell popup geometry. Pure rect math; no surface or runtime state here.

use flamewm_api::display::OutputSnapshot;
use flamewm_api::panels::PanelSnapshot;
use flamewm_api::{OutputId, PanelEdge, Point, Rect, Size};
use flamewm_ui_core::popover::{PopoverEdge, PopoverGeometry};
use flamewm_ui_core::style::ShellMetrics;
use flamewm_ui_core::{PopoverDirection, anchor_popover};

#[must_use]
pub fn anchor_popover_rect(anchor: Rect, size: Size) -> Rect {
    Rect::new(anchor.x, anchor.y, size.width, size.height)
}

#[must_use]
pub fn status_anchor(
    panel: &PanelSnapshot,
    output: &OutputSnapshot,
    metrics: ShellMetrics,
    slot: i32,
) -> Rect {
    panel_popup_rect(panel, output, metrics, Size::new(1, 1), slot)
}

#[must_use]
pub fn panel_anchor(
    panel: &PanelSnapshot,
    output: &OutputSnapshot,
    metrics: ShellMetrics,
    slot: i32,
) -> Rect {
    status_anchor(panel, output, metrics, slot)
}

#[must_use]
pub fn panel_popup_rect(
    panel: &PanelSnapshot,
    output: &OutputSnapshot,
    metrics: ShellMetrics,
    size: Size,
    slot: i32,
) -> Rect {
    let panel_rect = panel.geometry;
    let slot_width = i32::from(metrics.tray_button_width);
    let clock_width = i32::from(metrics.clock_min_width);
    let anchor = if panel.edge.is_horizontal() {
        let right = panel_rect.right();
        let x = right - clock_width - slot_width * (3 - slot.min(3));
        Rect::new(x, panel_rect.y, slot_width, panel_rect.height)
    } else {
        let bottom = panel_rect.bottom();
        let y = bottom - clock_width - slot_width * (3 - slot.min(3));
        Rect::new(panel_rect.x, y, panel_rect.width, slot_width)
    };
    let direction = match panel.edge {
        PanelEdge::Bottom => PopoverDirection::Above,
        PanelEdge::Top => PopoverDirection::Below,
        PanelEdge::Left => PopoverDirection::RightOf,
        PanelEdge::Right => PopoverDirection::LeftOf,
    };
    anchor_popover(
        panel.output.clone(),
        output.geometry,
        anchor,
        (size.width, size.height),
        direction,
        i32::from(metrics.popover_offset),
    )
    .rect
}

/// Panel work area: pure rect math from resolved snapshot rects.
/// Subtracts the panel strip along `edge` from `output_rect` using the
/// already-scaled `panel_rect` (`HiDPI` baked in). Clamps to non-negative
/// extents; handles negative origins.
#[must_use]
pub fn work_area_for_panel(output_rect: Rect, panel_rect: Rect, edge: PanelEdge) -> Rect {
    match edge {
        PanelEdge::Bottom => {
            let height = (panel_rect.y - output_rect.y).clamp(0, output_rect.height.max(0));
            Rect::new(
                output_rect.x,
                output_rect.y,
                output_rect.width.max(0),
                height,
            )
        }
        PanelEdge::Top => {
            let top = panel_rect.bottom().max(output_rect.y);
            let height = (output_rect.bottom() - top).max(0);
            Rect::new(output_rect.x, top, output_rect.width.max(0), height)
        }
        PanelEdge::Left => {
            let left = panel_rect.right().max(output_rect.x);
            let width = (output_rect.right() - left).max(0);
            Rect::new(left, output_rect.y, width, output_rect.height.max(0))
        }
        PanelEdge::Right => {
            let width = (panel_rect.x - output_rect.x).clamp(0, output_rect.width.max(0));
            Rect::new(
                output_rect.x,
                output_rect.y,
                width,
                output_rect.height.max(0),
            )
        }
    }
}

/// Native context-menu geometry (C08): width is the canonical outer
/// minimum (195px row + 2x5px padding = 205px, border-box); height derives
/// from the actual visible parts (one 31px row each + 2x5px padding).
/// Only `menu_rect` clamps to the work area/output.
#[must_use]
pub fn context_menu_size(visible_rows: usize) -> Size {
    crate::menu_policy::context_menu_size(&crate::menu_policy::menu_parts_for_rows(visible_rows))
}

/// Pointer-anchored popup rect: point-anchored at the root click point,
/// clamped inside the output work area. Falls back to output origin only
/// when no pointer data exists.
#[must_use]
pub fn context_menu_rect(
    output: OutputId,
    output_rect: Rect,
    pointer: Option<Point>,
    size: Size,
) -> Rect {
    let _ = output;
    let anchor = pointer.unwrap_or(Point::new(output_rect.x, output_rect.y));
    crate::menu_policy::context_menu_rect(output_rect, anchor, size)
}

/// Popup open refusal: the caller must not map a surface without a
/// measured layout and a resolved anchor. Mapping at 0,0 is never a valid
/// fallback for status/Start popovers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupRefusal {
    /// Intrinsic document size unavailable (layout pending).
    PendingLayout,
    /// Anchor node rect unavailable (anchor pending).
    PendingAnchor,
    /// Pointer grab contention (transient; retry on next toggle).
    PointerGrabRefused,
}

impl std::fmt::Display for PopupRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingLayout => write!(f, "popup layout pending measurement"),
            Self::PendingAnchor => write!(f, "popup anchor pending resolution"),
            Self::PointerGrabRefused => write!(f, "popup pointer grab refused"),
        }
    }
}

/// Measured placement: source rect + intrinsic size placed with
/// `PopoverGeometry::place_aligned` + the given align, clamped to the work
/// area. Refuses `PendingAnchor`/`PendingLayout` instead of falling back
/// to 0,0.
pub fn measured_popup_rect(
    source: Option<Rect>,
    intrinsic: Option<Size>,
    edge: PopoverEdge,
    align: flamewm_ui_core::popover::PopoverAlign,
    work_area: Rect,
    offset: i32,
) -> Result<Rect, PopupRefusal> {
    let source = source.ok_or(PopupRefusal::PendingAnchor)?;
    let size = intrinsic.ok_or(PopupRefusal::PendingLayout)?;
    if size.width <= 0 || size.height <= 0 {
        return Err(PopupRefusal::PendingLayout);
    }
    let geometry = PopoverGeometry::place_aligned(source, size, edge, align, work_area, offset);
    let rect = Rect::from_parts(geometry.origin, size);
    Ok(rect)
}

/// Start-menu fitted placement: pure rect math with no runtime state.
/// The popup opens adjacent to `anchor` on the side opposite `panel_edge`
/// and its extent along the opening axis is capped to the space available
/// in `work_area` minus `gap`. The cross axis is clamped inside the work
/// area without changing the adjacent edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartPlacement {
    pub rect: Rect,
    pub content_was_capped: bool,
}

pub fn fitted_start_placement(
    anchor: Rect,
    intrinsic: Size,
    panel_edge: PanelEdge,
    work_area: Rect,
    gap: i32,
) -> Result<StartPlacement, PopupRefusal> {
    if intrinsic.width <= 0 || intrinsic.height <= 0 {
        return Err(PopupRefusal::PendingLayout);
    }
    let area_w = work_area.width.max(0);
    let area_h = work_area.height.max(0);
    let placed = |rect: Rect| StartPlacement {
        rect,
        content_was_capped: rect.width != intrinsic.width || rect.height != intrinsic.height,
    };
    let fit = |available: i32| {
        (available > 0)
            .then_some(available)
            .ok_or(PopupRefusal::PendingLayout)
    };
    match panel_edge {
        PanelEdge::Bottom => {
            // Work-area edge (live panel outer top), not the inset node top.
            let limit = anchor.y.min(work_area.bottom());
            let height = intrinsic.height.min(fit(limit - work_area.y - gap)?);
            let width = intrinsic.width.min(area_w);
            let y = limit - gap - height;
            let x = clamp_start(anchor.x, width, work_area.x, work_area.right());
            Ok(placed(Rect::new(x, y, width, height)))
        }
        PanelEdge::Top => {
            let limit = anchor.bottom().max(work_area.y);
            let height = intrinsic.height.min(fit(work_area.bottom() - limit - gap)?);
            let width = intrinsic.width.min(area_w);
            let y = limit + gap;
            let x = clamp_start(anchor.x, width, work_area.x, work_area.right());
            Ok(placed(Rect::new(x, y, width, height)))
        }
        PanelEdge::Left => {
            let limit = anchor.right().max(work_area.x);
            let width = intrinsic.width.min(fit(work_area.right() - limit - gap)?);
            let height = intrinsic.height.min(area_h);
            let x = limit + gap;
            let y = clamp_start(anchor.y, height, work_area.y, work_area.bottom());
            Ok(placed(Rect::new(x, y, width, height)))
        }
        PanelEdge::Right => {
            let limit = anchor.x.min(work_area.right());
            let width = intrinsic.width.min(fit(limit - work_area.x - gap)?);
            let height = intrinsic.height.min(area_h);
            let x = limit - gap - width;
            let y = clamp_start(anchor.y, height, work_area.y, work_area.bottom());
            Ok(placed(Rect::new(x, y, width, height)))
        }
    }
}

fn clamp_start(pos: i32, len: i32, area_pos: i32, area_end: i32) -> i32 {
    let max_pos = area_end - len;
    if max_pos <= area_pos {
        area_pos
    } else {
        pos.clamp(area_pos, max_pos)
    }
}

/// Bottom-pinned Start action row footprint: `.start-search` height (45px)
/// plus its 6px bottom margin (`ui/shell/flamewm.css`:
/// `.start-search { height:45px; margin:0px 6px 6px 6px; }`). That row is the
/// last child of `.start-left-pane` (`justify-content:flex-end`), so it always
/// sits on the popup's bottom edge; a capped Start surface keeps at least this
/// extent whenever the work area allows it, so the row stays reachable.
pub const START_ACTION_ROW_HEIGHT: i32 = 51;

/// Bounded small-work-area fallback for [`fitted_start_placement`] (A12-2).
///
/// The fitted engine refuses (`PendingLayout`) when the work area leaves no
/// room for the `gap` between the popup and the panel — which a 320x240 or
/// 520x420 work area reaches at some anchors. Refusing there leaves Start
/// unmapped, so this fallback never fails:
/// - clamps the popup inside `work_area`, which already excludes the panel
///   strip, so the surface always stays on its own output;
/// - keeps the panel-adjacent edge `gap` away from the panel whenever that
///   fits (the bottom-pinned action row then sits just above the panel);
/// - floors the opening extent at [`START_ACTION_ROW_HEIGHT`] (never more than
///   the work area) so the action row cannot be squeezed out entirely.
#[must_use]
pub fn bounded_start_placement(
    anchor: Rect,
    intrinsic: Size,
    panel_edge: PanelEdge,
    work_area: Rect,
    gap: i32,
) -> StartPlacement {
    let area = Rect::new(
        work_area.x,
        work_area.y,
        work_area.width.max(0),
        work_area.height.max(0),
    );
    let gap = gap.max(0);
    let wanted_w = intrinsic.width.max(1);
    let wanted_h = intrinsic.height.max(1);
    // Opening-axis floor: the action row, capped by the work area itself.
    let floor_h = START_ACTION_ROW_HEIGHT.min(area.height).max(1);
    let fit = |available: i32, wanted: i32| wanted.min(available.max(0)).max(1);
    let (x, y, width, height) = match panel_edge {
        PanelEdge::Bottom => {
            let limit = anchor.y.clamp(area.y, area.bottom());
            let available = (limit - area.y - gap).max(0);
            let height = fit(available, wanted_h)
                .max(floor_h)
                .min(area.height.max(1));
            let width = wanted_w.min(area.width.max(1));
            let y = (limit - gap - height).clamp(area.y, (area.bottom() - height).max(area.y));
            (
                clamp_start(anchor.x, width, area.x, area.right()),
                y,
                width,
                height,
            )
        }
        PanelEdge::Top => {
            let limit = anchor.bottom().clamp(area.y, area.bottom());
            let available = (area.bottom() - limit - gap).max(0);
            let height = fit(available, wanted_h)
                .max(floor_h)
                .min(area.height.max(1));
            let width = wanted_w.min(area.width.max(1));
            let y = (limit + gap).clamp(area.y, (area.bottom() - height).max(area.y));
            (
                clamp_start(anchor.x, width, area.x, area.right()),
                y,
                width,
                height,
            )
        }
        PanelEdge::Left => {
            let limit = anchor.right().clamp(area.x, area.right());
            let available = (area.right() - limit - gap).max(0);
            let width = fit(available, wanted_w).min(area.width.max(1));
            let height = wanted_h.min(area.height.max(1));
            let x = (limit + gap).clamp(area.x, (area.right() - width).max(area.x));
            (
                x,
                clamp_start(anchor.y, height, area.y, area.bottom()),
                width,
                height,
            )
        }
        PanelEdge::Right => {
            let limit = anchor.x.clamp(area.x, area.right());
            let available = (limit - area.x - gap).max(0);
            let width = fit(available, wanted_w).min(area.width.max(1));
            let height = wanted_h.min(area.height.max(1));
            let x = (limit - gap - width).clamp(area.x, (area.right() - width).max(area.x));
            (
                x,
                clamp_start(anchor.y, height, area.y, area.bottom()),
                width,
                height,
            )
        }
    };
    let rect = Rect::new(x, y, width, height);
    StartPlacement {
        rect,
        content_was_capped: rect.width != intrinsic.width || rect.height != intrinsic.height,
    }
}

#[cfg(test)]
mod work_area_tests {
    use super::*;

    #[test]
    fn bottom_excludes_panel_strip() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(0, 1036, 1920, 44);
        assert_eq!(
            work_area_for_panel(output, panel, PanelEdge::Bottom),
            Rect::new(0, 0, 1920, 1036)
        );
    }

    #[test]
    fn top_excludes_panel_strip() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(0, 0, 1920, 44);
        assert_eq!(
            work_area_for_panel(output, panel, PanelEdge::Top),
            Rect::new(0, 44, 1920, 1036)
        );
    }

    #[test]
    fn left_and_right_exclude_panel_strip() {
        let output = Rect::new(0, 0, 1920, 1080);
        assert_eq!(
            work_area_for_panel(output, Rect::new(0, 0, 44, 1080), PanelEdge::Left),
            Rect::new(44, 0, 1876, 1080)
        );
        assert_eq!(
            work_area_for_panel(output, Rect::new(1876, 0, 44, 1080), PanelEdge::Right),
            Rect::new(0, 0, 1876, 1080)
        );
    }

    #[test]
    fn negative_origins_preserved() {
        let output = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(
            work_area_for_panel(output, Rect::new(-1920, 1036, 1920, 44), PanelEdge::Bottom),
            Rect::new(-1920, 0, 1920, 1036)
        );
        assert_eq!(
            work_area_for_panel(output, Rect::new(-44, 0, 44, 1080), PanelEdge::Right),
            Rect::new(-1920, 0, 1876, 1080)
        );
    }

    #[test]
    fn hidpi_thickness_already_in_panel_rect() {
        // 200% scale: 44 logical -> 88 physical; caller passes the
        // already-scaled panel rect, so no scale math happens here.
        let output = Rect::new(0, 0, 3840, 2160);
        let panel = Rect::new(0, 2072, 3840, 88);
        assert_eq!(
            work_area_for_panel(output, panel, PanelEdge::Bottom),
            Rect::new(0, 0, 3840, 2072)
        );
    }

    #[test]
    fn oversized_panel_clamps_to_zero() {
        let output = Rect::new(0, 0, 100, 100);
        let area = work_area_for_panel(output, Rect::new(0, -50, 100, 200), PanelEdge::Bottom);
        assert!(area.height >= 0 && area.width >= 0);
    }
}
#[cfg(test)]
mod measured_tests {
    use super::*;
    use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};

    fn work_area(width: i32, height: i32) -> Rect {
        Rect::new(0, 0, width, height)
    }

    fn bottom_panel_anchor(area: Rect) -> Rect {
        Rect::new(area.right() - 100, area.bottom() - 44, 30, 44)
    }

    #[test]
    fn status_popover_right_aligned_above_panel_all_resolutions() {
        for (width, height) in [(1350, 641), (1920, 1080), (2560, 1440)] {
            let area = work_area(width, height);
            let anchor = bottom_panel_anchor(area);
            let size = Size::new(310, 200);
            let rect = measured_popup_rect(
                Some(anchor),
                Some(size),
                PopoverEdge::Above,
                PopoverAlign::End,
                area,
                8,
            )
            .expect("measured");
            assert_eq!((rect.width, rect.height), (310, 200), "{width}x{height}");
            assert_eq!(
                rect.right(),
                anchor.right().min(area.right()),
                "{width}x{height}"
            );
            assert!(
                rect.x >= area.x && rect.right() <= area.right(),
                "{width}x{height}"
            );
            assert!(
                rect.y >= area.y && rect.bottom() <= area.bottom(),
                "{width}x{height}"
            );
            assert!(rect.bottom() <= anchor.y, "{width}x{height}");
        }
    }

    #[test]
    fn start_root_anchored_left_above_panel_all_resolutions() {
        for (width, height) in [(1350, 641), (1920, 1080), (2560, 1440)] {
            let area = work_area(width, height);
            let anchor = Rect::new(area.x, area.bottom() - 44, 43, 44);
            let size = Size::new(292, 380);
            let rect = measured_popup_rect(
                Some(anchor),
                Some(size),
                PopoverEdge::Above,
                PopoverAlign::Start,
                area,
                8,
            )
            .expect("measured");
            assert_eq!((rect.x, rect.width), (anchor.x, 292), "{width}x{height}");
            assert!(rect.bottom() <= anchor.y, "{width}x{height}");
            assert!(rect.y >= area.y, "{width}x{height}");
        }
    }

    #[test]
    fn measured_refuses_without_layout_or_anchor() {
        let area = work_area(1920, 1080);
        assert_eq!(
            measured_popup_rect(
                None,
                Some(Size::new(10, 10)),
                PopoverEdge::Above,
                PopoverAlign::End,
                area,
                8
            ),
            Err(PopupRefusal::PendingAnchor)
        );
        assert_eq!(
            measured_popup_rect(
                Some(Rect::new(0, 0, 10, 10)),
                None,
                PopoverEdge::Above,
                PopoverAlign::End,
                area,
                8
            ),
            Err(PopupRefusal::PendingLayout)
        );
    }
}

#[cfg(test)]
mod fitted_start_tests {
    use super::*;

    #[test]
    fn bottom_normal() {
        let area = Rect::new(0, 0, 1920, 1080);
        let anchor = Rect::new(10, 1036, 43, 44);
        let p = fitted_start_placement(anchor, Size::new(292, 380), PanelEdge::Bottom, area, 8)
            .expect("placed");
        assert!(!p.content_was_capped);
        assert_eq!((p.rect.width, p.rect.height), (292, 380));
        assert_eq!(p.rect.bottom(), anchor.y - 8);
    }

    #[test]
    fn bottom_too_tall_caps() {
        let area = Rect::new(0, 0, 1920, 1080);
        let anchor = Rect::new(10, 300, 43, 44);
        let p = fitted_start_placement(anchor, Size::new(292, 600), PanelEdge::Bottom, area, 8)
            .expect("placed");
        assert!(p.content_was_capped);
        assert_eq!(p.rect.height, 300 - 8);
        assert_eq!(p.rect.bottom(), anchor.y - 8);
    }

    #[test]
    fn top_normal_and_too_tall() {
        let area = Rect::new(0, 0, 1920, 1080);
        let anchor = Rect::new(10, 0, 43, 44);
        let p = fitted_start_placement(anchor, Size::new(292, 380), PanelEdge::Top, area, 8)
            .expect("placed");
        assert!(!p.content_was_capped);
        assert_eq!(p.rect.y, anchor.bottom() + 8);
        assert_eq!((p.rect.width, p.rect.height), (292, 380));
        let p2 = fitted_start_placement(anchor, Size::new(292, 2000), PanelEdge::Top, area, 8)
            .expect("placed");
        assert!(p2.content_was_capped);
        assert_eq!(p2.rect.height, area.bottom() - anchor.bottom() - 8);
        assert_eq!(p2.rect.y, anchor.bottom() + 8);
    }

    #[test]
    fn left_and_right() {
        let area = Rect::new(0, 0, 1920, 1080);
        let anchor = Rect::new(0, 100, 44, 43);
        let p = fitted_start_placement(anchor, Size::new(200, 150), PanelEdge::Left, area, 8)
            .expect("placed");
        assert_eq!(p.rect.x, anchor.right() + 8);
        assert_eq!((p.rect.width, p.rect.height), (200, 150));
        let anchor_r = Rect::new(1876, 100, 44, 43);
        let q = fitted_start_placement(anchor_r, Size::new(200, 150), PanelEdge::Right, area, 8)
            .expect("placed");
        assert_eq!(q.rect.right(), anchor_r.x - 8);
        assert_eq!((q.rect.width, q.rect.height), (200, 150));
    }

    #[test]
    fn horizontal_clamp_preserves_adjacency() {
        let area = Rect::new(0, 0, 1920, 1080);
        let anchor = Rect::new(1900, 1036, 20, 44);
        let p = fitted_start_placement(anchor, Size::new(400, 200), PanelEdge::Bottom, area, 8)
            .expect("placed");
        assert_eq!(p.rect.bottom(), anchor.y - 8);
        assert_eq!(p.rect.right(), area.right());
        assert_eq!(p.rect.height, 200);
    }

    #[test]
    fn zero_extent_refusal() {
        let area = Rect::new(0, 0, 1920, 1080);
        assert!(
            fitted_start_placement(
                Rect::new(10, 8, 43, 44),
                Size::new(292, 380),
                PanelEdge::Bottom,
                area,
                8
            )
            .is_err()
        );
        assert!(
            fitted_start_placement(
                Rect::new(10, 1036, 43, 44),
                Size::new(0, 380),
                PanelEdge::Bottom,
                area,
                8
            )
            .is_err()
        );
        assert!(
            fitted_start_placement(
                Rect::new(10, 1036, 43, 44),
                Size::new(292, 0),
                PanelEdge::Bottom,
                area,
                8
            )
            .is_err()
        );
    }

    #[test]
    fn nonzero_origin() {
        let area = Rect::new(-1920, 0, 1920, 1080);
        let anchor = Rect::new(-1900, 1036, 43, 44);
        let p = fitted_start_placement(anchor, Size::new(292, 380), PanelEdge::Bottom, area, 8)
            .expect("placed");
        assert_eq!(p.rect.bottom(), anchor.y - 8);
        assert!(p.rect.x >= area.x && p.rect.right() <= area.right());
    }

    #[test]
    fn all_resolutions() {
        for (w, h) in [(1350, 641), (1920, 1080), (2560, 1440)] {
            let area = Rect::new(0, 0, w, h);
            let anchor = Rect::new(area.x, area.bottom() - 44, 43, 44);
            let p = fitted_start_placement(anchor, Size::new(292, 380), PanelEdge::Bottom, area, 8)
                .expect("placed");
            assert_eq!(p.rect.bottom(), anchor.y - 8, "{w}x{h}");
            assert!(p.rect.y >= area.y, "{w}x{h}");
            let anchor_t = Rect::new(area.x, area.y, 43, 44);
            let q = fitted_start_placement(anchor_t, Size::new(292, 380), PanelEdge::Top, area, 8)
                .expect("placed");
            assert_eq!(q.rect.y, anchor_t.bottom() + 8, "{w}x{h}");
        }
    }
}

/// A12-2: bounded small-work-area fallback. Both 320x240 and 520x420 work
/// areas are smaller than the real Start intrinsic (550x420), so the fitted
/// engine caps — or, with no room for the gap, refuses. The fallback must
/// still produce an on-output rect inside the work area whose bottom-pinned
/// action row is reachable.
#[cfg(test)]
mod bounded_small_work_area_tests {
    use super::*;

    const SMALL_WORK_AREAS: [(i32, i32); 2] = [(320, 240), (520, 420)];

    fn assert_inside_work_area(rect: Rect, area: Rect) {
        assert!(rect.width > 0 && rect.height > 0, "{rect:?}");
        assert!(
            rect.x >= area.x && rect.right() <= area.right(),
            "{rect:?} must stay on output {area:?}"
        );
        assert!(
            rect.y >= area.y && rect.bottom() <= area.bottom(),
            "{rect:?} must stay inside the work area {area:?}"
        );
    }

    #[test]
    fn fitted_engine_caps_but_stays_on_output_and_keeps_the_action_row() {
        for (w, h) in SMALL_WORK_AREAS {
            let area = Rect::new(0, 0, w, h);
            // Start button on a bottom panel: its top edge is the work-area
            // bottom, so nothing is subtracted from the opening space except
            // the gap.
            let anchor = Rect::new(0, h, 43, 44);
            let placement =
                fitted_start_placement(anchor, Size::new(550, 420), PanelEdge::Bottom, area, 8)
                    .expect("small work area must still place");
            assert!(placement.content_was_capped, "{w}x{h}");
            assert_inside_work_area(placement.rect, area);
            // Bottom-pinned: the row sits `gap` above the panel, reachable.
            assert_eq!(placement.rect.bottom(), h - 8, "{w}x{h}");
            assert!(
                placement.rect.height >= START_ACTION_ROW_HEIGHT.min(h),
                "{w}x{h}: {} cannot hold the action row",
                placement.rect.height
            );
            assert!(placement.rect.right() <= area.right(), "{w}x{h}");
        }
    }

    #[test]
    fn bounded_fallback_survives_a_work_area_with_no_room_for_the_gap() {
        for (w, h) in SMALL_WORK_AREAS {
            let area = Rect::new(0, 0, w, h);
            // Hostile anchor: the panel edge is the work-area top, so the
            // fitted engine has zero opening space and refuses.
            let anchor = Rect::new(0, area.y, 43, 44);
            assert!(
                fitted_start_placement(anchor, Size::new(550, 420), PanelEdge::Bottom, area, 8)
                    .is_err(),
                "{w}x{h}: fitted engine refuses without gap room"
            );
            let bounded =
                bounded_start_placement(anchor, Size::new(550, 420), PanelEdge::Bottom, area, 8);
            assert!(bounded.content_was_capped, "{w}x{h}");
            assert_inside_work_area(bounded.rect, area);
            // Bounded floor: never a degenerate sliver, never taller than the
            // work area, and the action row still fits.
            assert!(
                bounded.rect.height >= START_ACTION_ROW_HEIGHT.min(h),
                "{w}x{h}: {} cannot hold the action row",
                bounded.rect.height
            );
            assert!(bounded.rect.height <= h, "{w}x{h}");
            assert_eq!(bounded.rect.width, w.min(550), "{w}x{h}");
        }
    }

    #[test]
    fn bounded_fallback_matches_the_fitted_rect_when_there_is_room() {
        for (w, h) in SMALL_WORK_AREAS {
            let area = Rect::new(0, 0, w, h);
            let anchor = Rect::new(0, h, 43, 44);
            let fitted =
                fitted_start_placement(anchor, Size::new(550, 420), PanelEdge::Bottom, area, 8)
                    .expect("placed");
            let bounded =
                bounded_start_placement(anchor, Size::new(550, 420), PanelEdge::Bottom, area, 8);
            assert_eq!(fitted, bounded, "{w}x{h}");
        }
    }

    #[test]
    fn bounded_fallback_stays_on_negative_origin_outputs() {
        let area = Rect::new(-1920, 0, 320, 240);
        let anchor = Rect::new(-1920, 0, 43, 44);
        let bounded =
            bounded_start_placement(anchor, Size::new(550, 420), PanelEdge::Bottom, area, 8);
        assert_inside_work_area(bounded.rect, area);
        assert!(bounded.rect.height >= START_ACTION_ROW_HEIGHT.min(area.height));
    }
}

#[cfg(test)]
mod panel_popup_edge_tests {
    use super::*;

    #[test]
    fn panel_popups_open_opposite_each_panel_edge_and_stay_on_output() {
        let output_rect = Rect::new(0, 0, 1000, 800);
        let output = OutputSnapshot {
            id: OutputId::new("test-output"),
            connector: "test".to_owned(),
            edid_identity: "test".to_owned(),
            connected: true,
            primary: true,
            geometry: output_rect,
            current_mode: flamewm_api::ModeId(1),
            modes: Vec::new(),
            shell_scale_percent: 100,
        };
        let cases = [
            (
                PanelEdge::Bottom,
                Rect::new(0, 400, 1000, 44),
                PopoverEdge::Above,
            ),
            (
                PanelEdge::Top,
                Rect::new(0, 300, 1000, 44),
                PopoverEdge::Below,
            ),
            (
                PanelEdge::Left,
                Rect::new(400, 0, 44, 800),
                PopoverEdge::Right,
            ),
            (
                PanelEdge::Right,
                Rect::new(500, 0, 44, 800),
                PopoverEdge::Left,
            ),
        ];

        for (edge, geometry, expected_edge) in cases {
            let panel = PanelSnapshot {
                output: output.id.clone(),
                edge,
                logical_size: 44,
                visible: true,
                geometry,
            };
            let rect = panel_popup_rect(
                &panel,
                &output,
                ShellMetrics::default(),
                Size::new(180, 120),
                3,
            );

            assert!(rect.x >= output_rect.x, "{edge:?}: {rect:?}");
            assert!(rect.y >= output_rect.y, "{edge:?}: {rect:?}");
            assert!(rect.right() <= output_rect.right(), "{edge:?}: {rect:?}");
            assert!(rect.bottom() <= output_rect.bottom(), "{edge:?}: {rect:?}");
            match expected_edge {
                PopoverEdge::Above => assert!(rect.bottom() <= geometry.y, "{edge:?}: {rect:?}"),
                PopoverEdge::Below => {
                    assert!(rect.y >= geometry.bottom(), "{edge:?}: {rect:?}");
                }
                PopoverEdge::Right => {
                    assert!(rect.x >= geometry.right(), "{edge:?}: {rect:?}");
                }
                PopoverEdge::Left => {
                    assert!(rect.right() <= geometry.x, "{edge:?}: {rect:?}");
                }
            }
        }
    }
}

#[cfg(test)]
mod start_panel_inset_tests {
    use super::*;

    // The task-start node sits inside the panel (inset below the outer top),
    // so the node top is NOT the panel top. Start must still stop `gap` above
    // the live panel outer top (= work-area edge), not `gap` above the node.
    #[test]
    fn bottom_start_bottom_is_gap_above_panel_top_when_node_is_inset() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(0, 1036, 1920, 44);
        let work = work_area_for_panel(output, panel, PanelEdge::Bottom);
        let node = Rect::new(1800, 1042, 30, 32);
        let p = fitted_start_placement(node, Size::new(400, 600), PanelEdge::Bottom, work, 8)
            .expect("placed");
        assert_eq!(p.rect.bottom(), panel.y - 8, "{:?}", p.rect);
    }

    // Floating margin is already excluded from the work area; it must not be
    // subtracted a second time.
    #[test]
    fn bottom_start_tracks_floating_panel_without_double_margin() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(8, 1028, 1904, 44);
        let work = work_area_for_panel(output, panel, PanelEdge::Bottom);
        let node = Rect::new(1800, 1034, 30, 32);
        let p = fitted_start_placement(node, Size::new(400, 600), PanelEdge::Bottom, work, 8)
            .expect("placed");
        assert_eq!(p.rect.bottom(), panel.y - 8, "{:?}", p.rect);
    }

    #[test]
    fn top_start_top_is_gap_below_panel_bottom_when_node_is_inset() {
        let output = Rect::new(0, 0, 1920, 1080);
        let panel = Rect::new(0, 0, 1920, 44);
        let work = work_area_for_panel(output, panel, PanelEdge::Top);
        let node = Rect::new(1800, 6, 30, 32);
        let p = fitted_start_placement(node, Size::new(400, 600), PanelEdge::Top, work, 8)
            .expect("placed");
        assert_eq!(p.rect.y, panel.bottom() + 8, "{:?}", p.rect);
    }
}
