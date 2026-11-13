//! Panel projection: dynamic refresh, partial panel projection, geometry resync, pager cells.

use super::*;

impl ShellLoop {
    /// Publishes each pager cell's window-relative rect on the panel X
    /// window (`FLAMEWM_PAGER_CELLS`), read from the panel's retained layout
    /// so the canary never hard-codes screen offsets. Nonfatal on failure.
    ///
    /// Entries are in workspace-index order (`index 0..count` -> its cell node),
    /// not cell order, so consumers that address the array by workspace index
    /// keep working; each entry stays 4 CARDINAL values (x, y, width, height).
    pub(crate) fn publish_pager_cells(&self, runtime: &mut SurfaceRuntime) {
        let count = self
            .shell
            .snapshot()
            .workspaces
            .as_ref()
            .map_or(0, |workspaces| workspaces.count);
        for panel in self.surfaces.panel_handles() {
            let Ok(origin) = runtime.surface_device_rect(panel) else {
                eprintln!("flamewm-shell: pager cells skipped (stage=panel-origin)");
                continue;
            };
            let mut cells: Vec<[u64; 4]> = Vec::with_capacity(count);
            let mut complete = true;
            for index in 0..count {
                let Some(cell) = flamewm_shell_core::workspaces::pager_cell(index, count) else {
                    continue;
                };
                let id = format!("workspace-{cell}");
                let Some(rect) = runtime.node_device_rect_by_id(panel, &id) else {
                    eprintln!("flamewm-shell: pager cells skipped (stage=layout, node={id})");
                    complete = false;
                    break;
                };
                cells.push(pager_cell_window_rect(
                    (rect.x, rect.y, rect.width, rect.height),
                    (origin.x, origin.y),
                ));
            }
            if complete {
                if let Err(error) = runtime.set_pager_cells(panel, &cells) {
                    eprintln!(
                        "flamewm-shell: pager cells publish failed (stage=x11-property): {error:?}"
                    );
                }
            }
        }
    }

