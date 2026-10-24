use super::*;
use flamewm_api::workspace::MAX_WORKSPACES;

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
        let visibility_changed = visibility?;
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
        if workspace_switch_touches_windows(visibility_changed, self.active != previous_active) {
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

    /// Move the already-grabbed window into an adjacent workspace without
    /// releasing its move controller or unmapping its frame. Relocating the
    /// controller's anchor together with XWarpPointer prevents the next motion
    /// from snapping the frame back to the original workspace coordinates.
    pub(super) fn switch_workspace_dragging(
        &mut self,
        client: Window,
        target: usize,
        new_pointer: RootPoint,
        side: DragEdge,
    ) -> Result<(), ReplyError> {
        if target >= self.workspace_count || target == self.current_workspace {
            return Ok(());
        }
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if state.sticky {
            return Ok(());
        }
        let Some(controller) = self.sessions.get(&client) else {
            return Ok(());
        };
        if !matches!(controller.session, InteractionSession::Move(m) if m.anchor.is_activated()) {
            return Ok(());
        }

        let old = self.current_workspace;
        // Plan the arrival through the existing pure move reducer BEFORE the
        // server grab. In particular, do not repaint while the server is
        // grabbed: paint_chrome synchronizes the WM connection and then uses
        // the renderer's independent X connection, which would deadlock.
        let starting_rect = controller.placement.current;
        let (delta_x, delta_y) = match side {
            DragEdge::Left | DragEdge::Right => (
                drag_entry_frame_x(new_pointer, starting_rect.w, side)
                    .saturating_sub(starting_rect.x),
                0,
            ),
            DragEdge::Top | DragEdge::Bottom => (
                0,
                drag_entry_frame_y(new_pointer, side).saturating_sub(starting_rect.y),
            ),
        };
        let synthetic_start = RootPoint::new(
            new_pointer.x.saturating_sub(delta_x),
            new_pointer.y.saturating_sub(delta_y),
        );
        let Some(mut planned) = self.controller_for(client) else {
            return Ok(());
        };
        if let InteractionSession::Move(ref mut move_session) = planned.session {
            move_session.start_pointer = synthetic_start;
            move_session.start_placement = PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: starting_rect,
            };
        }
        let (mut next, commit) = interaction::step_owned(
            planned,
            FrameEvent::Motion {
                pointer: new_pointer,
            },
        );
        // The pure effect plan is kept intact except for the separate-X11-
        // connection repaint; reject newly introduced effects at this boundary.
        let Some((commit, repaint)) = defer_drag_arrival_paint(commit) else {
            return Ok(());
        };
        if let InteractionSession::Move(ref mut move_session) = next.session {
            // The X pointer is warped during the transaction. The next genuine
            // motion must be relative to its new location, not the old edge.
            move_session.start_pointer = new_pointer;
            move_session.start_placement = PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: next.placement.current,
            };
        }
        self.snap_preview.hide();

        self.conn.grab_server()?;
        let result: Result<(), ReplyError> = (|| {
            if let Some(state) = self.clients.get_mut(&client) {
                state.workspace = target;
            }
            self.set_client_workspace(client, target)?;
            self.current_workspace = target;
            self.apply_workspace_visibility(old, target)?;
            self.publish_current_workspace()?;
            self.store_controller(client, &next);
            // Preserve the reducer/native-effect authority for frame position,
            // but defer only the separate-connection paint until after ungrab.
            self.execute_effects(client, &commit)?;
            self.conn.warp_pointer(
                x11rb::NONE,
                self.screen().root,
                0,
                0,
                0,
                0,
                new_pointer.x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                new_pointer.y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            )?;
            Ok(())
        })();
        let ungrab = self.conn.ungrab_server();
        // Flush the ungrab on the owning connection before touching Xlib's
        // separate connection. Otherwise even a deferred repaint can block.
        self.conn.flush()?;
        ungrab?;
        result?;
        if repaint {
            self.paint_chrome(client)?;
        }
        workspace_counters().switch_total.increment();
        // Do not keep the transferred window as the former workspace's focus.
        if let Some(slot) = self.workspace_focus.get_mut(old) {
            *slot = None;
        }
        if let Some(slot) = self.workspace_focus.get_mut(target) {
            *slot = Some(client);
        }
        self.focus(client)?;
        self.mark_windows();
        self.mark_workspaces();
        Ok(())
    }

    /// Maps/unmaps frames whose visibility differs between `old` and `target`.
    /// Returns whether any frame's visibility changed (no per-window allocation).
    pub(super) fn apply_workspace_visibility(
        &mut self,
        old: usize,
        target: usize,
    ) -> Result<bool, ReplyError> {
        let _guard = flamewm_profiler::start("wm.workspace.switch.visibility");
        // Iterate the authoritative client states in place. X11 requests use
        // self.conn, independent of the mutable clients field, so this needs
        // neither a copied ID list nor a visibility-pair allocation.
        let mut changed = false;
        for state in self.clients.values_mut() {
            let Some(should_map) =
                frame_visibility_delta(state.visible_on(old), state.visible_on(target))
            else {
                continue;
            };
            changed = true;
            if should_map {
                self.conn.map_window(state.frame)?;
            } else {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                self.conn.unmap_window(state.frame)?;
            }
        }
        workspace_counters().visibility.increment();
        Ok(changed)
    }

    pub(super) fn set_workspace_count(&mut self, requested: usize) -> Result<(), ReplyError> {
        let requested = requested.clamp(1, MAX_WORKSPACES);
        if requested == self.workspace_count {
            return Ok(());
        }
        self.snap_preview.hide();
        crate::bump_workspace_generation();
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
        // The focus table is indexed by workspace, just like the metadata.
        // Keep its length in sync when a workspace is inserted or removed;
        // otherwise newly added workspaces have no focus-restoration slot.
        self.workspace_focus.resize(requested, None);
        if let Some(active) = self.active {
            if self
                .clients
                .get(&active)
                .is_some_and(|state| state.visible_on(self.current_workspace))
            {
                self.workspace_focus[self.current_workspace] = Some(active);
            }
        }
        self.mark_workspaces();
        self.mark_windows();
        self.publish_workspace_metadata()?;
        self.publish_current_workspace()?;
        self.reconcile_workspace_visibility()?;
        Ok(())
    }

    pub(super) fn reconcile_workspace_visibility(&mut self) -> Result<(), ReplyError> {
        let current = self.current_workspace;
        for state in self.clients.values_mut() {
            if state.visible_on(current) {
                self.conn.map_window(state.frame)?;
            } else {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                self.conn.unmap_window(state.frame)?;
            }
        }
        Ok(())
    }

    pub(super) fn move_to_workspace(
        &mut self,
        client: Window,
        target: usize,
    ) -> Result<(), ReplyError> {
        if target >= self.workspace_count {
            return Ok(());
        }
        self.snap_preview.hide();
        let (frame, was_visible, visible) = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            let was_visible = state.visible_on(self.current_workspace);
            state.workspace = target;
            (
                state.frame,
                was_visible,
                state.visible_on(self.current_workspace),
            )
        };
        self.set_client_workspace(client, target)?;
        // A window moved out of the active workspace cannot remain the
        // EWMH active window. Clear its old focus-restoration slot too, or
        // switching back can try to refocus a window that now lives elsewhere.
        if !visible && self.active == Some(client) {
            self.active = None;
            if let Some(slot) = self.workspace_focus.get_mut(self.current_workspace) {
                if *slot == Some(client) {
                    *slot = None;
                }
            }
            self.publish_active()?;
        }
        self.mark_windows();
        self.mark_workspaces();
        // A minimized window or a window moved between two inactive
        // workspaces remains unmapped. Issuing another unmap would increment
        // ignore_unmap without a corresponding UnmapNotify and could later
        // suppress a genuine client withdrawal.
        if let Some(should_map) = frame_visibility_delta(was_visible, visible) {
            if should_map {
                self.conn.map_window(frame)?;
            } else {
                if let Some(state) = self.clients.get_mut(&client) {
                    state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                }
                self.conn.unmap_window(frame)?;
            }
        }
        Ok(())
    }

    pub(super) fn set_client_workspace(
        &self,
        client: Window,
        workspace: usize,
    ) -> Result<(), ReplyError> {
        self.set_property32(
            client,
            self.atoms.net_wm_desktop,
            AtomEnum::CARDINAL,
            &[workspace as u32],
        )
    }
}
