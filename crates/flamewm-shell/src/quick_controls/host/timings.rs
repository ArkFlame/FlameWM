use super::*;

/// True open stages: each label maps to one real turn in `open_request`
/// (project, measure, place, `prepare/commit_geometry`, present,
/// `pointer_grab`) plus the total and the real close
/// (close/ungrab/unmap). No fake spans.
pub const QUICK_STAGE_LABELS: &[&str] = &[
    "shell.quick.open.total",
    "shell.quick.project",
    "shell.quick.measure",
    "shell.quick.place",
    "shell.quick.prepare/commit_geometry",
    "shell.quick.present",
    "shell.quick.pointer_grab",
    "shell.quick.close/ungrab/unmap",
];

/// Slow-open threshold: one 60Hz frame (16.67ms).
pub const SLOW_OPEN_THRESHOLD: Duration = Duration::from_micros(16_670);

/// Pure seam: slow-open predicate shared by the log gate.
#[must_use]
pub fn is_slow_open(total: Duration) -> bool {
    total >= SLOW_OPEN_THRESHOLD
}

/// Pure seam: stage timings carried by one open turn, rendered by the
/// slow-open debug line. close is the pre-open close of the previous
/// popup on the same tick (zero when nothing was open).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenStageTimings {
    pub total: Duration,
    pub project: Duration,
    pub measure: Duration,
    pub place: Duration,
    pub prepare_commit: Duration,
    pub present: Duration,
    pub pointer_grab: Duration,
    pub close: Duration,
}

impl OpenStageTimings {
    #[must_use]
    pub fn render(&self, kind: QuickControlKind) -> String {
        format!(
            "flamewm-quick-control: slow open kind={kind:?} open.total={:?} project={:?} measure={:?} place={:?} prepare/commit_geometry={:?} present={:?} pointer_grab={:?} close/ungrab/unmap={:?}",
            self.total,
            self.project,
            self.measure,
            self.place,
            self.prepare_commit,
            self.present,
            self.pointer_grab,
            self.close,
        )
    }
}

/// Fitted placement plus the real measure/place stage split, so the
/// slow-open line can attribute each turn without fake spans.
pub struct FittedPlacement {
    pub rect: flamewm_api::Rect,
    pub measure_elapsed: Duration,
    pub place_elapsed: Duration,
}

pub(crate) fn fitted_rect(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    request: &OpenRequest,
) -> Result<(flamewm_api::Rect, Duration, Duration), PopupError> {
    let measure_start = std::time::Instant::now();
    let _measure = crate::runtime::shell_span("shell.quick.measure").start();
    let intrinsic = measure_helper(runtime, surface, request)?;
    let measure_elapsed = measure_start.elapsed();
    drop(_measure);
    let place_start = std::time::Instant::now();
    let _place = crate::runtime::shell_span("shell.quick.place").start();
    let rect = fit_helper(request, intrinsic)?;
    let place_elapsed = place_start.elapsed();
    drop(_place);
    Ok((rect, measure_elapsed, place_elapsed))
}
