use super::*;
use flamewm_skin::recipes::decoration::ChromeTokens;
use x11rb::protocol::xproto::ImageFormat;

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
        let window_margin = crate::live_settings::window_margin();
        let margin_changed = window_margin != self.window_margin_applied;
        if tokens == self.decoration_tokens && !titlebars_changed && !margin_changed {
            return Ok(());
        }
        self.decoration_tokens = tokens;
        if titlebars_changed {
            self.titlebars_applied = titlebars;
            self.reflow_titlebars(titlebars)?;
        }
        if margin_changed {
            self.window_margin_applied = window_margin;
            self.reconcile_work_area()?;
        }
        let clients: Vec<Window> = self.clients.keys().copied().collect();
        for client in clients {
            let Some(frame) = self.clients.get(&client).map(|state| state.frame) else {
                continue;
            };
            self.conn.change_window_attributes(
                frame,
                &ChangeWindowAttributesAux::new()
                    .border_pixel(crate::chrome::frame_border_pixel(&tokens)),
            )?;
            // The ring host decides the client mask and the client inset: settle it before
            // re-placing the client, because a changed outline width moves the client window edge.
            self.refresh_outline_overlay(client);
            self.apply_frame_geometry(client)?;
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
        // The ARGB ring (when a compositor can blend it) owns the outline for this frame; the
        // frame band ring is suppressed below so the two never stack.
        self.paint_outline_overlay(client);
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
                        floating: state.has_rounded_frame(),
                        tokens: self.outline_tokens(),
                    },
                    chrome_runtime::ChromeIdentity {
                        title: state.title.clone(),
                        wm_instance: state.wm_instance.clone(),
                        wm_class: state.wm_class.clone(),
                    },
                    state.icon.as_ref(),
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
        // A compositing-manager overlay owns the ring; the frame band stays clear of it. The
        // client's own edge pixels are read before the chrome cache is borrowed.
        let paint_ring = !self.outline_overlays.contains_key(&client);
        let client_edges = if paint_ring {
            self.client_edge_pixels(client)
        } else {
            None
        };
        let runtime = self.chrome_runtimes.entry(client).or_default();
        runtime.scene_for(&identity, chrome_state, icon);
        if let Some(renderer) = self.renderer.as_ref() {
            let mut renderer = renderer.borrow_mut();
            if let Err(error) = runtime.paint_cached(
                &mut renderer,
                u64::from(frame),
                (outer.x, outer.y),
                root,
                paint_ring,
                client_edges.as_ref(),
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

    /// INTERNAL: dev-only live outline tuning; remove or keep behind the env gate.
    ///
    /// Re-applies a changed `FLAMEWM_OUTLINE_TUNE` file to every managed frame: the scene cache is
    /// dropped (the radius is not a settings token), the shape is forced (a radius change must
    /// re-shape the frame and the client, not just repaint) and the chrome is repainted. Costs one
    /// env lookup per tick and nothing else when the variable is unset.
    pub(super) fn poll_outline_tune(&mut self) -> Result<(), ReplyError> {
        if crate::outline_tune::poll().is_none() {
            return Ok(());
        }
        let clients: Vec<Window> = self.clients.keys().copied().collect();
        for client in clients {
            let Some(state) = self.clients.get_mut(&client) else {
                continue;
            };
            state.shaped = None;
            self.chrome_runtimes.entry(client).or_default().invalidate();
            self.refresh_outline_overlay(client);
            // A tuned width moves the client window edge as well as the ring band.
            self.apply_frame_geometry(client)?;
            self.paint_chrome(client)?;
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.borrow().flush();
        }
        Ok(())
    }

    /// The client's own edge pixels, read back so the outline ring composites over the colour the
    /// window actually drew. The client window is placed inside the ring band, so its outermost
    /// painted column/row is its own first/last pixel. `None` when the client cannot be read
    /// (unmapped, obscured, or a driver that refuses `GetImage`); the ring then falls back to the
    /// frame's material.
    fn client_edge_pixels(&self, client: Window) -> Option<crate::frame::chrome::ClientEdges> {
        let state = self.clients.get(&client)?;
        let band = u16::try_from(self.extents_for(client).inset).unwrap_or(0);
        let (width, height) = crate::frame::geometry_shape::client_area_size(
            state.outer,
            self.titlebar_height(),
            band,
        );
        if width == 0 || height == 0 {
            return None;
        }
        // One pixel inside the ring band, i.e. the client's outermost painted column/row.
        let column = |x: u16| -> Option<Vec<u8>> {
            let reply = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    client,
                    i16::try_from(x).ok()?,
                    0,
                    1,
                    u16::try_from(height).ok()?,
                    !0,
                )
                .ok()?
                .reply()
                .ok()?;
            Some(bgrx_to_rgb(&reply.data))
        };
        let row = |y: u16| -> Option<Vec<u8>> {
            let reply = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    client,
                    0,
                    i16::try_from(y).ok()?,
                    u16::try_from(width).ok()?,
                    1,
                    !0,
                )
                .ok()?
                .reply()
                .ok()?;
            Some(bgrx_to_rgb(&reply.data))
        };
        Some(crate::frame::chrome::ClientEdges {
            left: column(0)?,
            right: column(u16::try_from(width.saturating_sub(1)).ok()?)?,
            bottom: row(u16::try_from(height.saturating_sub(1)).ok()?)?,
        })
    }

    /// Chrome tokens for the outline: the settings-derived tokens with the INTERNAL live tune
    /// applied. One owner, so the frame band, the shape and the overlay see the same numbers.
    ///
    /// INTERNAL: dev-only live outline tuning; remove or keep behind the env gate.
    pub(super) fn outline_tokens(&self) -> ChromeTokens {
        let tune = crate::outline_tune::current();
        let mut tokens = self.decoration_tokens;
        tokens.surface_outline_alpha = tune.alpha.unwrap_or(tokens.surface_outline_alpha);
        tokens.surface_outline_width = tune.width.unwrap_or(tokens.surface_outline_width);
        tokens
    }

    /// Ring parameters for a client's frame: `(physical radius, rgb, alpha, width)`, or `None`
    /// when the client has no server frame. The radius already carries the live tune.
    fn outline_spec(&self, client: Window) -> Option<(u32, u32, u8, u32)> {
        let state = self.clients.get(&client)?;
        if state.decoration != Decoration::Server {
            return None;
        }
        let tokens = self.outline_tokens();
        let radius = u32::from(crate::frame::geometry_shape::scaled_corner_radius(
            state.has_rounded_frame(),
            tokens.style,
            self.titlebar_height(),
        ));
        Some((
            radius,
            tokens.surface_outline_rgb.0,
            tokens.surface_outline_alpha,
            tokens.surface_outline_width,
        ))
    }

    /// Creates the ARGB ring window for `client` when the session can blend one, and drops it when
    /// the outline is off. Only a compositing manager with a 32-bit visual gets an overlay; every
    /// other session keeps the ring painted into the frame's own pixels.
    pub(super) fn refresh_outline_overlay(&mut self, client: Window) {
        let wanted = self.outline_overlay_available
            && self
                .outline_spec(client)
                .is_some_and(|(_, _, alpha, _)| outline_overlay_wanted(alpha));
        if !wanted {
            self.outline_overlays.remove(&client);
            return;
        }
        if self.outline_overlays.contains_key(&client) {
            return;
        }
        let Some(state) = self.clients.get(&client) else {
            return;
        };
        let (frame, outer) = (state.frame, state.outer);
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let opened = renderer.borrow().open_outline_overlay(
            u64::from(frame),
            (0, 0),
            (outer.width.max(1), outer.height.max(1)),
            0,
        );
        match opened {
            Ok(overlay) => {
                self.outline_overlays.insert(client, overlay);
            }
            Err(error) => {
                // Fail open: no overlay, the frame band ring stays the owner.
                eprintln!("FLAMEWM_OUTLINE_OVERLAY_FAILED client={client} reason={error}");
            }
        }
    }

    /// Paints the ring over the client's final pixels. Errors drop the overlay so the next paint
    /// falls back to the frame band instead of leaving a stale or misplaced ring on screen.
    pub(super) fn paint_outline_overlay(&mut self, client: Window) {
        // No compositor: the chrome paints the frame-band ring instead, so this costs one bool
        // check per paint.
        if !self.outline_overlay_available {
            return;
        }
        self.refresh_outline_overlay(client);
        let Some((radius, rgb, alpha, thickness)) = self.outline_spec(client) else {
            return;
        };
        let Some(state) = self.clients.get(&client) else {
            return;
        };
        let (width, height) = (state.outer.width.max(1), state.outer.height.max(1));
        let strips = flamewm_render_core::outline::outline_strips(
            width, height, radius, thickness, rgb, alpha,
        );
        let Some(overlay) = self.outline_overlays.get_mut(&client) else {
            return;
        };
        let painted = overlay
            .configure((0, 0), (width, height), radius)
            .and_then(|()| overlay.present(&strips));
        if let Err(error) = painted {
            eprintln!("FLAMEWM_OUTLINE_OVERLAY_PAINT_FAILED client={client} reason={error}");
            self.outline_overlays.remove(&client);
        }
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

/// Server `ZPixmap` bytes (BGRX on a little-endian server) to straight RGB triples kept in RGBA
/// stride so the frame-band ring can index a pixel uniformly.
fn bgrx_to_rgb(data: &[u8]) -> Vec<u8> {
    data.chunks_exact(4)
        .flat_map(|px| [px[2], px[1], px[0], 255])
        .collect()
}

/// Whether the outline should live in its own ARGB window: only a session that can blend it (the
/// probe already passed) and only while the ring is actually visible.
pub(super) fn outline_overlay_wanted(alpha: u8) -> bool {
    alpha > 0
}

#[cfg(test)]
mod outline_overlay_tests {
    use super::outline_overlay_wanted;

    /// The ring host is a per-session probe plus the live token: no compositor means the frame
    /// band keeps the ring, and `outlines=false` (alpha 0) means no ring at all.
    #[test]
    fn the_overlay_hosts_the_ring_only_when_it_is_visible() {
        assert!(outline_overlay_wanted(127));
        assert!(outline_overlay_wanted(255));
        assert!(!outline_overlay_wanted(0));
    }
}
