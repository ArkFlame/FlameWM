use super::*;

/// Frame geometry (root `x, y, w, h`) an overlay was frozen at when it started to linger.
type FrameAnchor = (i32, i32, u32, u32);

/// Holds whose overlay is waiting above a remapped frame: `(client, anchor, deadline)`. Kept here
/// rather than on `Wm` because the hold owner only keeps the overlay itself. The entry ends with
/// the hold (deadline, client damage, or a frame that moved).
static HOLD_ANCHORS: std::sync::Mutex<Vec<(Window, FrameAnchor, std::time::Instant)>> =
    std::sync::Mutex::new(Vec::new());

/// The root geometry of a frame, as an anchor for a lingering overlay.
fn anchor_of(outer: crate::geometry::Rect) -> FrameAnchor {
    (outer.x, outer.y, outer.width, outer.height)
}

/// Whether a held overlay still sits on the frame it was frozen above. A frame that moved or
/// resized would leave the finished transition as an afterimage at the old place for the rest of
/// the linger, so the hold is released instead (A01).
#[must_use]
pub(crate) fn hold_still_anchored(anchor: FrameAnchor, current: FrameAnchor) -> bool {
    anchor == current
}

/// Records the frame a lingering overlay was frozen above.
fn hold_anchor(client: Window, anchor: FrameAnchor, now: std::time::Instant) {
    let deadline = now + std::time::Duration::from_millis(animation::LINGER_MS);
    let Ok(mut guard) = HOLD_ANCHORS.lock() else {
        return;
    };
    guard.retain(|(held, _, _)| *held != client);
    guard.push((client, anchor, deadline));
}

/// Drops the anchor of a client whose hold ended.
fn forget_hold_anchor(client: Window) {
    let Ok(mut guard) = HOLD_ANCHORS.lock() else {
        return;
    };
    guard.retain(|(held, _, _)| *held != client);
}

/// Clients whose hold must end now: the frame they were frozen above moved or resized, or the
/// client no longer exists. Ended holds are dropped from the table. `current` resolves a frame
/// geometry; held entries past their deadline are dropped without being reported.
fn holds_off_anchor(
    now: std::time::Instant,
    mut current: impl FnMut(Window) -> Option<FrameAnchor>,
) -> Vec<Window> {
    let Ok(mut guard) = HOLD_ANCHORS.lock() else {
        return Vec::new();
    };
    let mut released = Vec::new();
    guard.retain(|(client, anchor, deadline)| {
        if now >= *deadline {
            return false;
        }
        match current(*client) {
            Some(now_anchor) if hold_still_anchored(*anchor, now_anchor) => true,
            Some(_) => {
                released.push(*client);
                false
            }
            None => {
                released.push(*client);
                false
            }
        }
    });
    released
}

#[cfg(test)]
fn anchored_clients() -> usize {
    HOLD_ANCHORS.lock().map(|guard| guard.len()).unwrap_or(0)
}

