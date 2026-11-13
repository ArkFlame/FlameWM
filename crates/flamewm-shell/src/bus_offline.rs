//! Degraded shell state for a session without a reachable control bus.
//!
//! One synthetic output covers the root window so the platform panel service can lay out the
//! taskbar. Every bus-owned domain (windows, workspaces, applications, system) stays empty until a
//! bootstrap attaches later.

use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
use flamewm_platform::services::panels::PanelService;

use crate::runtime::ShellSnapshot;

/// Snapshot for `root` (root window size in pixels) with one output and its panels.
#[must_use]
pub fn offline_snapshot(root: (u32, u32)) -> ShellSnapshot {
    let mut snapshot = ShellSnapshot {
        displays: DisplaySnapshot {
            generation: 0,
            outputs: vec![OutputSnapshot {
                id: flamewm_api::OutputId::new("root"),
                connector: "root".to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: flamewm_api::Rect::new(
                    0,
                    0,
                    i32::try_from(root.0).unwrap_or(i32::MAX),
                    i32::try_from(root.1).unwrap_or(i32::MAX),
                ),
                current_mode: flamewm_api::ModeId(0),
                modes: Vec::new(),
                shell_scale_percent: 100,

                rotation: flamewm_api::display::Rotation::Normal,
            }],
            pending: None,
        },
        ..ShellSnapshot::default()
    };
    let mut panels = PanelService::default();
    let _ = panels.sync_outputs(&snapshot.displays);
    snapshot.panels = panels.snapshot();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::offline_snapshot;

    #[test]
    fn offline_snapshot_has_one_connected_output_with_a_panel() {
        let snapshot = offline_snapshot((1280, 800));
        assert_eq!(snapshot.displays.outputs.len(), 1);
        assert!(snapshot.displays.outputs[0].connected);
        assert!(
            !snapshot.panels.panels.is_empty(),
            "taskbar panel must exist without a bus"
        );
        assert!(snapshot.workspaces.is_none());
        assert!(snapshot.applications.is_empty());
    }

    #[test]
    fn offline_snapshot_without_root_geometry_creates_no_panel() {
        let snapshot = offline_snapshot((0, 0));
        assert!(snapshot.panels.panels.is_empty());
    }
}
