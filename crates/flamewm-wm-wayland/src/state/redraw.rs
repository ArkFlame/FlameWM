use super::*;

impl State {
    /// Asks for a complete repaint of the tty backend: sets the coalescing dirty flag and pings the
    /// wake pipe, so the frame happens on the next loop turn instead of waiting for an unrelated
    /// event. Any number of calls before one paint cost a single frame.
    pub fn request_full_redraw(&self) {
        self.mirror_dirty.set(true);
        self.request_paint();
    }

    /// Repaint only: the frame has to show new pixels (pointer motion moves the cursor), but nothing
    /// the shell mirror reports has changed.
    pub fn request_paint(&self) {
        self.scene_dirty.set(true);
        self.request_cursor_paint();
    }

    /// Repaint for pointer motion only: the scene is unchanged, so the paint may reuse it and move the
    /// cursor element. The caller guarantees nothing else it draws changed (hover included).
    pub fn request_cursor_paint(&self) {
        self.redraw_dirty.set(true);
        self.request_winit_frame();
        if let Some(waker) = self.drm_waker.as_ref() {
            // The ping coalesces in the kernel and a closed source only reports an error, which is
            // nothing the compositor can act on.
            let () = waker.borrow_mut().ping();
        }
    }

    /// True when the host window draws the pointer itself: a named theme cursor (or none). Client
    /// cursor surfaces stay software-drawn. Pointer motion then needs no frame at all.
    pub fn host_draws_pointer_status(status: &CursorImageStatus) -> bool {
        matches!(
            status,
            CursorImageStatus::Named(_) | CursorImageStatus::Hidden
        )
    }

    /// Recomputes [`Self::cursor_status`]: a decoration resize cursor under the pointer wins over
    /// the client's request, which is kept so leaving the handle restores it.
    pub fn refresh_cursor_status(&mut self) {
        let wanted = match self.decor_cursor {
            Some(icon) => CursorImageStatus::Named(icon),
            None => self.client_cursor_status.clone(),
        };
        if wanted != self.cursor_status {
            self.cursor_status = wanted;
            self.sync_host_cursor();
            self.request_redraw();
        }
    }

    /// Nested winit only: mirrors the pointer image onto the host window. A surface or hidden
    /// cursor hides the host pointer; the software cursor then draws the surface, if any.
    pub fn sync_host_cursor(&self) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        let window = backend.window();
        match &self.cursor_status {
            CursorImageStatus::Named(icon) => {
                window.set_cursor_visible(true);
                window.set_cursor(*icon);
            }
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => {
                window.set_cursor_visible(false);
            }
        }
    }

    /// Installs the tty wake pipe. Called once by `drm::init`; the nested backend never sets it.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn set_drm_waker(&mut self, waker: Ping) {
        self.drm_waker = Some(RefCell::new(waker));
    }

    /// True while a repaint is outstanding. The tty backend uses it to know that a client frame
    /// callback wants a frame even before the scene is rebuilt.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn redraw_pending(&self) -> bool {
        self.redraw_dirty.get()
    }

    /// Consumes the pending request: the paint that takes it is the one that answers it.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn take_redraw(&self) -> bool {
        self.redraw_dirty.replace(false)
    }

    /// Consumes the scene-change flag: true when the next paint must rebuild every element.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn take_scene_dirty(&self) -> bool {
        self.scene_dirty.replace(false)
    }

    /// Consumes the mirror-change flag set by `request_full_redraw`.
    pub fn take_mirror_dirty(&self) -> bool {
        self.mirror_dirty.replace(false)
    }

    /// Records that the loop has to end as a startup failure (the tty backend's first-frame
    /// deadline calls this before stopping the loop). `main` reads it afterwards and exits non-zero,
    /// so the backend is torn down and the console comes back instead of a dead black VT.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub fn report_startup_failure(&self, reason: &'static str) {
        self.startup_failure.set(Some(reason));
    }

    /// The startup failure recorded during the run, if any.
    pub fn startup_failure(&self) -> Option<&'static str> {
        self.startup_failure.get()
    }

    /// True while the session still owes a desktop: autostart clients are configured but `XWayland`
    /// never became ready, so nothing can ever map a window.
    ///
    /// The first frame is presented before `XWayland` is even spawned, and it only ever contains the
    /// clear colour plus the cursor, so a completed flip is no evidence that a desktop will appear.
    /// This is the condition the post-present content deadline watches.
    pub fn desktop_pending(&self) -> bool {
        self.xwm.is_none() && !self.autostart.is_empty()
    }

    pub fn request_redraw(&self) {
        self.request_full_redraw();
    }

    /// Asks the nested window for a frame, at most one per [`WINIT_FRAME_INTERVAL`]. Pointer
    /// motion arrives at input rate and every frame is a full software repaint, so a burst of
    /// requests inside one interval collapses into a single timer-driven redraw.
    fn request_winit_frame(&self) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        let now = std::time::Instant::now();
        let Some(last) = self.winit_frame_at.get() else {
            return backend.window().request_redraw();
        };
        let elapsed = now.duration_since(last);
        if elapsed >= WINIT_FRAME_INTERVAL {
            return backend.window().request_redraw();
        }
        if self.winit_frame_armed.replace(true) {
            return;
        }
        let armed = self.loop_handle.insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(
                WINIT_FRAME_INTERVAL.saturating_sub(elapsed),
            ),
            |_, (), state| {
                let _span = crate::profile::timer();
                state.winit_frame_armed.set(false);
                if let Some(backend) = state.backend.as_ref() {
                    backend.window().request_redraw();
                }
                smithay::reexports::calloop::timer::TimeoutAction::Drop
            },
        );
        if armed.is_err() {
            self.winit_frame_armed.set(false);
            backend.window().request_redraw();
        }
    }

    /// Records that the nested window has just painted, starting the next pacing interval.
    pub fn note_winit_frame(&self) {
        self.winit_frame_at.set(Some(std::time::Instant::now()));
    }
}
