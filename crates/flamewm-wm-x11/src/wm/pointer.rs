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
        // A pointer warp during an active drag may enter a *different*
        // window. Do not let focus-follows-mouse steal focus from the window
        // we are carrying between workspaces. Ignore passive enters until
        // the pointer interaction completes, regardless of the event client.
        if self.session_client().is_some() {
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
        if self.is_session_active(client_id) {
            return Ok(());
        }
        let _ = self.reduce_and_execute(client_id, FrameEvent::Leave);
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.clear_hover();
        }
        self.paint_chrome(client_id)?;
        Ok(())
    }

    pub(super) fn handle_button_press(
        &mut self,
        event: ButtonPressEvent,
    ) -> Result<(), ReplyError> {
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
        self.edge_drag = None;
        let source = resolve_frame_event_source(&self.registry, event.event, event.child);
        let Some(client_id) = self.client_for(source) else {
            return Ok(());
        };
        self.focus(client_id)?;
        // Route press through controller reduce(); execute FrameEffects diff-aware.
        let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
        let event = FrameEvent::Press { source, pointer };
        let pre = self.session_of(client_id);
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
        let _ = self.reduce_and_execute(client_id, event);
        Ok(())
    }

    /// Meta (X11 Mod4) + primary button begins the canonical titlebar move
    /// regardless of client-area coordinates. Other buttons/modifiers replay
    /// unchanged, preserving Dolphin selection and context menus.
    fn is_meta_move_press(detail: u8, state: u16) -> bool {
        detail == BUTTON_PRIMARY && state & 0x0040 != 0
    }

    /// Ctrl + Super (Mod4) + right button (3) starts a resize, the GNOME/KDE convention for
    /// modifier-drag resize. Left stays Meta-move. Right is the chosen button because
    /// Ctrl+Super+left is still matched by the move rule above.
    fn is_meta_resize_press(detail: u8, state: u16) -> bool {
        const BUTTON_RIGHT: u8 = 3;
        detail == BUTTON_RIGHT && state & 0x0004 != 0 && state & 0x0040 != 0
    }

    /// Corner for a modifier-drag resize: the pointer's quadrant inside `outer` picks the
    /// corner, so the edge that moves is the one nearest the grab point. Directions match
    /// `resize_edges_for_direction` (0 top-left, 2 top-right, 4 bottom-right, 6 bottom-left).
    fn meta_resize_direction(outer: crate::geometry::Rect, pointer: RootPoint) -> u32 {
        let left = pointer.x < outer.x + outer.width as i32 / 2;
        let top = pointer.y < outer.y + outer.height as i32 / 2;
        match (top, left) {
            (true, true) => 0,
            (true, false) => 2,
            (false, false) => 4,
            (false, true) => 6,
        }
    }

    /// Press inside a managed client (sync passive grab). Focus/raise when needed, then
    /// always release the frozen pointer so the client receives the press.
    pub(super) fn handle_client_press(
        &mut self,
        event: ButtonPressEvent,
    ) -> Result<(), ReplyError> {
        let client = event.event;
        if Self::is_meta_resize_press(event.detail, u16::from(event.state)) {
            if let Some(outer) = self.clients.get(&client).map(|state| state.outer) {
                let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
                let direction = Self::meta_resize_direction(outer, pointer);
                // Consume the frozen press as the Meta-move path does, then reuse the
                // edge-handle resize reducer; the handles are mapped for client-drawn frames too.
                self.conn.allow_events(Allow::ASYNC_POINTER, event.time)?;
                self.begin_client_moveresize(client, pointer, direction)?;
                return Ok(());
            }
        }
        if Self::is_meta_move_press(event.detail, u16::from(event.state)) {
            if let Some(resources) = self.registry.lookup_by_client(client) {
                self.super_tap.press_other();
                self.focus(client)?;
                // The sync passive grab froze this physical click. Consume it
                // rather than replaying it into Dolphin, then use the exact
                // same move controller and pointer capture as title dragging.
                self.conn.allow_events(Allow::ASYNC_POINTER, event.time)?;
                let pointer = RootPoint::new(i32::from(event.root_x), i32::from(event.root_y));
                self.reduce_and_execute(
                    client,
                    FrameEvent::Press {
                        source: resources.title_drag,
                        pointer,
                    },
                )?;
                return Ok(());
            }
        }
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

    pub(super) fn handle_button_release(
        &mut self,
        event: ButtonReleaseEvent,
    ) -> Result<(), ReplyError> {
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
        // A layout zone wins over the free move: end the drag without moving the frame to the
        // pointer first, then apply only the layout.
        if release_from_move {
            let fallback = self
                .clients
                .get(&client_id)
                .map_or(self.work_area(), |s| s.outer);
            let target = self.snap_target_at(event.root_x, event.root_y, fallback);
            let action = release_action_for_move(release_move, mode_before, target);
            if action != ReleaseAction::None {
                self.reduce_and_execute(client_id, FrameEvent::Cancel)?;
                self.edge_drag = None;
                move_counters().record_snap_target(snap_target_bits(target));
                match action {
                    ReleaseAction::None => {}
                    ReleaseAction::Maximize => self.toggle_maximize(client_id)?,
                    ReleaseAction::Snap(target) => {
                        self.reduce_and_execute(client_id, FrameEvent::Snap { target })?;
                        self.mark_windows();
                        self.publish_window_state(client_id)?;
                    }
                }
                return Ok(());
            }
        }
        let _ = self.reduce_and_execute(
            client_id,
            FrameEvent::Release {
                pointer,
                time_ms: event.time,
            },
        );
        self.edge_drag = None;
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
        let active = self.is_session_active(client_id);
        if active {
            let frame_event = FrameEvent::Motion { pointer };
            let pre = self.session_of(client_id);
            // One snap target per motion, shared by the trace and the preview.
            let motion_snap = move_active.then(|| {
                let _guard = flamewm_profiler::start("wm.move.snap_target");
                let fallback = self
                    .clients
                    .get(&client_id)
                    .map_or(self.work_area(), |s| s.outer);
                let (output, work) = self.snap_zones_at(event.root_x, event.root_y, fallback);
                let target = core_snap_target_in_output(
                    flamewm_api::Point::new(i32::from(event.root_x), i32::from(event.root_y)),
                    to_core_rect(output),
                    to_core_rect(work),
                );
                (target, work)
            });
            // Same pointer and snap target as the last applied move: nothing new to
            // apply. Edge dwell is time-based, so it still runs; release never gets here.
            let repeat = match (&motion_snap, self.sessions.get_mut(&client_id)) {
                (Some((target, _)), Some(session)) => {
                    motion_repeats(&mut session.last_motion, (pointer, *target))
                }
                _ => false,
            };
            if repeat {
                self.evaluate_edge_drag(client_id, pointer, std::time::Instant::now())?;
                return Ok(());
            }
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
                motion_snap
                    .as_ref()
                    .map(|(target, _)| target as &dyn std::fmt::Debug),
                motion_snap.map(|(target, _)| target != SnapTarget::None),
            );
            // Layout wins over the free move: inside a snap zone the frame stays where the
            // drag started and only the zone preview shows; release applies the layout.
            if let Some((target, work)) = motion_snap {
                if zone_preview_holds(
                    activated_move(move_session).is_some(),
                    target,
                    crate::live_settings::snap_preview_enabled(),
                ) {
                    self.update_snap_preview(target, work, client_id);
                    self.evaluate_edge_drag(client_id, pointer, std::time::Instant::now())?;
                    return Ok(());
                }
            }
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
                // XP outline mode owns the drag feedback: a snap-zone update would hide it.
                if let Some((target, work)) = motion_snap {
                    if crate::live_settings::show_content_while_dragging() {
                        self.update_snap_preview(target, work, client_id);
                    }
                }
            } else if move_active {
                self.snap_preview.hide();
            }
            self.evaluate_edge_drag(client_id, pointer, std::time::Instant::now())?;
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

    /// Shared edge policy for live motion and the one-shot dwell deadline.
    /// The pointer must still be at the edge of an activated titlebar move.
    pub(super) fn evaluate_edge_drag(
        &mut self,
        client_id: Window,
        pointer: RootPoint,
        now: std::time::Instant,
    ) -> Result<(), ReplyError> {
        let moving = self.sessions.get(&client_id).is_some_and(
            |state| matches!(state.session, InteractionSession::Move(m) if m.anchor.is_activated()),
        );
        let edge = if moving && self.workspace_count > 1 {
            // Do not arm a zero-duration reactor timer for a row with only
            // one workspace; such a row has no destination in either direction.
            drag_edge_at(pointer, self.screen_rect).filter(|side| {
                drag_edge_target(self.current_workspace, self.workspace_count, *side).is_some()
            })
        } else {
            None
        };
        if let Some(side) = edge {
            let (pending, ready) = drag_edge_ready(self.edge_drag, client_id, side, now);
            self.edge_drag = pending;
            if ready
                && self
                    .mouse_workspace_cooldowns
                    .remaining(side.axis(), now)
                    .is_zero()
            {
                if let Some(target) =
                    drag_edge_target(self.current_workspace, self.workspace_count, side)
                {
                    let frame_height = self
                        .clients
                        .get(&client_id)
                        .map_or(1, |state| state.placement.current.h);
                    let destination =
                        drag_edge_destination(pointer, self.screen_rect, side, frame_height);
                    let previous = self.current_workspace;
                    self.switch_workspace_dragging(client_id, target, destination, side)?;
                    if self.current_workspace != previous {
                        // Start cooldown after the real native transfer has completed.
                        self.mouse_workspace_cooldowns
                            .record(side.axis(), std::time::Instant::now());
                    }
                    self.edge_drag = None;
                }
            }
        } else {
            self.edge_drag = None;
        }
        Ok(())
    }

    /// The reactor calls this at a pending edge deadline, even when the user
    /// holds the pointer completely stationary. Verify its current root
    /// position; never switch solely on an obsolete earlier event.
    pub(super) fn tick_edge_drag(&mut self) -> Result<(), ReplyError> {
        let Some(pending) = self.edge_drag else {
            return Ok(());
        };
        let now = std::time::Instant::now();
        if now.saturating_duration_since(pending.since)
            < std::time::Duration::from_millis(u64::from(DRAG_EDGE_DWELL_MS))
            || !self
                .mouse_workspace_cooldowns
                .remaining(pending.side.axis(), now)
                .is_zero()
        {
            return Ok(());
        }
        let reply = self.conn.query_pointer(self.screen().root)?.reply()?;
        // A pointer on another X screen does not have root coordinates in
        // this WM's screen. Cancel the dwell rather than interpreting them
        // as a side-edge crossing on our root.
        if !reply.same_screen {
            self.edge_drag = None;
            return Ok(());
        }
        // A queued ButtonRelease may not have reached this loop yet. A stale
        // Move session alone is not proof that the mouse button is still down.
        // Never transfer a window after the physical drag has ended.
        if !reply.mask.contains(KeyButMask::BUTTON1) {
            self.edge_drag = None;
            return Ok(());
        }
        let pointer = RootPoint::new(i32::from(reply.root_x), i32::from(reply.root_y));
        self.evaluate_edge_drag(pending.client, pointer, now)
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
            // Resize strips report y relative to themselves, so the titlebar-band fallback would
            // mistake the bottom edge for the titlebar and light a control at the same x.
            Some(FrameRegion::Resize(_)) => None,
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

/// An activated move inside a snap zone shows that zone's preview instead of following the
/// pointer. Leaving the zone (`SnapTarget::None`) hands the drag back to the free move.
fn zone_preview_holds(activated: bool, target: SnapTarget, preview_enabled: bool) -> bool {
    activated && preview_enabled && target != SnapTarget::None
}

/// Records `key` as the last applied move; true when it repeats the previous one.
fn motion_repeats(
    last: &mut Option<(RootPoint, SnapTarget)>,
    key: (RootPoint, SnapTarget),
) -> bool {
    std::mem::replace(last, Some(key)) == Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_move_applies_once_per_distinct_pointer_and_target() {
        let mut last = None;
        let p = RootPoint::new(10, 20);
        let q = RootPoint::new(11, 20);
        // 10 identical motions apply once; then a moved pointer applies once more.
        let applied = (0..10)
            .filter(|_| !motion_repeats(&mut last, (p, SnapTarget::None)))
            .count();
        assert_eq!(applied, 1);
        assert!(!motion_repeats(&mut last, (q, SnapTarget::None)));
        assert!(motion_repeats(&mut last, (q, SnapTarget::None)));
        // Same pointer but a new snap target applies again.
        assert!(!motion_repeats(&mut last, (q, SnapTarget::LeftHalf)));
    }

    #[test]
    fn zone_preview_only_for_activated_move_in_a_zone() {
        assert!(zone_preview_holds(true, SnapTarget::LeftHalf, true));
        assert!(!zone_preview_holds(true, SnapTarget::None, true));
        assert!(!zone_preview_holds(false, SnapTarget::LeftHalf, true));
        assert!(!zone_preview_holds(true, SnapTarget::LeftHalf, false));
    }
}
