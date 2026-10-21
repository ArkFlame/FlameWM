use std::collections::BTreeSet;

use flamewm_api::display::DisplaySnapshot;
use flamewm_api::panels::{
    PanelSnapshot, PanelsSnapshot, TaskEntry as ApiTaskEntry, TaskEntryKind as ApiTaskEntryKind,
};
use flamewm_api::window::WindowSnapshot;
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, OutputId, PanelEdge, TaskEntryId,
};

use crate::panel::{OutputPanel, PanelManager, StartController, TaskbarFrame};
use crate::task::{TaskEntryKind, TaskStrip};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PanelService {
    manager: PanelManager,
    start: StartController,
    tasks: TaskStrip,
    revision: u64,
}

impl PanelService {
    /// Read-only: output currently holding the pointer, if known. Not yet consumed by the shell.
    #[must_use]
    pub fn pointer_output(&self) -> Option<&OutputId> {
        self.manager.pointer_output()
    }

    pub fn manager(&self) -> &PanelManager {
        &self.manager
    }

    #[must_use]
    pub fn work_area_view(&self) -> Vec<(OutputId, flamewm_api::Rect, flamewm_api::Rect, bool)> {
        self.manager
            .panels()
            .map(|panel| {
                (
                    panel.output.clone(),
                    panel.output_rect,
                    panel.work_area(),
                    self.manager.primary_output() == Some(&panel.output),
                )
            })
            .collect()
    }

    #[must_use]
    pub fn tasks(&self) -> &TaskStrip {
        &self.tasks
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.max(1)
    }

    pub fn sync_outputs(&mut self, displays: &DisplaySnapshot) -> bool {
        let before_manager = self.manager.clone();
        let connected = displays
            .outputs
            .iter()
            .filter(|output| output.connected && output.geometry.is_valid())
            .collect::<Vec<_>>();
        let desired_ids = connected
            .iter()
            .map(|output| &output.id)
            .collect::<BTreeSet<_>>();

        for output in self.manager.outputs() {
            if !desired_ids.contains(&&output) {
                self.manager.remove_output(&output);
                self.start.output_removed(&output);
            }
        }
        for output in &connected {
            let (edge, logical_size, frame) = self
                .manager
                .panel(&output.id)
                .map_or((PanelEdge::Bottom, 44, TaskbarFrame::default()), |panel| {
                    (panel.edge, panel.logical_size, panel.frame)
                });
            let mut panel = OutputPanel::new(
                output.id.clone(),
                output.geometry,
                edge,
                output.shell_scale_percent,
            );
            panel.set_logical_size(logical_size);
            panel.frame = frame;
            self.manager.upsert_output(panel);
        }

        let primary = connected
            .iter()
            .find(|output| output.primary)
            .or_else(|| connected.first())
            .map(|output| output.id.clone());
        self.manager.set_primary_output(primary);

        let changed = self.manager != before_manager;
        if changed {
            self.bump_revision();
        }
        changed
    }

