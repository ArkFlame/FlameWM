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
    #[must_use]
    pub fn manager(&self) -> &PanelManager {
        &self.manager
    }

    #[must_use]
    pub fn work_area_view(&self) -> Vec<(OutputId, flamewm_api::Rect, flamewm_api::Rect, bool)> {
        self.manager.panels().map(|panel| (
            panel.output.clone(), panel.output_rect, panel.work_area(),
            self.manager.primary_output() == Some(&panel.output),
        )).collect()
    }

    #[must_use]
    pub fn tasks(&self) -> &TaskStrip {
        &self.tasks
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.max(1)
    }

    #[must_use]
    pub fn start_output(&self) -> Option<&OutputId> {
        self.start.open_output()
    }

    pub fn sync_outputs(&mut self, displays: &DisplaySnapshot) -> bool {
        let before_manager = self.manager.clone();
        let connected = displays
            .outputs
            .iter()
            .filter(|output| output.connected)
            .collect::<Vec<_>>();
        let desired_ids = connected
            .iter()
            .map(|output| output.id.clone())
            .collect::<BTreeSet<_>>();

        for output in self.manager.outputs() {
            if !desired_ids.contains(&output) {
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
        self.manager.set_primary_output(primary.clone());

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
        let before = self.manager.clone();
        for output in self.manager.outputs() {
            if let Some(panel) = self.manager.panel_mut(&output) {
                panel.set_logical_size(logical_size);
            }
        }
        let changed = self.manager != before;
        if changed {
            self.bump_revision();
        }
        changed
    }

    /// Reconcile every live output with the persisted taskbar style, margin and radius.
    pub fn reconcile_taskbar_frame(&mut self, frame: TaskbarFrame) -> bool {
        let before = self.manager.clone();
        for output in self.manager.outputs() {
            if let Some(panel) = self.manager.panel_mut(&output) {
                panel.frame = frame;
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

    pub fn reorder(&mut self, from: usize, to: usize, expected_revision: u64) -> FlameResult<bool> {
        self.mutate_tasks(expected_revision, |tasks| tasks.reorder_on_release(from, to))
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
        self.mutate_tasks(expected_revision, |tasks| tasks.reorder_on_release(from, to))
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
}
