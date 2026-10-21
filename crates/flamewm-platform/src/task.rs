use flamewm_api::{DesktopAppId, TaskEntryId, WindowRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskEntryKind {
    /// Persistent application slot. When `window` is set, the first live window of the pinned
    /// application is represented by the anchored pinned slot itself.
    Pinned { window: Option<WindowRef> },
    /// Independent live window entry. Same-application windows are never grouped.
    Window { window: WindowRef },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub app: DesktopAppId,
    pub kind: TaskEntryKind,
}

impl TaskEntry {
    #[must_use]
    pub fn pinned(app: DesktopAppId) -> Self {
        Self {
            app,
            kind: TaskEntryKind::Pinned { window: None },
        }
    }

    #[must_use]
    pub fn window(app: DesktopAppId, window: WindowRef) -> Self {
        Self {
            app,
            kind: TaskEntryKind::Window { window },
        }
    }

    #[must_use]
    pub fn id(&self) -> TaskEntryId {
        match self.kind {
            TaskEntryKind::Pinned { .. } => TaskEntryId::pinned(&self.app),
            TaskEntryKind::Window { window } => TaskEntryId::window(window),
        }
    }

    #[must_use]
    const fn represented_window(&self) -> Option<WindowRef> {
        match self.kind {
            TaskEntryKind::Pinned { window } => window,
            TaskEntryKind::Window { window } => Some(window),
        }
    }

    #[must_use]
    const fn is_pinned_slot(&self) -> bool {
        matches!(self.kind, TaskEntryKind::Pinned { .. })
    }

    fn set_pinned_window(&mut self, window: WindowRef) {
        self.kind = TaskEntryKind::Pinned {
            window: Some(window),
        };
    }
}

/// V8 task-strip authority.
///
/// The strip owns only FlameWM presentation ordering/pin identity. The engine remains authoritative
/// for the existence, focus, state and lifetime generation of windows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskStrip {
    entries: Vec<TaskEntry>,
}

impl TaskStrip {
    #[must_use]
    pub fn entries(&self) -> &[TaskEntry] {
        &self.entries
    }