    pub fn set_edge(
        &mut self,
        output: &OutputId,
        edge: PanelEdge,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.require_revision(expected_revision)?;
        let panel = self
            .manager
            .panel_mut(output)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "panel output not found"))?;
        if panel.edge == edge {
            return Ok(());
        }
        panel.edge = edge;
        self.bump_revision();
        Ok(())
    }

    pub fn set_size(
        &mut self,
        output: &OutputId,
        logical_size: u16,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.require_revision(expected_revision)?;
        let panel = self
            .manager
            .panel_mut(output)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "panel output not found"))?;
        if !(34..=72).contains(&logical_size) {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "panel size must be 34..72",
            ));
        }
        if !panel.set_logical_size(logical_size) {
            return Ok(());
        }
        self.bump_revision();
        Ok(())
    }

    /// Reconcile every live output with the persisted taskbar height owned by
    /// the settings service. Panel geometry and its revision remain owned here.
    pub fn reconcile_taskbar_height(&mut self, logical_size: u16) -> bool {
        self.reconcile_panels(|panel| {
            panel.set_logical_size(logical_size);
        })
    }

    /// Reconcile every live output with the persisted taskbar style, margin and radius.
    pub fn reconcile_taskbar_frame(&mut self, frame: TaskbarFrame) -> bool {
        self.reconcile_panels(|panel| panel.frame = frame)
    }

    fn reconcile_panels(&mut self, mut apply: impl FnMut(&mut OutputPanel)) -> bool {
        let before = self.manager.clone();
        for output in self.manager.outputs() {
            if let Some(panel) = self.manager.panel_mut(&output) {
                apply(panel);
            }
        }
        let changed = self.manager != before;
        if changed {
            self.bump_revision();
        }
        changed
    }

    pub fn toggle_start(&mut self) -> bool {
        self.start.toggle(self.manager.active_output())
    }

    pub fn close_start(&mut self) {
        self.start.close();
    }

    pub fn reconcile_windows(&mut self, windows: &[WindowSnapshot]) -> bool {
        let live = windows
            .iter()
            .map(|window| (window.app_id.clone(), window.reference))
            .collect::<Vec<_>>();
        let changed = self.tasks.reconcile_live(&live);
        if changed {
            self.bump_revision();
        }
        changed
    }

    pub fn pin_app(&mut self, app: DesktopAppId, expected_revision: u64) -> FlameResult<bool> {
        self.mutate_tasks(expected_revision, |tasks| tasks.pin_app(app))
    }

    pub fn unpin_app(&mut self, app: &DesktopAppId, expected_revision: u64) -> FlameResult<bool> {
        self.mutate_tasks(expected_revision, |tasks| tasks.unpin_app(app))
    }

    pub fn reorder_entry(
        &mut self,
        entry_id: &TaskEntryId,
        to: usize,
        expected_revision: u64,
    ) -> FlameResult<bool> {
        self.require_revision(expected_revision)?;
        let Some(from) = self
            .tasks
            .current_ids()
            .iter()
            .position(|id| id == entry_id)
        else {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "task entry no longer exists",
            ));
        };
        if to >= self.tasks.entries().len() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "task insertion index is invalid",
            ));
        }
        self.mutate_tasks(expected_revision, |tasks| {
            tasks.reorder_on_release(from, to)
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> PanelsSnapshot {
        let panels = self
            .manager
            .panels()
            .map(|panel| PanelSnapshot {
                output: panel.output.clone(),
                edge: panel.edge,
                logical_size: panel.logical_size,
                visible: true,
                geometry: panel.panel_rect(),
            })
            .collect();
        let tasks = self
            .tasks
            .entries()
            .iter()
            .enumerate()
            .map(|(order_index, entry)| ApiTaskEntry {
                id: entry.id(),
                app_id: entry.app.clone(),
                kind: match entry.kind {
                    TaskEntryKind::Pinned { window } => ApiTaskEntryKind::PinnedSlot { window },
                    TaskEntryKind::Window { window } => ApiTaskEntryKind::Window { window },
                },
                order_index,
            })
            .collect();
        PanelsSnapshot {
            revision: self.revision(),
            panels,
            tasks,
            pinned_apps: self.tasks.pinned_apps(),
        }
    }

    fn require_revision(&self, expected: u64) -> FlameResult<()> {
        if expected == self.revision() {
            Ok(())
        } else {
            Err(FlameError::new(
                ErrorCode::StaleRevision,
                "panel revision is stale",
            ))
        }
    }

    /// Checks `expected_revision`, applies `mutate` to the task strip, and bumps the revision only when
    /// `mutate` reports a change.
    fn mutate_tasks(
        &mut self,
        expected_revision: u64,
        mutate: impl FnOnce(&mut TaskStrip) -> bool,
    ) -> FlameResult<bool> {
        self.require_revision(expected_revision)?;
        let changed = mutate(&mut self.tasks);
        if changed {
            self.bump_revision();
        }
        Ok(changed)
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision().saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
    use flamewm_api::{ModeId, OutputId, Rect};

    use super::PanelService;

    #[test]
    fn attached_but_disabled_crtc_cannot_create_phantom_panel() {
        let mut service = PanelService::default();
        let output = |name: &str, x: i32, w: i32, primary: bool| OutputSnapshot {
            id: OutputId::new(name),
            connector: name.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary,
            geometry: Rect::new(x, 0, w, if w == 0 { 0 } else { 1080 }),
            current_mode: ModeId(if w == 0 { 0 } else { 1 }),
            modes: Vec::new(),
            shell_scale_percent: 100,
        };
        let first = DisplaySnapshot {
            generation: 1,
            outputs: vec![output("DP-1", 0, 1920, true), output("HDMI-1", 0, 0, false)],
            pending: None,
        };
        assert!(service.sync_outputs(&first));
        assert_eq!(service.snapshot().panels.len(), 1);
        assert!(service.manager().panel(&OutputId::new("HDMI-1")).is_none());
        let second = DisplaySnapshot {
            generation: 2,
            outputs: vec![
                output("DP-1", 0, 1920, true),
                output("HDMI-1", 1920, 1920, false),
            ],
            pending: None,
        };
        assert!(service.sync_outputs(&second));
        assert_eq!(service.snapshot().panels.len(), 2);
        let third = DisplaySnapshot {
            generation: 3,
            outputs: vec![output("DP-1", 0, 1920, true), output("HDMI-1", 0, 0, false)],
            pending: None,
        };
        assert!(service.sync_outputs(&third));
        assert_eq!(service.snapshot().panels.len(), 1);
    }

    #[test]
    fn legacy_xephyr_root_without_randr_mode_still_has_a_panel() {
        // A nested X server can have a real drawable root but no mode ID.
        // Rejecting mode 0 here removed FlameWM's entire shell in Xephyr.
        let mut service = PanelService::default();
        assert!(service.sync_outputs(&DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("default"),
                connector: "default".to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: Rect::new(0, 0, 1350, 641),
                current_mode: ModeId(0),
                modes: Vec::new(),
                shell_scale_percent: 100,
            }],
            pending: None,
        }));
        let panel = &service.snapshot().panels[0];
        assert_eq!(panel.output, OutputId::new("default"));
        assert_eq!(panel.geometry, Rect::new(0, 597, 1350, 44));
    }

    #[test]
    fn disconnected_or_geometryless_modes_never_create_panels() {
        let mut service = PanelService::default();
        let output = |name: &str, connected: bool, geometry: Rect| OutputSnapshot {
            id: OutputId::new(name),
            connector: name.to_owned(),
            edid_identity: String::new(),
            connected,
            primary: false,
            geometry,
            current_mode: ModeId(0),
            modes: Vec::new(),
            shell_scale_percent: 100,
        };
        assert!(!service.sync_outputs(&DisplaySnapshot {
            generation: 1,
            outputs: vec![
                output("DP-1", true, Rect::new(0, 0, 0, 0)),
                output("HDMI-1", false, Rect::new(0, 0, 1920, 1080)),
            ],
            pending: None,
        }));
        assert!(service.snapshot().panels.is_empty());
    }

    #[test]
    fn persisted_taskbar_height_reconciles_panel_geometry_once() {
        let mut service = PanelService::default();
        service.sync_outputs(&DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("eDP-1"),
                connector: "eDP-1".to_owned(),
                edid_identity: "panel".to_owned(),
                connected: true,
                primary: true,
                geometry: Rect::new(0, 0, 1920, 1080),
                current_mode: ModeId(1),
                modes: Vec::new(),
                shell_scale_percent: 100,
            }],
            pending: None,
        });

        assert!(service.reconcile_taskbar_height(52));
        assert_eq!(service.snapshot().panels[0].logical_size, 52);
        assert_eq!(
            service.snapshot().panels[0].geometry,
            Rect::new(0, 1028, 1920, 52)
        );
        assert!(!service.reconcile_taskbar_height(52));
    }

    #[test]
    fn output_scale_converts_panel_thickness_once_and_is_stable_across_syncs() {
        for (scale, thickness) in [(100_u16, 44_i32), (125, 55), (150, 66)] {
            let displays = DisplaySnapshot {
                generation: 1,
                outputs: vec![OutputSnapshot {
                    id: OutputId::new("eDP-1"),
                    connector: "eDP-1".to_owned(),
                    edid_identity: "panel".to_owned(),
                    connected: true,
                    primary: true,
                    geometry: Rect::new(0, 0, 1920, 1080),
                    current_mode: ModeId(1),
                    modes: Vec::new(),
                    shell_scale_percent: scale,
                }],
                pending: None,
            };
            let mut service = PanelService::default();
            service.sync_outputs(&displays);
            service.sync_outputs(&displays);
            let output = OutputId::new("eDP-1");
            let panel = service.manager().panel(&output).expect("panel exists");
            assert_eq!(panel.thickness(), thickness, "scale {scale}");
            assert_eq!(
                panel.work_area(),
                Rect::new(0, 0, 1920, 1080 - thickness),
                "scale {scale}"
            );
        }
    }

    #[test]
    fn floating_frame_survives_display_sync_and_reserves_margin() {
        use crate::panel::TaskbarFrame;

        let displays = DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("eDP-1"),
                connector: "eDP-1".to_owned(),
                edid_identity: "panel".to_owned(),
                connected: true,
                primary: true,
                geometry: Rect::new(0, 0, 1920, 1080),
                current_mode: ModeId(1),
                modes: Vec::new(),
                shell_scale_percent: 100,
            }],
            pending: None,
        };
        let mut service = PanelService::default();
        service.sync_outputs(&displays);
        let floating = TaskbarFrame {
            floating: true,
            margin: 8,
            radius: 12,
        };
        assert!(service.reconcile_taskbar_frame(floating));
        assert!(!service.reconcile_taskbar_frame(floating));
        service.sync_outputs(&displays);
        let output = OutputId::new("eDP-1");
        let panel = service.manager().panel(&output).expect("panel exists");
        assert_eq!(panel.frame, floating);
        assert_eq!(panel.strut().bottom, 52);
        assert_eq!(panel.work_area(), Rect::new(0, 0, 1920, 1028));
        assert_eq!(panel.panel_rect(), Rect::new(8, 1028, 1904, 44));
    }
    #[test]
    fn unequal_output_swap_updates_every_panel_and_work_area() {
        use flamewm_api::PanelEdge;
        let make =
            |id: &str, x: i32, y: i32, width: i32, height: i32, primary: bool| OutputSnapshot {
                id: OutputId::new(id),
                connector: id.to_owned(),
                edid_identity: id.to_owned(),
                connected: true,
                primary,
                geometry: Rect::new(x, y, width, height),
                current_mode: ModeId(1),
                modes: Vec::new(),
                shell_scale_percent: 100,
            };
        let mut service = PanelService::default();
        let mut snapshot = DisplaySnapshot {
            generation: 1,
            pending: None,
            outputs: vec![
                make("DP-1", 0, 0, 1920, 1080, true),
                make("HDMI-1", 1920, 0, 2560, 1440, false),
                make("DP-2", 4480, -100, 1200, 1920, false),
            ],
        };
        assert!(service.sync_outputs(&snapshot));
        let before = service.revision();
        service
            .set_edge(&OutputId::new("DP-1"), PanelEdge::Left, before)
            .unwrap();
        service
            .set_edge(
                &OutputId::new("HDMI-1"),
                PanelEdge::Right,
                service.revision(),
            )
            .unwrap();
        service
            .set_edge(&OutputId::new("DP-2"), PanelEdge::Top, service.revision())
            .unwrap();
        snapshot.generation += 1;
        snapshot.outputs[0].geometry = Rect::new(2560, 0, 1920, 1080);
        snapshot.outputs[1].geometry = Rect::new(0, 0, 2560, 1440);
        snapshot.outputs[2].geometry = Rect::new(4480, -100, 1200, 1920);
        snapshot.outputs[0].primary = false;
        snapshot.outputs[1].primary = true;
        assert!(service.sync_outputs(&snapshot));
        let dp = service.manager().panel(&OutputId::new("DP-1")).unwrap();
        assert_eq!(dp.panel_rect(), Rect::new(2560, 0, 44, 1080));
        assert_eq!(dp.work_area(), Rect::new(2604, 0, 1876, 1080));
        let hdmi = service.manager().panel(&OutputId::new("HDMI-1")).unwrap();
        assert_eq!(hdmi.panel_rect(), Rect::new(2516, 0, 44, 1440));
        assert_eq!(hdmi.work_area(), Rect::new(0, 0, 2516, 1440));
        let portrait = service.manager().panel(&OutputId::new("DP-2")).unwrap();
        assert_eq!(portrait.panel_rect(), Rect::new(4480, -100, 1200, 44));
        assert_eq!(portrait.work_area(), Rect::new(4480, -56, 1200, 1876));
        assert_eq!(
            service.manager().primary_output(),
            Some(&OutputId::new("HDMI-1"))
        );
        assert_eq!(service.snapshot().panels.len(), 3);
        assert!(!service.sync_outputs(&snapshot));
    }
}
