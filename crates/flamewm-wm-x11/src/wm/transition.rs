use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    /// Starts `effect` with an overlay unless it is disabled or unavailable. Returns `true`
    /// when a flight owns the end state (the caller must not apply it now). A rapid repeat
    /// or an opposite minimize/restore is absorbed; any other running flight finishes first.
    pub(super) fn begin_transition(&mut self, client: Window, effect: Effect) -> Result<bool, ReplyError> {
        let now = std::time::Instant::now();
        if let Some(current) = self.animator.running(client) {
            if current == effect {
                flamewm_debug::emit(
                    flamewm_debug::DebugEventId("restore.trace.wm.transition_absorbed"),
                    std::time::Duration::ZERO,
                    || {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis());
                        format!("t_ms={ms} window={client} effect={effect:?} reason=repeat")
                    },
                );
                return Ok(true);
            }
            if matches!(
                (current, effect),
                (Effect::Minimize, Effect::Restore) | (Effect::Restore, Effect::Minimize)
            ) {
                self.animator.start(client, effect, now);
                flamewm_debug::emit(
                    flamewm_debug::DebugEventId("restore.trace.wm.transition_absorbed"),
                    std::time::Duration::ZERO,
                    || {
                        let ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_millis());
                        format!(
                            "t_ms={ms} window={client} effect={effect:?} current={current:?} reason=opposite"
                        )
                    },
                );
                return Ok(true);
            }
            if let Some(running) = self.animator.cancel(client) {
                self.finish_transition(client, running)?;
            }
        }
        if self.animator.start(client, effect, now) != Begin::Started {
            return Ok(false);
        }
        let hidden = match self.open_overlay(client, effect) {
            Ok(hidden) => hidden,
            Err(error) => {
                log_once(&TRANSITION_SKIP_LOG, &format!("transition skipped: {error}"));
                self.animator.cancel(client);
                return Ok(false);
            }
        };
        if effect == Effect::Minimize && !hidden {
            // The overlay now shows the window; hide the real frame (counted like minimize does).
            if let Some(state) = self.clients.get_mut(&client) {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
                let frame = state.frame;
                self.conn.unmap_window(frame)?;
            }
        }
        Ok(true)
    }

    /// Advances flights once per loop turn: paints frames and applies each completed end state.
    pub(crate) fn pump_transitions(&mut self, now: std::time::Instant) -> Result<(), ReplyError> {
        for client in self.linger.expire(now) {
            // The bounded deadline released a client that never repainted (or was not watched).
            self.release_damage_watch(client);
            if std::env::var_os("FLAMEWM_LINGER_LOG").is_some() {
                eprintln!("flamewm: linger released by deadline client={client}");
            }
        }
        for tick in self.animator.tick(now) {
            match tick {
                Tick::Frame { client, progress } => {
                    let failed = self.overlays.get_mut(&client).is_some_and(|flight| {
                        flight
                            .overlay
                            .paint(animation::overlay_t(progress, flight.mirrored))
                            .is_err()
                    });
                    if failed {
                        if let Some(effect) = self.animator.cancel(client) {
                            self.finish_transition(client, effect)?;
                        }
                    }
                }
                Tick::Complete { client, effect } => {
                    if effect == Effect::Restore {
                        flamewm_debug::emit(
                            flamewm_debug::DebugEventId("restore.trace.wm.pump_complete"),
                            std::time::Duration::ZERO,
                            || {
                                let ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_or(0, |d| d.as_millis());
                                format!("t_ms={ms} window={client} effect={effect:?}")
                            },
                        );
                    }
                    self.finish_transition(client, effect)?
                }
            }
        }
        Ok(())
    }

    /// Releases the overlay and applies `effect`'s end state exactly once.
    pub(super) fn finish_transition(&mut self, client: Window, effect: Effect) -> Result<(), ReplyError> {
        crate::frame::titlebar_backdrop::release_backdrop_canvas();
        let flight = self.overlays.remove(&client);
        let hidden = flight.as_ref().is_some_and(|flight| flight.frame_hidden);
        match effect {
            Effect::Minimize => {
                if let Some(flight) = flight {
                    let capture = flight.overlay.finish();
                    let size = capture.argb.len();
                    self.minimized_captures.insert(client, capture, size);
                }
                self.commit_minimize(client, true)
            }
            Effect::Restore => {
                self.commit_restore(client)?;
                self.linger_overlay(client, flight);
                Ok(())
            }
            Effect::Maximize | Effect::Unmaximize => {
                if hidden {
                    // The frame was unmapped for the backdrop snapshot; show it again.
                    if let Some(frame) = self.clients.get(&client).map(|state| state.frame) {
                        self.conn.map_window(frame)?;
                    }
                }
                self.commit_maximize(client)?;
                self.linger_overlay(client, flight);
                Ok(())
            }
            Effect::Open => {
                if hidden {
                    // The frame was unmapped for the backdrop snapshot; show it again.
                    if let Some(frame) = self.clients.get(&client).map(|state| state.frame) {
                        self.conn.map_window(frame)?;
                    }
                }
                self.linger_overlay(client, flight);
                Ok(())
            }
            // The afterimage is dropped with its flight; nothing lingers over a closed window.
            Effect::Close => Ok(()),
        }
    }

    /// Keeps a finished overlay on top of its remapped window for a bounded linger, so the client
    /// repaints under it instead of showing an empty frame when the overlay is removed.
    pub(super) fn linger_overlay(&mut self, client: Window, flight: Option<TransitionOverlay>) {
        if let Some(flight) = flight {
            flight.overlay.raise();
            self.linger.hold(client, flight.overlay, std::time::Instant::now());
            self.watch_damage(client);
        }
    }

    /// Watches `client` for its first repaint after remap. A failed damage request leaves the
    /// bounded linger deadline as the only release, so errors are not fatal here.
    pub(super) fn watch_damage(&mut self, client: Window) {
        if self.damage_watch.contains_key(&client) {
            return;
        }
        let Ok(id) = self.conn.generate_id() else {
            return;
        };
        let version_ok = damage::query_version(&self.conn, 1, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        if version_ok && damage::create(&self.conn, id, client, damage::ReportLevel::NON_EMPTY).is_ok() {
            self.damage_watch.insert(client, id);
        }
    }

    /// The client repainted after remap: release its lingering overlay and stop watching it.
    pub(super) fn handle_damage_notify(&mut self, damage_id: u32) {
        let Some(client) = self
            .damage_watch
            .iter()
            .find(|(_, id)| **id == damage_id)
            .map(|(client, _)| *client)
        else {
            return;
        };
        self.release_damage_watch(client);
        self.linger.remove(client);
        if std::env::var_os("FLAMEWM_LINGER_LOG").is_some() {
            eprintln!("flamewm: linger released by damage client={client}");
        }
    }

    pub(super) fn release_damage_watch(&mut self, client: Window) {
        if let Some(id) = self.damage_watch.remove(&client) {
            let _ = damage::destroy(&self.conn, id);
        }
    }

    /// Ends every flight at its final state (output change).
    pub(super) fn finish_all_transitions(&mut self) -> Result<(), ReplyError> {
        for (client, effect) in self.animator.cancel_all() {
            self.finish_transition(client, effect)?;
        }
        Ok(())
    }

    /// Applies the settings sync point's animation values; disabling ends running flights.
    pub(crate) fn apply_transition_settings(&mut self, settings: Settings) -> Result<(), ReplyError> {
        if let Some(line) = self.animator.take_capability_log() {
            eprintln!("flamewm: {line}");
        }
        for (client, effect) in self.animator.apply_settings(settings) {
            self.finish_transition(client, effect)?;
        }
        Ok(())
    }

    /// Caps the loop's wait at one frame while a flight runs, so frames are painted on time.
    pub(crate) fn transition_wait(&self, wait: Option<std::time::Duration>) -> Option<std::time::Duration> {
        if self.animator.is_active() || !self.linger.is_empty() {
            Some(wait.map_or(TRANSITION_FRAME, |wait| wait.min(TRANSITION_FRAME)))
        } else {
            wait
        }
    }
}
