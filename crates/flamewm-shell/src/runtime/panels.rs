use super::*;

impl ShellSurfaces {
    /// Publishes the dock frame on every panel surface: floating reserves the bar's edge and
    /// rounds its corners; classic passes `None` so the surface returns to legacy behaviour.
    pub fn apply_dock_frame(
        &self,
        runtime: &mut SurfaceRuntime,
        frame: TaskbarFrame,
    ) -> Result<(), String> {
        for record in &self.panels {
            let placement = &record.placement;
            let mut panel = OutputPanel::new(
                placement.output.id.clone(),
                placement.output.geometry,
                placement.panel.edge,
                placement.output.shell_scale_percent,
            );
            panel.set_logical_size(placement.panel.logical_size);
            panel.frame = frame;
            let edge = frame.floating.then_some(match placement.panel.edge {
                PanelEdge::Bottom => DockEdge::Bottom,
                PanelEdge::Top => DockEdge::Top,
                PanelEdge::Left => DockEdge::Left,
                PanelEdge::Right => DockEdge::Right,
            });
            let radius = if frame.floating {
                u32::try_from(panel.corner_radius()).unwrap_or(0)
            } else {
                0
            };
            // A binary XShape mask alone makes jagged corners: the painted taskbar
            // must also use the same physical radius through its scaled UI scene.
            let scale_percent = placement.output.shell_scale_percent;
            let logical_radius = radius as f32 / panel_ui_scale(scale_percent);
            runtime
                .with_document(record.panel, |document| {
                    document.corner_radius("taskbar", logical_radius)
                })
                .map_err(|error| format!("panel corner paint: {error:?}"))?;
            runtime
                .set_dock_frame(record.panel, edge, radius)
                .map_err(|error| format!("panel dock frame: {error:?}"))?;
        }
        Ok(())
    }

    pub(super) fn primary(&self) -> &PanelSurfaceRecord {
        // Invariant: `create` and `sync_panel_context` never leave `panels`
        // empty, and the primary record is kept at index 0.
        &self.panels[0]
    }

    pub(super) fn selected(&self) -> &PanelSurfaceRecord {
        self.selected_output
            .as_ref()
            .and_then(|output| {
                self.panels
                    .iter()
                    .find(|record| &record.placement.output.id == output)
            })
            .unwrap_or_else(|| self.primary())
    }

    pub(super) fn for_output(&self, output: &flamewm_api::OutputId) -> Option<&PanelSurfaceRecord> {
        self.panels
            .iter()
            .find(|record| &record.placement.output.id == output)
    }

    /// Set the output that owns the next popup transaction.
    pub fn select_panel_output(&mut self, output: &flamewm_api::OutputId) -> bool {
        if self
            .panels
            .iter()
            .any(|record| &record.placement.output.id == output)
        {
            self.selected_output = Some(output.clone());
            true
        } else {
            false
        }
    }

    /// Resolve panel input to its output. Events from the one shared transient
    /// keep the opener's output, so popup actions stay on the monitor that opened it.
    pub fn input_output(&self, surface: SurfaceHandle) -> Option<flamewm_api::OutputId> {
        self.panels
            .iter()
            .find(|record| record.panel == surface)
            .map(|record| record.placement.output.id.clone())
            .or_else(|| {
                (surface == self.launcher)
                    .then(|| self.launcher_output.clone())
                    .flatten()
            })
            .or_else(|| {
                let owns_popup = match self.transient {
                    Some(Transient::StartGroup) => surface == self.start,
                    Some(Transient::ContextMenu) => surface == self.task_menu,
                    Some(Transient::Media) => surface == self.media,
                    None => false,
                };
                owns_popup.then(|| self.transient_output.clone()).flatten()
            })
    }

    /// Selected panel surface for popup anchors and monitor-local presentation.
    #[must_use]
    pub fn selected_panel(&self) -> SurfaceHandle {
        self.selected().panel
    }

