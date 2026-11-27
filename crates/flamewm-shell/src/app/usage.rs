//! AI usage card: settings, fetch scheduler tick and card projection.

use std::time::{Duration, Instant};

use super::*;

/// The open card is re-projected at this period so reset captions stay current (rides the wake).
pub(super) const USAGE_REPROJECT_INTERVAL: Duration = Duration::from_secs(60);

impl ShellLoop {
    /// Usage tick: applies settings, polls the fetch worker, starts a due fetch, and reprojects the
    /// open card on fresh snapshots or once a minute.
    pub(super) fn tick_usage(&mut self, runtime: &mut SurfaceRuntime) {
        // The provider list and interval change only with a settings snapshot; rebuilding the
        // provider Vec on every pointer-motion wake is wasted work.
        if self.usage_settings_key != Some((self.settings_revision, self.settings_cache.is_some()))
        {
            let (providers, interval) = crate::entry::settings_projection::usage::usage_settings(
                self.settings_cache.as_ref(),
            );
            self.usage.configure(providers, interval);
            self.usage_settings_key = Some((self.settings_revision, self.settings_cache.is_some()));
        }
        // Clone the wake writer only when a fetch actually starts, not on every tick:
        // the usage card is usually closed and most ticks start no worker.
        let wake = &self.result_wake;
        let now = Instant::now();
        // Credentials are read only by a fetch, and a fetch starts only while the popup is open.
        self.usage.set_needed(self.surfaces.is_usage_open());
        let changed = self.usage.tick(now, |providers| {
            flamewm_shell::usage_host::spawn_usage_fetch(providers, wake.waker())
        });
        if !self.surfaces.is_usage_open() {
            self.usage_projected_at = None;
            self.usage_focus = None;
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
    pub(crate) fn project_usage_card(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let now = chrono::Utc::now().timestamp();
        let snapshots = &self.usage.snapshots;
        let focused = self.usage_focus.is_some();
        runtime.with_document(self.surfaces.usage, |document| {
            projection::project_usage(document, snapshots, now)?;
            projection::project_usage_focus(document, focused)
        })?;
        runtime.redraw(self.surfaces.usage)?;
        self.usage_projected_at = Some(Instant::now());
        Ok(())
    }
}
