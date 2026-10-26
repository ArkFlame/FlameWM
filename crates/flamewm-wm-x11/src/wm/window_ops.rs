use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn focus(&mut self, client: Window) -> Result<(), ReplyError> {
        self.focus_with_stacking(client, true)
    }

    /// Focus-follows-mouse entry: takes input focus but never restacks, so hovering a window
    /// cannot raise it above the window the user is working in.
    pub(super) fn focus_hover(&mut self, client: Window) -> Result<(), ReplyError> {
        self.focus_with_stacking(client, false)
    }

    pub(super) fn focus_with_stacking(
        &mut self,
        client: Window,
        raise: bool,
    ) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if !state.visible_on(self.current_workspace) {
            return Ok(());
        }
        self.conn
            .set_input_focus(FOCUS_REVERT_TO, client, CURRENT_TIME)?;
        if raise && (state.kind != WindowKind::Utility || state.transient_for.is_some()) {
            self.conn.configure_window(
                state.frame,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
            self.raise_docks_above_normal(client, state.frame)?;
        }
        if self.supports_protocol(client, self.atoms.wm_take_focus)? {
            self.send_protocol(client, self.atoms.wm_take_focus)?;
        }
        self.active = Some(client);
        self.mru.touch(client);
        self.mark_windows();
        self.publish_active()?;
        self.publish_client_list()?;
        Ok(())
    }

    fn send_protocol(&self, client: Window, protocol: Atom) -> Result<(), ReplyError> {
        let event = ClientMessageEvent::new(
            32,
            client,
            self.atoms.wm_protocols,
            [protocol, CURRENT_TIME, 0, 0, 0],
        );
        self.conn
            .send_event(false, client, EventMask::NO_EVENT, event)?;
        Ok(())
    }

    pub(super) fn supports_protocol(
        &self,
        client: Window,
        protocol: Atom,
    ) -> Result<bool, ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.wm_protocols,
                AtomEnum::ATOM,
                0,
                32,
            )?
            .reply()?;
        Ok(reply
            .value32()
            .is_some_and(|mut values| values.any(|atom| atom == protocol)))
    }

    /// Client close request from the keyboard, frame button or `_NET_CLOSE_WINDOW`. The afterimage
    /// is captured first; the close request is sent right after and is never delayed by it.
    pub(super) fn request_close(&mut self, client: Window) -> Result<(), ReplyError> {
        self.begin_close_overlay(client);
        self.close(client)
    }

    /// Starts the close fade for `client` while its pixels still exist. Skipped without ARGB
    /// (an opaque backdrop would show black) or when the effect is disabled or cannot start.
    pub(super) fn begin_close_overlay(&mut self, client: Window) {
        let Some(renderer) = self.renderer.clone() else {
            return;
        };
        if !renderer.borrow().warp_argb_usable() {
            return;
        }
        if self
            .animator
            .start(client, Effect::Close, std::time::Instant::now())
            != Begin::Started
        {
            return;
        }
        if self.open_overlay(client, Effect::Close).is_err() {
            self.animator.cancel(client);
        }
    }

    pub(super) fn close(&self, client: Window) -> Result<(), ReplyError> {
        if self.supports_protocol(client, self.atoms.wm_delete_window)? {
            self.send_protocol(client, self.atoms.wm_delete_window)?;
        } else {
            self.conn.kill_client(client)?;
        }
        Ok(())
    }

    pub(super) fn minimize(&mut self, client: Window) -> Result<(), ReplyError> {
        // With the running-window area hidden there is no taskbar entry to restore from,
        // so a minimized window would be unreachable. Keep it on screen instead.
        if !crate::live_settings::taskbar_activities() {
            return Ok(());
        }
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if state.minimized && self.animator.running(client).is_none() {
            return Ok(());
        }
        if self.begin_transition(client, Effect::Minimize)? {
            return Ok(());
        }
        self.commit_minimize(client, false)
    }

    /// Applies the minimize end state. `hidden` means the frame was already unmapped
    /// when the transition began, so no second unmap (and no second ignore count) occurs.
    pub(super) fn commit_minimize(
        &mut self,
        client: Window,
        hidden: bool,
    ) -> Result<(), ReplyError> {
        let frame = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            if state.minimized {
                return Ok(());
            }
            state.minimized = true;
            if !hidden {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
            }
            state.frame
        };
        if !hidden {
            self.conn.unmap_window(frame)?;
        }
        self.set_wm_state(client, WM_STATE_ICONIC)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        if self.active == Some(client) {
            self.active = None;
            self.publish_active()?;
        }
        Ok(())
    }

    pub(super) fn restore_minimized(&mut self, client: Window) -> Result<(), ReplyError> {
        trace_restore("restore.trace.wm.restore_minimized", || {
            format!(
                "window={client} known={} minimized={:?} animator_running={:?}",
                self.clients.contains_key(&client),
                self.clients.get(&client).map(|state| state.minimized),
                self.animator.running(client)
            )
        });
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if !state.minimized && self.animator.running(client).is_none() {
            return Ok(());
        }
        let owned_by_flight = self.begin_transition(client, Effect::Restore)?;
        trace_restore("restore.trace.wm.begin_transition", || {
            format!("window={client} owned_by_flight={owned_by_flight}")
        });
        if owned_by_flight {
            return Ok(());
        }
        self.commit_restore(client)
    }

    pub(super) fn commit_restore(&mut self, client: Window) -> Result<(), ReplyError> {
        trace_restore("restore.trace.wm.commit_restore", || {
            format!("window={client}")
        });
        let frame = {
            let Some(state) = self.clients.get_mut(&client) else {
                return Ok(());
            };
            state.minimized = false;
            state.frame
        };
        // Mapping the frame produces no UnmapNotify, so no ignore count is added here.
        self.conn.map_window(frame)?;
        self.set_wm_state(client, WM_STATE_NORMAL)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        self.focus(client)?;
        let _ = self.conn.configure_window(
            frame,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        );
        self.raise_docks_above_normal(client, frame)?;
        self.publish_client_list()?;
        Ok(())
    }

    pub(super) fn toggle_maximize(&mut self, client: Window) -> Result<(), ReplyError> {
        let effect = self.clients.get(&client).and_then(|state| {
            if state.is_maximized() {
                unmaximize_target(&state.placement).map(|_| Effect::Unmaximize)
            } else if !state.is_fullscreen() && state.is_floating() {
                Some(Effect::Maximize)
            } else {
                None
            }
        });
        if let Some(effect) = effect {
            if self.begin_transition(client, effect)? {
                return Ok(());
            }
        }
        self.commit_maximize(client)
    }

    pub(super) fn commit_maximize(&mut self, client: Window) -> Result<(), ReplyError> {
        self.reduce_and_execute(client, FrameEvent::ToggleMax)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        Ok(())
    }

    pub(super) fn set_fullscreen(
        &mut self,
        client: Window,
        enable: bool,
    ) -> Result<(), ReplyError> {
        let fullscreen = self
            .clients
            .get(&client)
            .is_some_and(|state| state.is_fullscreen());
        if enable == fullscreen {
            return Ok(());
        }
        // ToggleFullscreen is a pure toggle; the early return above guarantees a single reduce.
        self.reduce_and_execute(client, FrameEvent::ToggleFullscreen)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        Ok(())
    }

    /// Toggle fullscreen from a keybind (`Super+F`). Unlike `FullscreenEscape`, this is a
    /// reversible toggle and never kills the client.
    pub(super) fn toggle_fullscreen(&mut self, client: Window) -> Result<(), ReplyError> {
        let fullscreen = self
            .clients
            .get(&client)
            .is_some_and(|state| state.is_fullscreen());
        self.set_fullscreen(client, !fullscreen)
    }

    /// `Super+V`: floating becomes the work-area fill ("pseudo-tile"), anything else returns
    /// to the remembered floating rect. There is no tiling engine to toggle against.
    pub(super) fn toggle_float(&mut self, client: Window) -> Result<(), ReplyError> {
        match self
            .clients
            .get(&client)
            .map(|state| state.placement.mode)
        {
            Some(PlacementMode::Floating) => self.toggle_maximize(client),
            Some(_) => self.return_to_floating(client),
            None => Ok(()),
        }
    }

    /// Leaves snap/maximize/fullscreen at the floating restore rect, without a pointer drag.
    pub(super) fn return_to_floating(&mut self, client: Window) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get_mut(&client) else {
            return Ok(());
        };
        let restore = state.placement.floating_restore;
        state.placement.mode = PlacementMode::Floating;
        state.placement.current = restore;
        state.placement.resume = None;
        state.set_outer(restore.to_legacy());
        self.apply_frame_geometry(client)?;
        self.layout_input_children(client)?;
        self.paint_chrome(client)?;
        self.mark_windows();
        self.publish_window_state(client)?;
        Ok(())
    }

    /// Focus the nearest visible window in `direction` from the active window's centre.
    pub(super) fn focus_direction(
        &mut self,
        direction: flamewm_api::workspace::GridDirection,
    ) -> Result<(), ReplyError> {
        match self.directional_target(direction) {
            Some(target) => self.focus(target),
            None => Ok(()),
        }
    }

    fn directional_target(
        &self,
        direction: flamewm_api::workspace::GridDirection,
    ) -> Option<Window> {
        use flamewm_api::workspace::GridDirection;
        let active = self.active?;
        let origin = self.clients.get(&active)?.outer;
        let ox = origin.x + i32::try_from(origin.width).unwrap_or(0) / 2;
        let oy = origin.y + i32::try_from(origin.height).unwrap_or(0) / 2;
        let mut best: Option<(i64, Window)> = None;
        for (&client, state) in &self.clients {
            if client == active
                || state.kind != WindowKind::Normal
                || !state.visible_on(self.current_workspace)
            {
                continue;
            }
            let cx = state.outer.x + i32::try_from(state.outer.width).unwrap_or(0) / 2;
            let cy = state.outer.y + i32::try_from(state.outer.height).unwrap_or(0) / 2;
            let (dx, dy) = (cx - ox, cy - oy);
            let (along, across) = match direction {
                GridDirection::Left => (-dx, dy.abs()),
                GridDirection::Right => (dx, dy.abs()),
                GridDirection::Up => (-dy, dx.abs()),
                GridDirection::Down => (dy, dx.abs()),
            };
            if along <= 0 {
                continue;
            }
            let score = i64::from(along) * 4 + i64::from(across);
            if best.is_none_or(|(best_score, _)| score < best_score) {
                best = Some((score, client));
            }
        }
        best.map(|(_, client)| client)
    }

    /// Keyboard move of the active floating window (`Super+Shift+arrows`).
    pub(super) fn move_active(
        &mut self,
        direction: flamewm_api::workspace::GridDirection,
    ) -> Result<(), ReplyError> {
        use flamewm_api::workspace::GridDirection;
        const STEP: i32 = 40;
        let Some(client) = self.active else {
            return Ok(());
        };
        let Some(state) = self.clients.get_mut(&client) else {
            return Ok(());
        };
        if !state.is_floating() {
            return Ok(());
        }
        let mut rect = state.outer;
        let (dx, dy) = match direction {
            GridDirection::Left => (-STEP, 0),
            GridDirection::Right => (STEP, 0),
            GridDirection::Up => (0, -STEP),
            GridDirection::Down => (0, STEP),
        };
        rect.x = rect.x.saturating_add(dx);
        rect.y = rect.y.saturating_add(dy);
        state.set_outer(rect);
        state.placement.current = RootRect::from_legacy(rect);
        state.placement.floating_restore = state.placement.current;
        self.apply_frame_geometry(client)?;
        self.paint_chrome(client)?;
        self.mark_windows();
        Ok(())
    }

    /// Keyboard resize of the active floating window (`Super+equal` / `Super+minus`).
    pub(super) fn resize_active(&mut self, grow: bool) -> Result<(), ReplyError> {
        const STEP_W: i32 = 40;
        const STEP_H: i32 = 30;
        let Some(client) = self.active else {
            return Ok(());
        };
        let Some(state) = self.clients.get_mut(&client) else {
            return Ok(());
        };
        if !state.is_floating() {
            return Ok(());
        }
        let mut rect = state.outer;
        rect.width = if grow {
            rect.width.saturating_add(STEP_W as u32)
        } else {
            rect.width.saturating_sub(STEP_W as u32).max(160)
        };
        rect.height = if grow {
            rect.height.saturating_add(STEP_H as u32)
        } else {
            rect.height.saturating_sub(STEP_H as u32).max(96)
        };
        state.set_outer(rect);
        state.placement.current = RootRect::from_legacy(rect);
        state.placement.floating_restore = state.placement.current;
        self.apply_frame_geometry(client)?;
        self.layout_input_children(client)?;
        self.paint_chrome(client)?;
        self.mark_windows();
        Ok(())
    }

    /// `Super+J` / `Super+K`: focus the next/previous visible normal window in stack order.
    pub(super) fn cycle_window(&mut self, step: i32) -> Result<(), ReplyError> {
        let tree = self.conn.query_tree(self.screen().root)?.reply()?;
        let stack: Vec<Window> = tree
            .children
            .iter()
            .rev()
            .filter_map(|frame| self.frame_to_client.get(frame).copied())
            .filter(|client| {
                self.clients.get(client).is_some_and(|state| {
                    state.kind == WindowKind::Normal
                        && state.visible_on(self.current_workspace)
                })
            })
            .collect();
        if stack.is_empty() {
            return Ok(());
        }
        let target = match self.active.and_then(|a| stack.iter().position(|c| *c == a)) {
            Some(index) => {
                let len = i32::try_from(stack.len()).unwrap_or(1).max(1);
                stack[(index as i32 + step).rem_euclid(len) as usize]
            }
            None => stack[0],
        };
        self.focus(target)
    }

    /// Drag-motion preview: candidate from shared `snap_target`, geometry from
    /// shared `snap_geometry`. `None` hides. Skips redraw when unchanged.
    /// No new native renderer: the compiled snap-preview surface moves/shows.
    pub(super) fn update_snap_preview(&mut self, target: SnapTarget, work: Rect, client: Window) {
        // Preview is cosmetic only: snapping on release is decided by the frame controller.
        if !crate::live_settings::snap_preview_enabled() {
            self.snap_preview.hide();
            return;
        }
        move_counters().record_snap_target(snap_target_bits(target));
        if target == SnapTarget::None {
            self.snap_preview.hide();
            return;
        }
        let current = self.clients.get(&client).map_or((800, 600), |state| {
            (
                i32::try_from(state.outer.width).unwrap_or(800),
                i32::try_from(state.outer.height).unwrap_or(600),
            )
        });
        let geometry = core_snap_geometry(
            target,
            to_core_rect(work),
            flamewm_api::Size::new(current.0, current.1),
        );
        let geometry = from_core_rect(geometry);
        let geometry = flamewm_api::Rect::new(
            geometry.x,
            geometry.y,
            i32::try_from(geometry.width).unwrap_or(1),
            i32::try_from(geometry.height).unwrap_or(1),
        );
        if self.snap_preview.is_current(
            target,
            geometry,
            snap_preview::DEFAULT_PREVIEW_OPACITY_PERCENT,
        ) {
            return;
        }
        {
            let _guard = flamewm_profiler::start("wm.move.preview");
            self.snap_preview.update(
                Some(target),
                Some(geometry),
                snap_preview::DEFAULT_PREVIEW_OPACITY_PERCENT,
            );
        }
        move_counters().record_preview();
    }

    pub(super) fn apply_frame_geometry(&mut self, client: Window) -> Result<(), ReplyError> {
        let (frame, outer, floating) = match self.clients.get(&client) {
            Some(state) => (state.frame, state.outer, state.is_floating()),
            None => return Ok(()),
        };
        let extents = self.extents_for(client);
        {
            let _guard = flamewm_profiler::start("wm.resize.shape");
            self.clear_frame_shape(frame, outer, floating)?;
        }
        {
            let _guard = flamewm_profiler::start("wm.resize.frame_configure");
            let titlebar = u32::try_from(extents.titlebar_h).unwrap_or(0);
            self.conn.configure_window(
                frame,
                &ConfigureWindowAux::new()
                    .x(outer.x)
                    .y(outer.y)
                    .border_width(u32::from(self.effective_frame_border(client)))
                    .width(outer.width.max(1))
                    .height(outer.height.max(titlebar + 1)),
            )?;
            self.set_frame_extents(client)?;
        }
        {
            let _guard = flamewm_profiler::start("wm.resize.client_configure");
            let (x, y, width, height) =
                lifecycle::client_configure(crate::client::rect_to_root(outer), extents);
            self.conn.configure_window(
                client,
                &ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(width)
                    .height(height),
            )?;
        }
        // Geometry commit only: child layout is idempotent for an unchanged
        // outer rect, and reconfiguring 12 children per commit sustains
        // ConfigureNotify churn under SUBSTRUCTURE_NOTIFY. Full relayout stays
        // on manage / WM_NORMAL_HINTS / final resize release.
        Ok(())
    }
}
