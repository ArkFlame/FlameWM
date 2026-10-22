use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn execute_effects(
        &mut self,
        client: Window,
        commit: &interaction::PointerCommit,
    ) -> Result<(), ReplyError> {
        let effect_boundary = self.resize_probe.as_ref().and_then(|probe| {
            if !probe.effect_seen {
                Some("first")
            } else if commit.ungrab {
                Some("last")
            } else {
                None
            }
        });
        if effect_boundary == Some("first") {
            self.emit_effect_boundary(client, "first", commit);
            if let Some(probe) = self.resize_probe.as_mut() {
                probe.effect_seen = true;
            }
        }
        if let Some((window, cursor)) = commit.grab {
            let ok = self.interaction_capture(window, cursor).unwrap_or(false);
            flamewm_debug::emit(
                flamewm_debug::WM_FRAME_GRAB,
                std::time::Duration::from_millis(100),
                || format!("client={client} window={window} result={ok}"),
            );
            if !ok {
                if commit.abort_on_grab_fail {
                    self.abort_session_to_idle(client);
                }
                self.resize_probe = None;
                return Ok(());
            }
        }
        let result: Result<(), ReplyError> = (|| {
            for effect in &commit.effects {
                match *effect {
                    // PointerCommit owns the single native grab transaction.
                    FrameEffect::Grab { .. } => {}
                    FrameEffect::Ungrab => {
                        if commit.ungrab {
                            self.release_pointer();
                        }
                    }
                    FrameEffect::ConfigureFrameRoot { x, y, w, h } => {
                        let Some(frame) = self.clients.get(&client).map(|state| state.frame) else {
                            continue;
                        };
                        let border = u32::from(self.effective_frame_border(client));
                        let result = self.conn.configure_window(
                            frame,
                            &ConfigureWindowAux::new()
                                .x(x)
                                .y(y)
                                .border_width(border)
                                .width(w.max(1) as u32)
                                .height(h.max(1) as u32),
                        );
                        if let Some(probe) = self.resize_probe.as_mut() {
                            if probe.client == client {
                                let outcome = match &result {
                                    Ok(_) => String::from("ok"),
                                    Err(error) => format!("error={error:?}"),
                                };
                                let detail =
                                    format!("xid={frame} rect=({x},{y},{w},{h}) result={outcome}");
                                if probe.native_first.is_none() {
                                    probe.native_first = Some(detail.clone());
                                }
                                probe.native_last = Some(detail);
                            }
                        }
                        emit_native_configure(
                            flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE,
                            client,
                            frame,
                            format!("effect=ConfigureFrameRoot rect=({x},{y},{w},{h})"),
                            &result,
                        );
                        result?;
                        self.commit_frame_rect(client, x, y, w, h);
                        self.set_frame_extents(client)?;
                    }
                    // Move is position-only: frame .x/.y + placement x/y commit;
                    // w/h and floating_restore size are preserved exactly.
                    FrameEffect::MoveFrameRoot { x, y } => {
                        let _guard = flamewm_profiler::start("wm.move.frame_position");
                        let Some(frame) = self.clients.get(&client).map(|state| state.frame) else {
                            continue;
                        };
                        let result = self
                            .conn
                            .configure_window(frame, &ConfigureWindowAux::new().x(x).y(y));
                        emit_native_configure(
                            flamewm_debug::WM_FRAME_GEOMETRY_MOVE_NATIVE,
                            client,
                            frame,
                            format!("effect=MoveFrameRoot position=({x},{y})"),
                            &result,
                        );
                        result?;
                        self.commit_frame_position(client, x, y);
                        move_counters().record_committed(1);
                    }
                    FrameEffect::ConfigureClientLocal { x, y, w, h } => {
                        let result = self.conn.configure_window(
                            client,
                            &ConfigureWindowAux::new()
                                .x(x)
                                .y(y)
                                .width(w.max(1))
                                .height(h.max(1)),
                        );
                        emit_native_configure(
                            flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE,
                            client,
                            client,
                            format!("effect=ConfigureClientLocal rect=({x},{y},{w},{h})"),
                            &result,
                        );
                        result?;
                    }
                    FrameEffect::LayoutInput => {
                        self.layout_input_children(client)?;
                    }
                    FrameEffect::PaintChrome => {
                        self.paint_chrome_paced(client, std::time::Instant::now())?;
                    }
                    FrameEffect::ApplyShape => {
                        let (frame, floating, outer) = match self.clients.get(&client) {
                            Some(state) => (state.frame, state.is_floating(), state.outer),
                            None => continue,
                        };
                        self.clear_frame_shape(frame, outer, floating)?;
                    }
                    FrameEffect::NotifyClientRoot { x, y, w, h } => {
                        let _guard = flamewm_profiler::start("wm.move.notify");
                        self.send_notify_event(
                            client,
                            clamp_i16(x),
                            clamp_i16(y),
                            clamp_u16(w.max(1) as u32),
                            clamp_u16(h.max(1)),
                        )?;
                    }
                    FrameEffect::SnapPreview { target } => {
                        if target.is_none() {
                            self.snap_preview.hide();
                        }
                    }
                    FrameEffect::DragOutline { rect } => match rect {
                        Some(frame) => self.snap_preview.show_outline(flamewm_api::Rect {
                            x: frame.x,
                            y: frame.y,
                            width: frame.w,
                            height: frame.h,
                        }),
                        None => self.snap_preview.hide(),
                    },
                    FrameEffect::Noop => {}
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.abort_session_to_idle(client);
        }
        if effect_boundary == Some("last") {
            self.emit_effect_boundary(client, "last", commit);
        }
        if commit.ungrab {
            self.resize_probe = None;
        }
        result
    }

    fn emit_effect_boundary(
        &self,
        client: Window,
        boundary: &str,
        commit: &interaction::PointerCommit,
    ) {
        let session = self.session_of(client);
        if let Some(probe) = self.resize_probe.as_ref() {
            emit_resize_probe(
                "effect",
                boundary,
                probe,
                Some(probe.last_pointer),
                session,
                Some(commit),
            );
        }
    }

    fn client_and_session_mut(
        &mut self,
        client: Window,
    ) -> Option<(&mut ManagedClient, &mut ControllerState)> {
        Some((
            self.clients.get_mut(&client)?,
            self.sessions.get_mut(&client)?,
        ))
    }

    pub(super) fn commit_frame_position(&mut self, client: Window, x: i32, y: i32) {
        let Some((state, session)) = self.client_and_session_mut(client) else {
            return;
        };
        let current = state.placement.current;
        let next = RootRect::new(x, y, current.w.max(1), current.h.max(1));
        state.placement.current = next;
        if state.placement.mode == PlacementMode::Floating {
            let restore = state.placement.floating_restore;
            state.placement.floating_restore =
                RootRect::new(x, y, restore.w.max(1), restore.h.max(1));
        }
        state.outer = crate::client::root_to_rect(next);
        session.placement.current = next;
    }

    pub(super) fn commit_frame_rect(&mut self, client: Window, x: i32, y: i32, w: i32, h: i32) {
        let Some((state, session)) = self.client_and_session_mut(client) else {
            return;
        };
        let frame = RootRect::new(x, y, w.max(1), h.max(1)).to_legacy();
        let mode = state.placement.mode;
        state.placement.current = crate::client::rect_to_root(frame);
        if mode == PlacementMode::Floating {
            state.placement.floating_restore = state.placement.current;
        }
        state.outer = frame;
        session.placement.current = state.placement.current;
    }

    pub(super) fn release_capture_unmap(&self) {
        if let Some(capture) = self.registry.capture() {
            let _ = self.conn.unmap_window(capture);
        }
    }

    pub(super) fn release_pointer(&self) {
        self.release_capture_unmap();
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
    }

    pub(super) fn interaction_capture(
        &mut self,
        grab_window: Window,
        cursor: flamewm_render_core::CursorKind,
    ) -> Result<bool, ReplyOrIdError> {
        let Some(grab_target) = self.live_pointer_grab_target(grab_window) else {
            self.release_pointer();
            return Ok(false);
        };
        let capture = match self.registry.capture() {
            Some(existing) => existing,
            None => {
                let id = self.conn.generate_id()?;
                let root = self.screen().root;
                self.conn.create_window(
                    COPY_DEPTH_FROM_PARENT,
                    id,
                    root,
                    clamp_i16(self.screen_rect.x),
                    clamp_i16(self.screen_rect.y),
                    clamp_u16(self.screen_rect.width),
                    clamp_u16(self.screen_rect.height),
                    0,
                    WindowClass::INPUT_ONLY,
                    0,
                    &CreateWindowAux::new(),
                )?;
                self.registry.set_capture(id);
                id
            }
        };
        if let Some(renderer) = self.renderer.as_ref() {
            let _ = renderer
                .borrow_mut()
                .define_cursor(u64::from(capture), cursor);
        }
        let _ = self.conn.map_window(capture);
        let result = self.conn.configure_window(
            capture,
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        );
        emit_native_configure(
            flamewm_debug::WM_FRAME_GEOMETRY_MOVE_NATIVE,
            0,
            capture,
            "effect=GrabCapture stack=above".to_owned(),
            &result,
        );
        let grab = self.conn.grab_pointer(
            false,
            grab_target,
            EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            0u32,
            0u32,
            CURRENT_TIME,
        );
        let reply = match grab
            .map_err(ReplyOrIdError::from)
            .and_then(|cookie| cookie.reply().map_err(ReplyOrIdError::from))
        {
            Ok(reply) => reply,
            Err(error) => {
                self.release_pointer();
                return Err(error);
            }
        };
        if reply.status == GrabStatus::SUCCESS {
            Ok(true)
        } else {
            self.release_pointer();
            flamewm_debug::emit(
                flamewm_debug::WM_FRAME_GRAB_FAIL,
                std::time::Duration::from_secs(1),
                || "grab refused".to_owned(),
            );
            Ok(false)
        }
    }

    pub(super) fn abort_session_to_idle(&mut self, client: Window) {
        if let Some(session) = self.sessions.get_mut(&client) {
            session.session.cancel();
        }
        self.snap_preview.hide();
        self.release_pointer();
    }

    pub(super) fn reduce_and_execute(
        &mut self,
        client: Window,
        event: FrameEvent,
    ) -> Result<(), ReplyError> {
        let Some(controller) = self.controller_for(client) else {
            return Ok(());
        };
        let placement = controller.placement.mode;
        let (source, region) = match event {
            FrameEvent::Press { source, .. } => (
                source,
                Some(
                    controller
                        .registry
                        .get(&source)
                        .copied()
                        .unwrap_or(FrameRegion::Client),
                ),
            ),
            _ => (0, None),
        };
        // Move accounting: pre-event session decides. Motion/Release out of a
        // Move session count as received; an all-Noop plan counts as coalesced
        // (zero-delta, no X commit). Committed counts land in MoveFrameRoot.
        let is_move_event = matches!(controller.session, InteractionSession::Move(_))
            && matches!(
                event,
                FrameEvent::Motion { .. } | FrameEvent::Release { .. }
            );
        if is_move_event {
            move_counters().record_received(1);
        }
        let pre_session = controller.session;
        let (next, commit) = {
            let _guard = if is_move_event {
                Some(flamewm_profiler::start("wm.move.plan_or_reduce"))
            } else {
                None
            };
            interaction::step_owned(controller, event)
        };
        let drag_away_from_tiled = matches!(pre_session, InteractionSession::Move(_))
            && matches!(
                placement,
                PlacementMode::Maximized | PlacementMode::Snapped(_)
            )
            && next.placement.mode == PlacementMode::Floating;
        let pointer = match event {
            FrameEvent::Press { pointer, .. }
            | FrameEvent::Motion { pointer }
            | FrameEvent::Release { pointer, .. } => Some(pointer),
            _ => None,
        };
        if let (Some(probe), Some(pointer)) = (self.resize_probe.as_mut(), pointer) {
            probe.last_pointer = pointer;
        }
        let resize_boundary = self.resize_probe.as_ref().and_then(|_| match event {
            FrameEvent::Press { .. } if matches!(region, Some(FrameRegion::Resize(_))) => {
                Some(("first", next.session))
            }
            FrameEvent::Release { .. } if matches!(pre_session, InteractionSession::Resize(_)) => {
                Some(("last", pre_session))
            }
            _ => None,
        });
        if let Some((boundary, session)) = resize_boundary {
            if let Some(probe) = self.resize_probe.as_ref() {
                emit_resize_probe("reduce", boundary, probe, pointer, session, Some(&commit));
            }
        }
        if matches!(event, FrameEvent::ToggleMax) {
            move_counters().record_toggle_max();
        }
        move_counters().record_placement_transition(placement, next.placement.mode);
        emit_pointer_trace(
            flamewm_debug::WM_FRAME_GEOMETRY_PLAN,
            "reduce",
            client,
            source,
            region,
            event,
            pre_session,
            Some(next.session),
            Some(&commit),
            Some(placement),
            Some(next.placement.mode),
            match event {
                FrameEvent::Snap { ref target } => Some(target as &dyn std::fmt::Debug),
                _ => None,
            },
            None,
        );
        if is_move_event
            && commit
                .effects
                .iter()
                .all(|e| matches!(e, FrameEffect::Noop))
        {
            move_counters().record_coalesced(1);
        }
        // Drag backdrop: freeze the content behind the frame when the move activates; drop it
        // when the move ends. Crops are taken per step in the titlebar fill.
        let was_dragging = drag_activated(&pre_session);
        let now_dragging = drag_activated(&next.session);
        if !was_dragging && now_dragging {
            self.begin_drag_backdrop(client);
            if matches!(next.session, InteractionSession::Resize(_)) {
                // Resize reuses the blur taken here for every step; it is recomputed after release.
                if let Some(frame) = self.clients.get(&client).map(|state| state.frame) {
                    crate::frame::titlebar_backdrop::set_drag_frozen(u64::from(frame), true);
                }
            }
        } else if was_dragging && !now_dragging {
            if let Some(frame) = self.clients.get(&client).map(|state| state.frame) {
                crate::frame::titlebar_backdrop::end_drag(u64::from(frame));
            }
        }
        // Zero-X-configure fast path: controller already returned Noop.
        self.store_controller(client, &next);
        let result = self.execute_effects(client, &commit);
        if result.is_ok() && drag_away_from_tiled {
            self.mark_windows();
            self.publish_window_state(client)?;
        }
        result
    }
}
