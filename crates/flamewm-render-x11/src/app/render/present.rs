use super::*;

impl X11App {
    /// Present per SurfaceAlphaMode: composited/shape via backbuffer copy,
    /// opaque fallback paints flattened pixels (already flattened in pixel()).
    ///
    /// Owner of the retained background pixmap: binds the presented
    /// backbuffer via XSetWindowBackgroundPixmap so expose repaints reuse
    /// the last painted frame without recompute. Never binds an unpainted
    /// pixmap: callers paint first, then call this, then present. The
    /// previously retired pixmap (kept alive across resize) is released
    /// only after the replacement is bound.
    pub(crate) unsafe fn bind_presented_backbuffer_as_window_background(
        &mut self,
    ) -> Result<(), String> {
        if self.backbuffer == 0 {
            return Err("no backbuffer to bind as window background".to_string());
        }
        // Painted by contract: redraw paints before binding; expose returns
        // only via present_retained (retained scene) or redraw.
        let (should_bind, _) = background_bind_plan(
            self.backbuffer,
            self.background_bound,
            self.retired_backbuffer,
            true,
        );
        if should_bind {
            unsafe { XSetWindowBackgroundPixmap(self.display, self.window, self.backbuffer) };
            self.background_bound = self.backbuffer;
        }
        self.release_retired_after_bind();
        Ok(())
    }

    /// Publishes the presented desktop frame on the root ([`crate::backdrop::DESKTOP_FRAME_PROPERTY`])
    /// with the next present serial. The window origin is only queried when the pixmap or size
    /// changed.
    pub(super) unsafe fn publish_desktop_frame(&mut self) {
        // The root origin is queried on every publish: the desktop can move (XMoveResizeWindow)
        // without its backbuffer or size changing, and readers would then sample the wrong rect.
        let root = unsafe { XRootWindow(self.display, self.screen) };
        // A synchronous round trip per desktop frame. Skip it while the cached origin is current:
        // every move arrives as a ConfigureNotify (which clears the cache), and queued events are
        // drained first so a pending move is never missed.
        let pending = unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } > 0;
        let (x, y) = match root_origin_reuse(self.root_origin, pending) {
            Some(origin) => origin,
            None => {
                let (mut x, mut y, mut child) = (0, 0, 0);
                unsafe {
                    crate::xlib_ext::XTranslateCoordinates(
                        self.display,
                        self.window,
                        root,
                        0,
                        0,
                        &mut x,
                        &mut y,
                        &mut child,
                    )
                };
                self.root_origin = Some((x, y));
                (x, y)
            }
        };
        let serial = self
            .published_desktop_frame
            .map_or(0, |last| last.wrapping_add(1));
        let atom = unsafe { self.desktop_frame_atom() };
        let data: [std::os::raw::c_long; 6] = [
            self.backbuffer as std::os::raw::c_long,
            std::os::raw::c_long::from(x as u32),
            std::os::raw::c_long::from(y as u32),
            std::os::raw::c_long::from(self.width),
            std::os::raw::c_long::from(self.height),
            std::os::raw::c_long::from(serial),
        ];
        // SAFETY: six 32-bit property units, matching format 32 (c_long items).
        unsafe {
            XChangeProperty(
                self.display,
                root,
                atom,
                XA_CARDINAL,
                32,
                PROP_MODE_REPLACE,
                data.as_ptr().cast(),
                6,
            );
        }
        self.published_desktop_frame = Some(serial);
    }

    /// Deletes the published desktop frame: its pixmap is about to be freed.
    pub(crate) unsafe fn withdraw_desktop_frame(&mut self) {
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let atom = unsafe { self.desktop_frame_atom() };
        unsafe { XDeleteProperty(self.display, root, atom) };
        self.published_desktop_frame = None;
    }

    pub(super) unsafe fn desktop_frame_atom(&mut self) -> Atom {
        if self.desktop_frame_atom == 0 {
            let name = std::ffi::CString::new(crate::backdrop::DESKTOP_FRAME_PROPERTY)
                .expect("property name has no NUL");
            self.desktop_frame_atom = unsafe { XInternAtom(self.display, name.as_ptr(), 0) };
        }
        self.desktop_frame_atom
    }

    /// Whether the server already restored an exposed area of the desktop surface: its window
    /// background is the backbuffer holding the last presented frame at the current size, so
    /// repainting would only rewrite identical pixels. Limited to the desktop, the root-sized
    /// surface every moving window and outline exposes; other surfaces keep repainting, since a
    /// resize can leave their window edges different from the retained backbuffer.
    pub(crate) fn exposure_restored_by_background(&self) -> bool {
        self.role == X11WindowRole::Desktop
            && self.backbuffer != 0
            && self.background_bound == self.backbuffer
            && self.presented_size == Some((self.width, self.height))
    }

    pub(super) fn release_retired_after_bind(&mut self) {
        let Some((pixmap, bytes)) = self.retired_backbuffer.take() else {
            return;
        };
        if pixmap == 0 || pixmap == self.background_bound {
            if pixmap != 0 {
                self.retired_backbuffer = Some((pixmap, bytes));
            }
            return;
        }
        unsafe {
            self.allocator.free_pixmap(self.display, pixmap, bytes);
        }
        refresh_backbuffer_gauge(self.backbuffer_bytes as u64);
    }
}

/// Pure background-pixmap lifecycle decision.
// Returns (should_bind, releasable_retired): never bind 0 or an unpainted
// replacement (painted=false); bind only when the painted backbuffer
// differs from bound; release retired only after the new pixmap is bound.
pub(crate) fn background_bind_plan(
    backbuffer: Pixmap,
    background_bound: Pixmap,
    retired: Option<(Pixmap, usize)>,
    painted: bool,
) -> (bool, bool) {
    if backbuffer == 0 || !painted {
        return (false, false);
    }
    let should_bind = background_bound != backbuffer;
    let releasable = match retired {
        Some((pixmap, _)) => pixmap != 0 && pixmap != backbuffer && should_bind,
        None => false,
    };
    (should_bind, releasable)
}

impl X11App {
    pub(crate) unsafe fn present_scene(&mut self) -> Result<(), String> {
        unsafe { XSetClipMask(self.display, self.gc, 0) };
        self.gc_clip = None;
        unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
        {
            let _copy_guard = flamewm_profiler::start("render.copy");
            unsafe {
                XCopyArea(
                    self.display,
                    self.backbuffer,
                    self.window,
                    self.gc,
                    0,
                    0,
                    self.width,
                    self.height,
                    0,
                    0,
                )
            };
        }
        {
            let _flush_guard = flamewm_profiler::start("render.flush");
            // XCopyArea precedes XMapWindow on first presentation, so first
            // observable frame is the finished document rather than black.
            if self.initial_map_pending {
                unsafe { XMapWindow(self.display, self.window) };
                if self.role == X11WindowRole::PopupMenu {
                    // Override-redirect popups are never focused by the WM; typing needs focus.
                    // SAFETY: live display and the window just mapped above.
                    unsafe {
                        crate::surface_controller::raw_press::XSetInputFocus(
                            self.display,
                            self.window,
                            crate::surface_controller::raw_press::REVERT_TO_POINTER_ROOT,
                            0,
                        );
                    }
                }
                self.initial_map_pending = false;
            }
            unsafe { XFlush(self.display) };
        }
        Ok(())
    }
}
