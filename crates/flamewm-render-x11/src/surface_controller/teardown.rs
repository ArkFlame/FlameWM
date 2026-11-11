use super::*;

/// Borrowed X connection fd wrapper for reactor registration.
/// The display outlives the registration; calloop never owns the fd.
pub(super) struct ReactorPebble(pub(super) RawFd);

impl AsFd for ReactorPebble {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        unsafe { std::os::fd::BorrowedFd::borrow_raw(self.0) }
    }
}

impl AsRawFd for ReactorPebble {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Drop for SurfaceController {
    fn drop(&mut self) {
        // Null guard mirrors ExternalDecorationRenderer::drop. On a broken
        // X connection (IO-error latch set) skip per-surface X teardown and
        // XCloseDisplay entirely: the fd may stay open so fd probes see
        // "alive", yet any X call touches dead state (SIGSEGV). Forgetting
        // leaks client-side state the dead server can no longer free; the
        // normal path is unchanged (surfaces -> windows -> close).
        if self.display.is_null()
            || x_io_broken()
            || x_error_seen()
            || !display_fd_alive(self.display)
        {
            let surfaces = std::mem::take(&mut self.surfaces);
            std::mem::forget(surfaces);
            self.windows.clear();
            let deferred = std::mem::take(&mut self.deferred_native_libraries);
            std::mem::forget(deferred);
            return;
        }
        if let Some(Some(scene)) = self.live_scene.as_mut() {
            // SAFETY: the display owner has verified a live connection; the scene's pictures and
            // damage objects are freed on this display before it closes.
            unsafe { scene.release(self.display) };
        }
        let mut deferred_native = std::mem::take(&mut self.deferred_native_libraries);
        for instance in self.surfaces.values_mut() {
            // SAFETY: the display owner has verified a live connection, and
            // this owner retains every dynamic handle beyond XCloseDisplay.
            deferred_native.push(unsafe { instance.app.take_deferred_native_libraries() });
        }
        self.surfaces.clear();
        self.windows.clear();
        unsafe {
            XCloseDisplay(self.display);
        }
        drop(deferred_native);
    }
}

/// Records a failed native call in the X error latch, then passes the error through.
pub(super) fn mark_x_error(error: String) -> String {
    mark_x_error_seen();
    error
}

/// Releases a popup's held pointer grab and restores focus. No-op when not grabbed.
pub(super) fn drop_popup_grab(
    display: *mut Display,
    root: Window,
    selection: &mut RawPressSelection,
    id: SurfaceId,
    app: &mut crate::app::X11App,
) {
    if app.pointer_grabbed {
        unsafe {
            release_popup_presses(display, root, selection, id);
            popup_restore_focus(display, app);
        }
        app.pointer_grabbed = false;
    }
}

/// Keeps a dock's strut, rounded mask, and root-space origin in step with its window rect.
pub(super) fn sync_dock_geometry(
    display: *mut Display,
    instance: &mut SurfaceInstance,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) {
    // Live root size: Xlib's cached screen size is not refreshed by a RandR mode change, so the
    // strut edge would be computed against the pre-resize root (1280x800 vs 1024x768).
    let (root_w, root_h) = crate::app::display_extent(display, instance.app.screen);
    crate::app::apply_dock_strut(
        instance.app.display,
        instance.app.window,
        root_w,
        root_h,
        instance.dock_edge,
        instance.dock_bar,
        x,
        y,
        w,
        h,
    );
    apply_dock_mask(instance, w, h, dock_resize_mask_plan(instance.dock_radius));
}

/// True while the X connection fd still exists. After an X IO error the
/// fd is gone; probing via /proc needs std only (no new deps).
pub(super) fn display_fd_alive(display: *mut Display) -> bool {
    let fd = unsafe { XConnectionNumber(display) };
    if fd < 0 {
        return false;
    }
    std::path::Path::new(&format!("/proc/self/fd/{fd}")).exists()
}
