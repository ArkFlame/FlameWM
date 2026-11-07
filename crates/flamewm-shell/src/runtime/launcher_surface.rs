//! Placement and open/close transaction for the top-center Launcher.
//!
//! The launcher has no panel anchor. It is centred horizontally on the active
//! output's work area with its top edge 20% down, and takes its size from the
//! measured layout. Surface ownership stays in `ShellSurfaces`; this module
//! owns only the placement and the map/grab transaction.

use flamewm_api::{OutputId, Rect, Size};
use flamewm_ui_x11::SurfaceRuntime;

use super::{panel_ui_scale, ui_error, ShellSurfaces};
use crate::popup_controller;
use crate::popup_role::PopupRole;

/// Share of the work-area height above the launcher's top edge.
const TOP_PERCENT: i32 = 20;

/// Pure placement: centred, top at 20% of the work area, clamped inside it.
#[must_use]
pub fn launcher_rect(work_area: Rect, size: Size) -> Rect {
    let width = size.width.clamp(1, work_area.width.max(1));
    let height = size.height.clamp(1, work_area.height.max(1));
    let x = work_area.x + (work_area.width - width) / 2;
    let preferred = work_area.y + work_area.height * TOP_PERCENT / 100;
    let lowest = work_area.bottom() - height;
    let y = preferred.min(lowest).max(work_area.y);
    Rect::new(x, y, width, height)
}

impl ShellSurfaces {
    /// Measures, places, maps and grabs the Launcher on `output`. Refuses through
    /// the shared popup measurement when the layout is not ready, so no 0,0 frame shows.
    pub fn open_launcher_on(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &OutputId,
    ) -> Result<Rect, String> {
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("launcher output {output} is unavailable"))?;
        let work_area = panel.placement.work_area;
        let scale = panel_ui_scale(panel.placement.output.shell_scale_percent);
        let launcher = self.launcher;
        runtime
            .apply_output_scale(launcher, scale)
            .map_err(ui_error)?;
        let size = popup_controller::measure_node(
            runtime,
            launcher,
            PopupRole::Launcher.intrinsic_node_id(),
            work_area,
        )
        .map_err(|error| error.to_string())?;
        let rect = launcher_rect(work_area, size);
        let width = u32::try_from(rect.width).map_err(|_| "launcher width".to_owned())?;
        let height = u32::try_from(rect.height).map_err(|_| "launcher height".to_owned())?;
        runtime
            .move_resize(launcher, rect.x, rect.y, width, height)
            .map_err(ui_error)?;
        runtime.show(launcher).map_err(ui_error)?;
        runtime
            .grab_pointer(launcher)
            .map_err(|_| "launcher pointer grab refused".to_owned())?;
        self.launcher_output = Some(output.clone());
        Ok(rect)
    }

    /// Closes the Launcher if it is open. Releases its grab through `close_surface`.
    pub fn close_launcher(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.launcher_output.take().is_none() {
            return Ok(());
        }
        runtime.close_surface(self.launcher).map_err(ui_error)
    }

    #[must_use]
    pub fn is_launcher_open(&self) -> bool {
        self.launcher_output.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        width: 1350,
        height: 597,
    };

    #[test]
    fn launcher_is_centred_horizontally_on_the_work_area() {
        let rect = launcher_rect(WORK, Size::new(600, 360));
        assert_eq!(rect.x, (1350 - 600) / 2);
        assert_eq!(rect.width, 600);
    }

    #[test]
    fn launcher_top_sits_twenty_percent_down_the_work_area() {
        let rect = launcher_rect(WORK, Size::new(600, 360));
        assert_eq!(rect.y, 597 * 20 / 100);
    }

    #[test]
    fn launcher_offset_follows_the_output_origin() {
        let work = Rect::new(1920, 40, 1000, 500);
        let rect = launcher_rect(work, Size::new(600, 300));
        assert_eq!(rect.x, 1920 + 200);
        assert_eq!(rect.y, 40 + 100);
        assert!(rect.bottom() <= work.bottom());
    }

    #[test]
    fn launcher_is_clamped_inside_a_short_work_area() {
        let work = Rect::new(0, 0, 320, 240);
        let rect = launcher_rect(work, Size::new(600, 360));
        assert_eq!(rect.width, 320, "width clamps to the work area");
        assert_eq!(rect.height, 240, "height clamps to the work area");
        assert_eq!((rect.x, rect.y), (0, 0));
    }
}
