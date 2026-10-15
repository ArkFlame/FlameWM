use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn handle_enter(&mut self, event: EnterNotifyEvent) -> Result<(), ReplyError> {
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        // Passive observers only: Enter/Motion-to-idle-session repaint paths
        // must not reconfigure native input children, or each Enter =>
        // ConfigureNotify => Enter feedback loop sustains a motion storm.
        // Focus/hover state updates stay; layout stays frozen.
        let active_session = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        if active_session {
            return Ok(());
        }
        // Default is click-to-focus: pointer entry (title bar included) only focuses when the user
        // opted in with focusFollowsMouse.
        if crate::live_settings::focus_follows_mouse() {
            self.focus_hover(client_id)?;
        }
        let _ = self.reduce_and_execute(client_id, FrameEvent::Enter);
        Ok(())
    }

    pub(super) fn handle_leave(&mut self, event: LeaveNotifyEvent) -> Result<(), ReplyError> {
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        let active_session = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        if active_session {
            return Ok(());
        }
        let _ = self.reduce_and_execute(client_id, FrameEvent::Leave);
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.clear_hover();
        }
        self.paint_chrome(client_id)?;
        Ok(())
    }

    pub(super) fn handle_button_press(&mut self, event: ButtonPressEvent) -> Result<(), ReplyError> {
        if self.clients.contains_key(&event.event) {
            return self.handle_client_press(event);
        }
        let target = resolve_frame_event_source(&self.registry, event.event, event.child);
        if let Some(client) = self.client_for(target) {
            self.cancel_open_for(client)?;
        }
        if event.detail != BUTTON_PRIMARY {
            if let Some(session) = self
                .client_for(target)
                .and_then(|client| self.sessions.get_mut(&client))
            {
                session.last_title_click = None;
            }
            return Ok(());
        }
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        self.focus(client_id)?;
        // Route press through controller reduce(); execute FrameEffects diff-aware.
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let had_session = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        let _ = had_session;
        let event = FrameEvent::Press { source, pointer };
        let pre = self
            .sessions
            .get(&client_id)
            .map_or(InteractionSession::Idle, |session| session.session);
        let region = self
            .registry
            .lookup_by_xid(source)
            .map_or(FrameRegion::Client, |target| target.region);
        if let FrameRegion::Resize(_) = region {
            self.resize_probe = Some(ResizeProbe {
                client: client_id,
                source,
                region,
                start_pointer: pointer,
                last_pointer: pointer,
                effect_seen: false,
                native_first: None,
                native_last: None,
            });
            if let Some(probe) = self.resize_probe.as_ref() {
                emit_resize_probe("press", "first", probe, Some(pointer), pre, None);
            }
        }
        emit_pointer_trace(
            flamewm_debug::WM_FRAME_EVENT_DISPATCH,
            "button_press",
            client_id,
            source,
            Some(region),
            event,
            pre,
            None,
            None,
            self.clients
                .get(&client_id)
                .map(|state| state.placement.mode),
            None,
            None,
            None,
        );
        match self.reduce_and_execute(client_id, event) {
            Ok(()) => {
                let grabbed = self
                    .sessions
                    .get(&client_id)
                    .map_or(false, |session| !session.session.is_idle());
                if !grabbed {
                    // No session (grab failed or Noop region): treat as control click.
                    self.handle_control_click(client_id)?;
                }
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    /// Press inside a managed client (sync passive grab). Focus/raise when needed, then
    /// always release the frozen pointer so the client receives the press.
    pub(super) fn handle_client_press(&mut self, event: ButtonPressEvent) -> Result<(), ReplyError> {
        let client = event.event;
        if let Some(session) = self.sessions.get_mut(&client) {
            session.last_title_click = None;
        }
        let focus = if matches!(event.detail, 1..=3) && should_focus_on_press(self.active, client) {
            self.focus(client)
        } else {
            Ok(())
        };
        self.conn.allow_events(Allow::REPLAY_POINTER, event.time)?;
        focus
    }

    pub(super) fn handle_control_click(&mut self, client_id: Window) -> Result<(), ReplyError> {
        let Some(res) = self.registry.lookup_by_client(client_id) else {
            return Ok(());
        };
        // Control clicks only arm via controller; a press that produced no
        // session means the region was Client/Frame or grab failed. Check
        // which control (if any) via registry region is impossible without
        // coords, so fall back to titlebar control hit via pointer query.
        let _ = res;
        Ok(())
    }

    pub(super) fn handle_button_release(&mut self, event: ButtonReleaseEvent) -> Result<(), ReplyError> {
        if event.detail != BUTTON_PRIMARY {
            return Ok(());
        }
        // Route release through controller reduce(); control clicks execute here.
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.session_client().or_else(|| self.client_for(source)) else {
            return Ok(());
        };
        // Snapshot the release origin BEFORE the reducer runs: the release
        // reducer always idles the session, so post-reducer reads cannot tell
        // Move from resize/control-terminal origins.
        let release_move =
            self.sessions
                .get(&client_id)
                .and_then(|session| match session.session {
                    InteractionSession::Move(move_session) => Some(move_session),
                    _ => None,
                });
        let release_from_move = release_move.is_some();
        let release_from_resize = self
            .sessions
            .get(&client_id)
            .is_some_and(|session| matches!(session.session, InteractionSession::Resize(_)));
        let (was_control, control) =
            match self.sessions.get(&client_id).map(|session| session.session) {
                Some(InteractionSession::ControlPress(session)) => (
                    true,
                    if session.armed {
                        Some(session.control)
                    } else {
                        None
                    },
                ),
                _ => (false, None),
            };
        if matches!(control, Some(FrameControl::MaximizeRestore)) {
            // The controller would toggle and move the frame during this release, before any
            // overlay can capture it. Only end the press here, then run the animated toggle.
            self.reduce_and_execute(client_id, FrameEvent::Cancel)?;
            move_counters().record_toggle_max();
            self.toggle_maximize(client_id)?;
            return Ok(());
        }
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let mode_before = self.clients.get(&client_id).map(|s| s.placement.mode);
        let _ = self.reduce_and_execute(
            client_id,
            FrameEvent::Release {
                pointer,
                time_ms: event.time,
            },
        );
        if release_from_move {
            self.snap_preview.hide();
        }
        let mode_after = self.clients.get(&client_id).map(|s| s.placement.mode);
        let drag_active = release_move.is_some_and(|m| m.anchor.is_activated());
        if release_from_move && !drag_active && mode_before != mode_after {
            // Title double-click toggled maximize in the reducer: publish state.
            move_counters().record_toggle_max();
            self.mark_windows();
            self.publish_window_state(client_id)?;
            return Ok(());
        }
        if let Some(control) = control {
            match control {
                FrameControl::Close => self.request_close(client_id)?,
                FrameControl::MaximizeRestore => {
                    move_counters().record_toggle_max();
                    self.mark_windows();
                    self.publish_window_state(client_id)?;
                }
                FrameControl::Minimize => self.minimize(client_id)?,
            }
            return Ok(());
        }
        if was_control {
            return Ok(());
        }
        // Move-release policy: only a Move session may evaluate/commit a
        // release action. Resize/control-terminal releases never reach this
        // path.
        if !release_from_move {
            if release_from_resize {
                // CONTRACT-REGRESSION: resize release never evaluates snap.
                // OBSERVABLE: bounded counter appears in the move summary.
                move_counters().record_resize_snap_blocked();
            }
            return Ok(());
        }
        let target = {
            let _guard = flamewm_profiler::start("wm.move.snap_target");
            self.snap_target_at(event.root_x, event.root_y, self.clients.get(&client_id).map_or(self.work_area(), |s| s.outer))
        };
        move_counters().record_snap_target(snap_target_bits(target));
        let placement = self
            .clients
            .get(&client_id)
            .map(|state| state.placement.mode);
        match release_action_for_move(release_move, placement, target) {
            ReleaseAction::None => {}
            ReleaseAction::Maximize => self.toggle_maximize(client_id)?,
            ReleaseAction::Snap(target) => {
                self.reduce_and_execute(client_id, FrameEvent::Snap { target })?;
                self.mark_windows();
                self.publish_window_state(client_id)?;
            }
        }
        Ok(())
    }

    pub(super) fn handle_motion(&mut self, event: MotionNotifyEvent) -> Result<(), ReplyError> {
        self.probe_motion_entry(&event);
        // Root points during active session; route through controller reduce().
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.session_client().or_else(|| self.client_for(source)) else {
            return Ok(());
        };
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let move_session =
            self.sessions
                .get(&client_id)
                .and_then(|session| match session.session {
                    InteractionSession::Move(move_session) => Some(move_session),
                    _ => None,
                });
        let move_active = move_session.is_some();
        let active = self
            .sessions
            .get(&client_id)
            .map_or(false, |session| !session.session.is_idle());
        if active {
            let frame_event = FrameEvent::Motion { pointer };
            let pre = self
                .sessions
                .get(&client_id)
                .map_or(InteractionSession::Idle, |session| session.session);
            // One snap target per motion, shared by the trace and the preview.
            let motion_snap = move_active.then(|| {
                let _guard = flamewm_profiler::start("wm.move.snap_target");
                let fallback = self.clients.get(&client_id).map_or(self.work_area(), |s| s.outer);
                let (output, work) = self.snap_zones_at(event.root_x, event.root_y, fallback);
                let target = core_snap_target_in_output(
                    flamewm_api::Point::new(i32::from(event.root_x), i32::from(event.root_y)),
                    to_core_rect(output),
                    to_core_rect(work),
                );
                (target, work)
            });
            emit_pointer_trace(
                flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                "motion",
                client_id,
                source,
                Some(
                    self.registry
                        .lookup_by_xid(source)
                        .map_or(FrameRegion::Client, |target| target.region),
                ),
                frame_event,
                pre,
                None,
                None,
                self.clients
                    .get(&client_id)
                    .map(|state| state.placement.mode),
                None,
                motion_snap.as_ref().map(|(target, _)| target as &dyn std::fmt::Debug),
                motion_snap.map(|(target, _)| target != SnapTarget::None),
            );
            {
                let _guard = if move_active {
                    Some(flamewm_profiler::start("wm.move.total"))
                } else {
                    None
                };
                let _ = self.reduce_and_execute(client_id, frame_event);
            }
            // Snap preview requires an already activated move and the
            // post-reducer placement to be Floating. Resize and control
            // motion never reach the preview path.
            let preview_allowed = activated_move(move_session).is_some()
                && self
                    .clients
                    .get(&client_id)
                    .is_some_and(|state| state.placement.mode == PlacementMode::Floating);
            if preview_allowed {
                if let Some((target, work)) = motion_snap {
                    self.update_snap_preview(target, work, client_id);
                }
            } else if move_active {
                self.snap_preview.hide();
            }
            return Ok(());
        }
        // Idle hover: track control hover, repaint chrome on change.
        // Never reconfigure input children here: Enter/Motion crossover
        // between adjacent children would otherwise reconfigure on every
        // crossing and sustain ConfigureNotify churn. (WM_NORMAL_HINTS
        // updates still relayout via handle_property; that path is rare.)
        let hover = self.idle_hover(client_id, event.event_x, event.event_y, source);
        let changed = if let Some(state) = self.clients.get_mut(&client_id) {
            let changed = state.hover_control != hover;
            state.hover_control = hover;
            changed
        } else {
            false
        };
        if changed {
            self.paint_chrome(client_id)?;
        }
        Ok(())
    }

    pub(super) fn session_client(&self) -> Option<Window> {
        self.sessions.iter().find_map(|(client, session)| {
            if !session.session.is_idle() {
                Some(*client)
            } else {
                None
            }
        })
    }

    pub(super) fn idle_hover(
        &self,
        client_id: Window,
        x: i16,
        y: i16,
        source: Window,
    ) -> Option<FrameControl> {
        let region = self
            .registry
            .lookup_by_xid(source)
            .map(|target| target.region);
        match region {
            Some(FrameRegion::Control(control)) => Some(control),
            _ => {
                let state = self.clients.get(&client_id)?;
                crate::frame::input::frame_control_fallback(
                    state.outer.width,
                    u16::try_from(self.extents_for(client_id).titlebar_h).unwrap_or(0),
                    x,
                    y,
                    state.hover_control,
                )
            }
        }
    }
}
