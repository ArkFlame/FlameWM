use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn handle_configure_notify(
        &mut self,
        event: ConfigureNotifyEvent,
    ) -> Result<(), ReplyError> {
        // Observation-only: classify and compare through configure policy;
        // never commit geometry from a ConfigureNotify.
        if event.response_type & 0x80 != 0 {
            return Ok(());
        }
        if event.window == self.screen().root {
            configure_bump!(notify_root, notify_root_total);
            self.screen_rect.width = u32::from(event.width).max(1);
            self.screen_rect.height = u32::from(event.height).max(1);
            // An exclusive game may temporarily resize the root framebuffer. Keep normal
            // windows and taskbar reservations at the saved desktop layout; only the game
            // framebuffer follows the temporary screen extent. The normal reflow resumes after
            // fullscreen exits and the desktop mode tracker restores the root resolution.
            let fullscreen: Vec<Window> = self
                .clients
                .iter()
                .filter(|(_, state)| state.is_fullscreen())
                .map(|(&client, _)| client)
                .collect();
            if !fullscreen.is_empty() {
                if self.output_work_areas.len() <= 1 {
                    for client in fullscreen {
                        if let Some(state) = self.clients.get_mut(&client) {
                            state.set_outer(self.screen_rect);
                        }
                        self.apply_frame_geometry(client)?;
                        self.publish_window_state(client)?;
                    }
                }
                self.display_topology_dirty.mark();
            } else {
                self.mark_work_area();
                self.reconcile_work_area()?;
                self.publish_workspace_metadata()?;
            }
            return Ok(());
        }
        // Classify the observed window before touching placement. Frame,
        // client, and InputOnly children all resolve to one managed client
        // via `client_for`, so disambiguate with the authoritative frame id
        // and the input-child registry first.
        let Some(client_id) = self.client_for(event.window) else {
            return Ok(());
        };
        let (frame_id, expected_frame, mode) = match self.clients.get(&client_id) {
            Some(state) => (state.frame, state.placement.current, state.placement.mode),
            None => return Ok(()),
        };
        let _guard = flamewm_profiler::start("wm.configure.notify.observe");
        let extents = self.extents_for(client_id);
        let class = configure::classify_notify(
            event.window,
            frame_id,
            client_id,
            self.registry.lookup_by_xid(event.window).is_some(),
            false,
            true,
        );
        match class {
            configure::NotifyClass::FrameObserved => {
                // Expected frame observation: outer rect in root coords.
                configure_bump!(notify_frame, notify_frame_total);
                let observed = RootRect::new(
                    i32::from(event.x),
                    i32::from(event.y),
                    i32::from(event.width).max(1),
                    i32::from(event.height).max(1),
                );
                match configure::observe_frame(expected_frame, observed) {
                    configure::Observation::Current => {
                        configure_bump!(notify_expected, notify_expected_total);
                    }
                    configure::Observation::Mismatch => {
                        configure_bump!(notify_mismatch, notify_mismatch_total);
                        flamewm_debug::emit(
                            flamewm_debug::WM_CONFIGURE_MISMATCH,
                            std::time::Duration::from_secs(1),
                            || {
                                format!(
                                    "client={client_id} source=frame expected=({},{},{},{}) observed=({},{},{},{}) mode={mode:?}",
                                    expected_frame.x,
                                    expected_frame.y,
                                    expected_frame.w,
                                    expected_frame.h,
                                    observed.x,
                                    observed.y,
                                    observed.w,
                                    observed.h,
                                )
                            },
                        );
                    }
                }
            }
            configure::NotifyClass::ClientObserved => {
                // Expected client observation: local (0,titlebar) + interior size.
                configure_bump!(notify_client, notify_client_total);
                let (ox, oy, expected_w, expected_h) =
                    configure::expected_client_geometry(expected_frame, extents);
                match configure::observe_client(
                    expected_frame,
                    extents,
                    (
                        i32::from(event.x),
                        i32::from(event.y),
                        i32::from(event.width),
                        i32::from(event.height),
                    ),
                ) {
                    configure::Observation::Current => {
                        configure_bump!(notify_expected, notify_expected_total);
                    }
                    configure::Observation::Mismatch => {
                        configure_bump!(notify_mismatch, notify_mismatch_total);
                        flamewm_debug::emit(
                            flamewm_debug::WM_CONFIGURE_MISMATCH,
                            std::time::Duration::from_secs(1),
                            || {
                                format!(
                                    "client={client_id} source=client expected=({ox},{oy},{expected_w},{expected_h}) observed=({},{},{},{}) mode={mode:?}",
                                    event.x, event.y, event.width, event.height,
                                )
                            },
                        );
                    }
                }
            }
            configure::NotifyClass::InputChildObserved => {
                // Registered InputOnly child: WM-owned detail, never placement.
                configure_bump!(notify_input_child, notify_input_child_total);
                configure_bump!(notify_expected, notify_expected_total);
            }
            configure::NotifyClass::SyntheticEchoDrop | configure::NotifyClass::StaleUnknown => {}
        }
        emit_configure_summary();
        Ok(())
    }

    pub(super) fn handle_configure_request(
        &mut self,
        event: ConfigureRequestEvent,
    ) -> Result<(), ReplyError> {
        let _guard = flamewm_profiler::start("wm.configure.request.total");
        let Some(client_id) = self.client_for(event.window) else {
            let override_redirect = self
                .conn
                .get_window_attributes(event.window)?
                .reply()?
                .override_redirect;
            let kind = self.window_kind(event.window, override_redirect)?;
            self.conn.configure_window(
                event.window,
                &desktop_stack_policy(ConfigureWindowAux::from_configure_request(&event), kind),
            )?;
            return Ok(());
        };
        let Some(controller) = self.controller_for(client_id) else {
            return Ok(());
        };
        configure_bump!(request_received, request_received_total);
        // Stack/sibling intent is orthogonal to the geometry decision:
        // carried alongside via configure policy, never flips accept/refuse.
        let has_stack = event.value_mask.contains(ConfigWindow::STACK_MODE);
        let stack = has_stack.then(|| {
            let frame = self
                .clients
                .get(&client_id)
                .map(|state| state.frame)
                .unwrap_or(client_id);
            let sibling = if event.value_mask.contains(ConfigWindow::SIBLING) && event.sibling != 0
            {
                self.client_for(event.sibling)
                    .and_then(|id| self.clients.get(&id).map(|state| state.frame))
                    .or(Some(event.sibling))
            } else {
                None
            };
            (frame, event.stack_mode, sibling)
        });
        // Single geometry policy: exactly one owner decides accept/refuse.
        let (decision, _) = configure::carry_stack_intent(
            configure::decide_request(&controller.session, controller.placement.mode),
            has_stack,
        );
        // Active gesture owns geometry, and WM-owned modes stay authoritative against client
        // geometry: acknowledge current root geometry, never mutate the frame from the request.
        if decision != configure::RequestDecision::AcceptOnce {
            if decision == configure::RequestDecision::RefuseInteractive {
                configure_bump!(
                    request_refused_interactive,
                    request_refused_interactive_total
                );
            } else {
                configure_bump!(request_refused_mode, request_refused_mode_total);
            }
            self.send_configure_notify(client_id)?;
            emit_configure_summary();
            if let Some((frame, mode, sibling)) = stack {
                self.restack(frame, mode, sibling)?;
                self.publish_client_list()?;
            }
            return Ok(());
        }
        let extents = controller.extents;
        let prev_root = frame_to_client_root(controller.placement.current, extents);
        let (prev_origin, prev_size) = (prev_root.origin(), prev_root.size_u32());
        let mask = event.value_mask;
        // ICCCM 4.1.5: ConfigureRequest coordinates from a top-level client
        // are in root coordinates irrespective of reparenting (verbatim
        // client-root origin; no frame-offset subtraction).
        let req_origin = configure::request_origin(
            mask.contains(ConfigWindow::X),
            mask.contains(ConfigWindow::Y),
            i32::from(event.x),
            i32::from(event.y),
            prev_origin,
        );
        let req_size = if mask.contains(ConfigWindow::WIDTH) || mask.contains(ConfigWindow::HEIGHT)
        {
            Some((
                if mask.contains(ConfigWindow::WIDTH) {
                    u32::from(event.width).max(1)
                } else {
                    prev_size.0
                },
                if mask.contains(ConfigWindow::HEIGHT) {
                    u32::from(event.height).max(1)
                } else {
                    prev_size.1
                },
            ))
        } else {
            None
        };
        let req = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            start: PlacementSnapshot {
                mode: controller.placement.mode,
                rect: controller.placement.current,
            },
            pointer_delta: (0, 0),
            edges: crate::frame::model::ResizeEdges::none(),
            work_area: controller.work_area,
            outputs: controller.outputs.as_slice(),
            hints: controller.hints,
            frame_extents: extents,
        };
        let plan = plan_client_configure(&req, req_origin, req_size);
        if !plan.noop {
            configure_bump!(request_accepted, request_accepted_total);
            let mode = controller.placement.mode;
            if let Some(state) = self.clients.get_mut(&client_id) {
                state.placement.mode = mode;
                state.placement.current = plan.frame;
                if mode == PlacementMode::Floating {
                    state.placement.floating_restore = plan.frame;
                }
                state.outer = crate::client::root_to_rect(plan.frame);
            }
            if let Some(session) = self.sessions.get_mut(&client_id) {
                session.placement.current = plan.frame;
            }
            self.apply_frame_geometry(client_id)?;
        }
        self.send_configure_notify(client_id)?;
        emit_configure_summary();
        if let Some((frame, mode, sibling)) = stack {
            self.restack(frame, mode, sibling)?;
            self.publish_client_list()?;
        }
        Ok(())
    }

    pub(super) fn restack(
        &self,
        window: Window,
        mode: StackMode,
        sibling: Option<Window>,
    ) -> Result<(), ReplyError> {
        let mut aux = ConfigureWindowAux::new().stack_mode(mode);
        if let Some(sibling) = sibling {
            aux = aux.sibling(sibling);
        }
        self.conn.configure_window(window, &aux)?;
        Ok(())
    }
}
