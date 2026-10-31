use super::raw_keys::is_escape_chord;
use super::*;

impl<C: Connection> Wm<'_, C> {
    /// Dispatches one event. A request that raced a client's destruction (`BadWindow` and friends)
    /// is expected on a window manager and must never end the session: the window is gone, and its
    /// `DestroyNotify` performs the cleanup.
    pub(super) fn handle_event(&mut self, event: Event) -> Result<(), ReplyOrIdError> {
        match self.handle_event_inner(event) {
            Err(error) if is_destroyed_window_race(&error) => {
                log_once(
                    &DESTROYED_WINDOW_RACE_LOG,
                    &format!("ignored a request that raced a destroyed window: {error}"),
                );
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
                if let Some(client) = self.client_for(event.window) {
                    self.note_client_gone_for_desktop_mode();
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
            Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
                self.display_topology_dirty.mark();
            }
            Event::ClientMessage(event) => self.handle_client_message(event)?,
            // XInput2 raw key records arrive as unknown generic events; see `raw_keys`.
            Event::Unknown(bytes) => {
                if key_diag_enabled() {
                    eprintln!(
                        "flamewm: unknown event head={:?}",
                        &bytes[..bytes.len().min(12)]
                    );
                }
                self.handle_raw_event(&bytes)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Dispatches a passive-grab `KeyPress` to the bound action. Only actions
    /// with an existing WM path are executed; the rest are logged once.
    pub(super) fn handle_key_press(&mut self, event: KeyPressEvent) -> Result<(), ReplyError> {
        // The raw feed may already have dispatched this key while a client held the keyboard.
        if self.raw_claimed(event.time, event.detail) {
            return Ok(());
        }
        let state = u16::from(event.state);
        // Passive path while the switcher is open (the raw twin is claimed, so it is skipped).
        if self.switcher.is_open() {
            self.claim_escape_chord(event.time, event.detail);
            return self.switcher_key(event.detail, state);
        }
        if let Some(active) = self.active {
            self.cancel_open_for(active)?;
        }
        let matched = crate::shortcut_action_for_key(event.detail, state);
        if key_diag_enabled() {
            eprintln!(
                "flamewm: keypress detail={} state={:04x} action={}",
                event.detail,
                state,
                matched.as_deref().unwrap_or("none")
            );
        }
        if let Some(action) = matched.as_deref().filter(|action| is_escape_chord(action)) {
            let action = action.to_owned();
            self.claim_escape_chord(event.time, event.detail);
            return self.dispatch_escape_chord(&action);
        }
        // The bare Start-bound Super tap is driven by the raw feed, so no passive press reaches
        // here as a tap. Any passive press completes or cancels a pending tap.
        let outcome = self.super_tap.press_other();
        self.apply_super_tap(outcome);
        let Some(action) = matched else {
            return Ok(());
        };
        match action.as_str() {
            ACTION_WINDOW_CLOSE => self.on_active(Self::request_close)?,
            ACTION_WINDOW_MINIMIZE => self.on_active(Self::minimize)?,
            ACTION_WINDOW_MAXIMIZE => self.on_active(Self::toggle_maximize)?,
            // Arrow shortcuts follow the product two-row grid: left/right stay inside the
            // current row, up/down switch rows, and a missing neighbour is a no-op.
            ACTION_WORKSPACE_LEFT
            | ACTION_WORKSPACE_RIGHT
            | ACTION_WORKSPACE_UP
            | ACTION_WORKSPACE_DOWN => {
                if let Some(target) = workspace_shortcut_target(
                    self.current_workspace,
                    self.workspace_count,
                    action.as_str(),
                ) {
                    self.switch_workspace(target)?;
                }
            }
            // Same spawn and supervision path as the Lock session action.
            ACTION_OPEN_FILES_SUPER | ACTION_OPEN_FILES_CTRL_ALT => {
                let home = std::env::var_os("HOME")
                    .map_or_else(|| std::path::PathBuf::from("/"), std::path::PathBuf::from);
                if let Err(error) = std::process::Command::new("xdg-open").arg(home).spawn() {
                    eprintln!("flamewm: file explorer shortcut failed: {error}");
                }
            }
            // The finder toggles: a second press stops the open window (see flamewm-finder).
            ACTION_OPEN_FINDER | ACTION_OPEN_FINDER_CTRL_ALT => {
                if let Err(error) = std::process::Command::new("flamewm-finder").spawn() {
                    eprintln!("flamewm: finder shortcut failed: {error}");
                }
            }
            // The assistant binary toggles its own window; a missing binary is logged here, once.
            ACTION_OPEN_ASSISTANT_SUPER | ACTION_OPEN_ASSISTANT_CTRL_ALT => {
                if let Err(error) = std::process::Command::new("flamewm-assistant").spawn() {
                    eprintln!("flamewm: assistant shortcut failed: {error}");
                }
            }
            ACTION_OPEN_TERMINAL_SUPER | ACTION_OPEN_TERMINAL_CTRL_ALT => {
                // The resolver owns the choice: `defaultTerminal` override, else FlameWM Terminal.
                let program = flamewm_applications::terminal::terminal_program();
                if let Err(error) = std::process::Command::new(&program).spawn() {
                    eprintln!("flamewm: terminal shortcut failed: {program}: {error}");
                }
            }
            ACTION_SCREENSHOT_FULL | ACTION_SCREENSHOT_REGION | ACTION_SCREENSHOT_WINDOW => {
                // Screenshot runs on demand in its own process; WM input processing must not wait
                // for a region gesture or PNG compression to complete.
                let options: &[&str] = match action.as_str() {
                    ACTION_SCREENSHOT_FULL => &["--full", "--copy"],
                    ACTION_SCREENSHOT_REGION => &["--region", "--annotate", "--copy"],
                    _ => &["--active", "--copy"],
                };
                if let Err(error) = std::process::Command::new("flamewm-screenshot")
                    .args(options)
                    .spawn()
                {
                    eprintln!("flamewm: screenshot shortcut failed: {error}");
                }
            }
            ACTION_SESSION_LOCK => {
                if let Err(error) = flamewm_x11_desktop::start_lock_screen() {
                    eprintln!("flamewm: lock shortcut failed: {error:?}");
                }
            }
            // Chorded start-menu binding: toggle on press. Bare Super is handled above
            // and toggles through the SuperTap commit at the end of the batch.
            ACTION_TOGGLE_START_MENU => self.changes.start_menu_toggle = true,
            ACTION_LAUNCHER => self.changes.launcher_toggle = true,
            ACTION_VOLUME_UP => self.changes.media_audio_key = Some(MediaAudioKey::VolumeUp),
            ACTION_VOLUME_DOWN => self.changes.media_audio_key = Some(MediaAudioKey::VolumeDown),
            ACTION_VOLUME_MUTE => self.changes.media_audio_key = Some(MediaAudioKey::VolumeMute),
            ACTION_WINDOW_FULLSCREEN => self.on_active(Self::toggle_fullscreen)?,
            ACTION_WINDOW_FLOAT => self.on_active(Self::toggle_float)?,
            ACTION_WINDOW_CYCLE_NEXT => self.cycle_window(1)?,
            ACTION_WINDOW_CYCLE_PREV => self.cycle_window(-1)?,
            ACTION_WINDOW_RESIZE_GROW => self.resize_active(true)?,
            ACTION_WINDOW_RESIZE_SHRINK => self.resize_active(false)?,
            other => {
                if !self.dispatch_hyprland_binding(other)? {
                    log_unhandled_shortcut_once(other);
                }
            }
        }
        Ok(())
    }

    /// Hyprland-style binds whose target is encoded in the action id: absolute workspace
    /// activation/move and directional focus/move. Returns false for any other action.
    fn dispatch_hyprland_binding(&mut self, action: &str) -> Result<bool, ReplyError> {
        if let Some(index) = workspace_activate_index(action) {
            if index < self.workspace_count {
                self.switch_workspace(index)?;
            }
            return Ok(true);
        }
        if let Some(index) = workspace_move_index(action) {
            if index < self.workspace_count {
                if let Some(client) = self.active {
                    self.move_to_workspace(client, index)?;
                }
            }
            return Ok(true);
        }
        if let Some(direction) = focus_direction(action) {
            self.focus_direction(direction)?;
            return Ok(true);
        }
        if let Some(direction) = move_direction(action) {
            self.move_active(direction)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Runs an escape chord from either feed.
    pub(super) fn dispatch_escape_chord(&mut self, action: &str) -> Result<(), ReplyError> {
        let outcome = self.super_tap.press_other();
        self.apply_super_tap(outcome);
        match action {
            // Without the raw feed there is no release to commit on, so keep the direct switch.
            ACTION_WINDOW_SWITCH if self.raw_keys.is_none() => self.switch_window(),
            ACTION_WINDOW_SWITCH => self.switcher_step(switcher::Step::Next),
            ACTION_SHOW_DESKTOP | ACTION_SHOW_DESKTOP_CTRL_ALT => self.toggle_show_desktop(),
            ACTION_FULLSCREEN_ESCAPE => self.fullscreen_escape(),
            _ => Ok(()),
        }
    }

    /// Takes every fullscreen client out of fullscreen and iconifies it, so the desktop is
    /// reachable. When another client still owns the keyboard, it is also asked to close.
    pub(super) fn break_fullscreen_trap(&mut self) -> Result<(), ReplyError> {
        let targets = self.fullscreen_clients();
        for &client in &targets {
            self.set_fullscreen(client, false)?;
            self.minimize(client)?;
        }
        if !targets.is_empty() && self.keyboard_grabbed_by_other()? {
            for &client in &targets {
                self.close(client)?;
            }
        }
        Ok(())
    }

    /// Ctrl+Alt+Escape. The first press breaks the fullscreen client out and asks it to close.
    /// A second press within the window kills it when it still owns the keyboard.
    fn fullscreen_escape(&mut self) -> Result<(), ReplyError> {
        if let Some((client, since)) = self.escape_pending.take() {
            if since.elapsed() < ESCAPE_KILL_WINDOW
                && self.clients.contains_key(&client)
                && self.keyboard_grabbed_by_other()?
            {
                self.conn.kill_client(client)?;
                return Ok(());
            }
        }
        let Some(&client) = self.fullscreen_clients().first() else {
            return Ok(());
        };
        self.escape_pending = Some((client, std::time::Instant::now()));
        self.break_fullscreen_trap()
    }

    /// Alt+Tab: focuses the topmost visible normal client other than the active one. Focus
    /// raises the target, so the next press returns to the previous window.
    fn switch_window(&mut self) -> Result<(), ReplyError> {
        let tree = self.conn.query_tree(self.screen().root)?.reply()?;
        // `children` is bottom-to-top; walk it topmost-first.
        let stack: Vec<Window> = tree
            .children
            .iter()
            .rev()
            .filter_map(|frame| self.frame_to_client.get(frame).copied())
            .collect();
        let target = switch_target(&stack, self.active, |client| {
            self.clients.get(&client).is_some_and(|state| {
                state.kind == WindowKind::Normal && state.visible_on(self.current_workspace)
            })
        });
        match target {
            Some(client) => self.focus(client),
            None => Ok(()),
        }
    }

    fn on_active(
        &mut self,
        act: fn(&mut Self, Window) -> Result<(), ReplyError>,
    ) -> Result<(), ReplyError> {
        match self.active {
            Some(client) => act(self, client),
            None => Ok(()),
        }
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

    pub(super) fn handle_map_request(
        &mut self,
        event: MapRequestEvent,
    ) -> Result<(), ReplyOrIdError> {
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

    /// `MapNotify` without a preceding `MapRequest` arrives for windows the WM
    /// itself mapped (frame, client, children: `map_window` generates no
    /// `MapRequest`). A client `MapNotify` means the client is visible now; if it
    /// is still unframed (e.g. WM missed the `MapRequest` because it was not
    /// yet selecting `SubstructureRedirect`), manage it now. Frame/child
    /// `MapNotify` events are ignored.
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
        if matches!(kind, WindowKind::Normal | WindowKind::Utility) {
            let geometry = self.conn.get_geometry(window)?.reply()?;
            return self.manage(window, &geometry, false, kind);
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
            self.note_client_gone_for_desktop_mode();
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
                let value = self.read_first_cardinal(window, atom);
                if desktop_change_is_external(value, self.current_workspace) {
                    crate::bump_workspace_generation();
                    // Count adoption reads the root current desktop too, so a target
                    // beyond the old count is picked up when the count arrives.
                    if let Some(target) = external_desktop_target(value)
                        .filter(|target| *target < self.workspace_count)
                    {
                        self.switch_workspace(target)?;
                    }
                }
            } else if atom == self.atoms.net_number_of_desktops
                || atom == self.atoms.net_desktop_names
            {
                // Our own metadata publish echoes back here; only a value that
                // differs from what we wrote is a real external change.
                // Pipelined: both root reads are sent before either reply is awaited.
                let count_cookie = self
                    .conn
                    .get_property(
                        false,
                        window,
                        self.atoms.net_number_of_desktops,
                        AtomEnum::ANY,
                        0,
                        1,
                    )
                    .ok();
                let names_cookie = self
                    .conn
                    .get_property(
                        false,
                        window,
                        self.atoms.net_desktop_names,
                        AtomEnum::ANY,
                        0,
                        u32::MAX,
                    )
                    .ok();
                let count = count_cookie
                    .and_then(|cookie| cookie.reply().ok())
                    .and_then(|reply| reply.value32().and_then(|mut values| values.next()));
                let names = names_cookie
                    .and_then(|cookie| cookie.reply().ok())
                    .map(|reply| reply.value)
                    .unwrap_or_default();
                if workspace_metadata_is_external(count, &names, self.workspace_count) {
                    match count.map(|value| value as usize) {
                        Some(external) if external != self.workspace_count => {
                            // Adopt the root's current desktop before republishing so the
                            // metadata publish does not revert an external switch.
                            let current =
                                self.read_first_cardinal(window, self.atoms.net_current_desktop);
                            if let Some(value) = current.map(|v| v as usize) {
                                if value < external {
                                    self.current_workspace = value;
                                }
                            }
                            // set_workspace_count bumps the workspace generation once.
                            self.set_workspace_count(external)?;
                        }
                        _ => {
                            crate::bump_workspace_generation();
                            self.mark_workspaces();
                        }
                    }
                }
            }
        }
        if atom == self.atoms.net_wm_desktop && self.clients.contains_key(&window) {
            // Client-side or transactional desktop rewrites (e.g. a workspace plan
            // applied off-WM) must update the WM's per-client workspace.
            let value = self.read_first_cardinal(window, atom);
            // Not filtered by the old count: the root count may still be pending. The
            // count adoption (shrink) moves clients beyond the new count.
            if let Some(target) = external_desktop_target(value) {
                if let Some(state) = self.clients.get_mut(&window) {
                    if state.workspace != target {
                        state.workspace = target;
                        self.mark_windows();
                        self.reconcile_workspace_visibility()?;
                    }
                }
            }
            return Ok(());
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
                    client.title = title.into();
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
            self.icon_requests
                .retain(|_, requesters| !requesters.is_empty());
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.wm_instance = wm_instance.into();
                client.wm_class = wm_class.into();
                client.icon = icon;
                client.icon_fallback = fallback;
                client.icon_request_token = self.next_client_lifetime;
                client.catalog_icon_key = None;
            }
            self.next_client_lifetime = self.next_client_lifetime.saturating_add(1);
            if self
                .clients
                .get(&client_id)
                .is_some_and(|client| client.icon.is_none())
            {
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

    fn read_first_cardinal(&self, window: Window, atom: Atom) -> Option<u32> {
        self.read_cardinals(window, atom, 1)
            .and_then(|values| values.first().copied())
    }

    fn is_maximize_atom(&self, atom: Atom) -> bool {
        atom == self.atoms.net_wm_state_maximized_horz
            || atom == self.atoms.net_wm_state_maximized_vert
    }

    pub(super) fn handle_client_message(
        &mut self,
        event: ClientMessageEvent,
    ) -> Result<(), ReplyError> {
        if event.type_ == self.atoms.wm_change_state && self.clients.contains_key(&event.window) {
            let data = event.data.as_data32();
            trace_restore("restore.trace.wm.change_state", || {
                format!("window={} state={}", event.window, data[0])
            });
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
                if self
                    .clients
                    .get(&event.window)
                    .is_some_and(|state| state.minimized)
                {
                    // Restore retains the PlacementState, including Maximized.
                    // A focus-only activation of an iconic client is not a restore.
                    self.restore_minimized(event.window)?;
                } else {
                    self.focus(event.window)?;
                }
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
            let wants_maximize = [data[1], data[2]]
                .iter()
                .any(|&atom| self.is_maximize_atom(atom));
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
                let session = self
                    .sessions
                    .get(&event.window)
                    .map(|session| session.session);
                flamewm_debug::emit(
                    flamewm_debug::WM_FRAME_EVENT_DISPATCH,
                    std::time::Duration::ZERO,
                    || {
                        format!(
                            "phase=client_message client={} source={} region=None pointer=None pre_session={session:?} post_session={session:?} placement={before:?} post_placement={after:?} maximize_request={wants_maximize} state_mutation={}",
                            event.window,
                            event.window,
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
        let wants_maximize = atoms.iter().any(|&atom| self.is_maximize_atom(atom));
        let wants_fullscreen = atoms.contains(&self.atoms.net_wm_state_fullscreen);
        let wants_hidden = atoms.contains(&self.atoms.net_wm_state_hidden);
        if wants_hidden {
            trace_restore("restore.trace.wm.net_wm_state_hidden", || {
                format!("window={client} action={action}")
            });
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
                .is_some_and(super::super::client::ManagedClient::is_fullscreen);
            return self.set_fullscreen(client, net_wm_state_enable(action, currently));
        }
        if wants_maximize {
            let currently = self
                .clients
                .get(&client)
                .is_some_and(super::super::client::ManagedClient::is_maximized);
            if net_wm_state_enable(action, currently) != currently {
                self.toggle_maximize(client)?;
            }
        }
        Ok(())
    }
}

/// Resolves a `_NET_WM_STATE` action (0 remove, 1 add, 2 toggle, else keep) against the current state.
fn net_wm_state_enable(action: u32, currently: bool) -> bool {
    match action {
        0 => false,
        1 => true,
        2 => !currently,
        _ => currently,
    }
}

/// First client in `topmost_first` that is not `active` and passes `eligible`.
fn switch_target(
    topmost_first: &[Window],
    active: Option<Window>,
    eligible: impl Fn(Window) -> bool,
) -> Option<Window> {
    topmost_first
        .iter()
        .copied()
        .find(|&client| Some(client) != active && eligible(client))
}

#[cfg(test)]
mod window_switch_tests {
    use super::*;

    #[test]
    fn switch_target_skips_active_and_ineligible_windows() {
        let stack = [30, 20, 10];
        assert_eq!(switch_target(&stack, Some(30), |_| true), Some(20));
        assert_eq!(switch_target(&stack, Some(30), |w| w != 20), Some(10));
        assert_eq!(switch_target(&stack, None, |_| true), Some(30));
    }

    #[test]
    fn switch_target_is_none_without_another_eligible_window() {
        assert_eq!(switch_target(&[30], Some(30), |_| true), None);
        assert_eq!(switch_target(&[], None, |_| true), None);
        assert_eq!(switch_target(&[30, 20], Some(30), |_| false), None);
    }
}

/// Desktop named by an external `_NET_WM_DESKTOP` / `_NET_CURRENT_DESKTOP` value, filtered
/// only by the product maximum. Absent and sticky (`0xFFFF_FFFF`) values yield `None`.
fn external_desktop_target(value: Option<u32>) -> Option<usize> {
    value
        .map(|v| v as usize)
        .filter(|target| *target < flamewm_api::workspace::MAX_WORKSPACES)
}

#[cfg(test)]
mod desktop_adoption_tests {
    use super::*;

    // Root `_NET_NUMBER_OF_DESKTOPS` may be processed after a remapped client
    // `_NET_WM_DESKTOP`, so the old WM count must not filter an external value.
    #[test]
    fn external_target_at_old_count_is_kept_for_either_event_order() {
        // The target is not filtered by the WM's old count, so either order keeps it.
        assert_eq!(external_desktop_target(Some(3)), Some(3));
    }

    #[test]
    fn external_target_rejects_absent_sticky_and_beyond_maximum() {
        assert_eq!(external_desktop_target(None), None);
        assert_eq!(external_desktop_target(Some(u32::MAX)), None);
        assert_eq!(
            external_desktop_target(Some(flamewm_api::workspace::MAX_WORKSPACES as u32)),
            None
        );
    }
}
