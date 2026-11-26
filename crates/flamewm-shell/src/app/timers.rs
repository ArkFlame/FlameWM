//! Shell tick scheduler: wake deadlines, OSD and switcher timers.

use super::control::active_output_target;
use super::*;

impl ShellLoop {
    /// Makes the reactor return from dispatch immediately so the next tick runs.
    pub(super) fn request_tick(&self) {
        if let Some(wake) = &self.wake {
            wake.set(Some(std::time::Instant::now()));
        }
    }

    pub(crate) fn tick(
        &mut self,
        runtime: &mut SurfaceRuntime,
        clock_due: &std::sync::atomic::AtomicBool,
    ) -> Result<(), UiBackendError> {
        self.drain_raw_press_observations(runtime);
        if self.quick_kind_open.is_none() {
            runtime.disarm_raw_press_observer();
        }
        // Startup panel projection runs with no loader (main.rs
        // project_panel_critical), so task-slot icons were recorded as empty
        // placeholders with no request. Re-project task slots once with the
        // loader now that it exists so the requests are actually submitted.
        // Stays armed while the snapshot has no task states (the first tick
        // often precedes any managed window), and disarms only after a run
        // with task states and a loader projected successfully.
        if self.task_icon_prime_armed {
            self.ensure_loader();
            let snapshot = self.shell.snapshot();
            let task_state_count = flamewm_shell_core::task_visual_states_on_output_and_workspace(
                &snapshot.panels,
                &snapshot.windows,
                &snapshot.applications,
                snapshot.panels.panels.first().map(|panel| &panel.output),
                snapshot
                    .workspaces
                    .as_ref()
                    .map(|workspaces| workspaces.active_index),
            )
            .len();
            let loader_present = self.icon_loader.is_some();
            if async_projection::task_icon_prime_should_run(
                self.task_icon_prime_armed,
                task_state_count,
                loader_present,
            ) {
                let mut loader = self.icon_loader.as_mut();
                let icon_cache = &mut self.task_icon_cache;
                let changed = flamewm_shell::runtime::ChangedDomains {
                    windows: true,
                    ..Default::default()
                };
                let items =
                    crate::entry::settings_projection::taskbar_items(self.settings_cache.as_ref());
                let panel_outputs = self.surfaces.panel_outputs();
                let mut projection_ok = true;
                for (handle, _) in panel_outputs {
                    let result = runtime.with_document(handle, |document| {
                        async_projection::project_panel_for_changed(
                            document,
                            PanelView::first(snapshot)
                                .with_items(items)
                                .with_task_progress(&self.task_progress),
                            loader.as_deref_mut(),
                            icon_cache,
                            changed,
                        )
                    });
                    if let Err(error) = result {
                        eprintln!(
                            "flamewm-shell: task icon prime projection failed (stage=task-slots, states={task_state_count}): {error:?}"
                        );
                        projection_ok = false;
                    }
                }
                eprintln!(
                    "flamewm-shell: task icon prime ran (stage=task-slots, states={task_state_count}, ok={projection_ok})"
                );
                self.task_icon_prime_armed = async_projection::task_icon_prime_still_armed(
                    self.task_icon_prime_armed,
                    task_state_count,
                    loader_present,
                    projection_ok,
                );
            }
        }
        self.drain_icons(runtime);
        // J07: nonblocking helper poll; restart is bounded, failures are
        // nonfatal diagnostics, the shell loop always survives.
        flamewm_shell::quick_controls::host::poll_supervisor_nonfatal(&mut self.quick_control);
        match self.quick_control.take_open_results() {
            Ok(results) => {
                for result in results {
                    match apply_quick_open_result(&mut self.popup_session, result) {
                        QuickOpenResolution::Presented | QuickOpenResolution::Stale => {}
                        QuickOpenResolution::Failed => {
                            self.quick_audio_open = false;
                            self.quick_kind_open = None;
                        }
                    }
                }
            }
            Err(error) => crate::log_limit::emit(
                "quick-control result poll failed",
                format_args!("flamewm-shell: quick-control result poll failed: {error}"),
            ),
        }
        self.tick_auto_location();
        let settled = self
            .zone_settle_at
            .is_some_and(|at| std::time::Instant::now() >= at);
        if settled {
            self.zone_settle_at = None;
        }
        let now_instant = Instant::now();
        let clock_key = (self.settings_revision, self.settings_cache.is_some());
        let clock_settings_changed = self.clock_settings_key != Some(clock_key);
        if clock_settings_changed {
            self.clock_settings_key = Some(clock_key);
            self.rearm_clock(now_instant);
        }
        // The reactor minute timer covers minute boundaries and suspend realignment; the
        // shell deadline covers seconds mode and a hidden clock (it is `None` then).
        let clock_boundary_due = self.clock_next_at.is_some_and(|at| now_instant >= at);
        let clock_forced =
            std::mem::take(&mut self.time_zone_changed) | settled | clock_settings_changed;
        let clock_fired =
            clock_due.swap(false, Ordering::Acquire) | clock_boundary_due | clock_forced;
        if clock_fired {
            self.rearm_clock(now_instant);
            // A hidden clock consumes the tick without formatting or repainting a panel.
            if self.clock_next_at.is_some() {
                let now = Local::now();
                let epoch_sec = now.timestamp();
                if clock_forced || self.clock_projected_sec != Some(epoch_sec) {
                    self.clock_projected_sec = Some(epoch_sec);
                    let clock = self
                        .settings_cache
                        .as_ref()
                        .map(crate::entry::settings_projection::clock_display)
                        .unwrap_or_default();
                    for (handle, _) in self.surfaces.panel_outputs() {
                        runtime.with_document(handle, |document| {
                            projection::project_clock_at(document, now, clock)
                        })?;
                        runtime.redraw(handle)?;
                    }
                }
                // Heap trim is gated to once per CLOCK_HEAP_TRIM_INTERVAL, so a seconds-mode
                // clock does not release the free heap every second.
                LAST_CLOCK_HEAP_TRIM.with(|last| {
                    if last
                        .get()
                        .is_none_or(|at| now_instant.duration_since(at) >= CLOCK_HEAP_TRIM_INTERVAL)
                    {
                        last.set(Some(now_instant));
                        flamewm_ui_x11::release_free_heap();
                    }
                });

                let date = (now.year(), now.month() as u8, now.day() as u8);
                if self.date_tracker.date_changed(date) {
                    self.local_date = date;
                }
            }
        }
        // Steady state never polls: reconcile signal-flagged domains at most
        // once per tick, after the clock projection above.
        // Start launch results arrive via the mutation result drain; apply
        // them here so a failed launch shows its error on this turn.
        self.apply_start_launch_outcomes(runtime)?;
        let changed = self.refresh_dynamic(runtime);
        self.project_settings(runtime)?;
        self.sync_dock(runtime, changed, false);
        self.tick_weather(runtime);
        self.tick_usage(runtime);
        // Signal-queued controls (StartMenuToggleRequested -> ToggleStart) arrive
        // on the control-signal fd, not an X event, so no event route drains
        // them. Drain through the same apply_controls the click path uses, then
        // flush damage: the reactor only redraws dirty surfaces when X events
        // were pumped on this turn.
        if self.shell.has_pending_controls() {
            self.apply_controls(runtime)?;
            runtime.redraw_dirty()?;
        }
        self.tick_osd(runtime, changed);
        self.tick_switcher(runtime);
        self.tick_dock_bounce(runtime, std::time::Instant::now());
        self.publish_wake(std::time::Instant::now());
        Ok(())
    }

