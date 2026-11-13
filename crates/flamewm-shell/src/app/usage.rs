//! AI usage card: settings, fetch scheduler tick and card projection.

use std::time::{Duration, Instant};

use super::*;

/// The open card is re-projected at this period so reset captions stay current (rides the wake).
pub(super) const USAGE_REPROJECT_INTERVAL: Duration = Duration::from_secs(60);

impl ShellLoop {
    /// Usage tick: applies settings, polls the fetch worker, starts a due fetch, and reprojects the
    /// open card on fresh snapshots or once a minute.
    pub(super) fn tick_usage(&mut self, runtime: &mut SurfaceRuntime) {
        let (providers, interval) =
            crate::entry::settings_projection::usage::usage_settings(self.settings_cache.as_ref());
        self.usage.configure(providers, interval);
        let waker = self.result_wake.waker();
        let now = Instant::now();
        let changed = self.usage.tick(now, |providers| {
            flamewm_shell::usage_host::spawn_usage_fetch(providers, waker)
        });
        if !self.surfaces.is_usage_open() {
            self.usage_projected_at = None;
            return;
        }
        let minute_due = self
            .usage_projected_at
            .is_none_or(|at| now.duration_since(at) >= USAGE_REPROJECT_INTERVAL);
        if changed || minute_due {
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
        self.usage_projected_at = Some(Instant::now());
        Ok(())
    }
}
