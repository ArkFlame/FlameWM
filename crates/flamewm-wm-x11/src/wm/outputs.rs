//! Output and work-area bookkeeping for the Wm.
use super::*;
use flamewm_window_core::dock_reserve::{ReserveEdge, reserve_edge};

/// Cuts the Dock from a work area. The taskbar strut is already out of `work_area`, so a Dock on
/// the same edge stacks beyond it.
pub(super) fn with_dock_reserve(work_area: Rect, dock: Option<(ReserveEdge, i32)>) -> Rect {
    match dock {
        Some((edge, extent)) => from_core_rect(reserve_edge(to_core_rect(work_area), edge, extent)),
        None => work_area,
    }
}

/// The part of the Dock reserve that a strut-derived work area does not already cover. A Dock
/// window touching its root edge publishes that edge as `_NET_WM_STRUT`, and `strut` (left, top,
/// right, top, bottom: EWMH order, as `work_area` reads it) already holds it, so reserving it again
/// would count the Dock twice.
pub(super) fn dock_reserve_uncovered_by_struts(
    strut: [u32; 4],
    dock: Option<(ReserveEdge, i32)>,
) -> Option<(ReserveEdge, i32)> {
    let (edge, extent) = dock?;
    let index = match edge {
        ReserveEdge::Left => 0,
        ReserveEdge::Right => 1,
        ReserveEdge::Top => 2,
        ReserveEdge::Bottom => 3,
    };
    (i64::from(strut[index]) < i64::from(extent)).then_some((edge, extent))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputWorkArea {
    pub(super) output: flamewm_api::OutputId,
    pub(super) geometry: Rect,
    pub(super) work_area: Rect,
    pub(super) primary: bool,
}

impl OutputWorkArea {
    pub(super) fn from_host(
        (output, geometry, work_area, primary): (
            flamewm_api::OutputId,
            flamewm_api::Rect,
            flamewm_api::Rect,
            bool,
        ),
    ) -> Self {
        Self {
            output,
            geometry: from_core_rect(geometry),
            work_area: from_core_rect(work_area),
            primary,
        }
    }
}

/// `RandR` topology-change flag. Set by notifications; consumed once per runtime turn.
#[derive(Debug, Default)]
pub(crate) struct TopologyDirty(bool);

impl TopologyDirty {
    pub(super) fn mark(&mut self) {
        self.0 = true;
    }

    pub(super) fn take(&mut self) -> bool {
        std::mem::take(&mut self.0)
    }

    pub(super) fn is_pending(&self) -> bool {
        self.0
    }
}

/// Work area of the output holding `rect`, else the global work area.
pub(super) fn work_area_for_rect_in(outputs: &[OutputWorkArea], global: Rect, rect: Rect) -> Rect {
    select_output_for_rect(outputs, rect).map_or(global, |index| outputs[index].work_area)
}

pub(super) fn select_output_for_rect(outputs: &[OutputWorkArea], rect: Rect) -> Option<usize> {
    let center_x = rect
        .x
        .saturating_add(i32::try_from(rect.width / 2).unwrap_or(i32::MAX));
    let center_y = rect
        .y
        .saturating_add(i32::try_from(rect.height / 2).unwrap_or(i32::MAX));
    if let Some(index) = outputs.iter().position(|output| {
        center_x >= output.geometry.x
            && center_x < output.geometry.right()
            && center_y >= output.geometry.y
            && center_y < output.geometry.bottom()
    }) {
        return Some(index);
    }
    outputs
        .iter()
        .enumerate()
        .map(|(index, output)| (index, rect_overlap_area(rect, output.geometry)))
        .filter(|&(_, area)| area > 0)
        .max_by_key(|&(index, area)| (area, std::cmp::Reverse(index)))
        .map(|(index, _)| index)
        .or_else(|| outputs.iter().position(|output| output.primary))
}

impl<C: Connection> Wm<'_, C> {
    pub(super) fn screen(&self) -> &Screen {
        &self.conn.setup().roots[self.screen_num]
    }

    pub(super) fn work_area(&self) -> Rect {
        let (height, width) = (self.screen_rect.height, self.screen_rect.width);
        // (extent, start, end, limit) per edge: left, top, right, bottom.
        let edges = [
            (0, 4, 5, height),
            (1, 6, 7, height),
            (2, 8, 9, width),
            (3, 10, 11, width),
        ];
        let mut strut = self
            .dock_struts
            .values()
            .fold([0_u32; 4], |mut strut, dock| {
                for (edge, (extent, start, end, limit)) in edges.into_iter().enumerate() {
                    if dock[extent] > 0 && dock[end] >= dock[start] && dock[start] < limit {
                        strut[edge] = strut[edge].max(dock[extent]);
                    }
                }
                strut
            });
        if strut == [0; 4] {
            strut[3] = u32::from(self.config.panel_reserve);
        }
        let left = strut[0].min(self.screen_rect.width.saturating_sub(1));
        let right = strut[1].min(self.screen_rect.width.saturating_sub(left + 1));
        let top = strut[2].min(self.screen_rect.height.saturating_sub(1));
        let bottom = strut[3].min(self.screen_rect.height.saturating_sub(top + 1));
        with_dock_reserve(
            Rect::new(
                self.screen_rect
                    .x
                    .saturating_add(i32::try_from(left).unwrap_or(i32::MAX)),
                self.screen_rect
                    .y
                    .saturating_add(i32::try_from(top).unwrap_or(i32::MAX)),
                self.screen_rect.width.saturating_sub(left + right).max(1),
                self.screen_rect.height.saturating_sub(top + bottom).max(1),
            ),
            dock_reserve_uncovered_by_struts(strut, crate::live_settings::dock_reserve()),
        )
    }

    /// Consumes the `RandR` topology-change flag; true when the display snapshot must be refreshed.
    pub(crate) fn take_display_topology_dirty(&mut self) -> bool {
        self.display_topology_dirty.take()
    }

    pub(crate) fn set_output_work_areas(
        &mut self,
        areas: Vec<(
            flamewm_api::OutputId,
            flamewm_api::Rect,
            flamewm_api::Rect,
            bool,
        )>,
    ) -> Result<(), ReplyError> {
        let dock = crate::live_settings::dock_reserve();
        let next = areas
            .into_iter()
            .map(OutputWorkArea::from_host)
            .map(|mut output| {
                if output.primary {
                    output.work_area = with_dock_reserve(output.work_area, dock);
                }
                output
            })
            .collect::<Vec<_>>();
        let dock_changed = std::mem::replace(&mut self.applied_dock, dock) != dock;
        if self.output_work_areas != next || dock_changed {
            self.finish_all_transitions()?;
            self.output_work_areas = next;
            // The wallpaper behind titlebars is laid out per output, as the desktop draws it.
            let geometries = self
                .output_work_areas
                .iter()
                .map(|output| {
                    let rect = output.geometry;
                    (rect.x, rect.y, rect.width, rect.height)
                })
                .collect::<Vec<_>>();
            crate::frame::titlebar_backdrop::set_wallpaper_outputs(&geometries);
            self.publish_wallpaper_layout()?;
            self.reconcile_work_area()?;
            // `_NET_WORKAREA` is published from `work_area()`, so it must follow the Dock reserve too.
            self.publish_workspace_metadata()?;
        }
        Ok(())
    }

    pub(super) fn output_for_rect(&self, rect: Rect) -> Option<&OutputWorkArea> {
        select_output_for_rect(&self.output_work_areas, rect)
            .map(|index| &self.output_work_areas[index])
    }

    pub(super) fn work_area_for_rect(&self, rect: Rect) -> Rect {
        work_area_for_rect_in(&self.output_work_areas, self.work_area(), rect)
    }

    pub(super) fn output_for_point(
        &self,
        x: i32,
        y: i32,
        fallback: Rect,
    ) -> Option<&OutputWorkArea> {
        self.output_work_areas
            .iter()
            .find(|output| {
                x >= output.geometry.x
                    && x < output.geometry.right()
                    && y >= output.geometry.y
                    && y < output.geometry.bottom()
            })
            .or_else(|| self.output_for_rect(fallback))
    }

    /// Output geometry and its work area at a root point; falls back to the primary work area.
    pub(super) fn snap_zones_at(&self, root_x: i16, root_y: i16, fallback: Rect) -> (Rect, Rect) {
        self.output_for_point(i32::from(root_x), i32::from(root_y), fallback)
            .map_or_else(
                || (self.work_area(), self.work_area()),
                |o| (o.geometry, o.work_area),
            )
    }

    /// Snap target for a root point. Far edges use the physical output so bottom/right
    /// docks do not hide the corners from the pointer.
    pub(super) fn snap_target_at(&self, root_x: i16, root_y: i16, fallback: Rect) -> SnapTarget {
        let (output, work) = self.snap_zones_at(root_x, root_y, fallback);
        crate::live_settings::snap_target_in_output(
            flamewm_api::Point::new(i32::from(root_x), i32::from(root_y)),
            to_core_rect(output),
            to_core_rect(work),
        )
    }
}

