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
/// already-scaled `panel_rect` (HiDPI baked in). Clamps to non-negative
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
#[must_use]
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

#[must_use]
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
    match panel_edge {
        PanelEdge::Bottom => {
            let available = anchor.y - work_area.y - gap;
            if available <= 0 {
                return Err(PopupRefusal::PendingLayout);
            }
            let height = intrinsic.height.min(available);
            let width = intrinsic.width.min(area_w);
            let y = anchor.y - gap - height;
            let x = clamp_start(anchor.x, width, work_area.x, work_area.right());
            Ok(StartPlacement {
                rect: Rect::new(x, y, width, height),
                content_was_capped: width != intrinsic.width || height != intrinsic.height,
            })
        }
        PanelEdge::Top => {
            let available = work_area.bottom() - anchor.bottom() - gap;
            if available <= 0 {
                return Err(PopupRefusal::PendingLayout);
            }
            let height = intrinsic.height.min(available);
            let width = intrinsic.width.min(area_w);
            let y = anchor.bottom() + gap;
            let x = clamp_start(anchor.x, width, work_area.x, work_area.right());
            Ok(StartPlacement {
                rect: Rect::new(x, y, width, height),
                content_was_capped: width != intrinsic.width || height != intrinsic.height,
            })
        }
        PanelEdge::Left => {
            let available = work_area.right() - anchor.right() - gap;
            if available <= 0 {
                return Err(PopupRefusal::PendingLayout);
            }
            let width = intrinsic.width.min(available);
            let height = intrinsic.height.min(area_h);
            let x = anchor.right() + gap;
            let y = clamp_start(anchor.y, height, work_area.y, work_area.bottom());
            Ok(StartPlacement {
                rect: Rect::new(x, y, width, height),
                content_was_capped: width != intrinsic.width || height != intrinsic.height,
            })
        }
        PanelEdge::Right => {
            let available = anchor.x - work_area.x - gap;
            if available <= 0 {
                return Err(PopupRefusal::PendingLayout);
            }
            let width = intrinsic.width.min(available);
            let height = intrinsic.height.min(area_h);
            let x = anchor.x - gap - width;
            let y = clamp_start(anchor.y, height, work_area.y, work_area.bottom());
            Ok(StartPlacement {
                rect: Rect::new(x, y, width, height),
                content_was_capped: width != intrinsic.width || height != intrinsic.height,
            })
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
/// Context-menu placement: Below/Start preferred, Above/Start opposite.
/// Caps height to the work area and reports scrolling when capped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextMenuPlacement {
    pub rect: Rect,
    pub requires_scroll: bool,
}

#[must_use]
pub fn context_menu_placement(
    anchor: Rect,
    size: Size,
    work_area: Rect,
    offset: i32,
) -> ContextMenuPlacement {
    let width = size.width.max(0).min(work_area.width.max(0));
    let below = PopoverGeometry::place(
        anchor,
        Size::new(width, size.height.max(0)),
        PopoverEdge::Below,
        work_area,
        offset,
    );
    let above = PopoverGeometry::place(
        anchor,
        Size::new(width, size.height.max(0)),
        PopoverEdge::Above,
        work_area,
        offset,
    );
    // Preferred: Below/Start. Opposite: Above/Start. Start (x) is clamped
    // to the work area in both cases.
    let below_fits_height =
        size.height.max(0) <= work_area.height.max(0) && below.edge == PopoverEdge::Below;
    let above_fits_height =
        size.height.max(0) <= work_area.height.max(0) && above.edge == PopoverEdge::Above;
    let geometry = if below_fits_height {
        below
    } else if above_fits_height {
        above
    } else {
        below
    };
    let available = work_area.height.max(0);
    let requires_scroll = size.height.max(0) > available;
    let height = size.height.max(0).min(available);
    let rect = Rect::from_parts(geometry.origin, Size::new(width, height)).clamp_inside(work_area);
    ContextMenuPlacement {
        rect,
        requires_scroll,
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
                    assert!(rect.y >= geometry.bottom(), "{edge:?}: {rect:?}")
                }
                PopoverEdge::Right => {
                    assert!(rect.x >= geometry.right(), "{edge:?}: {rect:?}")
                }
                PopoverEdge::Left => {
                    assert!(rect.right() <= geometry.x, "{edge:?}: {rect:?}")
                }
            }
        }
    }
}
