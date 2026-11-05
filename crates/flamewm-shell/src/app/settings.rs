//! Settings snapshot application, auto time zone and location fixes.

use super::popups::quick_kind_shown;
use super::*;

impl ShellLoop {
    pub(crate) fn apply_settings_snapshot(&mut self, snapshot: SettingsSnapshot) {
        if self
            .settings_cache
            .as_ref()
            .is_some_and(|current| snapshot.revision <= current.revision)
        {
            return;
        }
        self.settings_revision = snapshot.revision;
        flamewm_ui_x11::apply_render_acceleration(&snapshot);
        self.cursor_theme.sync_snapshot(&snapshot);
        self.weather
            .apply_settings(flamewm_shell_core::weather::WeatherSettings::from_snapshot(
                &snapshot,
            ));
        self.weather_feed_pending = true;
        let theme = crate::entry::settings_projection::icon_theme_name(Some(&snapshot));
        if let Some(loader) = self.icon_loader.as_mut() {
            if loader.set_theme(&theme) {
                // Re-prime task slots so they re-request under the new theme.
                self.task_icon_prime_armed = true;
                // Drop rasters cached under the old theme; Start re-requests.
                async_projection::invalidate_icon_cache();
                self.start_icons_stale = true;
            }
        }
        if let Some(SettingValue::Boolean(raise)) = snapshot.values.get(AudioSettings::SETTINGS_KEY)
        {
            self.audio_settings.raise_maximum = *raise;
        }
        // A hidden taskbar item closes its open quick-control popup.
        let items = crate::entry::settings_projection::taskbar_items(Some(&snapshot));
        if self
            .quick_kind_open
            .is_some_and(|kind| !quick_kind_shown(kind, items))
        {
            self.close_quick_control_nonfatal();
        }
        let desired_zone = flamewm_shell::weather_auto::desired_time_zone(&snapshot);
        self.settings_cache = Some(snapshot);
        self.settings_projection_pending = true;
        self.set_auto_time_zone(desired_zone);
        // This runs on the settings signal, not an X event: wake the loop so the tick
        // projects the change now instead of at the next pointer or clock event.
        self.request_tick();
    }

    /// Makes `desired` the process zone when it differs from what this shell applied. `None`
    /// restores the system zone. A failed zone file leaves the current zone unchanged.
    pub(super) fn set_auto_time_zone(&mut self, desired: Option<String>) {
        if desired == self.auto_time_zone_applied {
            return;
        }
        if let Some(zone) = desired {
            if !flamewm_shell::weather_auto::apply_time_zone(&zone) {
                eprintln!("flamewm-shell: time zone not applied (zone file missing or invalid)");
                return;
            }
            self.auto_time_zone_applied = Some(zone);
        } else {
            flamewm_shell::weather_auto::restore_system_time_zone();
            self.auto_time_zone_applied = None;
        }
        self.time_zone_changed = true;
        self.zone_settle_at = Some(std::time::Instant::now() + ZONE_SETTLE);
    }

    /// Automatic location tick: one lookup on the cadence, then the fix is written as the
    /// weather location and cached zone. Failures keep the last values and are not fatal.
    pub(super) fn tick_auto_location(&mut self) {
        let Some(enabled) = self.settings_cache.as_ref().map(|s| {
            flamewm_shell::weather_auto::flag(s, flamewm_shell::weather_auto::AUTO_LOCATION_KEY)
        }) else {
            return;
        };
        let online = self.shell.snapshot().system.network.availability
            == flamewm_api::system::ServiceAvailability::Available;
        let Some(result) = self.auto_lookup.tick(
            std::time::Instant::now(),
            enabled,
            online,
            flamewm_shell::weather_auto::spawn_ip_lookup,
        ) else {
            return;
        };
        match result {
            Ok(fix) => self.apply_auto_fix(fix),
            Err(error) => eprintln!("flamewm-shell: automatic location unavailable: {error:?}"),
        }
    }

