//! AI usage card: fetch scheduler tick and card projection.

use super::*;

impl ShellLoop {
    /// Usage tick: polls the fetch worker, starts a due fetch, and reprojects the card only
    /// when fresh snapshots arrive while the card is open. Opening the card projects it first.
    pub(super) fn tick_usage(&mut self, runtime: &mut SurfaceRuntime) {
        let waker = self.result_wake.waker();
        let changed = self.usage.tick(std::time::Instant::now(), |providers| {
            flamewm_shell::usage_host::spawn_usage_fetch(providers, waker)
        });
        if changed && self.surfaces.is_usage_open() {
            if let Err(error) = self.project_usage_card(runtime) {
                eprintln!("flamewm-shell: usage card projection failed (nonfatal): {error:?}");
            }
        }
    }

    /// Projects the current snapshots into the usage popup and redraws it.
    pub(super) fn project_usage_card(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let now = chrono::Utc::now().timestamp();
        let snapshots = &self.usage.snapshots;
        runtime.with_document(self.surfaces.usage, |document| {
            projection::project_usage(document, snapshots, now)
        })?;
        runtime.redraw(self.surfaces.usage)?;
        Ok(())
    }
}
