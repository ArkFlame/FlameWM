//! Output and work-area bookkeeping for the Wm.
use super::*;

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
        Rect::new(
            self.screen_rect
                .x
                .saturating_add(i32::try_from(left).unwrap_or(i32::MAX)),
            self.screen_rect
                .y
                .saturating_add(i32::try_from(top).unwrap_or(i32::MAX)),
            self.screen_rect.width.saturating_sub(left + right).max(1),
            self.screen_rect.height.saturating_sub(top + bottom).max(1),
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
        let next = areas
            .into_iter()
            .map(OutputWorkArea::from_host)
            .collect::<Vec<_>>();
        if self.output_work_areas != next {
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
        core_snap_target_in_output(
            flamewm_api::Point::new(i32::from(root_x), i32::from(root_y)),
            to_core_rect(output),
            to_core_rect(work),
        )
    }
}