    pub(super) fn apply_auto_fix(&mut self, fix: flamewm_shell::weather_auto::IpFix) {
        use flamewm_shell::weather_auto as auto;
        let Some(snapshot) = self.settings_cache.clone() else {
            return;
        };
        // Re-check: a manual city pick may have turned automatic location off meanwhile.
        if !auto::flag(&snapshot, auto::AUTO_LOCATION_KEY) {
            return;
        }
        let current = flamewm_shell_core::weather::WeatherSettings::from_snapshot(&snapshot);
        let mut changes: Vec<flamewm_api::settings::SettingsChange> =
            auto::location_change(&current, &fix).into_iter().collect();
        if let Some(zone) = fix.time_zone.as_ref() {
            if snapshot.values.get(auto::AUTO_TIME_ZONE_KEY)
                != Some(&SettingValue::Text(zone.clone()))
            {
                changes.push(flamewm_api::settings::SettingsChange {
                    key: auto::AUTO_TIME_ZONE_KEY.to_owned(),
                    value: Some(SettingValue::Text(zone.clone())),
                });
            }
        }
        if !changes.is_empty() {
            self.submit(ControlRequest::ApplySettings(SettingsTransaction {
                expected_revision: self.settings_revision,
                changes,
                reset_section: None,
            }));
        }
        if let Some(zone) = fix
            .time_zone
            .filter(|_| auto::flag(&snapshot, auto::AUTO_TIME_ZONE_ENABLED_KEY))
        {
            self.set_auto_time_zone(Some(zone));
        }
    }

    pub(super) fn project_settings(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        if !self.settings_projection_pending {
            return Ok(());
        }
        let Some(snapshot) = self.settings_cache.as_ref() else {
            self.settings_projection_pending = false;
            return Ok(());
        };
        // Borrowed, not cloned: the snapshot is last used before any `&mut self` call below.
        // Every monitor has an independent native panel document. Project the same
        // surfaceOpacity/surfaceColor settings into all of them.
        for panel in self.surfaces.panel_handles() {
            runtime.with_document(panel, |document| {
                crate::entry::settings_projection::project(
                    document,
                    snapshot,
                    crate::entry::settings_projection::SurfaceKind::Panel,
                )
            })?;
        }
        for (surface, kind) in [
            (
                self.surfaces.launcher,
                crate::entry::settings_projection::SurfaceKind::Launcher,
            ),
            (
                self.surfaces.start,
                crate::entry::settings_projection::SurfaceKind::Start,
            ),
            (
                self.surfaces.task_menu,
                crate::entry::settings_projection::SurfaceKind::TaskMenu,
            ),
            (
                self.surfaces.media,
                crate::entry::settings_projection::SurfaceKind::Media,
            ),
        ] {
            runtime.with_document(surface, |document| {
                crate::entry::settings_projection::project(document, snapshot, kind)
            })?;
        }
        // The focused task underline takes its colour from the settings accent; re-project the task slots
        // now so a settings change (or a first settings arrival after the last task update) cannot leave it stale.
        let shell_snapshot = self.shell.snapshot();
        // Taskbar item settings (tray, clock, pager visibility) follow the same reprojection.
        let task_changed = flamewm_shell::runtime::ChangedDomains {
            windows: true,
            workspaces: true,
            system: true,
            ..Default::default()
        };
        let loader = &mut self.icon_loader;
        let items = crate::entry::settings_projection::taskbar_items(self.settings_cache.as_ref());
        let icon_cache = &mut self.task_icon_cache;
        let _ = projection::for_each_panel_record(
            shell_snapshot,
            self.surfaces.panel_outputs(),
            |handle, view| {
                runtime.with_document(handle, |document| {
                    async_projection::project_panel_for_changed(
                        document,
                        view.with_items(items)
                            .with_task_progress(&self.task_progress),
                        loader.as_mut(),
                        icon_cache,
                        task_changed,
                    )
                })
            },
        );
        self.apply_start_backdrop(runtime);
        self.apply_start_slide(runtime);
        self.apply_taskbar_backdrop(runtime);
        for panel in self.surfaces.panel_handles() {
            let _ = runtime.redraw(panel);
        }
        if self.surfaces.is_start_group_open() {
            let _ = runtime.redraw(self.surfaces.start);
        }
        if self.context_menu.is_some() {
            let _ = runtime.redraw(self.surfaces.task_menu);
        }
        if self.surfaces.is_media_open() {
            let _ = runtime.redraw(self.surfaces.media);
        }
        self.settings_projection_pending = false;
        self.apply_taskbar_frame(runtime);
        Ok(())
    }
}
