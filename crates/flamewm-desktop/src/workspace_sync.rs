//! Desktop workspace snapshot authority and projection-boundary regressions.

use flamewm_api::workspace::WorkspaceSnapshot;

pub(crate) fn is_legacy_workspace_signal(signal: &flamewm_control_wire::ControlSignal) -> bool {
    matches!(signal, flamewm_control_wire::ControlSignal::WorkspacesChanged { .. })
}

#[cfg(test)]
#[path = "settings_sync.rs"]
mod settings_sync;

#[cfg(test)]
#[path = "wallpaper_loader.rs"]
mod wallpaper_loader;

/// The desktop keeps the complete delivered snapshot as its only workspace
/// authority. Presentation code reads the active index from this value; it
/// does not maintain a second active-workspace source.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceAuthority {
    snapshot: WorkspaceSnapshot,
}

impl WorkspaceAuthority {
    pub(crate) fn new(snapshot: WorkspaceSnapshot) -> Self {
        Self { snapshot }
    }

    pub(crate) fn snapshot(&self) -> &WorkspaceSnapshot {
        &self.snapshot
    }

    pub(crate) fn active_index(&self) -> usize {
        self.snapshot.active_index
    }

    /// Replace the authority with the delivered snapshot and report whether
    /// sticky/context presentation must be projected again.
    pub(crate) fn apply_snapshot(&mut self, snapshot: WorkspaceSnapshot) -> bool {
        let active_changed = self.snapshot.active_index != snapshot.active_index;
        self.snapshot = snapshot;
        active_changed
    }
}

#[cfg(test)]
mod tests {
    use super::settings_sync::{signal_callback_source, source_between};
    use super::*;

    #[test]
    fn t01_snapshot_signal_drives_active_transition_without_control_fetch() {
        // CONTRACT: WorkspacesSnapshotChanged carries the authoritative live snapshot.
        // TRIGGER: deliver a workspace snapshot signal with a changed active_index.
        // OBSERVABLE: the desktop transition consumes snapshot.active_index directly.
        // CURRENT BAD MECHANISM: the callback only handles WorkspacesChanged and calls
        // query_active_workspace(), adding a control fetch instead of consuming payload.
        // FAILURE MUTATION: replace WorkspacesSnapshotChanged with WorkspacesChanged or
        // remove snapshot.active_index consumption; this regression must turn red.
        let callback = signal_callback_source();
        assert!(
            callback.contains("ControlSignal::WorkspacesSnapshotChanged"),
            "snapshot signal must be the active-workspace transition trigger"
        );
        assert!(
            callback.contains("snapshot.active_index"),
            "active transition must consume the delivered snapshot"
        );
        assert!(
            !callback.contains("query_active_workspace()"),
            "snapshot delivery must not fetch the workspace snapshot"
        );
    }

    #[test]
    fn t02_legacy_workspaces_changed_is_ignored() {
        // CONTRACT: the revision-only WorkspacesChanged signal is not a visual authority.
        // TRIGGER: deliver the legacy signal after a snapshot signal.
        // OBSERVABLE: no desktop transition or control fetch is attached to that signal.
        // CURRENT BAD MECHANISM: the callback matches WorkspacesChanged and queries the
        // control service before changing active_workspace.
        // FAILURE MUTATION: restore an executable WorkspacesChanged match; this test must
        // fail rather than silently accepting the legacy path.
        assert!(
            !signal_callback_source().contains("ControlSignal::WorkspacesChanged"),
            "legacy workspace signal must not drive the desktop transition"
        );
    }

    #[test]
    fn t03_workspace_visual_dirty_does_not_rescan_or_refresh_launchers() {
        // CONTRACT: a workspace visual transition is separate from filesystem dirtiness.
        // TRIGGER: apply a workspace signal without an inotify desktop-directory event.
        // OBSERVABLE: no model rescan and no launcher-cache refresh are scheduled.
        // CURRENT MECHANISM: refresh_workspace_projection consumes workspace_visual_dirty and
        // only projects sticky/menu state; the filesystem dirty flag is never touched.
        // FAILURE MUTATION: reintroduce the shared dirty store or either filesystem action in
        // refresh_workspace_projection; this test must fail on that mutation.
        let source = include_str!("main.rs");
        let projection = source_between(
            source,
            "fn refresh_workspace_projection",
            "/// Launcher metadata cache",
        );
        assert!(
            !projection.contains("dirty.store"),
            "workspace visual dirtiness must not enter the filesystem dirty flag"
        );
        assert!(
            !projection.contains("dirty.swap"),
            "workspace projection must not consume the filesystem dirty flag"
        );
        assert!(
            !projection.contains("refresh_if_changed"),
            "workspace projection must not rescan through the filesystem refresh path"
        );
        assert!(!projection.contains("rescan"));
        assert!(!projection.contains("model.rescan"));
        assert!(!projection.contains("refresh_launchers"));
    }

    #[test]
    fn t04_workspace_transition_uses_isolated_sticky_projection() {
        // CONTRACT: switching workspaces projects old/new sticky visibility without
        // routing through filesystem synchronization.
        // TRIGGER: change active_index in a delivered workspace snapshot.
        // OBSERVABLE: the transition path contains sticky projection and no direct dirty
        // bridge to refresh_if_changed_direct.
        // CURRENT BAD MECHANISM: the signal callback has no snapshot projection seam and
        // reaches sticky visibility only through the shared filesystem refresh path.
        // FAILURE MUTATION: route the transition back through refresh_if_changed_direct;
        // this test must fail and preserve the old/new projection boundary.
        let callback = signal_callback_source();
        assert!(
            callback.contains("snapshot.active_index"),
            "workspace transition must carry the new active index into projection"
        );
        assert!(
            callback.contains("sync_sticky"),
            "workspace transition must use the isolated sticky projection"
        );
        assert!(
            !callback.contains("refresh_if_changed_direct"),
            "sticky projection must not use the filesystem dirty bridge"
        );
    }

    fn snapshot(
        revision: u64,
        count: usize,
        active_index: usize,
        last_index: Option<usize>,
    ) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision,
            count,
            active_index,
            last_index,
            names: (1..=count).map(|index| index.to_string()).collect(),
        }
    }

    #[test]
    fn workspace_authority_reports_only_active_projection_changes() {
        let mut authority = WorkspaceAuthority::new(snapshot(1, 2, 0, None));
        assert!(!authority.apply_snapshot(snapshot(2, 3, 0, None)));
        assert!(authority.apply_snapshot(snapshot(3, 3, 1, Some(0))));
        assert_eq!(authority.active_index(), 1);
    }
}