    /// Resolve the native panel surface for an output without changing the
    /// current input selection.
    #[must_use]
    pub fn panel_for_output(&self, output: &flamewm_api::OutputId) -> Option<SurfaceHandle> {
        self.panels
            .iter()
            .find(|record| &record.placement.output.id == output)
            .map(|record| record.panel)
    }

    /// Snapshot geometry of the selected panel, used only before native layout exists.
    #[must_use]
    pub fn selected_panel_rect(&self) -> flamewm_api::Rect {
        self.selected().placement.panel.geometry
    }

    /// Selected panel placement used by output-local popup transactions.
    #[must_use]
    pub fn selected_placement(&self) -> &PanelPlacementContext {
        &self.selected().placement
    }

    /// Native handle of the primary panel surface.
    #[must_use]
    pub fn panel(&self) -> SurfaceHandle {
        self.primary().panel
    }

    /// Every native panel surface handle, primary first.
    #[must_use]
    pub fn panel_handles(&self) -> Vec<SurfaceHandle> {
        self.panels.iter().map(|record| record.panel).collect()
    }

    /// Resolve the output that originated input on a shell surface.
    pub fn select_input_output(&self, surface: SurfaceHandle) -> Option<flamewm_api::OutputId> {
        self.input_output(surface)
    }

    /// Installs each panel's desktop-layer backdrop, or none when it is opaque. Separate mode
    /// installs the unblurred layer: the tint and blur live on each element capsule, so the gaps
    /// must not be blurred. An unchanged panel, source and material installs nothing.
    pub fn apply_panel_backdrop(
        &mut self,
        runtime: &mut SurfaceRuntime,
        source: Option<&DesktopSource>,
        opacity_percent: u8,
        blur_percent: u8,
        separate: bool,
    ) -> Result<(), String> {
        let install = separate || opacity_percent < 100;
        let (opacity, blur) = if separate {
            (100, 0)
        } else {
            (opacity_percent, blur_percent)
        };
        for record in &mut self.panels {
            let panel = record.panel;
            let device = runtime.surface_device_rect(panel).map_err(ui_error)?;
            let rect = (
                device.x as i32,
                device.y as i32,
                device.width as u32,
                device.height as u32,
            );
            let wanted = PanelBackdrop {
                rect,
                opacity,
                blur,
                source: source.filter(|_| install).cloned(),
            };
            if record.backdrop.as_ref() == Some(&wanted) {
                continue;
            }
            let words = wanted
                .source
                .as_ref()
                .filter(|_| rect.2 > 0 && rect.3 > 0)
                .and_then(|_| runtime.desktop_backdrop(rect, opacity, blur));
            crate::surface_backdrop::backdrop_trace(format_args!(
                "panel={panel:?} apply rect={rect:?} opacity={opacity} blur={blur} install={}",
                words.is_some(),
            ));
            match words {
                Some(words) => runtime
                    .set_surface_backdrop(panel, Some((&words, rect.2, rect.3)))
                    .map_err(ui_error)?,
                None => runtime
                    .set_surface_backdrop(panel, None)
                    .map_err(ui_error)?,
            }
            record.backdrop = Some(wanted);
        }
        Ok(())
    }

    /// Every panel record as (native handle, output id), primary first.
    #[must_use]
    pub fn panel_outputs(&self) -> Vec<(SurfaceHandle, flamewm_api::OutputId)> {
        self.panels
            .iter()
            .map(|record| (record.panel, record.placement.output.id.clone()))
            .collect()
    }

    pub fn placement_for_output(
        &self,
        output: &flamewm_api::OutputId,
    ) -> Option<&PanelPlacementContext> {
        self.for_output(output).map(|panel| &panel.placement)
    }