    /// Signal-driven dynamic refresh (§6): reconcile snapshot domains flagged
    /// by `ControlSignal` delivery, then project only the surfaces each
    /// changed domain owns.
    pub(crate) fn refresh_dynamic(&mut self, runtime: &mut SurfaceRuntime) {
        use flamewm_shell::runtime::ProjectionKind;
        let changed = self.shell.reconcile_dirty();
        if changed.panels {
            self.refresh_task_progress();
        }
        let progress_changed = self.take_task_progress_due(std::time::Instant::now());
        let mut projection_changed = changed;
        if progress_changed {
            projection_changed.windows = true;
        }
        // F10: display geometry change may arrive without a panels dirty
        // flag, so check generation/revision/geometry drift even when no
        // changed domain fired. Sync happens BEFORE next open/projection.
        // The drift check borrows the snapshot; share the Rc only on the resync path.
        let resynced = if self.panel_geometry_drifted(runtime, self.shell.snapshot()) {
            let snapshot = self.shell.snapshot_rc();
            self.resync_panel_geometry(runtime, &snapshot)
        } else {
            false
        };
        if !changed.any() && !resynced && !progress_changed {
            return;
        }
        // Context menus belong to their opening workspace. A workspace change
        // invalidates the menu before any panel or popup projection can paint
        // stale rows on the destination workspace.
        if changed.workspaces && self.context_menu.is_some() {
            if let Err(error) = self.close_transients(runtime) {
                eprintln!("flamewm-shell: workspace context-menu dismissal failed: {error:?}");
            }
        }
        // Domain projectors retain their own semantic defaults. Reapply the
        // already-authoritative settings payload after them so a later task
        // or workspace update cannot erase the active presentation accent.
        if self.settings_cache.is_some() {
            self.settings_projection_pending = true;
        }
        let request_id = self
            .shell
            .snapshot()
            .system
            .network
            .pending_secret
            .as_ref()
            .map(|request| request.request_id);
        if request_id != self.network_secret_request_id {
            self.network_secret.clear();
            self.network_secret_request_id = request_id;
        }
        let snapshot = self.shell.snapshot_rc();
        self.ensure_loader();
        // Windows or panels or workspaces own the visible panel document.
        if panel_projection_needed(projection_changed, resynced) {
            // C04: workspace/output/panel geometry changes invalidate the
            // helper anchor. F10: when the pre-projection resync above did
            // not fire but panels changed, resync now.
            let resynced_now = changed.panels && self.resync_panel_geometry(runtime, &snapshot);
            if !resynced_now
                && !self.project_panels_partial(
                    runtime,
                    &snapshot,
                    projection_changed,
                    "tick-partial",
                )
                && progress_changed
            {
                self.task_progress_pending = true;
            }
        }
        // Media popup closes when its tray slot is hidden (player gone or idle).
        let media_gone = changed.system
            && media_popup_must_close(&snapshot.system, self.surfaces.is_media_open());
        if media_gone {
            if let Err(error) = self.close_transients(runtime) {
                eprintln!("flamewm-shell: media popup close failed (nonfatal): {error:?}");
            }
        }
        // System owns only the currently open in-process status popup.
        if changed.system {
            let open = self.open_popup_name();
            let projected = match open {
                Some("media") => runtime.with_document(self.surfaces.media, |document| {
                    projection::project_media(document, &snapshot)
                }),
                _ => Ok(()),
            };
            if projected.is_ok() && open.is_some() {
                flamewm_shell::runtime::projection_counter(ProjectionKind::Popup).increment();
                if let Some("media") = open {
                    let _ = runtime.redraw(self.surfaces.media);
                }
            }
        }
        // Applications own the unified Start document, only when the app
        // set changed and Start is open; hidden Start stays untouched.
        if start_icon_refresh_due(
            &mut self.start_icons_stale,
            changed.applications,
            self.start_open,
        ) {
            // Borrow the start fields directly: no `&mut self` call sits between
            // here and the projection, and `icon_loader` is a disjoint field.
            let loader = self.icon_loader.as_mut();
            let projected = runtime.with_document(self.surfaces.start, |document| {
                async_projection::project_start_unified(
                    document,
                    &self.start_model,
                    self.start_category,
                    &self.start_query,
                    &snapshot.session,
                    loader,
                    self.start_scroll,
                )
            });
            if projected.is_ok() {
                flamewm_shell::runtime::projection_counter(ProjectionKind::Start).increment();
                let _ = runtime.redraw(self.surfaces.start);
            }
        }
        if changed.windows || changed.workspaces || changed.panels || resynced {
            self.sync_icon_geometry(runtime, &snapshot);
        }
    }

    /// Projects the changed domains into every panel, redraws them and republishes pager cells.
    /// Returns `false` when projection failed; `stage` names the caller in the diagnostic.
    pub(super) fn project_panels_partial(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
        changed: flamewm_shell::runtime::ChangedDomains,
        stage: &str,
    ) -> bool {
        let loader = &mut self.icon_loader;
        let icon_cache = &mut self.task_icon_cache;
        let items = crate::entry::settings_projection::taskbar_items(self.settings_cache.as_ref());
        let panel_outputs = self.surfaces.panel_outputs();
        let projected =
            projection::for_each_panel_record(snapshot, panel_outputs.clone(), |handle, view| {
                runtime.with_document(handle, |document| {
                    async_projection::project_panel_for_changed(
                        document,
                        view.with_items(items)
                            .with_task_progress(&self.task_progress),
                        loader.as_mut(),
                        icon_cache,
                        changed,
                    )
                })
            });
        if let Err(error) = &projected {
            crate::log_limit::emit(
                "panel projection failed",
                format_args!("flamewm-shell: panel projection failed (stage={stage}): {error:?}"),
            );
        }
        if projected.is_ok() {
            flamewm_shell::runtime::projection_counter(
                flamewm_shell::runtime::ProjectionKind::Partial,
            )
            .increment();
            for (handle, _) in &panel_outputs {
                let _ = runtime.redraw(*handle);
            }
            self.publish_pager_cells(runtime);
        }
        projected.is_ok()
    }