    /// Wires the reactor wake slot. OSD, weather, clock and auto-location share it, so the tick
    /// publishes the earliest of their deadlines in one place (a per-owner set would clobber the
    /// other).
    pub(crate) fn attach_wake(&mut self, wake: flamewm_reactor::WakeDeadline) {
        self.wake = Some(wake);
    }

    /// Re-arms the panel clock cadence from the current settings: every second when
    /// `taskbarShowSeconds` is on, otherwise every minute. A hidden taskbar clock re-arms to
    /// `None`, so it costs neither wakeups nor panel repaints.
    pub(super) fn rearm_clock(&mut self, now: Instant) {
        let display = self
            .settings_cache
            .as_ref()
            .map(crate::entry::settings_projection::clock_display)
            .unwrap_or_default();
        let shown =
            crate::entry::settings_projection::taskbar_items(self.settings_cache.as_ref()).clock;
        // The epoch read inside the delay runs after `now` was captured, so arm one millisecond
        // past the boundary: the tick then never paints the second/minute that is ending.
        self.clock_next_at = shown.then(|| {
            now + clock_boundary_delay(display.show_seconds) + std::time::Duration::from_millis(1)
        });
    }

    pub(super) fn publish_wake(&self, now: std::time::Instant) {
        let Some(wake) = &self.wake else { return };
        wake.set(earliest_wake([
            self.osd.deadline(),
            self.switcher.deadline(),
            self.zone_settle_at,
            self.task_progress_deadline,
            self.clock_next_at,
            self.weather.next_deadline(now),
            self.dock_stage
                .autohide
                .and_then(|model| model.deadline(now)),
            self.dock_bounce
                .and_then(|(_, bounce)| bounce.deadline(now)),
            self.bluetooth_agent
                .as_ref()
                .and_then(|agent| agent.borrow().next_deadline()),
            self.usage.next_deadline(),
            self.usage_projected_at
                .filter(|_| self.surfaces.is_usage_open())
                .map(|at| at + super::usage::USAGE_REPROJECT_INTERVAL),
        ]));
    }