    /// Recompute the panel placement records from a fresh snapshot. Updates
    /// every retained output's record in place, creates a native panel
    /// surface for a newly connected panel output, and destroys records
    /// whose output disconnected. The primary record stays at index 0 and
    /// tracks the snapshot's first panel. Returns `Ok(false)` when no panel
    /// resolves (state untouched). On an edge switch (stored Top, snapshot
    /// Bottom) the stored edge misses, so the snapshot's own edge is used.
    pub fn sync_panel_context(
        &mut self,
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> Result<bool, String> {
        self.native_verified.set(None);
        // Each output's panel has its own authoritative edge and geometry.
        // Filtering against the previous primary edge silently discarded
        // panels after an output swap (or a mixed-edge arrangement).
        let contexts: Vec<_> = snapshot
            .panels
            .panels
            .iter()
            .filter_map(|panel| panel_context_for_panel(snapshot, panel))
            .collect();
        let Some(first) = contexts.first() else {
            return Ok(false);
        };
        let primary_output = first.output.id.clone();
        let existing: Vec<_> = self
            .panels
            .iter()
            .map(|record| (record.placement.output.id.clone(), record.placement.clone()))
            .collect();
        let plan = panel_reconciliation_plan(&existing, &contexts);

        // Stage all new panel surfaces hidden. A failure leaves the live
        // records and their placements untouched; staged handles are retired.
        let mut staged = Vec::new();
        for id in &plan.create {
            let Some(context) = contexts.iter().find(|context| &context.output.id == id) else {
                continue;
            };
            let mut hidden = context.clone();
            hidden.panel.visible = false;
            let panel = match create_panel_surface(runtime, &hidden) {
                Ok(panel) => panel,
                Err(error) => {
                    cleanup_staged_panels(runtime, &mut staged);
                    return Err(error);
                }
            };
            staged.push(PanelSurfaceRecord {
                panel,
                placement: context.clone(),
                backdrop: None,
            });
            if context.panel.visible {
                if let Err(error) = runtime.show(panel) {
                    cleanup_staged_panels(runtime, &mut staged);
                    return Err(ui_error(error));
                }
            }
        }

        // Retire stale native surfaces before publishing new placement
        // records. If one destroy fails, discard staged additions and leave
        // every still-live record available for the next topology retry.
        for id in &plan.remove {
            if let Some(index) = self
                .panels
                .iter()
                .position(|record| &record.placement.output.id == id)
            {
                let panel = self.panels[index].panel;
                if let Err(error) = runtime.destroy(panel) {
                    cleanup_staged_panels(runtime, &mut staged);
                    return Err(ui_error(error));
                }
                self.panels.remove(index);
            }
        }

        for id in &plan.update {
            let Some(context) = contexts.iter().find(|context| &context.output.id == id) else {
                continue;
            };
            if let Some(record) = self
                .panels
                .iter_mut()
                .find(|record| &record.placement.output.id == id)
            {
                record.placement = context.clone();
            }
        }
        self.panels.extend(staged);
        let retained: Vec<_> = contexts
            .iter()
            .map(|context| context.output.id.clone())
            .collect();
        if self
            .selected_output
            .as_ref()
            .is_some_and(|id| !retained.contains(id))
        {
            self.selected_output = None;
        }
        if self
            .transient_output
            .as_ref()
            .is_some_and(|id| !retained.contains(id))
        {
            self.transient_output = None;
        }
        if let Some(index) = self
            .panels
            .iter()
            .position(|record| record.placement.output.id == primary_output)
        {
            let record = self.panels.remove(index);
            self.panels.insert(0, record);
        }
        Ok(true)
    }

    /// Parent-side anchor inputs for a helper open: retained anchor +
    /// work area + panel edge. The parent computes, the helper places.
    #[must_use]
    pub fn work_area(&self) -> flamewm_api::Rect {
        self.selected().placement.work_area
    }

    /// F10: current context generation read by every Start/Quick open
    /// path in main.rs before projection. Proves the open uses the
    /// post-sync context, never a stale cached rect.
    #[must_use]
    pub fn placement_generation(&self) -> (u64, u64) {
        (
            self.selected().placement.display_generation,
            self.selected().placement.panels_revision,
        )
    }

    /// Whether an open popup anchored to `output` remains valid under the
    /// incoming snapshot, ignoring global revision counters and other outputs.
    #[must_use]
    pub fn popup_context_unchanged(
        &self,
        output: &flamewm_api::OutputId,
        snapshot: &ShellSnapshot,
    ) -> bool {
        let Some(current) = self
            .panels
            .iter()
            .find(|record| &record.placement.output.id == output)
            .map(|record| &record.placement)
        else {
            return false;
        };
        let Some(next) = snapshot
            .panels
            .panels
            .iter()
            .find(|panel| &panel.output == output)
            .and_then(|panel| panel_context_for_panel(snapshot, panel))
        else {
            return false;
        };
        popup_context_unchanged(current, &next)
    }

    /// F10: last-synced panel snapshot geometry.
    #[must_use]
    pub fn panel_geometry(&self) -> flamewm_api::Rect {
        self.primary().placement.panel.geometry
    }

    /// Detect changes in every output by stable ID, not the primary panel's
    /// position alone. `RandR` can change CRTC coordinates without a reliable
    /// panels revision/generation bump. This gate runs before every popup.
    #[must_use]
    pub fn panel_topology_drifted(&self, snapshot: &ShellSnapshot) -> bool {
        if self.panels.len() != snapshot.panels.panels.len() {
            return true;
        }
        if snapshot.panels.panels.first().map(|panel| &panel.output)
            != Some(&self.primary().placement.output.id)
        {
            return true;
        }
        self.panels
            .iter()
            .any(|record| !panel_context_is_current(&record.placement, snapshot))
    }

    /// F10: true when any panel output's shell scale differs from the scale
    /// last synced into its panel document. A scale change keeps the logical
    /// panel geometry, so geometry/generation alone cannot detect it.
    #[must_use]
    pub fn output_scale_drifted(&self, snapshot: &ShellSnapshot) -> bool {
        self.panels.iter().any(|record| {
            let id = &record.placement.output.id;
            snapshot
                .panels
                .panels
                .iter()
                .any(|panel| panel.output == *id)
                && snapshot
                    .displays
                    .outputs
                    .iter()
                    .find(|output| output.id == *id && output.connected)
                    .is_some_and(|output| {
                        output.shell_scale_percent != record.placement.output.shell_scale_percent
                    })
        })
    }

    /// F10: edge/geometry consistency gate for every panel record.
    /// Refusal strings are returned to callers, which refuse popup open on `Err`.
    pub fn check_edge_consistent(&self) -> Result<(), String> {
        for record in &self.panels {
            let panel = record.placement.panel.geometry;
            let output = record.placement.output.geometry;
            let edge = record.placement.panel.edge;
            let max_inset = (FLOATING_MARGIN_MAX_LOGICAL
                * i32::from(record.placement.output.shell_scale_percent)
                + 99)
                / 100;
            let consistent = edge_geometry_consistent(edge, panel, output, max_inset);
            debug_assert!(
                consistent,
                "panel edge {edge:?} inconsistent: panel={panel:?} output={output:?}"
            );
            if !consistent {
                return Err(format!(
                    "panel edge {edge:?} inconsistent: panel={panel:?} output={output:?}"
                ));
            }
        }
        Ok(())
    }

    /// Reapplies each panel document UI scale from its synced output. The
    /// scale is otherwise set only at surface creation, so a live scale
    /// change left the retained bar laid out at the old scale. Setting a
    /// changed scale bumps the document revision, forcing a layout recompute
    /// on the next redraw.
    pub fn sync_panel_ui_scale(&self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        for record in &self.panels {
            runtime
                .apply_output_scale(
                    record.panel,
                    panel_ui_scale(record.placement.output.shell_scale_percent),
                )
                .map(|_| ())
                .map_err(|error| format!("panel ui scale: {error:?}"))?;
        }
        Ok(())
    }

    /// The retained placement record can be current while the X11 surface is
    /// stale (for example if a previous `ConfigureWindow` failed). Compare the
    /// actual native rectangles for *every* output, not just snapshot revisions.
    /// `key` is the (display generation, panels revision) under which the
    /// result is cached: a clean result is not re-probed until it changes, and
    /// a drifted result is re-probed every tick until a re-sync repairs it.
    // shortcut: an external move of a panel surface is not observed here; only
    // RandR (display generation) and our own re-sync invalidate the cache.
    // revisit when ConfigureNotify for panel surfaces is routed into the shell.
    #[must_use]
    pub fn panel_native_geometry_drifted(&self, runtime: &SurfaceRuntime, key: (u64, u64)) -> bool {
        native_geometry_drifted(
            &self.native_verified,
            key,
            self.panels
                .iter()
                .map(|record| (record.panel, record.placement.panel.geometry)),
            |panel| {
                runtime.surface_device_rect(panel).ok().map(|rect| {
                    flamewm_api::Rect::new(
                        rect.x as i32,
                        rect.y as i32,
                        rect.width as i32,
                        rect.height as i32,
                    )
                })
            },
        )
    }

    /// F10: `move_resize` every panel native surface to its synced snapshot
    /// geometry. Returns `true` when at least one `move_resize` was issued.
    pub fn move_panel_to_synced(&self, runtime: &mut SurfaceRuntime) -> Result<bool, String> {
        let mut moved = false;
        for record in &self.panels {
            let target = record.placement.panel.geometry;
            let current = runtime
                .surface_device_rect(record.panel)
                .map(|rect| {
                    flamewm_api::Rect::new(
                        rect.x as i32,
                        rect.y as i32,
                        rect.width as i32,
                        rect.height as i32,
                    )
                })
                .unwrap_or(target);
            if current == target {
                continue;
            }
            let width = checked_size(target.width)?;
            let height = checked_size(target.height)?;
            runtime
                .move_resize(record.panel, target.x, target.y, width, height)
                .map_err(ui_error)?;
            moved = true;
        }
        Ok(moved)
    }

    #[must_use]
    pub fn placement(&self) -> &PanelPlacementContext {
        &self.selected().placement
    }

    #[must_use]
    pub fn panel_edge(&self) -> flamewm_api::PanelEdge {
        self.selected().placement.panel.edge
    }

    /// Resolve a surface-local pointer position (`ActionEvent` x/y from the
    /// panel surface) to root coordinates via the retained panel device
    /// rect. Falls back to the snapshot geometry only when no retained
    /// layout exists (pre-map).
    #[must_use]
    pub fn root_pointer(&self, runtime: &SurfaceRuntime, x: f32, y: f32) -> flamewm_api::Point {
        let origin = runtime
            .surface_device_rect(self.selected().panel)
            .map(|rect| (rect.x as i32, rect.y as i32))
            .unwrap_or((
                self.selected().placement.panel.geometry.x,
                self.selected().placement.panel.geometry.y,
            ));
        flamewm_api::Point::new(origin.0 + x as i32, origin.1 + y as i32)
    }

    pub fn root_pointer_for_output(
        &self,
        runtime: &SurfaceRuntime,
        output: &flamewm_api::OutputId,
        x: f32,
        y: f32,
    ) -> Option<flamewm_api::Point> {
        let panel = self.for_output(output)?;
        let origin = runtime
            .surface_device_rect(panel.panel)
            .map(|rect| (rect.x as i32, rect.y as i32))
            .unwrap_or((
                panel.placement.panel.geometry.x,
                panel.placement.panel.geometry.y,
            ));
        Some(flamewm_api::Point::new(
            origin.0 + x as i32,
            origin.1 + y as i32,
        ))
    }
}