    /// F10: panel/display geometry resync. Close popup/quick helper,
    /// sync the placement context BEFORE the next open/projection,
    /// `move_resize` the panel native surface when geometry changed,
    /// then reproject the panel. Returns true when a resync happened.
    /// Pure drift check against a borrowed snapshot; no clone, no mutation.
    pub(super) fn panel_geometry_drifted(
        &self,
        runtime: &SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> bool {
        let (display_generation, panels_revision) = self.surfaces.placement_generation();
        snapshot.displays.generation != display_generation
            || snapshot.panels.revision != panels_revision
            || self.surfaces.panel_topology_drifted(snapshot)
            || self.surfaces.panel_native_geometry_drifted(
                runtime,
                (snapshot.displays.generation, snapshot.panels.revision),
            )
    }

    pub(crate) fn resync_panel_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> bool {
        if !self.panel_geometry_drifted(runtime, snapshot) {
            return false;
        }
        let active_popup = self.popup_session.active().cloned();
        let preserve_popup = active_popup.as_ref().is_some_and(|lease| {
            self.surfaces
                .popup_context_unchanged(&lease.output, snapshot)
        });
        let untracked_popup_open = active_popup.is_none()
            && (self.surfaces.is_start_group_open()
                || self.surfaces.is_media_open()
                || self.surfaces.is_usage_open()
                || self.context_menu.is_some()
                || self.quick_kind_open.is_some());
        if !preserve_popup {
            // Retire only the popup whose own output context is changing. A
            // global generation bump on another monitor leaves this lease up.
            if let Some(lease) = active_popup.as_ref() {
                if is_quick_popup_kind(&lease.kind) {
                    self.close_quick_control_nonfatal();
                } else if let Err(error) = self.close_transients(runtime) {
                    eprintln!("flamewm-shell: close affected popup failed: {error:?}");
                }
            } else if untracked_popup_open {
                let _ = self.close_transients(runtime);
                self.close_quick_control_nonfatal();
            }
            if untracked_popup_open || active_popup.is_some() {
                self.end_start_session();
                self.context_menu = None;
            }
        }
        // Sync BEFORE next open/projection; refusal keeps old context.
        match self.surfaces.sync_panel_context(runtime, snapshot) {
            Ok(true) => (),
            Ok(false) => {
                eprintln!("flamewm-shell: panel context sync refused (missing output/panel)");
                return false;
            }
            Err(error) => {
                crate::log_limit::emit(
                    "panel surface sync failed",
                    format_args!("flamewm-shell: panel surface sync failed: {error}"),
                );
                return false;
            }
        }
        if preserve_popup {
            if let Some(lease) = active_popup.as_ref() {
                self.popup_session.refresh_target(
                    &lease.output,
                    snapshot.displays.generation,
                    snapshot.panels.revision,
                );
            }
        }
        if let Err(error) = self.surfaces.sync_panel_ui_scale(runtime) {
            crate::log_limit::emit(
                "panel ui scale resync failed",
                format_args!("flamewm-shell: panel ui scale resync failed (nonfatal): {error}"),
            );
        }
        if let Err(error) = self.surfaces.move_panel_to_synced(runtime) {
            crate::log_limit::emit(
                "panel move_resize failed",
                format_args!("flamewm-shell: panel move_resize failed (nonfatal): {error}"),
            );
        }
        // The XShape mask must use the resized native panel's new dimensions.
        self.apply_taskbar_frame(runtime);
        self.apply_taskbar_backdrop(runtime);
        self.settings_projection_pending = true;
        let changed = flamewm_shell::runtime::ChangedDomains {
            panels: true,
            windows: true,
            ..Default::default()
        };
        self.project_panels_partial(runtime, snapshot, changed, "resync-partial");
        // A panel created or resynced here starts from the template: project the tray again.
        self.project_tray_panels(runtime);
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("shell.panel.resync"),
            Duration::from_millis(500),
            || {
                let (generation, revision) = self.surfaces.placement_generation();
                format!(
                    "F10 resync generation={generation} revision={revision} edge={:?}",
                    self.surfaces.panel_edge()
                )
            },
        );
        true
    }