#[cfg(test)]
mod dock_work_area_tests {
    use super::*;

    fn output() -> Rect {
        Rect::new(0, 0, 1280, 800)
    }

    #[test]
    fn dock_above_a_bottom_taskbar_stacks_on_the_remaining_area() {
        let after_taskbar = Rect::new(0, 0, 1280, 756);
        let dock = Some((ReserveEdge::Bottom, 72));
        assert_eq!(
            with_dock_reserve(after_taskbar, dock),
            Rect::new(0, 0, 1280, 684)
        );
    }

    #[test]
    fn dock_without_taskbar_cuts_only_its_edge() {
        assert_eq!(
            with_dock_reserve(output(), Some((ReserveEdge::Left, 72))),
            Rect::new(72, 0, 1208, 800)
        );
    }

    #[test]
    fn hidden_dock_leaves_the_taskbar_area_untouched() {
        let after_taskbar = Rect::new(0, 0, 1280, 756);
        assert_eq!(with_dock_reserve(after_taskbar, None), after_taskbar);
    }

    #[test]
    fn dock_strut_on_its_edge_is_not_reserved_twice() {
        // Left Dock at 72 px: its own strut already holds 72 px on the left.
        let strut = [72, 0, 0, 44];
        let dock = Some((ReserveEdge::Left, 72));
        assert_eq!(dock_reserve_uncovered_by_struts(strut, dock), None);
        assert_eq!(
            with_dock_reserve(output(), dock_reserve_uncovered_by_struts(strut, dock)),
            output()
        );
    }

    #[test]
    fn dock_strut_uses_ewmh_slots_for_top_and_right() {
        // `work_area` reads the strut as [left, right, top, bottom].
        let right = [0, 76, 0, 0];
        assert_eq!(
            dock_reserve_uncovered_by_struts(right, Some((ReserveEdge::Right, 76))),
            None
        );
        let top = [0, 0, 104, 0];
        assert_eq!(
            dock_reserve_uncovered_by_struts(top, Some((ReserveEdge::Top, 76))),
            None
        );
        assert_eq!(
            dock_reserve_uncovered_by_struts([0, 0, 0, 44], Some((ReserveEdge::Top, 76))),
            Some((ReserveEdge::Top, 76))
        );
    }

    #[test]
    fn dock_without_strut_on_its_edge_keeps_the_reserve() {
        // Bottom Dock above a bottom taskbar: the Dock does not touch the root edge.
        let strut = [0, 0, 0, 44];
        let dock = Some((ReserveEdge::Bottom, 72));
        assert_eq!(dock_reserve_uncovered_by_struts(strut, dock), dock);
        assert_eq!(dock_reserve_uncovered_by_struts(strut, None), None);
    }
}
