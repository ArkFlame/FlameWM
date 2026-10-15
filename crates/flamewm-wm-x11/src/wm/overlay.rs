use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    /// Captures the frame (or reuses the minimize capture for restore) and opens its overlay.
    /// Returns `true` when the frame was unmapped here for the opaque backdrop snapshot.
    pub(super) fn open_overlay(&mut self, client: Window, effect: Effect) -> Result<bool, String> {
        let (frame, outer) = self
            .clients
            .get(&client)
            .map(|state| (state.frame, state.outer))
            .ok_or("transition: unknown client")?;
        let renderer = self.renderer.clone().ok_or("transition: no renderer")?;
        let (capture, mirrored, target) = match effect {
            Effect::Restore => {
                let capture = self
                    .minimized_captures
                    .take(client)
                    .ok_or("transition: no minimize capture")?;
                (capture, true, self.minimize_target_for(client, outer))
            }
            Effect::Maximize => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, false, anim_rect(self.work_area_for_rect(outer)))
            }
            Effect::Unmaximize => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                let restored = self
                    .clients
                    .get(&client)
                    .and_then(|state| unmaximize_target(&state.placement))
                    .ok_or("transition: no restored rect")?;
                (capture, false, anim_rect(crate::client::root_to_rect(restored)))
            }
            Effect::Open => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, true, animation::open_start_rect(anim_rect(outer)))
            }
            Effect::Close => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, false, anim_rect(outer))
            }
            _ => {
                let capture = renderer.borrow().capture_window(u64::from(frame))?;
                (capture, false, self.minimize_target_for(client, outer))
            }
        };
        // The window pixels are captured above. Without ARGB the overlay is opaque and snapshots
        // the root under it, so the frame is unmapped first, then synced before the snapshot.
        // Restore needs no hide: its frame is already unmapped while minimized.
        let hide = !renderer.borrow().warp_argb_usable()
            && matches!(effect, Effect::Minimize | Effect::Maximize | Effect::Unmaximize | Effect::Open);
        if hide {
            self.conn.unmap_window(frame).map_err(|e| e.to_string())?;
            if let Some(state) = self.clients.get_mut(&client) {
                state.ignore_unmap = state.ignore_unmap.saturating_add(1);
            }
            // Round trip on the WM connection: the unmap is processed before the root snapshot.
            let sync = self
                .conn
                .get_input_focus()
                .map_err(|e| e.to_string())
                .and_then(|cookie| cookie.reply().map_err(|e| e.to_string()));
            if let Err(error) = sync {
                // The unmap was sent and its UnmapNotify stays ignored; restore the frame.
                let _ = self.conn.map_window(frame);
                return Err(error);
            }
        }
        // Fade keeps the window rect (minimize, restore, open) or moves it between the two rects
        // (maximize, unmaximize) while crossfading. Magic Lamp keeps the lamp warp; its panel edge is
        // derived inside the renderer from the same source and target rects.
        let style = self.animator.style(effect);
        let mirrored = mirrored && style != animation::Style::Fade;
        let source = warp_rect(anim_rect(outer));
        let opened = match (effect, style) {
            (Effect::Minimize | Effect::Restore | Effect::Open | Effect::Close, animation::Style::Fade) => {
                let direction = if matches!(effect, Effect::Minimize | Effect::Close) {
                    TransitionDirection::Out
                } else {
                    TransitionDirection::In
                };
                renderer.borrow().open_warp_overlay_styled(
                    capture,
                    source,
                    source,
                    PanelEdge::Bottom,
                    TransitionStyle::Fade,
                    direction,
                )
            }
            (Effect::Maximize | Effect::Unmaximize, animation::Style::Fade) => {
                renderer.borrow().open_warp_overlay_styled(
                    capture,
                    source,
                    warp_rect(target),
                    PanelEdge::Bottom,
                    TransitionStyle::Fade,
                    TransitionDirection::In,
                )
            }
            (Effect::Maximize | Effect::Unmaximize, animation::Style::MagicLamp) => {
                renderer.borrow().open_warp_overlay_styled(
                    capture,
                    source,
                    warp_rect(target),
                    PanelEdge::Bottom,
                    TransitionStyle::Stretch,
                    TransitionDirection::In,
                )
            }
            _ => renderer.borrow().open_warp_overlay(capture, source, warp_rect(target)),
        };
        let mut overlay = match opened {
            Ok(overlay) => overlay,
            Err(error) => {
                if hide {
                    let _ = self.conn.map_window(frame);
                }
                return Err(error);
            }
        };
        // Map and paint the first frame before the caller hides the client, so the overlay
        // takes over the window's pixels with no empty frame in between.
        if let Err(error) = overlay.paint(animation::overlay_t(0.0, mirrored)) {
            if hide {
                let _ = self.conn.map_window(frame);
            }
            return Err(error);
        }
        self.linger.remove(client);
        self.overlays
            .insert(client, TransitionOverlay { overlay, mirrored, frame_hidden: hide });
        Ok(hide)
    }

    /// Starts the window-open effect for an armed client on an Expose. Starting only here
    /// means the capture sees painted contents; the map and focus are never waiting on it.
    pub(super) fn try_start_open(&mut self, client: Window) -> Result<(), ReplyError> {
        if !self.open_gate.is_armed(client) {
            return Ok(());
        }
        let eligible = self
            .clients
            .get(&client)
            .filter(|state| {
                state.is_floating() && !state.minimized && state.kind == WindowKind::Normal
            })
            .map(|state| (state.frame, state.outer));
        let (Some((frame, outer)), Some(renderer)) = (eligible, self.renderer.clone()) else {
            self.open_gate.clear(client);
            return Ok(());
        };
        let Ok(capture) = renderer.borrow().capture_window(u64::from(frame)) else {
            self.open_gate.clear(client);
            return Ok(());
        };
        // A blank capture is an unpainted window: keep the gate armed for a later Expose.
        if !self.open_gate.attempt(client, animation::is_blank_capture(&capture.argb)) {
            return Ok(());
        }
        if self.animator.start(client, Effect::Open, std::time::Instant::now()) != Begin::Started {
            return Ok(());
        }
        // Fade is a crossfade of the window rect, which works with or without a compositing manager.
        if self.animator.style(Effect::Open) == animation::Style::Fade {
            if self.open_overlay(client, Effect::Open).is_err() {
                self.animator.cancel(client);
            }
            return Ok(());
        }
        // Without a compositing manager the transparent fade would show black, so the open uses
        // the opaque warp from a smaller rect, the same backdrop path as minimize.
        if !renderer.borrow().warp_argb_usable() {
            if self.open_overlay(client, Effect::Open).is_err() {
                self.animator.cancel(client);
            }
            return Ok(());
        }
        let opened = renderer.borrow().open_fade_overlay(capture, outer.x, outer.y);
        match opened {
            Ok(overlay) => {
                self.overlays.insert(
                    client,
                    TransitionOverlay { overlay, mirrored: false, frame_hidden: false },
                );
            }
            Err(_) => {
                self.animator.cancel(client);
            }
        }
        Ok(())
    }

    /// Cancels a pending or running window-open effect for `client` on input to it or its
    /// unmap. A running open ends without completing and never delays anything.
    pub(super) fn cancel_open_for(&mut self, client: Window) -> Result<(), ReplyError> {
        self.open_gate.clear(client);
        if self.animator.running(client) == Some(Effect::Open) {
            if let Some(effect) = self.animator.cancel(client) {
                self.finish_transition(client, effect)?;
            }
        }
        Ok(())
    }

    pub(super) fn minimize_target_for(&self, client: Window, outer: Rect) -> animation::Rect {
        let output = self
            .output_for_rect(outer)
            .map_or(self.screen_rect, |output| output.geometry);
        animation::minimize_target(self.read_icon_geometry(client), anim_rect(output))
    }

    /// `_NET_WM_ICON_GEOMETRY` as set by the client's taskbar entry, when present.
    pub(super) fn read_icon_geometry(&self, client: Window) -> Option<animation::Rect> {
        let reply = self
            .conn
            .get_property(false, client, self.atoms.net_wm_icon_geometry, AtomEnum::CARDINAL, 0, 4)
            .ok()?
            .reply()
            .ok()?;
        let values: Vec<u32> = reply.value32()?.collect();
        if values.len() != 4 {
            return None;
        }
        Some(animation::Rect {
            x: values[0] as i32,
            y: values[1] as i32,
            width: values[2] as i32,
            height: values[3] as i32,
        })
    }
}
