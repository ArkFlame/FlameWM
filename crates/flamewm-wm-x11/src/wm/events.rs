use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    /// Dispatches one event. A request that raced a client's destruction (BadWindow and friends)
    /// is expected on a window manager and must never end the session: the window is gone, and its
    /// DestroyNotify performs the cleanup.
    pub(super) fn handle_event(&mut self, event: Event) -> Result<(), ReplyOrIdError> {
        match self.handle_event_inner(event) {
            Err(error) if is_destroyed_window_race(&error) => {
                log_once(&DESTROYED_WINDOW_RACE_LOG, &format!("ignored a request that raced a destroyed window: {error}"));
                Ok(())
            }
            other => other,
        }
    }

    pub(super) fn handle_event_inner(&mut self, event: Event) -> Result<(), ReplyOrIdError> {
        match event {
            Event::MapRequest(event) => self.handle_map_request(event)?,
            Event::ConfigureRequest(event) => self.handle_configure_request(event)?,
            Event::DestroyNotify(event) => {
                if self.dock_struts.remove(&event.window).is_some() {
                    self.changes.panels = true;
                    self.mark_work_area();
                    self.reconcile_work_area()?;
                }
                let client = self.client_for(event.window);
                if let Some(client) = client {
                    self.unmanage(client, false)?;
                }
                self.publish_workspace_metadata()?;
            }
            Event::UnmapNotify(event) => self.handle_unmap(event.window)?,
            Event::MapNotify(event) => self.handle_map_notify(event.window)?,
            Event::ConfigureNotify(event) => self.handle_configure_notify(event)?,
            Event::DamageNotify(event) => self.handle_damage_notify(event.damage),
            Event::Expose(event) => {
                if event.count == 0 {
                    if let Some(client) = self.client_for(event.window) {
                        self.paint_chrome(client)?;
                        self.try_start_open(client)?;
                    }
                }
            }
            Event::EnterNotify(event) => self.handle_enter(event)?,
            Event::LeaveNotify(event) => self.handle_leave(event)?,
            Event::ButtonPress(event) => self.handle_button_press(event)?,
            Event::ButtonRelease(event) => self.handle_button_release(event)?,
            Event::KeyPress(event) => self.handle_key_press(event)?,
            Event::KeyRelease(event) => self.handle_key_release(event)?,
            Event::FocusOut(_) => self.super_tap.focus_lost(),
            Event::MotionNotify(event) => self.handle_motion(event)?,
            Event::PropertyNotify(event) => self.handle_property(event.window, event.atom)?,
            Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => self.display_topology_dirty.mark(),
            Event::ClientMessage(event) => self.handle_client_message(event)?,
            _ => {}
        }
        Ok(())
    }

    /// Dispatches a passive-grab KeyPress to the bound action. Only actions
    /// with an existing WM path are executed; the rest are logged once.
    pub(super) fn handle_key_press(&mut self, event: KeyPressEvent) -> Result<(), ReplyError> {
        if let Some(active) = self.active {
            self.cancel_open_for(active)?;
        }
        let state = u16::from(event.state);
        let matched = crate::shortcut_action_for_key(event.detail, state);
        if key_diag_enabled() {
            eprintln!(
                "flamewm: keypress detail={} state={:04x} action={}",
                event.detail,
                state,
                matched.as_deref().unwrap_or("none")
            );
        }
        // A bare Start-bound Super press only arms the tap; Start toggles on its
        // completed release. Any other press completes or cancels a pending tap.
        if matched.as_deref() == Some(ACTION_TOGGLE_START_MENU) && is_bare_state(state) {
            let outcome = self.super_tap.press_super(event.detail, event.time);
            self.apply_super_tap(outcome);
            return Ok(());
        }
        let outcome = self.super_tap.press_other();
        self.apply_super_tap(outcome);
        let Some(action) = matched else {
            return Ok(());
        };
        match action.as_str() {
            ACTION_WINDOW_CLOSE => {
                if let Some(client) = self.active {
                    self.request_close(client)?;
                }
            }
            ACTION_WINDOW_MINIMIZE => {
                if let Some(client) = self.active {
                    self.minimize(client)?;
                }
            }
            ACTION_WINDOW_MAXIMIZE => {
                if let Some(client) = self.active {
                    self.toggle_maximize(client)?;
                }
            }
            // Left/Up step to the previous workspace, Right/Down to the next; both clamp.
            ACTION_WORKSPACE_LEFT | ACTION_WORKSPACE_RIGHT | ACTION_WORKSPACE_UP
            | ACTION_WORKSPACE_DOWN => {
                if let Some(delta) = workspace_action_delta(action.as_str()) {
                    if let Some(target) =
                        next_workspace(self.current_workspace, self.workspace_count, delta)
                    {
                        self.switch_workspace(target)?;
                    }
                }
            }
            // Same spawn and supervision path as the Lock session action.
            ACTION_SESSION_LOCK => {
                if let Err(error) = flamewm_x11_desktop::start_lock_screen() {
                    eprintln!("flamewm: lock shortcut failed: {error:?}");
                }
            }
            // Chorded start-menu binding: toggle on press. Bare Super is handled above
            // and toggles through the SuperTap commit at the end of the batch.
            ACTION_TOGGLE_START_MENU => self.changes.start_menu_toggle = true,
            ACTION_VOLUME_UP => self.changes.media_audio_key = Some(MediaAudioKey::VolumeUp),
            ACTION_VOLUME_DOWN => self.changes.media_audio_key = Some(MediaAudioKey::VolumeDown),
            ACTION_VOLUME_MUTE => self.changes.media_audio_key = Some(MediaAudioKey::VolumeMute),
            other => log_unhandled_shortcut_once(other),
        }
        Ok(())
    }

    /// Bare-Super release only completes a tap; Start toggles at batch commit.
    pub(super) fn handle_key_release(&mut self, event: KeyReleaseEvent) -> Result<(), ReplyError> {
        self.super_tap.release_super(event.detail, event.time);
        Ok(())
    }

    /// Completes a pending bare-Super tap. Called at each event-batch boundary.
    pub(super) fn commit_super_tap(&mut self) {
        let outcome = self.super_tap.commit();
        self.apply_super_tap(outcome);
    }

    pub(super) fn apply_super_tap(&mut self, outcome: SuperTapOutcome) {
        if outcome == SuperTapOutcome::Toggle {
            self.changes.start_menu_toggle = true;
        }
    }

    pub(super) fn handle_map_request(&mut self, event: MapRequestEvent) -> Result<(), ReplyOrIdError> {
        if self.clients.contains_key(&event.window) {
            self.conn.map_window(event.window)?;
            if let Some(client) = self.clients.get(&event.window) {
                self.conn.map_window(client.frame)?;
            }
            self.publish_client_list()?;
            return Ok(());
        }
        let kind = self.window_kind(
            event.window,
            self.conn
                .get_window_attributes(event.window)?
                .reply()?
                .override_redirect,
        )?;
        match kind {
            WindowKind::Popup => {
                self.map_unframed(event.window, StackMode::ABOVE)?;
                return Ok(());
            }
            WindowKind::Desktop => {
                self.map_unframed(event.window, StackMode::BELOW)?;
                return Ok(());
            }
            WindowKind::Dock => return self.map_dock(event.window),
            WindowKind::Normal | WindowKind::Utility => {}
        }
        let geometry = self.conn.get_geometry(event.window)?.reply()?;
        self.manage(event.window, &geometry, false, kind)
    }

    /// MapNotify without a preceding MapRequest arrives for windows the WM
    /// itself mapped (frame, client, children: `map_window` generates no
    /// MapRequest). A client MapNotify means the client is visible now; if it
    /// is still unframed (e.g. WM missed the MapRequest because it was not
    /// yet selecting SubstructureRedirect), manage it now. Frame/child
    /// MapNotify events are ignored.
    pub(super) fn handle_map_notify(&mut self, window: Window) -> Result<(), ReplyOrIdError> {
        if self.registry.capture() == Some(window) {
            return Ok(());
        }
        if window == self.screen().root || self.clients.contains_key(&window) {
            return Ok(());
        }
        if self.client_for(window).is_some() {
            return Ok(());
        }
        if self
            .conn
            .get_window_attributes(window)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_none_or(|attrs| attrs.override_redirect)
        {
            return Ok(());
        }
        let kind = self.window_kind(window, false)?;
        match kind {
            WindowKind::Normal | WindowKind::Utility => {
                let geometry = self.conn.get_geometry(window)?.reply()?;
                return self.manage(window, &geometry, false, kind);
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_unmap(&mut self, window: Window) -> Result<(), ReplyError> {
        let Some(client_id) = self.client_for(window) else {
            return Ok(());
        };
        // An unmap the WM expects (the transition's own backdrop hide) must not cancel the flight.
        if let Some(client) = self.clients.get_mut(&client_id) {
            if consume_expected_unmap(&mut client.ignore_unmap) {
                return Ok(());
            }
        }
        self.cancel_open_for(client_id)?;
        if window == client_id {
            self.unmanage(client_id, true)?;
        }
        Ok(())
    }

    pub(super) fn handle_property(&mut self, window: Window, atom: Atom) -> Result<(), ReplyError> {
        if self.dock_struts.contains_key(&window)
            && (atom == self.atoms.net_wm_strut || atom == self.atoms.net_wm_strut_partial)
        {
            self.dock_struts.insert(window, self.read_strut(window)?);
            self.changes.panels = true;
            self.mark_work_area();
            self.reconcile_work_area()?;
            self.publish_workspace_metadata()?;
            return Ok(());
        }
        if window == self.conn.setup().roots[self.screen_num].root {
            if atom == self.atoms.net_current_desktop {
                // Our own publish echoes back here after the snapshot was already
                // emitted; bumping for it would make the shell's revision stale.
                let value = self
                    .read_cardinals(window, atom, 1)
                    .and_then(|values| values.first().copied());
                if desktop_change_is_external(value, self.current_workspace) {
                    crate::bump_workspace_generation();
                }
            } else if atom == self.atoms.net_number_of_desktops
                || atom == self.atoms.net_desktop_names
            {
                // Our own metadata publish echoes back here; only a value that
                // differs from what we wrote is a real external change.
                let count = self
                    .read_cardinals(window, self.atoms.net_number_of_desktops, 1)
                    .and_then(|values| values.first().copied());
                let names = self
                    .conn
                    .get_property(false, window, self.atoms.net_desktop_names, AtomEnum::ANY, 0, u32::MAX)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .map(|reply| reply.value)
                    .unwrap_or_default();
                let own_names = desktop_names_payload(self.workspace_count);
                if workspace_metadata_is_external(count, &names, self.workspace_count, &own_names)
                {
                    crate::bump_workspace_generation();
                    self.mark_workspaces();
                }
            }
        }
        if (atom == self.atoms.motif_wm_hints || atom == self.atoms.gtk_frame_extents)
            && self.clients.contains_key(&window)
        {
            return self.refresh_decoration(window);
        }
        let Some(client_id) = self.client_for(window) else {
            return Ok(());
        };
        if atom == self.atoms.net_wm_name || atom == AtomEnum::WM_NAME.into() {
            if let Ok(title) = self.read_title(client_id) {
                let width = flamewm_render_x11::external_text_measure(&title, 12.0)
                    .0
                    .round() as i32;
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.title_text_width = width;
                    client.title = title;
                }
                self.mark_windows();
                self.paint_chrome(client_id)?;
            }
        } else if atom == self.atoms.net_wm_icon {
            // A fresh icon property invalidates every catalog result/request.
            let (wm_instance, wm_class) = self.wm_identity(client_id);
            let (icon, fallback) = self.read_native_frame_icon(client_id);
            self.deferred_icon_jobs.remove(&client_id);
            for requesters in self.icon_requests.values_mut() {
                requesters.retain(|(id, _)| *id != client_id);
            }
            self.icon_requests.retain(|_, requesters| !requesters.is_empty());
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.wm_instance = wm_instance;
                client.wm_class = wm_class;
                client.icon = icon;
                client.icon_fallback = fallback;
                client.icon_request_token = self.next_client_lifetime;
                client.catalog_icon_key = None;
            }
            self.next_client_lifetime = self.next_client_lifetime.saturating_add(1);
            if self.clients.get(&client_id).is_some_and(|client| client.icon.is_none()) {
                self.request_catalog_icon(client_id);
            }
            self.mark_windows();
            self.paint_chrome(client_id)?;
        } else if atom == AtomEnum::WM_TRANSIENT_FOR.into() {
            let transient_for = self.read_transient_for(client_id)?;
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.transient_for = transient_for;
            }
            self.mark_windows();
        } else if atom == AtomEnum::WM_NORMAL_HINTS.into() {
            let hints = self.read_hints(client_id);
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.hints = hints;
            }
            if let Some(session) = self.sessions.get_mut(&client_id) {
                session.hints = hints;
            }
            self.layout_input_children(client_id)?;
        }
        Ok(())
    }

    pub(super) fn handle_client_message(&mut self, event: ClientMessageEvent) -> Result<(), ReplyError> {
        if event.type_ == self.atoms.wm_change_state && self.clients.contains_key(&event.window) {
            let data = event.data.as_data32();
            flamewm_debug::emit(
                flamewm_debug::DebugEventId("restore.trace.wm.change_state"),
                std::time::Duration::ZERO,
                || {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("t_ms={ms} window={} state={}", event.window, data[0])
                },
            );
            if data[0] == WM_STATE_ICONIC {
                return self.minimize(event.window);
            }
            return self.restore_minimized(event.window);
        }
        let data = event.data.as_data32();
        if event.type_ == self.atoms.net_current_desktop {
            self.switch_workspace(data[0] as usize)?;
        } else if event.type_ == self.atoms.net_number_of_desktops {
            self.set_workspace_count(data[0] as usize)?;
        } else if event.type_ == self.atoms.net_wm_desktop {
            if self.clients.contains_key(&event.window) {
                self.move_to_workspace(event.window, data[0] as usize)?;
            }
        } else if event.type_ == self.atoms.net_active_window {
            if self.clients.contains_key(&event.window) {
                self.focus(event.window)?;
            }
        } else if event.type_ == self.atoms.net_wm_moveresize
            && self.clients.contains_key(&event.window)
        {
            let pointer = RootPoint::new(data[0] as i32, data[1] as i32);
            self.begin_client_moveresize(event.window, pointer, data[2])?;
        } else if event.type_ == self.atoms.net_close_window {
            if self.clients.contains_key(&event.window) {
                self.request_close(event.window)?;
            }
        } else if event.type_ == self.atoms.net_wm_state && self.clients.contains_key(&event.window)
        {
            let wants_maximize = [data[1], data[2]].iter().any(|atom| {
                *atom == self.atoms.net_wm_state_maximized_horz
                    || *atom == self.atoms.net_wm_state_maximized_vert
            });
            let before = self
                .clients
                .get(&event.window)
                .map(|state| state.placement.mode);
            if wants_maximize {
                move_counters().record_client_message_maximize();
            }
            self.apply_net_wm_state(event.window, data[0], data[1], data[2])?;
            let after = self
                .clients
                .get(&event.window)
                .map(|state| state.placement.mode);
            if let (Some(before), Some(after)) = (before, after) {
                flamewm_debug::emit(
                    flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                    std::time::Duration::ZERO,
                    || {
                        format!(
                            "phase=client_message client={} source={} region=None pointer=None pre_session={:?} post_session={:?} placement={before:?} post_placement={after:?} maximize_request={wants_maximize} state_mutation={}",
                            event.window,
                            event.window,
                            self.sessions
                                .get(&event.window)
                                .map(|session| session.session),
                            self.sessions
                                .get(&event.window)
                                .map(|session| session.session),
                            before != after,
                        )
                    },
                );
            }
        }
        Ok(())
    }

    pub(super) fn apply_net_wm_state(
        &mut self,
        client: Window,
        action: u32,
        first: Atom,
        second: Atom,
    ) -> Result<(), ReplyError> {
        let atoms = [first, second];
        let wants_maximize = atoms.iter().any(|atom| {
            *atom == self.atoms.net_wm_state_maximized_horz
                || *atom == self.atoms.net_wm_state_maximized_vert
        });
        let wants_fullscreen = atoms.contains(&self.atoms.net_wm_state_fullscreen);
        let wants_hidden = atoms.contains(&self.atoms.net_wm_state_hidden);
        if wants_hidden {
            flamewm_debug::emit(
                flamewm_debug::DebugEventId("restore.trace.wm.net_wm_state_hidden"),
                std::time::Duration::ZERO,
                || {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("t_ms={ms} window={client} action={action}")
                },
            );
            // Add/toggle hides; Remove (the control plane's RestoreWindow) un-minimizes.
            return if action == 0 {
                self.restore_minimized(client)
            } else {
                self.minimize(client)
            };
        }
        if wants_fullscreen {
            let currently = self
                .clients
                .get(&client)
                .is_some_and(|state| state.is_fullscreen());
            let enable = match action {
                0 => false,
                1 => true,
                2 => !currently,
                _ => currently,
            };
            return self.set_fullscreen(client, enable);
        }
        if wants_maximize {
            let currently = self
                .clients
                .get(&client)
                .is_some_and(|state| state.is_maximized());
            let enable = match action {
                0 => false,
                1 => true,
                2 => !currently,
                _ => currently,
            };
            if enable != currently {
                self.toggle_maximize(client)?;
            }
        }
        Ok(())
    }
}
