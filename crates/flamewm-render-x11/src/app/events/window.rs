use super::*;

impl X11App {
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn event_loop_with_reactor<F, R, P>(
        &mut self,
        document: &mut RuntimeDocument,
        on_action: &mut F,
        reactor: &mut Reactor,
        on_reactor: &mut R,
        geometry: &WindowGeometrySlot,
        press_watch: &RawPressWatch,
        on_press: &mut P,
    ) -> Result<(), String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
        R: FnMut(&mut RuntimeDocument) -> Result<(), String>,
        P: FnMut(&RawPress, &mut RuntimeDocument) -> Result<(), String>,
    {
        // No unconditional redraw on entry; reactor ticks/damage drive paints.
        loop {
            // SAFETY: the press watch is only read here; sync touches this app's live display.
            unsafe { self.sync_raw_press_observer(press_watch) };
            // XNextEvent can leave already-read events in Xlib's queue after
            // single_event returns. Avoid blocking on the fd reactor in that case.
            // A throttled live backdrop refresh wakes the loop when it is due.
            let backdrop_wait = self
                .backdrop_deadline
                .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()));
            let timeout = reactor_wait_timeout(
                unsafe { XEventsQueued(self.display, 0) } > 0,
                !self.pending_damage.is_empty() || self.pending_window_resize.is_some(),
                backdrop_wait,
            );
            reactor
                .dispatch(timeout)
                .map_err(|error| error.to_string())?;
            if self
                .backdrop_deadline
                .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            {
                self.backdrop_deadline = None;
                self.mark_full();
            }
            on_reactor(document)?;
            // A backdrop requested in this reactor turn is installed before the paint gate below.
            self.apply_window_backdrop_request(document);
            // Pending geometry request (if any) is applied before the damage
            // gate so the resized window repaints in this same wake.
            let requested = geometry.take();
            unsafe { self.apply_window_geometry(document, requested)? };
            // Reactor callbacks may mutate the document without producing
            // an X event. The retained damage/revision gate presents that
            // mutation once and skips no-op wakes. Queued X events paint first.
            // SAFETY: self.display is the live display owned by this app.
            if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                unsafe { self.flush_pending_window_resize(document, on_action)? };
                unsafe { self.redraw_if_dirty(document)? };
                if self.close_requested {
                    return Ok(());
                }
                continue;
            }
            self.single_event = true;
            // SAFETY: The caller owns the live X11 display and app resources.
            unsafe { self.event_loop(document, on_action)? };
            self.single_event = false;
            // A UI request made while handling that event starts its modal XDND drag here.
            unsafe { self.start_pending_file_drag() };
            // The event may have opened a menu: arm before the next wake.
            unsafe { self.sync_raw_press_observer(press_watch) };
            unsafe { self.deliver_raw_presses(document, on_press)? };
            if self.close_requested {
                return Ok(());
            }
        }
    }

    /// True while more X input is already queued, so a paint can wait for the burst to drain.
    pub(super) fn input_backlog(&self) -> bool {
        // SAFETY: the display is live; the probe does not consume events.
        unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) > 0 }
    }

    pub(crate) fn hit_test(&self, document: &RuntimeDocument, x: f32, y: f32) -> Option<u32> {
        // A pointer pixel is painted when its center is covered, so sample the
        // center: a fractional layout edge (e.g. y=451.3) must not hide the
        // top pixel row that the retained paint already shows as the control.
        let (x, y) = pointer_sample_point(x, y, document.ui_scale());
        self.layout
            .as_ref()
            .and_then(|layout| layout.hit_test_action(document, x, y))
    }

    pub(crate) unsafe fn refresh_shape_mask(&self, document: &RuntimeDocument) {
        // Retained path: derive VisualCoverage from the cached paint vector.
        // No layout clone, no command rebuild, no second coverage per frame.
        let Some(cached) = self.cached_paint.as_ref() else {
            return;
        };
        let backdrop_radius =
            super::render::backdrop_corner_radius_px(document, self.width, self.height);
        unsafe {
            self.refresh_shape_mask_from_coverage_scaled(&cached.commands, 1.0, backdrop_radius)
        };
    }

    /// Makes the backbuffer hold the current window size. Grow-only pool: a window that
    /// still fits the existing capacity reuses the pixmap and its XRender picture, so a
    /// resize step costs no allocation. Shrinking happens only on settle (see below).
    pub(crate) unsafe fn recreate_backbuffer(&mut self) -> Result<(), String> {
        let root = self.root_extent();
        let target = super::backbuffer_pool::grown_capacity(
            self.backbuffer_cap,
            (self.width, self.height),
            root,
        );
        if target == self.backbuffer_cap {
            return Ok(());
        }
        // SAFETY: forwarded caller contract; the swap owns the display-live handles.
        unsafe { self.swap_backbuffer(target) }
    }

    /// Settle-time shrink: releases an oversized backbuffer once the resize has stopped.
    pub(crate) unsafe fn shrink_backbuffer_if_settled(&mut self) -> Result<(), String> {
        let root = self.root_extent();
        match super::backbuffer_pool::settled_capacity(
            self.backbuffer_cap,
            (self.width, self.height),
            root,
        ) {
            // SAFETY: forwarded caller contract; the swap owns the display-live handles.
            Some(target) => unsafe { self.swap_backbuffer(target) },
            None => Ok(()),
        }
    }

    /// Replaces the backbuffer pixmap with one of `capacity` pixels.
    pub(super) unsafe fn swap_backbuffer(&mut self, capacity: (u32, u32)) -> Result<(), String> {
        // Resize retarget lifecycle: checked replacement first, retarget
        // Xft/XRender (Pictures freed before Pixmaps on failure paths),
        // then swap + free the stale pixmap through the allocator ledger.
        // SAFETY: self.display and self.window are live handles owned by this app.
        let (root_w, root_h) = self.root_extent();
        let (replacement, replacement_bytes) = unsafe {
            self.allocator.create_pixmap(
                self.display,
                self.window,
                capacity.0,
                capacity.1,
                self.depth as u32,
                root_w,
                root_h,
            )
        }
        .map_err(|error| format!("retained backbuffer resize: {error}"))?;
        // Free the old XRender destination Picture before the old Pixmap:
        // retarget creates the replacement picture first, then the backend
        // drops the stale picture; the stale pixmap is freed only after swap.
        if let Some(xft) = self.xft.as_mut() {
            // SAFETY: `replacement` is a fresh pixmap on the live display.
            unsafe { xft.set_drawable(replacement) };
        }
        if let Some(xrender) = self.xrender.as_mut() {
            // SAFETY: `replacement` is a fresh pixmap on the live display.
            if let Err(error) = unsafe { xrender.set_drawable(replacement) } {
                if let Some(xft) = self.xft.as_mut() {
                    // SAFETY: restore the previous live backbuffer on the same display.
                    unsafe { xft.set_drawable(self.backbuffer) };
                }
                // SAFETY: replacement is allocator-owned on self.display.
                unsafe {
                    self.allocator
                        .free_pixmap(self.display, replacement, replacement_bytes)
                };
                return Err(error);
            }
        }
        let previous = self.backbuffer;
        let previous_bytes = self.backbuffer_bytes;
        self.backbuffer = replacement;
        self.backbuffer_bytes = replacement_bytes;
        self.backbuffer_cap = capacity;
        refresh_backbuffer_gauge(replacement_bytes as u64);
        if let Some(surface) = self.surface.as_mut() {
            surface.pixmap = replacement;
        }
        if previous != 0 {
            // Keep the old backbuffer alive until the replacement is
            // painted and bound via
            // bind_presented_backbuffer_as_window_background (first full
            // redraw after this swap). The server may still reference the
            // old pixmap as the window background; freeing it now would
            // leave a dangling background reference. Never bind the
            // unpainted replacement here.
            if let Some((stale, stale_bytes)) =
                self.retired_backbuffer.replace((previous, previous_bytes))
            {
                // Defensive: only one retired generation is expected; free
                // any superseded stale id that is not the bound background.
                if stale != 0 && stale != self.background_bound && stale != replacement {
                    unsafe {
                        self.allocator.free_pixmap(self.display, stale, stale_bytes);
                    }
                }
            }
        }
        Ok(())
    }
}