    #[must_use]
    pub fn is_pinned(&self, app: &DesktopAppId) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.app == *app && entry.is_pinned_slot())
    }

    #[must_use]
    pub fn pinned_apps(&self) -> Vec<DesktopAppId> {
        self.entries
            .iter()
            .filter(|entry| entry.is_pinned_slot())
            .map(|entry| entry.app.clone())
            .collect()
    }

    /// Reconcile against the live window list in any order. Identity is the X window id:
    /// a generation bump (focus, restore) refreshes the stored reference in place, and
    /// existing entries never move. Closed ids are removed; new ids are appended in list order.
    pub fn reconcile_live(&mut self, live: &[(DesktopAppId, WindowRef)]) -> bool {
        let before = self.clone();
        let stale: Vec<WindowRef> = self
            .entries
            .iter()
            .filter_map(TaskEntry::represented_window)
            .filter(|window| !live.iter().any(|(_, current)| current.id == window.id))
            .collect();
        for window in stale {
            if std::env::var_os("FLAMEWM_DEBUG_TASKS").is_some() {
                eprintln!(
                    "flamewm-task: remove window={} gen={} (absent from live list)",
                    window.id, window.generation
                );
            }
            self.remove_window(window);
        }
        for entry in &mut self.entries {
            let Some(window) = entry.represented_window() else {
                continue;
            };
            let Some((_, current)) = live.iter().find(|(_, current)| current.id == window.id)
            else {
                continue;
            };
            match &mut entry.kind {
                TaskEntryKind::Pinned { window: Some(slot) } => *slot = *current,
                TaskEntryKind::Window { window: slot } => *slot = *current,
                TaskEntryKind::Pinned { window: None } => {}
            }
        }
        for (app, window) in live {
            self.add_window(app.clone(), *window);
        }
        *self != before
    }

    pub fn add_window(&mut self, app: DesktopAppId, window: WindowRef) -> bool {
        if self.window_index(window).is_some() {
            return false;
        }
        if let Some(slot) = self.entries.iter_mut().find(|entry| {
            entry.app == app && matches!(entry.kind, TaskEntryKind::Pinned { window: None })
        }) {
            slot.set_pinned_window(window);
            return true;
        }
        self.entries.push(TaskEntry::window(app, window));
        true
    }

    fn remove_window(&mut self, window: WindowRef) -> bool {
        let Some(index) = self.window_index(window) else {
            return false;
        };
        let app = self.entries[index].app.clone();
        let was_pinned_slot = self.entries[index].is_pinned_slot();

        if was_pinned_slot {
            // Keep the anchored slot. Promote one sibling window into it when available.
            if let Some(sibling) = self.entries.iter().enumerate().find_map(|(i, entry)| {
                (i != index
                    && entry.app == app
                    && matches!(entry.kind, TaskEntryKind::Window { .. }))
                .then_some(i)
            }) {
                let sibling_entry = self.entries.remove(sibling);
                if let Some(sibling_window) = sibling_entry.represented_window() {
                    let slot_index = if sibling < index { index - 1 } else { index };
                    self.entries[slot_index].set_pinned_window(sibling_window);
                }
            } else {
                self.entries[index].kind = TaskEntryKind::Pinned { window: None };
            }
        } else {
            self.entries.remove(index);
        }
        true
    }

    pub fn pin_window(&mut self, window: WindowRef) -> bool {
        let Some(index) = self.window_index(window) else {
            return false;
        };
        let app = self.entries[index].app.clone();
        if !self.is_pinned(&app) {
            self.entries[index].set_pinned_window(window);
        }
        true
    }

    pub fn pin_app(&mut self, app: DesktopAppId) -> bool {
        if self.is_pinned(&app) {
            return false;
        }
        if let Some(index) = self.first_window_index_for_app(&app) {
            if let Some(window) = self.entries[index].represented_window() {
                self.entries[index].set_pinned_window(window);
            }
        } else {
            self.entries.push(TaskEntry::pinned(app));
        }
        true
    }

    pub fn unpin_app(&mut self, app: &DesktopAppId) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.app == *app && entry.is_pinned_slot())
        else {
            return false;
        };
        match self.entries[index].kind {
            TaskEntryKind::Pinned {
                window: Some(window),
            } => {
                self.entries[index].kind = TaskEntryKind::Window { window };
            }
            TaskEntryKind::Pinned { window: None } => {
                self.entries.remove(index);
            }
            TaskEntryKind::Window { .. } => return false,
        }
        true
    }

    /// Commit insertion ordering on release. `to` is the final insertion index after removal.
    pub fn reorder_on_release(&mut self, from: usize, to: usize) -> bool {
        if from >= self.entries.len() || to >= self.entries.len() || from == to {
            return false;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        true
    }

    #[must_use]
    pub fn begin_reorder(&self, from: usize) -> Option<ReorderSession> {
        (from < self.entries.len()).then(|| ReorderSession {
            source: from,
            original_order: self.entries.iter().map(TaskEntry::id).collect(),
        })
    }

    #[must_use]
    pub fn current_ids(&self) -> Vec<TaskEntryId> {
        self.entries.iter().map(TaskEntry::id).collect()
    }

    fn window_index(&self, window: WindowRef) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.represented_window() == Some(window))
    }

    fn first_window_index_for_app(&self, app: &DesktopAppId) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.app == *app && entry.represented_window().is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReorderSession {
    source: usize,
    original_order: Vec<TaskEntryId>,
}

impl ReorderSession {
    #[must_use]
    pub const fn source(&self) -> usize {
        self.source
    }

    #[must_use]
    pub fn original_order(&self) -> &[TaskEntryId] {
        &self.original_order
    }

    /// Cancellation is intentionally a no-op because the strip is never mutated during drag.
    #[must_use]
    pub fn cancel(self, strip: &TaskStrip) -> bool {
        strip.current_ids() == self.original_order
    }