    /// Publishes the desktop layer from the wallpaper settings and installs the taskbar backdrop
    /// for the current opacity and blur.
    pub(super) fn apply_taskbar_backdrop(&mut self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let (opacity, blur) =
            crate::entry::settings_projection::taskbar_backdrop_percents(settings);
        let source = flamewm_shell::surface_backdrop::desktop_source(
            settings,
            &flamewm_api::paths::workspace_packaged_root(),
        );
        runtime.set_desktop_source(Some(source.clone()));
        let separate = crate::settings_projection::taskbar_items(Some(settings)).separate;
        if let Err(error) =
            self.surfaces
                .apply_panel_backdrop(runtime, Some(&source), opacity, blur, separate)
        {
            crate::log_limit::emit(
                "taskbar backdrop failed",
                format_args!("flamewm-shell: taskbar backdrop failed (nonfatal): {error}"),
            );
        }
    }

    /// Publishes the taskbar dock frame (strut edge and corner radius); a no-op while unchanged.
    pub(super) fn apply_taskbar_frame(&self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let frame = taskbar_frame(settings);
        if let Err(error) = self.surfaces.apply_dock_frame(runtime, frame) {
            crate::log_limit::emit(
                "panel error passthrough",
                format_args!("flamewm-shell: {error}"),
            );
        }
        if let Err(error) = self
            .surfaces
            .apply_taskbar_enabled(runtime, taskbar_enabled(settings))
        {
            crate::log_limit::emit("taskbar visibility", format_args!("flamewm-shell: {error}"));
        }
        if let Err(error) = self.surfaces.apply_taskbar_gradient(runtime, settings) {
            crate::log_limit::emit("taskbar gradient", format_args!("flamewm-shell: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn panel_projection_needed_true_for_system_only_change() {
        let changed = flamewm_shell::runtime::ChangedDomains {
            system: true,
            ..Default::default()
        };
        assert!(super::panel_projection_needed(changed, false));
        assert!(super::panel_projection_needed(changed, true));
    }

    #[test]
    fn panel_projection_needed_panels_only_when_not_resynced() {
        let changed = flamewm_shell::runtime::ChangedDomains {
            panels: true,
            ..Default::default()
        };
        assert!(super::panel_projection_needed(changed, false));
        assert!(!super::panel_projection_needed(changed, true));
    }

    #[test]
    fn panel_projection_needed_false_without_projection_domains() {
        let changed = flamewm_shell::runtime::ChangedDomains {
            applications: true,
            ..Default::default()
        };
        assert!(!super::panel_projection_needed(changed, false));
    }

    #[test]
    fn pager_cell_rect_follows_panel_resize_scale() {
        // Panel origin at (0, 1000) on a 1080 px output: 44 px bar at 100%,
        // 55 px bar at 125%. Cell 1 sits at window y=6 in both cases.
        let origin = (0.0, 1000.0);
        assert_eq!(
            super::pager_cell_window_rect((1166.0, 1006.0, 22.0, 16.0), origin),
            [1166, 6, 22, 16]
        );
        // 125% layout: cell y and height scale by 1.25, rounded.
        assert_eq!(
            super::pager_cell_window_rect((1457.5, 1007.5, 27.5, 20.0), origin),
            [1458, 8, 28, 20]
        );
        // Negative offset clamps to zero.
        assert_eq!(
            super::pager_cell_window_rect((5.0, 990.0, 10.0, 10.0), origin),
            [5, 0, 10, 10]
        );
    }
}
