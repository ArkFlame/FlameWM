//! Bus signal publication: system, window, workspace, and panel snapshots.

use super::*;

/// Returns the system snapshot to announce when it differs from the last one published.
///
/// The compositor reads every system domain through one snapshot, so this comparison keeps a
/// quiet turn silent while still announcing a changed backlight or audio endpoint.
pub(super) fn system_change(
    published: &mut Option<SystemSnapshot>,
    snapshot: SystemSnapshot,
) -> Option<SystemSnapshot> {
    if published.as_ref() == Some(&snapshot) {
        return None;
    }
    *published = Some(snapshot.clone());
    Some(snapshot)
}

impl Service {
    /// Emits `SystemSnapshotChanged` when the host's system state moved since the last signal.
    ///
    /// The host borrow is released before the signal is emitted, so a D-Bus callback cannot see a
    /// half-updated host through a re-entrant read.
    pub(super) fn publish_system(&self) {
        let snapshot = self.host.borrow().system_snapshot();
        let Some(snapshot) = system_change(&mut self.published_system.borrow_mut(), snapshot)
        else {
            return;
        };
        if let Err(error) = self
            .server
            .emit_signal(&ControlSignal::SystemSnapshotChanged {
                snapshot: snapshot.clone(),
            })
        {
            eprintln!("flamewm-wayland: system signal failed: {error}");
        }
        if let Err(error) = self.server.emit_signal(&ControlSignal::SystemChanged {
            revision: snapshot.revision,
        }) {
            eprintln!("flamewm-wayland: system revision signal failed: {error}");
        }
    }

    pub(super) fn publish_changes(&self) {
        let windows = self.mirror.borrow().windows.clone();
        if *self.published.borrow() != windows {
            let revision = windows
                .iter()
                .map(|window| window.state_generation)
                .max()
                .unwrap_or(0);
            (*self.published.borrow_mut()).clone_from(&windows);
            if let Err(error) = self
                .server
                .emit_signal(&ControlSignal::WindowsSnapshotChanged { revision, windows })
            {
                eprintln!("flamewm-wayland: windows signal failed: {error}");
            }
            self.publish_panels();
        }
        self.publish_workspaces();
    }

    /// Announces the workspace topology/active pair the way the X11 runtime does.
    ///
    /// Without this the pager keeps the snapshot it read at bootstrap: clicking a workspace queues a
    /// switch, the compositor switches, and no client is ever told, so the click looks like a no-op.
    /// `last_index` is `None` because the compositor has no "previous workspace" source of its own.
    pub(super) fn publish_workspaces(&self) {
        let (revision, count, active_index) = {
            let mirror = self.mirror.borrow();
            (
                mirror.workspace_revision,
                mirror.workspace_count,
                mirror.active_workspace,
            )
        };
        let snapshot = WorkspaceSnapshot {
            revision,
            count,
            active_index,
            last_index: None,
            names: (1..=count).map(|index| index.to_string()).collect(),
        };
        if self.published_workspaces.borrow().as_ref() == Some(&snapshot) {
            return;
        }
        *self.published_workspaces.borrow_mut() = Some(snapshot.clone());
        for signal in [
            ControlSignal::WorkspacesSnapshotChanged {
                snapshot: snapshot.clone(),
            },
            ControlSignal::WorkspacesChanged { revision },
        ] {
            if let Err(error) = self.server.emit_signal(&signal) {
                eprintln!("flamewm-wayland: workspace signal failed: {error}");
            }
        }
        // The task strip shows the active workspace's windows, so it moves with the pager.
        self.publish_panels();
    }

    /// The taskbar task strip is reconciled from the platform window cache. The X11 runtime refreshes
    /// that cache every loop turn; here it is refreshed only when the windows change, and the panels
    /// snapshot is published when the strip moved (otherwise windows opened later have no task button).
    pub(super) fn publish_panels(&self) {
        let panels = {
            let mut host = self.host.borrow_mut();
            let before = host.panels_snapshot();
            if let Err(error) = host.refresh_windows() {
                eprintln!("flamewm-wayland: window refresh failed: {error}");
                return;
            }
            let after = host.panels_snapshot();
            (before != after).then_some(after)
        };
        if let Some(snapshot) = panels {
            if let Err(error) = self
                .server
                .emit_signal(&ControlSignal::PanelsSnapshotChanged { snapshot })
            {
                eprintln!("flamewm-wayland: panels signal failed: {error}");
            }
        }
    }
}
