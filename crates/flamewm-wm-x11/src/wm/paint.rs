use super::*;

/// Titlebar repaint cadence during an interactive move (about 60 Hz). The frame still follows
/// every motion; only the chrome repaint is paced.
pub(super) const MOVE_CHROME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Pure pacing decision: a paint is due when none ran yet or the interval has elapsed.
#[must_use]
pub(crate) fn chrome_paint_due(last: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last.is_none_or(|last| now.saturating_duration_since(last) >= MOVE_CHROME_INTERVAL)
}

/// Caps `wait` at the time left until the next chrome paint is due.
#[must_use]
pub(crate) fn chrome_wait_cap(
    last: Option<std::time::Instant>,
    now: std::time::Instant,
    wait: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    let remaining = last.map_or(std::time::Duration::ZERO, |last| {
        (last + MOVE_CHROME_INTERVAL).saturating_duration_since(now)
    });
    Some(wait.map_or(remaining, |wait| wait.min(remaining)))
}

impl<'a, C: Connection> Wm<'a, C> {
    /// True while `client` has an activated move; its titlebar repaints are paced.
    fn is_moving(&self, client: Window) -> bool {
        matches!(
            self.session_of(client),
            InteractionSession::Move(session) if session.anchor.is_activated()
        )
    }

    /// `PaintChrome` executor. During a move, a repaint inside the interval is deferred to
    /// `flush_deferred_chrome`; resize and release paint immediately so no exposed titlebar stays
    /// unpainted.
    pub(super) fn paint_chrome_paced(
        &mut self,
        client: Window,
        now: std::time::Instant,
    ) -> Result<(), ReplyError> {
        if self.is_moving(client) && !chrome_paint_due(self.chrome_paint_at, now) {
            self.chrome_deferred = Some(client);
            return Ok(());
        }
        self.chrome_deferred = None;
        self.paint_chrome(client)?;
        self.chrome_paint_at = Some(now);
        Ok(())
    }

    /// Paints a deferred titlebar once its interval elapsed, or at once when the move has ended.
    pub(super) fn flush_deferred_chrome(
        &mut self,
        now: std::time::Instant,
    ) -> Result<(), ReplyError> {
        let Some(client) = self.chrome_deferred else {
            return Ok(());
        };
        if self.is_moving(client) && !chrome_paint_due(self.chrome_paint_at, now) {
            return Ok(());
        }
        self.chrome_deferred = None;
        self.paint_chrome(client)?;
        self.chrome_paint_at = Some(now);
        Ok(())
    }

    /// Caps the reactor wait so a deferred titlebar paint runs on its frame tick.
    pub(super) fn deferred_chrome_wait(
        &self,
        wait: Option<std::time::Duration>,
    ) -> Option<std::time::Duration> {
        if self.chrome_deferred.is_none() {
            return wait;
        }
        chrome_wait_cap(self.chrome_paint_at, std::time::Instant::now(), wait)
    }

    /// Captures the root once (24-bit LSB only; otherwise the wallpaper is used) and freezes
    /// the backdrop for the dragged frame.
    ///
    /// This is the only root read of a move: every step and the release crop the frozen snapshot
    /// (`drag_titlebar_pixels`), and `end_drag` keeps it alive until the paint at the frame's final
    /// origin has blurred that origin from it. Nothing here or in the settle path hides the window
    /// or reads the root again (A01), so a moved frame never disappears and reappears.
    pub(super) fn begin_drag_backdrop(&self, client: Window) {
        let _drag_guard = flamewm_profiler::start("wm.drag.begin_backdrop");
        let Some((frame, outer)) = self
            .clients
            .get(&client)
            .map(|state| (state.frame, state.outer))
        else {
            return;
        };
        // Opaque titlebars never sample the backdrop: skip the root capture entirely. Drop any
        // snapshot or kept crop this frame still holds so none outlives a skipped drag.
        if !crate::frame::titlebar_backdrop::needs_drag_backdrop(
            self.decoration_tokens.titlebar_alpha,
        ) {
            crate::frame::titlebar_backdrop::forget(u64::from(frame));
            return;
        }
        let screen = self.screen();
        let root = (
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        );
        let raw_root = (screen.root_depth == 24
            && self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST)
            .then(|| {
                self.conn
                    .get_image(
                        ImageFormat::Z_PIXMAP,
                        screen.root,
                        0,
                        0,
                        screen.width_in_pixels,
                        screen.height_in_pixels,
                        u32::MAX,
                    )
                    .ok()?
                    .reply()
                    .ok()
            })
            .flatten()
            .filter(|reply| reply.depth == 24)
            .map(|reply| reply.data);
        crate::frame::titlebar_backdrop::begin_drag(
            u64::from(frame),
            root,
            raw_root,
            (outer.x, outer.y, outer.width, outer.height),
        );
    }

    /// Apply live chrome tokens from the settings sync point. A change
    /// repaints every frame and reapplies its shape (radius/glass).
    pub(crate) fn apply_decoration_tokens(
        &mut self,
        tokens: flamewm_skin::recipes::decoration::ChromeTokens,
    ) -> Result<(), ReplyError> {
        if tokens == self.decoration_tokens {
            return Ok(());
        }
        self.decoration_tokens = tokens;
        let clients: Vec<Window> = self.clients.keys().copied().collect();
        for client in clients {
            let Some((frame, outer, floating)) = self
                .clients
                .get(&client)
                .map(|state| (state.frame, state.outer, state.is_floating()))
            else {
                continue;
            };
            self.clear_frame_shape(frame, outer, floating)?;
            self.paint_chrome(client)?;
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.borrow().flush();
        }
        Ok(())
    }