    /// Volume and brightness OSD: shows a kind's snapshot level when it changes and
    /// hides at the single deadline. Nonfatal: a failure never stops the loop.
    pub(super) fn tick_osd(
        &mut self,
        runtime: &mut SurfaceRuntime,
        changed: flamewm_shell::runtime::ChangedDomains,
    ) {
        let snapshot = self.shell.snapshot();
        let system = &snapshot.system;
        // The live "raise maximum volume" cap, the same value the mixer slider uses.
        let max = self.audio_settings.max_percent();
        // Same output source as the panel scale: the shown panel's placement output. The lookup is
        // lazy: the controller calls it only when the card shows, so idle ticks allocate nothing.
        let target = || active_output_target(&self.surfaces, snapshot);
        let now = Instant::now();
        let settings = self.settings_cache.as_ref();
        let active = snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index);
        let names: &[String] = snapshot
            .workspaces
            .as_ref()
            .map_or(&[], |workspaces| workspaces.names.as_slice());
        if let Err(error) = self
            .osd
            .update_workspace(runtime, active, names, target, settings, now)
        {
            eprintln!("flamewm-shell: workspace OSD failed (nonfatal): {error:?}");
        }
        // The two levels only move with a system snapshot change, and the hide deadline owns
        // expiry. Once the baseline observation has run, an idle or pointer-motion turn (no
        // system change, no deadline) can skip the sink/brightness reads and the card update.
        let sample = !self.osd_primed || changed.system || self.osd.deadline().is_some();
        if !sample {
            return;
        }
        // Raw reading only: building a row's owned id/label strings per turn would allocate on
        // every shell wake for two fields the card needs.
        let volume = flamewm_shell::taskbar::status::audio::default_sink_level(&system.audio)
            .map(|(volume, muted)| flamewm_shell::osd::Level::new(volume, muted, max));
        let brightness = (system.brightness.availability
            == flamewm_api::system::ServiceAvailability::Available)
            .then(|| flamewm_shell::osd::Level::brightness(system.brightness.percent));
        match self
            .osd
            .update(runtime, volume, brightness, target, settings, now)
        {
            Ok(()) => self.osd_primed = true,
            Err(error) => eprintln!("flamewm-shell: OSD failed (nonfatal): {error:?}"),
        }
    }

    /// Alt+Tab overlay: renders the WM's latest switcher snapshot after its hold delay. Nonfatal:
    /// a failed card leaves the WM switch untouched. Idle ticks (no snapshot, no deadline) return
    /// before any output lookup.
    pub(super) fn tick_switcher(&mut self, runtime: &mut SurfaceRuntime) {
        let frame = self.shell.take_switcher_frame();
        let due = self
            .switcher
            .deadline()
            .is_some_and(|at| Instant::now() >= at);
        if frame.is_none() && !due {
            return;
        }
        let (output, scale) = active_output_target(&self.surfaces, self.shell.snapshot());
        let settings = self.settings_cache.as_ref();
        if let Err(error) =
            self.switcher
                .update(runtime, frame, || (output, scale), settings, Instant::now())
        {
            eprintln!("flamewm-shell: window switcher failed (nonfatal): {error:?}");
        }
    }
}

/// Delay to the next wall-clock second or minute boundary, measured from the current epoch
/// time so repeated calls never drift. The reactor's wall-clock owner exposes the same
/// alignment rule; the shell deadline reuses it because the reactor timer is minute-only.
fn clock_boundary_delay(with_seconds: bool) -> std::time::Duration {
    use flamewm_reactor::{SystemWallClock, WallClockSource};
    let now_ms = SystemWallClock.now_epoch_ms();
    std::time::Duration::from_millis(flamewm_reactor::wall_clock_delay_ms(now_ms, with_seconds))
}

#[cfg(test)]
mod tests {
    #[test]
    fn clock_boundary_delay_never_exceeds_the_cadence() {
        assert!(super::clock_boundary_delay(false) <= std::time::Duration::from_secs(60));
        assert!(super::clock_boundary_delay(true) <= std::time::Duration::from_secs(1));
        assert!(!super::clock_boundary_delay(true).is_zero());
    }

    #[test]
    fn shared_wake_slot_takes_the_earliest_owner_deadline() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let soon = t0 + Duration::from_millis(100);
        assert_eq!(
            super::earliest_wake([None, None, None, None, None, None]),
            None,
            "idle loop"
        );
        assert_eq!(
            super::earliest_wake([Some(t0), None, Some(soon), None, None, None]),
            Some(t0)
        );
        assert_eq!(
            super::earliest_wake([None, Some(soon), None, None, None, None]),
            Some(soon)
        );
        assert_eq!(
            super::earliest_wake([None, None, None, Some(soon), None, None]),
            Some(soon)
        );
    }
}
