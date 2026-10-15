use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn switch_workspace(&mut self, target: usize) -> Result<(), ReplyError> {
        if target >= self.workspace_count || target == self.current_workspace {
            return Ok(());
        }
        self.snap_preview.hide();
        let old = self.current_workspace;
        if let Some(slot) = self.workspace_focus.get_mut(old) {
            *slot = self.active;
        }
        self.current_workspace = target;
        workspace_counters().switch_total.increment();
        // One server grab so the map/unmap batch presents as a single frame.
        self.conn.grab_server()?;
        let visibility = self.apply_workspace_visibility(old, target);
        let _ = self.conn.ungrab_server();
        let flips = visibility?;
        let previous_active = self.active;
        self.active = None;
        let restored = self
            .workspace_focus
            .get(target)
            .copied()
            .flatten()
            .filter(|w| {
                self.clients
                    .get(w)
                    .is_some_and(|state| state.visible_on(target))
            });
        if let Some(window) = restored {
            self.focus(window)?;
        }
        if workspace_switch_touches_windows(&flips, self.active != previous_active) {
            self.mark_windows();
        }
        self.mark_workspaces();
        self.publish_current_workspace()?;
        {
            let _guard = flamewm_profiler::start("wm.workspace.switch.shell_publish");
            self.publish_active()?;
            workspace_counters().shell_publish.increment();
        }
        Ok(())
    }

    /// Maps/unmaps frames whose visibility differs between `old` and `target`.
    /// Returns the (old_visible, new_visible) pair for every client considered.
    pub(super) fn apply_workspace_visibility(
        &mut self,
        old: usize,
        target: usize,
    ) -> Result<Vec<(bool, bool)>, ReplyError> {
        let _guard = flamewm_profiler::start("wm.workspace.switch.visibility");
        let mut ids = Vec::with_capacity(self.clients.len());
        ids.extend(self.clients.keys().copied());
        let mut pairs = Vec::with_capacity(ids.len());
        for id in ids {
            let (frame, old_visible, new_visible) = {
                let Some(state) = self.clients.get(&id) else {
                    continue;
                };
                (state.frame, state.visible_on(old), state.visible_on(target))
            };
            pairs.push((old_visible, new_visible));
            if old_visible == new_visible {
                continue;
            }
            if new_visible {
                self.conn.map_window(frame)?;
            } else {
                if let Some(state) = self.clients.get_mut(&id) {
                    state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                }
                self.conn.unmap_window(frame)?;
            }
        }
        workspace_counters().visibility.increment();
        Ok(pairs)
    }

    pub(super) fn set_workspace_count(&mut self, requested: usize) -> Result<(), ReplyError> {
        let requested = requested.clamp(1, 18);
        if requested == self.workspace_count {
            return Ok(());
        }
        self.snap_preview.hide();
        let mut moved = Vec::new();
        if requested < self.workspace_count {
            for state in self.clients.values_mut() {
                if state.workspace >= requested {
                    state.workspace = requested - 1;
                    moved.push((state.client, state.workspace));
                }
            }
            self.current_workspace = self.current_workspace.min(requested - 1);
        }
        for (client, workspace) in moved {
            self.set_client_workspace(client, workspace)?;
        }
        self.workspace_count = requested;
        self.mark_workspaces();
        self.mark_windows();
        self.publish_workspace_metadata()?;
        self.publish_current_workspace()?;
        self.reconcile_workspace_visibility()?;
        Ok(())
    }

    pub(super) fn reconcile_workspace_visibility(&mut self) -> Result<(), ReplyError> {
        let mut ids = Vec::with_capacity(self.clients.len());
        ids.extend(self.clients.keys().copied());
        for id in ids {
            let (frame, visible) = {
                let Some(state) = self.clients.get(&id) else {
                    continue;
                };
                (state.frame, state.visible_on(self.current_workspace))
            };
            if visible {
                self.conn.map_window(frame)?;
            } else {
                if let Some(state) = self.clients.get_mut(&id) {
                    state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                }
                self.conn.unmap_window(frame)?;
            }
        }
        Ok(())
    }

    pub(super) fn move_to_workspace(&mut self, client: Window, target: usize) -> Result<(), ReplyError> {
        if target >= self.workspace_count {
            return Ok(());
        }
        self.snap_preview.hide();
        let (frame, visible) = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            state.workspace = target;
            (state.frame, state.visible_on(self.current_workspace))
        };
        self.set_client_workspace(client, target)?;
        self.mark_windows();
        self.mark_workspaces();
        if visible {
            self.conn.map_window(frame)?;
        } else {
            if let Some(state) = self.clients.get_mut(&client) {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
            }
            self.conn.unmap_window(frame)?;
        }
        Ok(())
    }

    pub(super) fn set_client_workspace(&self, client: Window, workspace: usize) -> Result<(), ReplyError> {
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_wm_desktop,
            AtomEnum::CARDINAL,
            &[workspace as u32],
        )?;
        Ok(())
    }
}