    pub fn commit(self, strip: &mut TaskStrip, insertion_index: usize) -> bool {
        strip.reorder_on_release(self.source, insertion_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(value: &str) -> DesktopAppId {
        DesktopAppId::new(value)
    }

    fn win(value: u64) -> WindowRef {
        WindowRef::new(value, 1)
    }

    #[test]
    fn pinned_app_uses_one_anchored_entry_and_same_app_windows_remain_independent() {
        let mut strip = TaskStrip::default();
        strip.pin_app(app("org.kde.dolphin"));
        strip.add_window(app("org.kde.dolphin"), win(1));
        strip.add_window(app("org.kde.dolphin"), win(2));
        strip.add_window(app("org.kde.dolphin"), win(3));
        assert_eq!(strip.entries().len(), 3);
        assert_eq!(strip.entries()[0].id().as_str(), "pin:org.kde.dolphin");
        assert_eq!(strip.entries()[1].id().as_str(), "win:2:1");
        assert_eq!(strip.entries()[2].id().as_str(), "win:3:1");
    }

    #[test]
    fn closing_pinned_representative_promotes_sibling_without_moving_slot() {
        let mut strip = TaskStrip::default();
        strip.pin_app(app("firefox"));
        strip.add_window(app("firefox"), win(1));
        strip.add_window(app("firefox"), win(2));
        strip.add_window(app("terminal"), win(9));
        assert!(strip.remove_window(win(1)));
        assert_eq!(strip.entries()[0].id().as_str(), "pin:firefox");
        assert_eq!(strip.entries()[0].represented_window(), Some(win(2)));
        assert_eq!(strip.entries()[1].represented_window(), Some(win(9)));
    }

    #[test]
    fn final_pinned_window_close_restores_placeholder() {
        let mut strip = TaskStrip::default();
        strip.pin_app(app("firefox"));
        strip.add_window(app("firefox"), win(1));
        assert!(strip.remove_window(win(1)));
        assert_eq!(strip.entries().len(), 1);
        assert_eq!(strip.entries()[0].represented_window(), None);
        assert_eq!(strip.entries()[0].id().as_str(), "pin:firefox");
    }

    #[test]
    fn unpin_active_app_preserves_live_window_identity_and_location() {
        let mut strip = TaskStrip::default();
        strip.add_window(app("terminal"), win(9));
        strip.add_window(app("firefox"), win(1));
        strip.pin_window(win(1));
        assert!(strip.unpin_app(&app("firefox")));
        assert_eq!(strip.entries()[1].id().as_str(), "win:1:1");
    }

    #[test]
    fn cancelled_drag_cannot_change_order() {
        let mut strip = TaskStrip::default();
        strip.pin_app(app("a"));
        strip.pin_app(app("b"));
        strip.add_window(app("c"), win(3));
        let session = strip.begin_reorder(0).expect("source exists");
        assert!(session.cancel(&strip));
        assert_eq!(
            strip
                .current_ids()
                .iter()
                .map(TaskEntryId::as_str)
                .collect::<Vec<_>>(),
            vec!["pin:a", "pin:b", "win:3:1"]
        );
    }

    #[test]
    fn release_commits_before_between_after_style_insertion() {
        let mut strip = TaskStrip::default();
        strip.pin_app(app("a"));
        strip.pin_app(app("b"));
        strip.pin_app(app("c"));
        let session = strip.begin_reorder(0).expect("source exists");
        assert!(session.commit(&mut strip, 2));
        assert_eq!(
            strip
                .current_ids()
                .iter()
                .map(TaskEntryId::as_str)
                .collect::<Vec<_>>(),
            vec!["pin:b", "pin:c", "pin:a"]
        );
    }

    fn snapshot(
        app: DesktopAppId,
        id: u64,
        generation: u64,
        state: flamewm_api::window::WindowState,
    ) -> flamewm_api::window::WindowSnapshot {
        flamewm_api::window::WindowSnapshot {
            reference: WindowRef::new(id, generation),
            title: format!("w{id}"),
            app_id: app,
            outer_geometry: flamewm_api::Rect::new(0, 0, 100, 100),
            restore_geometry: flamewm_api::Rect::new(0, 0, 100, 100),
            state,
            sticky: false,
            focused: false,
            workspace: flamewm_api::WorkspaceRef::new(0, 0),
            output: flamewm_api::OutputId::new("eDP-1"),
            state_generation: generation,
        }
    }

    #[test]
    fn three_windows_minimize_restore_close_keep_identity_and_state() {
        use flamewm_api::window::WindowState;
        fn cycle(
            strip: &mut TaskStrip,
            snapshots: &mut [flamewm_api::window::WindowSnapshot],
            state: WindowState,
        ) {
            for snap in snapshots.iter_mut() {
                let old = snap.reference;
                snap.state = state;
                snap.state_generation += 10;
                snap.reference = WindowRef::new(old.id, snap.state_generation);
                assert!(strip.remove_window(old));
                assert!(strip.add_window(snap.app_id.clone(), snap.reference));
                assert_eq!(
                    TaskEntryId::window(snap.reference).as_str(),
                    format!("win:{}:{}", old.id, snap.state_generation)
                );
            }
        }
        let mut strip = TaskStrip::default();
        let apps = [app("a"), app("b"), app("c")];
        let mut snapshots: Vec<flamewm_api::window::WindowSnapshot> = [1u64, 2, 3]
            .iter()
            .zip(apps.iter().cloned())
            .map(|(id, app)| snapshot(app, *id, *id, WindowState::Normal))
            .collect();
        for snap in &snapshots {
            assert!(strip.add_window(snap.app_id.clone(), snap.reference));
        }
        assert_eq!(
            strip
                .current_ids()
                .iter()
                .map(TaskEntryId::as_str)
                .collect::<Vec<_>>(),
            vec!["win:1:1", "win:2:2", "win:3:3"]
        );
        cycle(&mut strip, &mut snapshots, WindowState::Minimized);
        assert_eq!(
            snapshots
                .iter()
                .map(|s| s.state_generation)
                .collect::<Vec<_>>(),
            vec![11, 12, 13]
        );
        assert!(snapshots.iter().all(|s| s.state == WindowState::Minimized));
        cycle(&mut strip, &mut snapshots, WindowState::Normal);
        assert_eq!(
            snapshots
                .iter()
                .map(|s| s.state_generation)
                .collect::<Vec<_>>(),
            vec![21, 22, 23]
        );
        assert!(snapshots.iter().all(|s| s.state == WindowState::Normal));
        let revisions: Vec<u64> = snapshots.iter().map(|s| s.state_generation).collect();
        assert!(revisions.iter().all(|g| *g != 0));
        assert_eq!(
            revisions
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        for snap in &snapshots {
            assert!(strip.remove_window(snap.reference));
        }
        assert!(strip.entries().is_empty());
    }
}

#[cfg(test)]
mod order_invariant_tests {
    use super::*;

    fn live(list: &[(&str, u64, u64)]) -> Vec<(DesktopAppId, WindowRef)> {
        list.iter()
            .map(|(name, id, generation)| {
                (DesktopAppId::new(*name), WindowRef::new(*id, *generation))
            })
            .collect()
    }

    fn order(strip: &TaskStrip) -> Vec<u64> {
        strip
            .entries()
            .iter()
            .filter_map(|entry| entry.represented_window().map(|window| window.id))
            .collect()
    }

    fn abc() -> TaskStrip {
        let mut strip = TaskStrip::default();
        strip.reconcile_live(&live(&[("a", 1, 1), ("b", 2, 1), ("c", 3, 1)]));
        strip
    }

    #[test]
    fn stacking_permutation_does_not_reorder() {
        let mut strip = abc();
        strip.reconcile_live(&live(&[("c", 3, 1), ("a", 1, 1), ("b", 2, 1)]));
        assert_eq!(order(&strip), vec![1, 2, 3]);
    }

    #[test]
    fn generation_bump_on_restore_keeps_slot() {
        // Minimize/restore and focus bump the engine generation; the slot must not move.
        let mut strip = abc();
        strip.reconcile_live(&live(&[("a", 1, 1), ("b", 2, 5), ("c", 3, 1)]));
        assert_eq!(order(&strip), vec![1, 2, 3]);
        assert_eq!(
            strip.entries()[1].represented_window(),
            Some(WindowRef::new(2, 5))
        );
    }

    #[test]
    fn minimized_window_that_stays_listed_keeps_slot() {
        let mut strip = abc();
        // Minimized b stays in the live list (state change only), possibly with a new generation.
        strip.reconcile_live(&live(&[("a", 1, 1), ("b", 2, 2), ("c", 3, 1)]));
        strip.reconcile_live(&live(&[("c", 3, 1), ("b", 2, 2), ("a", 1, 1)]));
        assert_eq!(order(&strip), vec![1, 2, 3]);
    }

    #[test]
    fn new_window_is_appended_right() {
        let mut strip = abc();
        strip.reconcile_live(&live(&[("c", 3, 1), ("a", 1, 1), ("b", 2, 1), ("d", 4, 1)]));
        assert_eq!(order(&strip), vec![1, 2, 3, 4]);
    }

    #[test]
    fn closed_window_is_removed_and_others_keep_order() {
        let mut strip = abc();
        strip.reconcile_live(&live(&[("c", 3, 1), ("b", 2, 1)]));
        assert_eq!(order(&strip), vec![2, 3]);
    }

    #[test]
    fn explicit_move_survives_permuted_lists() {
        let mut strip = abc();
        assert!(strip.reorder_on_release(2, 0));
        assert_eq!(order(&strip), vec![3, 1, 2]);
        strip.reconcile_live(&live(&[("a", 1, 1), ("b", 2, 1), ("c", 3, 1)]));
        assert_eq!(order(&strip), vec![3, 1, 2]);
    }

    #[test]
    fn negative_control_resort_by_list_order_is_rejected() {
        // A rebuild-from-list fix would yield [3, 1, 2] here instead of the stable [1, 2, 3].
        let mut strip = abc();
        strip.reconcile_live(&live(&[("c", 3, 1), ("a", 1, 1), ("b", 2, 1)]));
        assert_ne!(order(&strip), vec![3, 1, 2]);
    }
}
