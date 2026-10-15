
use super::*;

/// Single-present stage order for first show: project -> measure ->
/// anchor -> prepare -> paint -> shape -> commit -> map+raise ->
/// present -> grab -> flush. Each stage runs at most once per cycle;
/// the cycle owns show/raise/present/grab with no feature redraw.
pub const PRESENT_STAGE_ORDER: [&str; 10] = [
    "project",
    "measure",
    "anchor",
    "prepare",
    "paint",
    "shape",
    "commit",
    "map_raise",
    "present",
    "grab_flush",
];

/// Pure single-cycle recorder for the present operation. X calls stay in
/// the controller methods; this enforces one ordered pass per surface.
#[derive(Debug, Default)]
pub struct PresentCycle {
    done: [bool; 10],
}

impl PresentCycle {
    pub fn new() -> Self {
        Self { done: [false; 10] }
    }

    /// Advance exactly one stage in order. Rejects skips, repeats, and
    /// unknown stage names so a frame can never double-present.
    pub fn advance(&mut self, stage: &str) -> Result<(), String> {
        let index = PRESENT_STAGE_ORDER
            .iter()
            .position(|known| *known == stage)
            .ok_or_else(|| format!("unknown present stage {stage}"))?;
        if self.done[index] {
            return Err(format!("present stage {stage} already ran this cycle"));
        }
        if (0..index).any(|earlier| !self.done[earlier]) {
            return Err(format!("present stage {stage} ran out of order"));
        }
        self.done[index] = true;
        Ok(())
    }

    pub fn run_full_cycle(&mut self) -> Result<(), String> {
        for stage in PRESENT_STAGE_ORDER {
            self.advance(stage)?;
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.done.iter().all(|done| *done)
    }
}

impl SurfaceController {
    pub fn redraw(&mut self, id: SurfaceId) -> Result<(), String> {
        let _guard = flamewm_profiler::start("render.present");
        let instance = self.instance_mut(id)?;
        unsafe {
            instance.app.redraw(&instance.document).map_err(|error| {
                mark_x_error_seen();
                error
            })
        }
    }

    pub fn redraw_dirty(&mut self) -> Result<(), String> {
        let _guard = flamewm_profiler::start("render.present");
        let ids: Vec<SurfaceId> = self.surfaces.keys().copied().collect();
        for id in ids {
            let instance = self.instance_mut(id)?;
            // SAFETY: instance owns a live display + app resources.
            let painted = unsafe {
                instance
                    .app
                    .redraw_if_dirty(&mut instance.document)
                    .map_err(|error| {
                        mark_x_error_seen();
                        error
                    })?
            };
            if painted {
                let instance = self.instance_mut(id)?;
                // SAFETY: display/window live; mask follows repaint.
                unsafe { instance.app.refresh_shape_mask(&instance.document) };
            }
        }
        Ok(())
    }

}