impl<'a, C: Connection> Wm<'a, C> {
    /// Starts `effect` with an overlay unless it is disabled or unavailable. Returns `true`
    /// when a flight owns the end state (the caller must not apply it now). A rapid repeat
    /// or an opposite minimize/restore is absorbed; any other running flight finishes first.
    pub(super) fn begin_transition(
        &mut self,
        client: Window,
        effect: Effect,
    ) -> Result<bool, ReplyError> {
        let now = std::time::Instant::now();
        if let Some(current) = self.animator.running(client) {
            if current == effect {
                trace_restore("restore.trace.wm.transition_absorbed", || {
                    format!("window={client} effect={effect:?} reason=repeat")
                });
                return Ok(true);
            }
            if matches!(
                (current, effect),
                (Effect::Minimize, Effect::Restore) | (Effect::Restore, Effect::Minimize)
            ) {
                self.animator.start(client, effect, now);
                trace_restore("restore.trace.wm.transition_absorbed", || {
                    format!("window={client} effect={effect:?} current={current:?} reason=opposite")
                });
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
                log_once(
                    &TRANSITION_SKIP_LOG,
                    &format!("transition skipped: {error}"),
                );
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
            self.release_linger(client, "deadline");
        }
        // A01: a finished overlay must not stay above a frame that moved or resized. The hold
        // records the frame it was frozen above, so a moved frame releases it on its first frame
        // tick instead of leaving an afterimage at the old place for the rest of the linger.
        for client in holds_off_anchor(now, |client| {
            self.clients
                .get(&client)
                .map(|state| anchor_of(state.outer))
        }) {
            self.release_linger(client, "moved");
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
                        trace_restore("restore.trace.wm.pump_complete", || {
                            format!("window={client} effect={effect:?}")
                        });
                    }
                    self.finish_transition(client, effect)?
                }
            }
        }
        Ok(())
    }

    /// Releases the overlay and applies `effect`'s end state exactly once.
    pub(super) fn finish_transition(
        &mut self,
        client: Window,
        effect: Effect,
    ) -> Result<(), ReplyError> {
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
            let now = std::time::Instant::now();
            // Freeze the frame geometry this overlay covers: a later move or resize releases the
            // hold instead of leaving the afterimage behind (A01).
            if let Some(anchor) = self
                .clients
                .get(&client)
                .map(|state| anchor_of(state.outer))
            {
                hold_anchor(client, anchor, now);
            }
            self.linger.hold(client, flight.overlay, now);
            self.watch_damage(client);
        }
    }

    /// Ends a linger hold: the overlay drops off the root, the damage watch and the frozen frame
    /// anchor go with it. `reason` only feeds the optional linger log.
    pub(super) fn release_linger(&mut self, client: Window, reason: &str) {
        self.release_damage_watch(client);
        self.linger.remove(client);
        forget_hold_anchor(client);
        if std::env::var_os("FLAMEWM_LINGER_LOG").is_some() {
            eprintln!("flamewm: linger released by {reason} client={client}");
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
        if version_ok
            && damage::create(&self.conn, id, client, damage::ReportLevel::NON_EMPTY).is_ok()
        {
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
        self.release_linger(client, "damage");
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
    pub(crate) fn apply_transition_settings(
        &mut self,
        settings: Settings,
    ) -> Result<(), ReplyError> {
        if let Some(line) = self.animator.take_capability_log() {
            eprintln!("flamewm: {line}");
        }
        for (client, effect) in self.animator.apply_settings(settings) {
            self.finish_transition(client, effect)?;
        }
        Ok(())
    }

    /// Caps the loop's wait at one frame while a flight runs, so frames are painted on time.
    pub(crate) fn transition_wait(
        &self,
        wait: Option<std::time::Duration>,
    ) -> Option<std::time::Duration> {
        if self.animator.is_active() || !self.linger.is_empty() {
            Some(wait.map_or(TRANSITION_FRAME, |wait| wait.min(TRANSITION_FRAME)))
        } else {
            wait
        }
    }
}

#[cfg(test)]
mod linger_anchor_tests {
    use super::*;

    #[test]
    fn a_frame_that_moved_or_resized_is_no_longer_anchored() {
        let anchor = (10, 20, 800, 600);
        assert!(
            hold_still_anchored(anchor, anchor),
            "same geometry keeps the hold"
        );
        assert!(!hold_still_anchored(anchor, (11, 20, 800, 600)), "moved x");
        assert!(!hold_still_anchored(anchor, (10, 21, 800, 600)), "moved y");
        assert!(
            !hold_still_anchored(anchor, (10, 20, 801, 600)),
            "resized w"
        );
        assert!(
            !hold_still_anchored(anchor, (10, 20, 800, 601)),
            "resized h"
        );
    }

    /// A01: a moved frame releases its hold on the next pump; a still frame keeps it, and the
    /// table ends with the linger deadline. This is the only test that touches the hold table.
    #[test]
    fn only_a_moved_frame_releases_the_hold_and_every_hold_ends() {
        let now = std::time::Instant::now();
        hold_anchor(7, (0, 0, 10, 10), now);
        hold_anchor(8, (5, 5, 10, 10), now);
        assert_eq!(anchored_clients(), 2);
        let released = holds_off_anchor(now, |client| match client {
            7 => Some((0, 1, 10, 10)),
            8 => Some((5, 5, 10, 10)),
            _ => None,
        });
        assert_eq!(released, vec![7], "only the moved frame is released");
        assert_eq!(anchored_clients(), 1, "the still frame keeps its anchor");

        // Past the linger deadline nothing is released twice and the table is empty again.
        let later = now + std::time::Duration::from_millis(animation::LINGER_MS + 1);
        assert!(holds_off_anchor(later, |_| Some((5, 5, 10, 10))).is_empty());
        assert_eq!(anchored_clients(), 0);

        // A client that vanished drops its hold instead of holding it forever.
        hold_anchor(9, (0, 0, 10, 10), now);
        assert_eq!(holds_off_anchor(now, |_| None), vec![9]);
        assert_eq!(anchored_clients(), 0);
    }
}
