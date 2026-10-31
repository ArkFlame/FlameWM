use super::*;

impl ShellSurfaces {
    pub fn create(runtime: &mut SurfaceRuntime, snapshot: &ShellSnapshot) -> Result<Self, String> {
        let first = snapshot.panels.panels.first().cloned().ok_or_else(|| {
            let outputs = snapshot
                .displays
                .outputs
                .iter()
                .map(|output| {
                    format!(
                        "{} connected={} mode={} geometry={:?}",
                        output.id.as_str(),
                        output.connected,
                        output.current_mode.0,
                        output.geometry,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("GetPanels returned no panel (displays: [{outputs}])")
        })?;
        let contexts = panel_contexts_for_snapshot(snapshot, first.edge);
        let primary_placement = contexts
            .iter()
            .find(|context| context.panel.output == first.output)
            .cloned()
            .ok_or_else(|| "GetDisplays returned no connected panel output".to_owned())?;
        let metrics = ShellMetrics::default();
        let primary_panel = create_panel_surface(runtime, &primary_placement)?;
        let output = primary_placement.output.clone();
        let panel_snapshot = primary_placement.panel.clone();
        let start = create_surface(
            runtime,
            START_ARTIFACT,
            popup_config(
                "FlameWM start",
                SurfaceRole::PopupMenu,
                primary_placement.start_layout.popover,
            ),
        )?;
        let task_menu = create_surface(
            runtime,
            TASK_MENU_ARTIFACT,
            popup_config(
                "FlameWM task menu",
                SurfaceRole::DropdownMenu,
                flamewm_api::Rect::new(output.geometry.x, output.geometry.y, 217, 180),
            ),
        )?;
        let media = create_surface(
            runtime,
            MEDIA_ARTIFACT,
            status_popup_config(
                crate::quick_controls::host::MEDIA_POPUP_TITLE,
                flamewm_shell_core::MEDIA_POPOVER_SIZE,
                panel_anchor(&panel_snapshot, &output, metrics, 1),
            ),
        )?;
        let launcher = create_surface(
            runtime,
            LAUNCHER_ARTIFACT,
            popup_config(
                "FlameWM launcher",
                SurfaceRole::PopupMenu,
                flamewm_api::Rect::new(output.geometry.x, output.geometry.y, 600, 360),
            ),
        )?;
        let mut panels = vec![PanelSurfaceRecord {
            panel: primary_panel,
            placement: primary_placement.clone(),
            backdrop: None,
        }];
        for context in contexts {
            if context.output.id == primary_placement.output.id {
                continue;
            }
            let panel = create_panel_surface(runtime, &context)?;
            panels.push(PanelSurfaceRecord {
                panel,
                placement: context,
                backdrop: None,
            });
        }
        Ok(Self {
            start,
            task_menu,
            media,
            launcher,
            launcher_output: None,
            panels,
            selected_output: None,
            transient_output: None,
            transient: None,
            start_motion: false,
            popup_slide: None,
            native_verified: std::cell::Cell::new(None),
        })
    }

    /// Proven close branch: single-turn close per surface (ungrab-once
    /// when grabbed + unmap + Expose-present, same turn, no sleeps, no
    /// compositor), then clear the open marker. Never touches panel or
    /// mapped popup geometry.
    /// Dismiss of every transient: a configured slide slides out through `hide`; otherwise
    /// each surface unmaps at once. Reopen paths use it too, so a closing overlay is reversed
    /// by the following `show`.
    pub fn close_transients(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for surface in [self.start, self.task_menu, self.media] {
            let animated = dismiss_animates(
                surface == self.start,
                self.start_motion,
                self.popup_slide.is_some(),
            );
            if animated {
                runtime.hide(surface).map_err(ui_error)?;
            } else {
                runtime.close_surface(surface).map_err(ui_error)?;
            }
        }
        self.transient = None;
        self.transient_output = None;
        Ok(())
    }

    /// Sets whether Start toggles animate out (a Start slide, fade, or lamp is configured).
    pub fn set_start_motion(&mut self, configured: bool) {
        self.start_motion = configured;
    }

    /// Sets the slide for task menu, media, and status popups on their next open. `None`
    /// keeps the immediate path. Start keeps its own slide from `set_start_motion`.
    pub fn set_popup_slide(&mut self, slide: Option<SlideConfig>) {
        self.popup_slide = slide;
    }

    pub(super) fn show_grabbed(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
        transient: Transient,
    ) -> Result<(), String> {
        self.map_raise_grab(runtime, surface, rect)?;
        self.transient = Some(transient);
        self.transient_output = Some(self.selected().placement.output.id.clone());
        Ok(())
    }

    pub(super) fn prepare_traced(
        &self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
        trace: &GeometryTrace,
    ) -> Result<(), String> {
        if rect.width <= 0 || rect.height <= 0 {
            return Err(PopupRefusal::PendingLayout.to_string());
        }
        runtime
            .move_resize_traced(
                surface,
                rect.x,
                rect.y,
                checked_size(rect.width)?,
                checked_size(rect.height)?,
                Some(*trace),
            )
            .map_err(ui_error)
    }

    /// Present step for the grabbed tail: canonical `present_mapped` path
    /// (show = `geometry/move_resize` origin-tracked + map/raise/present).
    /// No separate raise or redraw chains: `show` owns the single ordered
    /// present cycle, then the single group grab.
    pub(super) fn map_raise_grab(
        &mut self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
    ) -> Result<(), String> {
        self.prepare_traced(runtime, surface, rect, &GeometryTrace::begin("shell-popup"))?;
        runtime.show(surface).map_err(ui_error)?;
        runtime
            .grab_pointer(surface)
            .map_err(|_| PopupRefusal::PointerGrabRefused.to_string())
    }

    /// Present step without grab (Start root / remeasure tail): canonical
    /// present via `show` only.
    pub(super) fn map_raise(
        &self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        rect: flamewm_api::Rect,
    ) -> Result<(), String> {
        self.prepare_traced(runtime, surface, rect, &GeometryTrace::begin("shell-popup"))?;
        runtime.show(surface).map_err(ui_error)
    }
}
