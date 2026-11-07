//! Icon loader wake and drain, dependent-surface icon geometry.

use super::*;

impl ShellLoop {
    pub(crate) fn ensure_loader(&mut self) {
        if self.icon_loader.is_none() {
            let theme =
                crate::entry::settings_projection::icon_theme_name(self.settings_cache.as_ref());
            self.icon_loader = Some(IconLoader::spawn(self.loader_root.clone(), theme));
        }
    }

    /// Canonical `IconService` wake FD for the reactor loop. Ensures the
    /// loader exists (single owner) and returns the service descriptor;
    /// the reactor only wakes on it, `drain_icons` drains on the tick.
    pub(crate) fn icon_wake_fd(&mut self) -> std::os::unix::io::RawFd {
        self.ensure_loader();
        self.icon_loader
            .as_ref()
            .map(IconLoader::wake_fd)
            .expect("icon loader present after ensure")
    }

    /// FD wake path: drain the wake pipe only. Result payloads stay queued
    /// for the tick (`drain_icons`), which owns document mutation.
    pub(crate) fn drain_icons_pending(&mut self) {
        self.ensure_loader();
        if let Some(loader) = self.icon_loader.as_mut() {
            loader.wake_drain();
        }
    }

    pub(crate) fn drain_icons(&mut self, runtime: &mut SurfaceRuntime) {
        self.ensure_loader();
        let results = self
            .icon_loader
            .as_mut()
            .map(IconLoader::drain_ready)
            .unwrap_or_default();
        let loader = self.icon_loader.as_ref();
        if loader.map_or(0, |l| l.queue_full_drops) > 0 || loader.map_or(0, |l| l.stale_drops) > 0 {
            if let Some(loader) = self.icon_loader.as_ref() {
                async_projection::note_loader_stats(loader);
            }
        }
        if results.is_empty() {
            return;
        }
        // Exact-target completion: worker result -> wake -> reactor ->
        // verify (key + generation + view epoch) -> cache -> mutate the
        // matching image node on every per-output panel, then redraw each
        // surface. The slot binding is shared because each output projects
        // the same task model and image key.
        let mut need_panel = false;
        let mut need_submenu = false;
        let panel_edges: Vec<(flamewm_ui_x11::SurfaceHandle, u32)> = {
            let snapshot = self.shell.snapshot();
            self.surfaces
                .panel_outputs()
                .into_iter()
                .map(|(handle, output_id)| {
                    let output = snapshot
                        .displays
                        .outputs
                        .iter()
                        .find(|output| output.id == output_id);
                    (handle, projection::task_icon_device_px(output))
                })
                .collect()
        };
        for res in results {
            let Some(raster) = res.raster else { continue };
            let image = async_projection::IconRaster::from(flamewm_ui_x11::RuntimeImage {
                source: raster.source.clone(),
                width: raster.width,
                height: raster.height,
                pixels: raster.pixels,
            });
            async_projection::note_icon_result(
                &res.name,
                raster.width,
                raster.height,
                image.clone(),
            );
            let surface = match res.target {
                IconTarget::StartSlot(_) if !self.start_open => {
                    continue;
                }
                target => IconLoader::dependent_surface(target),
            };
            let apply = |document: &mut flamewm_ui_x11::UiDocumentView<'_>| {
                async_projection::apply_icon_result(
                    document,
                    res.target,
                    &res.name,
                    res.width,
                    res.height,
                    res.generation,
                    image.clone(),
                )
            };
            match surface {
                DependentSurface::Panel => {
                    // Each panel projects its own icon edge; only panels at
                    // this result's edge may take it (no cross-scale paint).
                    for (handle, task_edge) in &panel_edges {
                        // Tray slots project at the tray edge, not the task-slot edge.
                        let edge = match res.target {
                            IconTarget::TraySlot(_) => flamewm_shell::tray_host::icon_device_px(),
                            _ => *task_edge,
                        };
                        if edge != res.width {
                            continue;
                        }
                        let applied = runtime.with_document(*handle, |document| {
                            if apply(document)? {
                                Ok(true)
                            } else {
                                async_projection::apply_cached_binding(document, res.target, edge)
                            }
                        });
                        if matches!(applied, Ok(true)) {
                            need_panel = true;
                        }
                    }
                }
                DependentSurface::Dock => {
                    if let Some(surface) = self.dock_surface {
                        if matches!(runtime.with_document(surface, apply), Ok(true)) {
                            let _ = runtime.redraw(surface);
                        }
                    }
                }
                DependentSurface::Start => {
                    if matches!(runtime.with_document(self.surfaces.start, apply), Ok(true)) {
                        need_submenu = true;
                    }
                }
                DependentSurface::Launcher => {
                    if matches!(
                        runtime.with_document(self.surfaces.launcher, apply),
                        Ok(true)
                    ) {
                        let _ = runtime.redraw(self.surfaces.launcher);
                    }
                }
            }
        }
        if need_panel {
            for handle in self.surfaces.panel_handles() {
                let _ = runtime.redraw(handle);
            }
        }
        if need_submenu {
            let _ = runtime.redraw(self.surfaces.start);
        }
    }

    /// Publishes `_NET_WM_ICON_GEOMETRY` on each task slot's client window
    /// from the slot's retained root-space rect. Called only after a layout
    /// or task change; the tracker writes and clears just the deltas.
    pub(crate) fn sync_icon_geometry(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) {
        let panels = self.surfaces.panel_outputs();
        let workspace = snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index);
        let output_slots: Vec<_> = panels
            .iter()
            .map(|(_, output)| {
                let slots = flamewm_shell_core::task_visual_states_on_output_and_workspace(
                    &snapshot.panels,
                    &snapshot.windows,
                    &snapshot.applications,
                    Some(output),
                    workspace,
                )
                .iter()
                .map(|state| state.window)
                .collect();
                (output.clone(), slots)
            })
            .collect();
        let desired =
            icon_geometry::desired_output_slot_geometry(&output_slots, |output, index| {
                let node = format!("task-slot-{}", index + 1);
                panels
                    .iter()
                    .find(|(_, panel_output)| panel_output == output)
                    .and_then(|(panel, _)| runtime.node_device_rect_by_id(*panel, &node))
                    .map(|rect| (rect.x, rect.y, rect.width, rect.height))
            });
        let mut sink = RuntimeIconGeometrySink { runtime };
        self.icon_geometry.sync(&desired, &mut sink);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stale_start_icons_project_once_while_open() {
        // A theme change projects the open Start once; later unrelated refreshes do not.
        let mut stale = true;
        let mut projections = 0;
        for applications_changed in [false, false, true, false] {
            if super::start_icon_refresh_due(&mut stale, applications_changed, true) {
                projections += 1;
            }
        }
        assert_eq!(
            projections, 2,
            "one theme projection plus one app-set projection"
        );
        assert!(!stale, "the theme refresh consumes the stale flag");

        // A hidden Start keeps the flag until it is next projected on open.
        let mut hidden_stale = true;
        assert!(!super::start_icon_refresh_due(
            &mut hidden_stale,
            true,
            false
        ));
        assert!(hidden_stale);
    }
}