    pub(super) fn paint_chrome(&mut self, client: Window) -> Result<(), ReplyError> {
        if self
            .clients
            .get(&client)
            .is_some_and(|state| state.decoration != Decoration::Server)
        {
            return Ok(());
        }
        // Single chrome paint path: delegate scene build to chrome_runtime
        // (J08 paint_plan seam); this owner only bridges cached rasters.
        let titlebar_height = u16::try_from(self.extents_for(client).titlebar_h)
            .unwrap_or(self.config.titlebar_height);
        let Some((chrome_state, identity, icon, frame, outer)) =
            self.clients.get(&client).map(|state| {
            (
                chrome_runtime::ChromeState {
                    frame_w: state.outer.width.max(1),
                    frame_h: state.outer.height.max(1),
                    titlebar_height,
                    active: self.active.is_none_or(|current| current == client),
                    hover: state.hover_control,
                    pressed: self.pressed_control(client),
                    maximized: state.is_maximized(),
                    fullscreen: state.is_fullscreen(),
                    floating: state.is_floating(),
                    tokens: self.decoration_tokens,
                },
                chrome_runtime::ChromeIdentity {
                    title: state.title.clone(),
                    wm_instance: state.wm_instance.clone(),
                    wm_class: state.wm_class.clone(),
                },
                state.icon.clone(),
                state.frame,
                state.outer,
            )
        }) else {
            return Ok(());
        };
        let _paint_guard = flamewm_profiler::start("wm.chrome.paint");
        // The WM already knows the frame's root origin; pass it instead of syncing this
        // connection and querying the frame geometry on the renderer's connection.
        let screen = self.screen();
        let root = (
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        );
        let runtime = self.chrome_runtimes.entry(client).or_default();
        runtime.scene_for(&identity, chrome_state, icon);
        if let Some(renderer) = self.renderer.as_ref() {
            let mut renderer = renderer.borrow_mut();
            if let Err(error) = runtime.paint_cached(
                &mut renderer,
                u64::from(frame),
                (outer.x, outer.y),
                root,
            ) {
                // A failed chrome paint leaves the frame's dark background visible
                // during resize/maximize. Surface the error instead of silently
                // continuing as though the titlebar was painted successfully.
                eprintln!(
                    "FLAMEWM_CHROME_PAINT_FAILED frame={frame} client={client} reason={error}"
                );
            }
        }
        Ok(())
    }

    pub(super) fn pressed_control(&self, client: Window) -> Option<FrameControl> {
        match self.sessions.get(&client).map(|session| session.session) {
            Some(InteractionSession::ControlPress(session)) if session.armed => {
                Some(session.control)
            }
            _ => None,
        }
    }

    pub(super) fn layout_input_children(&mut self, client: Window) -> Result<(), ReplyError> {
        let Some(res) = self.registry.lookup_by_client(client) else {
            return Ok(());
        };
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        // Single input-child layout owner: lifecycle placements (J19 wiring).
        let ids = lifecycle::LifecycleIds::new(client, state.frame, res.children());
        for placement in &lifecycle::input_child_placements(
            state.outer.width.max(1) as i32,
            state.outer.height.max(1) as i32,
            self.extents_for(client).titlebar_h,
            ids,
        ) {
            let result = self.conn.configure_window(
                placement.xid,
                &ConfigureWindowAux::new()
                    .x(placement.x)
                    .y(placement.y)
                    .width(placement.w)
                    .height(placement.h),
            );
            emit_native_configure(
                flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE,
                client,
                placement.xid,
                format!(
                    "effect=LayoutInput rect=({},{},{},{})",
                    placement.x, placement.y, placement.w, placement.h
                ),
                &result,
            );
            result?;
        }
        Ok(())
    }

    pub(super) fn bind_child_cursors(&mut self, client: Window) {
        let Some(res) = self.registry.lookup_by_client(client) else {
            return;
        };
        for xid in res.children() {
            let region = self.registry.lookup_by_xid(xid).map(|target| target.region);
            if let Some(region) = region {
                let kind = cursor_for_region(region);
                if let Some(renderer) = self.renderer.as_ref() {
                    let _ = renderer.borrow_mut().define_cursor(u64::from(xid), kind);
                }
            }
        }
    }
}

#[cfg(test)]
mod chrome_pacing_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn first_paint_is_always_due_and_later_ones_wait_one_interval() {
        let t0 = Instant::now();
        assert!(chrome_paint_due(None, t0));
        assert!(!chrome_paint_due(Some(t0), t0 + Duration::from_millis(15)));
        assert!(chrome_paint_due(Some(t0), t0 + MOVE_CHROME_INTERVAL));
    }

    #[test]
    fn a_burst_of_motions_paints_once_per_interval() {
        // 1000 motions at 1 kHz over one second: only ~60 paints fall due.
        let t0 = Instant::now();
        let mut last = None;
        let mut paints = 0;
        for step in 0..1000u64 {
            let now = t0 + Duration::from_millis(step);
            if chrome_paint_due(last, now) {
                last = Some(now);
                paints += 1;
            }
        }
        assert!((60..=63).contains(&paints), "paints={paints}");
    }

    #[test]
    fn wait_is_capped_to_the_next_tick_and_never_extended() {
        let t0 = Instant::now();
        let now = t0 + Duration::from_millis(5);
        assert_eq!(
            chrome_wait_cap(Some(t0), now, None),
            Some(Duration::from_millis(11))
        );
        assert_eq!(
            chrome_wait_cap(Some(t0), now, Some(Duration::from_millis(2))),
            Some(Duration::from_millis(2))
        );
        assert_eq!(
            chrome_wait_cap(Some(t0), t0 + Duration::from_millis(40), None),
            Some(Duration::ZERO)
        );
    }
}
