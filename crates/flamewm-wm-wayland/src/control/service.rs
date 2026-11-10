//! `Service` lifecycle: bus connection, one bus turn per loop wake, and the post-dispatch hooks.

use super::*;

impl Service {
    /// Connects to the session bus, owns `com.arkflame.FlameWM1` and starts the platform host.
    pub fn connect(state: &State) -> Result<Self, Box<dyn Error>> {
        let server = ControlServer::connect_session().map_err(|error| format!("{error}"))?;
        let mirror = Rc::new(RefCell::new(build_mirror(state, 1)));
        let commands = Rc::new(RefCell::new(Vec::new()));
        let catalog = match flamewm_applications::ApplicationCatalog::discover() {
            Ok(catalog) => Some(Arc::new(catalog)),
            Err(error) => {
                eprintln!("flamewm-wayland: application catalog unavailable: {error}");
                None
            }
        };
        let engine = WaylandDesktop {
            mirror: Rc::clone(&mirror),
            commands: Rc::clone(&commands),
            catalog: catalog.clone(),
            staged_shortcuts: RefCell::new(None),
            shell_scales: saved_shell_scales(),
        };
        // Read the panel backlight before the host starts so the shell bootstrap already carries
        // the current level: its first OSD observation is a baseline, not a card.
        let backlight = Rc::new(RefCell::new(BacklightProvider::connect()));
        // Same owner the X11 runtime uses. A missing bus is an `Unavailable` slot, not an error.
        let network = Rc::new(RefCell::new(
            NetworkManagerSlot::connect().map_err(|error| format!("{error}"))?,
        ));
        network
            .borrow_mut()
            .start()
            .map_err(|error| format!("NetworkManager start failed: {error}"))?;
        // Same shared facade the X11 runtime drives; an absent bus hides the row, never fabricates devices.
        let bluetooth = Rc::new(RefCell::new(
            BluetoothDeviceControl::connect().map_err(|error| format!("{error}"))?,
        ));
        let audio_updates = Arc::new(Mutex::new(Vec::new()));
        let audio_link = match smithay::reexports::calloop::ping::make_ping() {
            Ok((ping, source)) => {
                let queue = Arc::clone(&audio_updates);
                let wake = ping.clone();
                match AudioHelper::connect(move |snapshot| {
                    if let Ok(mut pending) = queue.lock() {
                        pending.push(snapshot);
                    }
                    // Wake the compositor's loop; no polling thread and no busy turn.
                    wake.ping();
                }) {
                    Ok(helper) => Some((Rc::new(helper), ping, source)),
                    Err(error) => {
                        eprintln!("flamewm-wayland: audio helper unavailable: {error}");
                        None
                    }
                }
            }
            Err(error) => {
                eprintln!("flamewm-wayland: audio wake source unavailable: {error:?}");
                None
            }
        };
        let mut host = PlatformHost::new(engine, settings_path());
        if let Some(catalog) = catalog {
            let _ = host.replace_applications(catalog.all());
        }
        host.system_mut()
            .update_brightness(backlight.borrow_mut().snapshot());
        // Audio that cannot be reached is published as `Unavailable`, so the shell shows the
        // absence instead of an empty device list that looks like a working system.
        match &audio_link {
            Some((helper, _, _)) => {
                // Best effort: one eager request so the first volume key steps from a real snapshot.
                if let Err(error) = helper.request_snapshot() {
                    eprintln!("flamewm-wayland: audio snapshot request failed: {error}");
                    mark_audio_unavailable(&mut host);
                }
            }
            None => mark_audio_unavailable(&mut host),
        }
        let backlight_for_actions = Rc::clone(&backlight);
        let network_for_actions = Rc::clone(&network);
        let bluetooth_for_actions = Rc::clone(&bluetooth);
        let audio_for_actions = audio_link.as_ref().map(|(helper, _, _)| Rc::clone(helper));
        host.set_system_action_handler(Box::new(move |action, snapshot| match action {
            SystemAction::SetBrightness(action) => backlight_for_actions
                .borrow_mut()
                .set_percent(action.percent, action.generation),
            SystemAction::SetVolume(action) => audio_for_actions.as_ref().map_or_else(
                || {
                    Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "audio helper is unavailable",
                    ))
                },
                |pulse| pulse.set_volume(action),
            ),
            SystemAction::SetMute(action) => audio_for_actions.as_ref().map_or_else(
                || {
                    Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "audio helper is unavailable",
                    ))
                },
                |pulse| pulse.set_mute(action),
            ),
            SystemAction::SetWifiEnabled(_)
            | SystemAction::ConnectKnown { .. }
            | SystemAction::ConnectWifi { .. }
            | SystemAction::SubmitNetworkSecret { .. }
            | SystemAction::CancelNetworkSecret { .. }
            | SystemAction::Disconnect
            | SystemAction::Scan => network_action(&network_for_actions, action, &snapshot),
            SystemAction::BluetoothDevice(action) => {
                bluetooth_for_actions.borrow().perform_action(&action)
            }
            _ => Err(FlameError::unsupported(
                "the Wayland compositor does not integrate media providers yet",
            )),
        }));
        host.start()
            .map_err(|error| format!("platform host start failed: {error}"))?;
        // Without this the compositor keeps its compiled-in defaults, so a hotkey rebind saved in
        // Settings would be lost on the next start. X11 commits the same set from its runtime.
        match host.commit_startup_shortcuts() {
            Ok(()) => {}
            Err(error) => eprintln!("flamewm-wayland: startup shortcuts were not applied: {error}"),
        }
        // The helper's snapshots wake the loop through this source. Losing it only delays audio
        // updates to the next bus turn; startup never depends on the helper being installed.
        let audio_ping = audio_link.as_ref().map(|(_, ping, _)| ping.clone());
        if let Some((_, _, source)) = audio_link {
            let inserted = state.loop_handle.insert_source(source, |(), (), state| {
                if let Some(service) = state.control.clone() {
                    service.drain_audio();
                }
            });
            if let Err(error) = inserted {
                eprintln!("flamewm-wayland: audio wake source was not installed: {error}");
            }
        }
        Ok(Self {
            server,
            host: RefCell::new(host),
            mirror,
            commands,
            published: RefCell::new(Vec::new()),
            published_workspaces: RefCell::new(None),
            published_system: RefCell::new(None),
            published_panels: RefCell::new(None),
            audio_updates,
            _audio_ping: audio_ping,
            backlight,
            network,
            bluetooth,
            workspace_revision: std::cell::Cell::new(1),
            popup_focus: RefCell::new(None),
            switcher: RefCell::new(None),
            started: Instant::now(),
            publish_pending: std::cell::Cell::new(false),
            write_watch: std::cell::Cell::new(None),
        })
    }

    pub(super) fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Dispatches pending bus traffic, applies the queued commands and keeps signals current.
    /// Runs one bus turn. Returns whether anything happened (a message was dispatched or a command
    /// applied), which drives the idle back-off in `install`.
    pub fn turn(&self, state: &mut State) -> bool {
        self.sync(state);
        let mut busy = false;
        loop {
            let visited = {
                let mut host = self.host.borrow_mut();
                match self.server.on_ready(&mut *host, self.now_ms()) {
                    Ok(visited) => visited,
                    Err(error) => {
                        eprintln!("flamewm-wayland: control dispatch failed: {error}");
                        0
                    }
                }
            };
            if visited == 0 {
                break;
            }
            busy = true;
        }
        let queued: Vec<Command> = self.commands.borrow_mut().drain(..).collect();
        busy |= !queued.is_empty();
        for command in queued {
            apply_command(state, command);
        }
        // Settings transactions land in the host above, so the model's copies are refreshed here:
        // this is the Wayland equivalent of the X11 settings sync point. It costs one map lookup on
        // an unchanged snapshot.
        self.sync_chrome_settings(state);
        // Helper snapshots normally arrive through their own wake source; draining here too means a
        // missed or unavailable wake source only delays audio by one bus turn.
        self.drain_audio();
        self.sync(state);
        self.publish_changes();
        self.publish_system();
        busy
    }

    /// Called after every loop dispatch: emits `WindowsSnapshotChanged` when the windows moved.
    pub fn after_dispatch(&self, state: &mut State) {
        // Pointer motion repaints without touching the mirror: rebuilding it on every loop turn
        // was per-motion work on the input path. Every path that changes the mirror also requests a
        // redraw (`State::request_full_redraw` sets `mirror_dirty`), so this gate cannot miss a change.
        if state.take_mirror_dirty() {
            self.sync(state);
        }
        if self.publish_pending.replace(false) {
            self.publish_changes();
        }
        self.focus_new_popup(state);
        self.finish_switcher_if_released(state);
        self.sync_write_watch(state);
    }

    /// Arms write-readiness on the bus only while libdbus holds bytes it could not send, so an idle
    /// bus costs no wakeups. Called after every dispatch; a fired source removes itself and the next
    /// call re-arms it if the outbound queue is still not empty.
    pub(super) fn sync_write_watch(&self, state: &State) {
        let wants = self.server.watch().events.contains(FdEvents::WRITABLE);
        match (wants, self.write_watch.get()) {
            (true, None) => {
                let fd = self.server.watch().fd;
                let source = Generic::new(
                    flamewm_reactor::BorrowedRawFd(fd),
                    Interest::WRITE,
                    Mode::Level,
                );
                match state.loop_handle.insert_source(source, |_, _, state| {
                    if let Some(service) = state.control.clone() {
                        service.write_watch.set(None);
                        service.turn(state);
                    }
                    Ok(PostAction::Remove)
                }) {
                    Ok(token) => self.write_watch.set(Some(token)),
                    Err(error) => eprintln!("flamewm-wayland: bus write watch failed: {error}"),
                }
            }
            (false, Some(token)) => {
                state.loop_handle.remove(token);
                self.write_watch.set(None);
            }
            _ => {}
        }
    }
}
