use super::*;

impl ShellSurfaces {
    pub fn toggle_start(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup) {
            if start_toggle_animates(self.start_motion) {
                // Animated close: `hide` keeps the grab/focus bookkeeping and plays the motion.
                // The task menu and media are not open here (`transient` names only Start), so
                // unmapping them would only repeat a present on unrelated surfaces.
                runtime.hide(self.start).map_err(ui_error)?;
                self.transient = None;
                self.transient_output = None;
                return Ok(());
            }
            return self.close_transients(runtime);
        }
        // A closing Start overlay is reversed by `show`, so it must not be cut short here.
        if start_toggle_animates(self.start_motion) {
            for surface in [self.task_menu, self.media] {
                runtime.close_surface(surface).map_err(ui_error)?;
            }
        } else {
            self.close_transients(runtime)?;
        }
        self.open_start_group_measured(runtime)
    }

    /// Single measured unified Start show via the shared popup
    /// transaction (`runtime::start_surface`). Never mapped at 0,0:
    /// missing layout/anchor refuses via `PopupError`.
    pub(super) fn open_start_group_measured(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), String> {
        let output = self.selected().placement.output.id.clone();
        self.open_start_group_on(runtime, &output)
    }

    pub(super) fn open_start_group_on(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("start output {output} is unavailable"))?;
        runtime
            .apply_output_scale(
                self.start,
                panel_ui_scale(panel.placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        start_surface::open(
            runtime,
            panel.panel,
            self.start,
            panel.placement.work_area,
            panel.placement.panel.edge,
        )?;
        self.transient = Some(Transient::StartGroup);
        self.transient_output = Some(output.clone());
        Ok(())
    }

    /// outside release, Escape, or launch closes the unified surface, and
    /// the grab is released exactly once through `close_transients`.
    /// Already-open category switches resize transactionally: the surface
    /// is re-measured and re-prepared before re-map, so no 0,0 or
    /// half-resized frame is ever visible.
    pub fn open_start_group(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup) {
            return self.resize_start_group_measured(runtime);
        }
        self.close_transients(runtime)?;
        self.open_start_group_measured(runtime)
    }

    pub fn open_start_group_for_output(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        if self.transient == Some(Transient::StartGroup)
            && self.transient_output.as_ref() == Some(output)
        {
            return self.resize_start_group_for_output(runtime, output);
        }
        if self.transient.is_some() {
            self.close_transients(runtime)?;
        }
        self.open_start_group_on(runtime, output)
    }

    pub(super) fn resize_start_group_for_output(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("start output {output} is unavailable"))?;
        runtime
            .apply_output_scale(
                self.start,
                panel_ui_scale(panel.placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        let rect = start_surface::refit(
            runtime,
            panel.panel,
            self.start,
            panel.placement.work_area,
            panel.placement.panel.edge,
            POPUP_GAP,
        )
        .map_err(|error| error.to_string())?;
        self.map_raise(runtime, self.start, rect)
    }

    /// Transactional resize of the already-open unified Start surface
    /// via the shared `start_surface::refit` (anchor -> measure -> fit);
    /// prepared before re-map without dropping the group grab.
    pub(super) fn resize_start_group_measured(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), String> {
        let output = self.selected().placement.output.id.clone();
        self.resize_start_group_for_output(runtime, &output)
    }

    #[must_use]
    pub fn is_start_group_open(&self) -> bool {
        self.transient == Some(Transient::StartGroup)
    }

    #[must_use]
    pub fn is_media_open(&self) -> bool {
        self.transient == Some(Transient::Media)
    }

    pub fn open_status(&mut self, runtime: &mut SurfaceRuntime, name: &str) -> Result<(), String> {
        // J07: media stays in-process; audio/network/clock delegate to the
        // helper via the supervisor (caller path in main.rs). Direct
        // in-process opens of helper-owned popups are refused here so the
        // parent never regrows native ownership.
        if matches!(name, "volume" | "audio" | "network" | "clock") {
            return Err(format!("quick-control {name} owned by helper process"));
        }
        // Live path: stable panel node ids, edge from the panel edge, Align
        // End; no fixed popup_rect. Single measured show via the shared
        // `status_surface` transaction (see open_status_anchored_id).
        self.open_status_anchored_id(runtime, name, status_surface::spec().source_node_id)
    }

    pub fn open_status_for_output(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
        output: &flamewm_api::OutputId,
    ) -> Result<(), String> {
        if name != "media" {
            return Err(format!("unsupported shell popup {name}"));
        }
        let (panel_handle, scale_percent, work_area, edge) = self
            .for_output(output)
            .map(|panel| {
                (
                    panel.panel,
                    panel.placement.output.shell_scale_percent,
                    panel.placement.work_area,
                    panel.placement.panel.edge,
                )
            })
            .ok_or_else(|| format!("popup output {output} is unavailable"))?;
        self.close_transients(runtime)?;
        runtime
            .set_slide(self.media, self.popup_slide)
            .map_err(ui_error)?;
        runtime
            .apply_output_scale(self.media, panel_ui_scale(scale_percent))
            .map_err(ui_error)?;
        status_surface::open(runtime, panel_handle, self.media, work_area, edge)?;
        self.transient = Some(Transient::Media);
        self.transient_output = Some(output.clone());
        Ok(())
    }

    /// Anchored open from a stable panel node id (`tray-media`,
    /// `tray-volume`, `tray-network`, `clock-button`).
    /// Single measured show via the shared `status_surface` transaction.
    /// Refuses `PendingLayout`/`PendingAnchor`; never maps at 0,0.
    pub fn open_status_anchored_id(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
        source_id: &str,
    ) -> Result<(), String> {
        let (surface, transient) = match name {
            "media" => (self.media, Transient::Media),
            _ => return Err(format!("unsupported shell popup {name}")),
        };
        // Contract: this entry resolves the source id from the role spec;
        // the id path keeps accepting the legacy caller-supplied id only
        // when it matches the role's canonical source node.
        let spec = status_surface::spec();
        if source_id != spec.source_node_id {
            return Err(format!("unsupported shell popup source {source_id}"));
        }
        self.close_transients(runtime)?;
        runtime
            .set_slide(surface, self.popup_slide)
            .map_err(ui_error)?;
        runtime
            .apply_output_scale(
                surface,
                panel_ui_scale(self.selected().placement.output.shell_scale_percent),
            )
            .map_err(ui_error)?;
        status_surface::open(
            runtime,
            self.selected().panel,
            surface,
            self.selected().placement.work_area,
            self.selected().placement.panel.edge,
        )?;
        self.transient = Some(transient);
        self.transient_output = Some(self.selected().placement.output.id.clone());
        Ok(())
    }

    pub fn open_context_menu(
        &mut self,
        runtime: &mut SurfaceRuntime,
        state: &ContextMenuState,
        snapshot: &ShellSnapshot,
        backdrop: (u8, u8),
    ) -> Result<(), String> {
        let output = state
            .output
            .as_ref()
            .ok_or_else(|| "context menu has no source output".to_owned())?;
        if self.for_output(output).is_none() {
            return Err(format!("context menu output {output} is unavailable"));
        }
        self.select_panel_output(output);
        self.close_transients(runtime)?;
        // Shrink the native surface to its visible content rows so no
        // spare (black) area remains for the shape mask to cover.
        let rows = crate::taskbar::context_menu::visible_row_count_for_kind(
            &snapshot.panels,
            &snapshot.windows,
            snapshot.workspaces.as_ref(),
            &state.kind,
        );
        let panel = self
            .for_output(output)
            .ok_or_else(|| format!("context menu output {output} is unavailable"))?;
        let percent = panel.placement.output.shell_scale_percent;
        let logical = context_menu_size(rows);
        let size = flamewm_api::Size::new(
            flamewm_platform::panel::logical_to_physical(logical.width.max(0) as u16, percent),
            flamewm_platform::panel::logical_to_physical(logical.height.max(0) as u16, percent),
        );
        runtime
            .apply_output_scale(self.task_menu, panel_ui_scale(percent))
            .map_err(ui_error)?;
        // A12-1: clamp inside the panel-aware work area so a menu opened near
        // the panel stops above the panel strip instead of under it.
        let rect = context_menu_rect_for_placement(&panel.placement, state.anchor, size);
        runtime
            .set_popup_backdrop(self.task_menu, Some(backdrop))
            .map_err(ui_error)?;
        self.show_grabbed(runtime, self.task_menu, rect, Transient::ContextMenu)
    }

    /// Retained-device anchor for a quick-control source id, with the
    /// snapshot-geometry fallback used only pre-map. Mirrors the
    /// in-process contract without regrowing native ownership.
    pub fn quick_anchor(
        &self,
        runtime: &SurfaceRuntime,
        fallback: &flamewm_api::Rect,
        source_id: &str,
    ) -> flamewm_api::Rect {
        crate::runtime::retained_source_rect(runtime, self.selected().panel, source_id)
            .unwrap_or(*fallback)
    }

    pub fn quick_anchor_for_output(
        &self,
        runtime: &SurfaceRuntime,
        output: &flamewm_api::OutputId,
        fallback: &flamewm_api::Rect,
        source_id: &str,
    ) -> Option<flamewm_api::Rect> {
        let panel = self.for_output(output)?;
        Some(
            crate::runtime::retained_source_rect(runtime, panel.panel, source_id)
                .unwrap_or(*fallback),
        )
    }
}
