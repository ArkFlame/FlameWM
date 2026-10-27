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

impl<C: Connection> Wm<'_, C> {
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

    /// Apply live chrome tokens from the settings sync point. A change
    /// repaints every frame and reapplies its shape (radius/glass).
    pub(crate) fn apply_decoration_tokens(
        &mut self,
        tokens: flamewm_skin::recipes::decoration::ChromeTokens,
    ) -> Result<(), ReplyError> {
        let titlebars = crate::live_settings::titlebars();
        let titlebars_changed = titlebars != self.titlebars_applied;
        if tokens == self.decoration_tokens && !titlebars_changed {
            return Ok(());
        }
        self.decoration_tokens = tokens;
        if titlebars_changed {
            self.titlebars_applied = titlebars;
            self.reflow_titlebars(titlebars)?;
        }
        let clients: Vec<Window> = self.clients.keys().copied().collect();
        for client in clients {
            let Some((frame, outer, floating)) = self
                .clients
                .get(&client)
                .map(|state| (state.frame, state.outer, state.is_floating()))
            else {
                continue;
            };
            self.conn.change_window_attributes(
                frame,
                &ChangeWindowAttributesAux::new()
                    .border_pixel(crate::chrome::frame_border_pixel(&tokens)),
            )?;
            self.clear_frame_shape(frame, outer, floating)?;
            self.paint_chrome(client)?;
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.borrow().flush();
        }
        Ok(())
    }

    /// Resize every Server frame by the titlebar delta when the setting flips. The frame's
    /// top edge moves so the client area keeps its on-screen position.
    fn reflow_titlebars(&mut self, show: bool) -> Result<(), ReplyError> {
        let h = i32::from(self.config.titlebar_height);
        if h == 0 {
            return Ok(());
        }
        let (old, new) = if show { (0, h) } else { (h, 0) };
        let clients: Vec<Window> = self
            .clients
            .iter()
            .filter(|(_, state)| state.decoration == Decoration::Server)
            .map(|(client, _)| *client)
            .collect();
        for client in clients {
            let Some(state) = self.clients.get_mut(&client) else {
                continue;
            };
            let outer = state.outer;
            let y = outer.y + old - new;
            let height = (i32::try_from(outer.height).unwrap_or(i32::MAX) - old + new).max(1);
            state.set_outer(crate::geometry::Rect::new(
                outer.x,
                y,
                outer.width,
                u32::try_from(height).unwrap_or(1),
            ));
            if let Some(resources) = self.registry.lookup_by_client(client) {
                for xid in resources.children() {
                    if input_child_mapped(state.decoration, resources.region_of(xid), show) {
                        self.conn.map_window(xid)?;
                    } else {
                        self.conn.unmap_window(xid)?;
                    }
                }
            }
            self.apply_frame_geometry(client)?;
            self.layout_input_children(client)?;
            self.set_frame_extents(client)?;
            self.paint_chrome(client)?;
        }
        self.mark_windows();
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
        let titlebar_height =
            u16::try_from(self.extents_for(client).titlebar_h).unwrap_or(self.titlebar_height());
        if titlebar_height == 0 {
            // No band to paint; the frame keeps its flat background and the client covers it.
            return Ok(());
        }
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
            })
        else {
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
            if let Err(error) =
                runtime.paint_cached(&mut renderer, u64::from(frame), (outer.x, outer.y), root)
            {
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
        let placements = lifecycle::input_child_placements(
            state.outer.width.max(1) as i32,
            state.outer.height.max(1) as i32,
            // Handle thickness follows the effective band; a hidden titlebar keeps the
            // base resize thickness so edges stay grabbable.
            self.extents().titlebar_h,
            ids,
        );
        if state.input_layout == Some(placements) {
            return Ok(());
        }
        for placement in &placements {
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
                || {
                    format!(
                        "effect=LayoutInput rect=({},{},{},{})",
                        placement.x, placement.y, placement.w, placement.h
                    )
                },
                &result,
            );
            result?;
        }
        if let Some(state) = self.clients.get_mut(&client) {
            state.input_layout = Some(placements);
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
