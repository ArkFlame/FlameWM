use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::time::Instant;

use super::*;
use crate::native::cursor::DeferredNativeLibraryHandles;
use crate::xinput::*;
use crate::xlib::*;
use flamewm_reactor::{Easing, Reactor, Start, Timeline};
use flamewm_render_core::{Rect, SurfaceDamage};

mod backdrop_shape;
mod dock;
mod geometry_trace;
mod lifecycle;
mod present;
mod pump;
pub(crate) mod raw_press;
mod slide;
mod teardown;
mod window_state;
use teardown::*;

#[cfg(test)]
mod tests;

use dock::*;
pub use geometry_trace::*;
pub use present::*;
pub use raw_press::*;
pub use slide::*;

/// Stable role label for geometry traces (one id/role per placement).
pub fn surface_role_label(role: SurfaceRole) -> &'static str {
    match role {
        SurfaceRole::Normal => "normal",
        SurfaceRole::Desktop => "desktop",
        SurfaceRole::Dock => "dock",
        SurfaceRole::PopupMenu => "popup-menu",
        SurfaceRole::DropdownMenu => "dropdown-menu",
        SurfaceRole::Overlay => "overlay",
    }
}

/// Reserved generic action for a ButtonRelease that lands outside the
/// grabbed surface while a pointer grab is active. Feature modules share
/// this single transient-lifecycle signal; it is never Start-specific.
pub const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";

/// Pure bounds check: window-relative coords outside `[0, width) x [0, height)`
/// mean "outside the surface". Popups take no active pointer grab; outside
/// presses arrive as raw root presses (see `RawPressSelection`).
pub fn button_release_is_outside(x: i32, y: i32, width: u32, height: u32) -> bool {
    x < 0 || y < 0 || x >= width.max(1) as i32 || y >= height.max(1) as i32
}

/// Build the generic outside-release controller event for one surface.
pub fn outside_release_event(
    surface: SurfaceId,
    button: u32,
    x: f32,
    y: f32,
    time_ms: u64,
) -> SurfaceControllerEvent {
    SurfaceControllerEvent {
        surface,
        event: ActionEvent {
            action: OUTSIDE_RELEASE_ACTION.to_string(),
            phase: ActionPhase::Release,
            button,
            modifiers: 0,
            x,
            y,
            inside: false,
            time_ms,
            text: None,
        },
    }
}

/// Headless/test fallback extent (device pixels at scale 1.0). Used only
/// when no retained layout exists; mapped-surface queries never hit it.
pub const HEADLESS_FALLBACK_EXTENT: (f32, f32) = (1350.0, 641.0);

/// DEBUG-ONLY gate: true only when `FLAMEWM_DEBUG=1`. Normal mode must
/// issue zero extra X syncs; callers check this before any probe.
pub fn debug_enabled() -> bool {
    std::env::var_os("FLAMEWM_DEBUG").is_some_and(|v| v == "1")
}

/// Raises `window` (a flyout or any shell surface) and then every taskbar window above it, so
/// the taskbar covers whatever part of a flyout overlaps it. Returns the raise's X status.
pub(crate) fn raise_above_taskbars(
    display: *mut Display,
    window: Window,
    taskbars: &[Window],
) -> std::os::raw::c_int {
    // SAFETY: display and every window id are live surfaces owned by the caller's controller.
    unsafe {
        let status = XRaiseWindow(display, window);
        for bar in taskbars {
            XRaiseWindow(display, *bar);
        }
        status
    }
}

/// Taskbar windows a surface's raise may lift above it. An OSD (`Overlay`) raises only itself:
/// lifting the docks too pulled the taskbar above fullscreen clients whenever an OSD appeared.
pub(crate) fn raise_targets(role: SurfaceRole, taskbars: &[Window]) -> &[Window] {
    if role == SurfaceRole::Overlay {
        &[]
    } else {
        taskbars
    }
}

/// DEBUG-ONLY pure stack-order predicate (no X): bottom-to-top roles must
/// satisfy desktop < normal < popup (menu/dropdown/overlay) < dock. The taskbar
/// stays above flyouts. Test/canary use only.
pub fn stack_order_ok(bottom_to_top: &[SurfaceRole]) -> bool {
    fn rank(role: SurfaceRole) -> u8 {
        match role {
            SurfaceRole::Desktop => 0,
            SurfaceRole::Normal => 1,
            SurfaceRole::PopupMenu | SurfaceRole::DropdownMenu | SurfaceRole::Overlay => 2,
            SurfaceRole::Dock => 3,
        }
    }
    let mut last: Option<u8> = None;
    for role in bottom_to_top {
        let rank = rank(*role);
        // Non-decreasing rank keeps equal-class siblings total without
        // imposing an order inside one class.
        if last.is_some_and(|prev| rank < prev) {
            return false;
        }
        last = Some(rank);
    }
    true
}

/// Core X event types and SubstructureNotifyMask, defined here because `xlib.rs` is pinned.
const ROOT_CHILD_DESTROY_NOTIFY: std::os::raw::c_int = 17;
const ROOT_CHILD_UNMAP_NOTIFY: std::os::raw::c_int = 18;
const ROOT_CHILD_MAP_NOTIFY: std::os::raw::c_int = 19;
const ROOT_CHILD_CONFIGURE_NOTIFY: std::os::raw::c_int = 22;
const ROOT_SUBSTRUCTURE_NOTIFY_MASK: std::ffi::c_long = 1 << 19;
const ROOT_STRUCTURE_NOTIFY_MASK: std::ffi::c_long = 1 << 17;

fn is_root_geometry_configure(
    kind: std::os::raw::c_int,
    event_window: Window,
    root: Window,
) -> bool {
    kind == ROOT_CHILD_CONFIGURE_NOTIFY && event_window == root
}

/// Whether a root-window event reports a foreign root child whose map, unmap, move or destroy can
/// change what lies beneath pseudo-transparent surfaces. Our own children are excluded: showing or
/// hiding a surface (a dock label, a launch badge) must not recompose the dock backdrop.
fn is_foreign_root_child_change(kind: std::os::raw::c_int, is_own: bool) -> bool {
    !is_own
        && matches!(
            kind,
            ROOT_CHILD_MAP_NOTIFY
                | ROOT_CHILD_UNMAP_NOTIFY
                | ROOT_CHILD_CONFIGURE_NOTIFY
                | ROOT_CHILD_DESTROY_NOTIFY
        )
}

pub struct SurfaceController {
    display: *mut Display,
    screen: i32,
    next_id: u64,
    /// Applied to surfaces created afterwards; see `set_node_wheel_targets`.
    node_wheel_targets: bool,
    surfaces: HashMap<SurfaceId, SurfaceInstance>,
    windows: HashMap<Window, SurfaceId>,
    /// Dynamic Xft/XRender/XShape/Xcursor handles outlive all apps and
    /// XCloseDisplay. Backend X resources are drained before this owner closes
    /// the display; the handles are dropped only afterwards.
    deferred_native_libraries: Vec<DeferredNativeLibraryHandles>,
    /// Interned `_NET_WM_ICON_GEOMETRY` atom; 0 until first use.
    icon_geometry_atom: Atom,
    /// Active slide transitions keyed by surface id. Empty when idle (no frame wakeups).
    slides: Timeline<SlideKind>,
    /// Dock rect published by a parent process; used only when this controller has no Dock surface.
    external_dock: Option<(i32, i32, u32, u32)>,
    /// A root child was mapped, unmapped, configured or destroyed since the last `take_root_changed`.
    root_changed: bool,
    /// The root window itself changed dimensions since the last take.
    root_geometry_changed: Option<(u32, u32)>,
    /// XInput2 raw-press selection that dismisses popups on outside presses.
    press_selection: RawPressSelection,
    /// Set while the quick-popup press observer holds its root selection.
    press_observer_armed: bool,
    /// Root presses observed while armed, drained by the owner.
    raw_press_observations: Vec<RawPressObservation>,
    /// Desktop layer translucent popups and panels blend over; `None` keeps them solid.
    desktop: Option<crate::backdrop::DesktopLayer>,
    /// Live desktop-plus-windows composition for translucent surfaces. `None`: not probed yet;
    /// `Some(None)`: unavailable (no picture transform), the desktop-only path is used.
    live_scene: Option<Option<crate::app::live_backdrop::LiveScene>>,
    /// The last live backdrop request composed from the redirected frames (not the desktop-only path).
    live_in_use: bool,
}

impl SurfaceController {
    pub fn show(&mut self, id: SurfaceId) -> Result<(), String> {
        // First-show single present: project->measure->anchor->prepare->
        // paint->shape->commit->map+raise->present->grab->flush in one
        // ordered cycle, no feature redraw. Second show sees clean damage
        // and maps/presents without repainting.
        let role = self.surface_role(id)?;
        let display = self.display;
        let taskbars = self.taskbar_windows();
        let key = id.get();
        let now = Instant::now();
        let (slide, shown, size, origin, window) = {
            let instance = self.instance_mut(id)?;
            (
                instance.slide,
                instance.shown,
                (instance.app.width, instance.app.height),
                instance.origin,
                instance.app.window,
            )
        };
        // A shown popup may have been covered by a managed frame since it was mapped: the WM
        // raises frames on focus/manage and does not track override-redirect menus. Raise it
        // before any early return so every show call restores the popup-above-frame invariant.
        // The first-show path raises again after the map, where the taskbars must stay above it.
        if shown && surface_role_needs_popup_raise(role) {
            raise_above_taskbars(display, window, raise_targets(role, &taskbars));
        }
        if !shown {
            // Before any paint or map: the root under the unmapped popup is still clean.
            if let Err(error) = self.install_popup_backdrop(id) {
                eprintln!("flamewm-render-x11: popup backdrop skipped (nonfatal): {error}");
            }
        }
        if let Some(config) = slide.filter(|config| config.transition != PopupTransition::Slide)
            && self.begin_overlay_show(id, config, shown, now)?
        {
            return Ok(());
        }
        let mut emerging: Option<SlideEdge> = None;
        if let Some(config) = slide.filter(|config| config.transition == PopupTransition::Slide) {
            self.slides.set_speed(config.speed);
            let opening = SlideKind {
                edge: config.edge,
                opening: true,
            };
            if shown {
                match self.slides.kind(key) {
                    // Double open while opening: already converging on the final frame.
                    Some(active) if active.opening => return Ok(()),
                    // Reverse from the current progress; the window is still mapped.
                    Some(_) => {
                        self.slides.start_towards(
                            key,
                            opening,
                            now,
                            SLIDE_OPEN_BASE_MS,
                            Easing::EaseOutCubic,
                            (0.0, 1.0),
                        );
                        return Ok(());
                    }
                    None => {}
                }
            } else if self.slides.start_towards(
                key,
                opening,
                now,
                SLIDE_OPEN_BASE_MS,
                Easing::EaseOutCubic,
                (0.0, 1.0),
            ) == Start::Started
            {
                // Place off-edge before mapping so the first visible frame is the start frame.
                let (x, y) = slide_origin(origin, size, config.edge, 0.0);
                unsafe { XMoveResizeWindow(display, window, x, y, size.0, size.1) };
                self.instance_mut(id)?.slide_drawn = Some((x, y));
                emerging = Some(config.edge);
            }
            // A refused start (timeline full or disabled) maps at the final origin with no slide.
        }
        let dock = self.dock_rect();
        let instance = self.instance_mut(id)?;
        let mut cycle = PresentCycle::new();
        cycle.advance("project")?;
        cycle.advance("measure")?;
        cycle.advance("anchor")?;
        cycle.advance("prepare")?;
        unsafe {
            instance
                .app
                .redraw_if_dirty(&mut instance.document)
                .map_err(mark_x_error)?;
        }
        cycle.advance("paint")?;
        // SAFETY: display/window live; shape follows retained repaint. A slide clips to the
        // panel side before the window maps, so its first visible frame is already clipped.
        match emerging {
            Some(edge) => unsafe {
                let start = slide_origin(origin, size, edge, 0.0);
                instance.app.refresh_shape_mask_clipped(
                    &instance.document,
                    panel_clip(dock, start, size, edge),
                );
            },
            None => unsafe { instance.app.refresh_shape_mask(&instance.document) },
        }
        cycle.advance("shape")?;
        cycle.advance("commit")?;
        unsafe {
            XMapWindow(display, window);
            // Slides stack above application windows; the panel clip keeps the emerging part hidden.
            if emerging.is_some() || surface_role_needs_popup_raise(role) {
                raise_above_taskbars(display, window, raise_targets(role, &taskbars));
            }
        }
        cycle.advance("map_raise")?;
        let instance = self.instance_mut(id)?;
        unsafe {
            instance.app.present_scene().map_err(mark_x_error)?;
            XFlush(display);
        }
        cycle.advance("present")?;
        cycle.advance("grab_flush")?;
        debug_assert!(cycle.is_complete());
        self.instance_mut(id)?.shown = true;
        Ok(())
    }

    /// Registers the X connection as a read wakeup source; X readiness only wakes dispatch.
    fn register_x_wakeup(
        &self,
        reactor: &mut Reactor,
    ) -> Result<calloop::RegistrationToken, String> {
        reactor
            .register_fd(
                ReactorPebble(self.connection_fd()),
                calloop::Interest::READ,
                |_, _| {},
            )
            .map_err(|error| error.to_string())
    }

    /// Dispatch one reactor cycle and return typed events after document callbacks finish.
    ///
    /// The UI bridge uses this boundary to apply surface operations without exposing the
    /// borrowed runtime document or holding the controller borrow across those operations.
    pub fn run_with_reactor_step(
        &mut self,
        reactor: &mut Reactor,
    ) -> Result<(Vec<SurfaceControllerEvent>, usize), String> {
        let token = self.register_x_wakeup(reactor)?;
        let mut events = Vec::new();
        let result: Result<usize, String> = (|| {
            reactor
                .dispatch(self.motion_frame_timeout())
                .map_err(|error| error.to_string())?;
            let drained = self.pump_events(|event, _document| {
                events.push(event.clone());
                Ok(())
            })?;
            self.advance_motion(Instant::now());
            Ok(drained)
        })();
        reactor.remove(token).map_err(|error| error.to_string())?;
        Ok((events, result?))
    }

    pub fn has_surfaces(&self) -> bool {
        !self.surfaces.is_empty()
    }

    fn instance_ref(&self, id: SurfaceId) -> Result<&SurfaceInstance, String> {
        self.surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))
    }

    fn instance_mut(&mut self, id: SurfaceId) -> Result<&mut SurfaceInstance, String> {
        self.surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))
    }

    fn with_window<F>(&self, id: SurfaceId, operation: F) -> Result<(), String>
    where
        F: FnOnce(*mut Display, Window) -> i32,
    {
        let instance = self.instance_ref(id)?;
        if operation(self.display, instance.app.window) == 0 {
            mark_x_error_seen();
            return Err(format!("X11 operation failed for surface {}", id.0));
        }
        unsafe {
            XFlush(self.display);
        }
        Ok(())
    }

    pub fn move_resize(
        &mut self,
        id: SurfaceId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        self.move_resize_traced(id, x, y, width, height, None)
    }

    /// Traced variant: records the native-request + retained stages under
    /// one txn (role resolved from the surface). Emits no X roundtrip.
    pub fn move_resize_traced(
        &mut self,
        id: SurfaceId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        trace: Option<GeometryTrace>,
    ) -> Result<(), String> {
        // Native move is single and authoritative: size changes go through
        // the transactional geometry commit (retained size -> backbuffer ->
        // window resize -> repaint); pure moves keep the retained size with
        // one XMoveResizeWindow.
        let size_changed = {
            let instance = self.instance_ref(id)?;
            width.max(1) != instance.app.width || height.max(1) != instance.app.height
        };
        let role_label = surface_role_label(self.surface_role(id)?);
        let span_trace = trace
            .map(|t| t.for_surface(id.get()))
            .unwrap_or_else(|| GeometryTrace {
                txn: next_geometry_txn(),
                surface: id.get(),
                role: role_label,
            });
        span_trace.native_request((x, y, width.max(1) as i32, height.max(1) as i32));
        let display = self.display;
        let instance = self.instance_mut(id)?;
        if size_changed {
            unsafe {
                instance
                    .app
                    .commit_geometry(&instance.document, width, height)
                    .map_err(mark_x_error)?
            };
        }
        let (w, h) = (instance.app.width, instance.app.height);
        unsafe { XMoveResizeWindow(display, instance.app.window, x, y, w, h) };
        instance.origin = (x, y);
        instance.slide_drawn = None;
        span_trace.retained((x, y, w as i32, h as i32));
        if instance.role == SurfaceRole::Dock {
            sync_dock_geometry(display, instance, x, y, w, h);
        }
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Native owner for an X11App window's requested move+size. Keeps the
    /// established order: one XMoveResizeWindow to the requested rect, then
    /// the transactional commit for the requested size.
    pub(crate) unsafe fn move_app_window_geometry(
        app: &mut crate::app::X11App,
        document: &RuntimeDocument,
        request: crate::WindowGeometry,
    ) -> Result<(), String> {
        unsafe {
            XMoveResizeWindow(
                app.display,
                app.window,
                request.x,
                request.y,
                request.width,
                request.height,
            )
        };
        unsafe { app.commit_geometry(document, request.width, request.height) }
    }

    /// Moves every active slide to `now`: one native move per changed pixel, the exact anchored
    /// origin on the last frame, and unmap when a slide-out completes. Content is never
    /// repainted here.
    /// Reapply the moving popup bounding shape after any retained repaint/Expose.
    /// Paint refresh may have restored the full base shape; the active flight is the
    /// authoritative owner of the dock clip until the native window is unmapped.
    fn refresh_moving_mask(
        instance: &SurfaceInstance,
        dock: Option<(i32, i32, u32, u32)>,
        edge: SlideEdge,
    ) {
        let at = instance.slide_drawn.unwrap_or(instance.origin);
        let size = (instance.app.width, instance.app.height);
        let clip = panel_clip(dock, at, size, edge);
        // SAFETY: instance owns the live window; no geometry mutation here.
        unsafe {
            instance
                .app
                .refresh_shape_mask_clipped(&instance.document, clip)
        };
    }

    pub(super) fn advance_motion(&mut self, now: Instant) {
        let display = self.display;
        let root = self.root_window();
        let dock = self.dock_rect();
        let taskbars = self.taskbar_windows();
        for (id, instance) in self.surfaces.iter_mut() {
            let key = id.get();
            let (Some(active), Some(progress)) =
                (self.slides.kind(key), self.slides.progress(key, now))
            else {
                continue;
            };
            let finished = self.slides.finished(key, now) == Some(true);
            if let Some((transition, overlay)) = instance.overlay.as_mut() {
                let landed = finished
                    || overlay
                        .paint(overlay_paint_progress(*transition, progress))
                        .is_err();
                if !landed {
                    continue;
                }
                self.slides.finish(key);
                if active.opening {
                    // Map and present the real popup, then drop the overlay: same turn, no gap.
                    // SAFETY: the window is this controller's live child of the root.
                    unsafe { XMapWindow(display, instance.app.window) };
                    raise_above_taskbars(
                        display,
                        instance.app.window,
                        raise_targets(instance.role, &taskbars),
                    );
                    let _ = unsafe { instance.app.present_scene() };
                    instance.shown = true;
                    if std::mem::take(&mut instance.grab_pending) {
                        // SAFETY: the window was just mapped; display/root are live.
                        unsafe {
                            grab_popup_presses(display, root, &mut self.press_selection, *id)
                        };
                        instance.app.pointer_grabbed = true;
                        unsafe { popup_take_focus(display, &mut instance.app) };
                    }
                } else {
                    instance.grab_pending = false;
                }
                instance.overlay = None;
                instance.slide_drawn = None;
                continue;
            }
            let window = instance.app.window;
            let size = (instance.app.width, instance.app.height);
            let (x, y) = landing_frame_origin(instance.origin, size, active.edge, progress);
            if instance.slide_drawn != Some((x, y)) {
                unsafe { XMoveResizeWindow(display, window, x, y, size.0, size.1) };
                instance.slide_drawn = Some((x, y));
            }
            // Shape is updated EVEN when the rounded popup did not move a pixel this
            // frame: a repaint or Expose can reset it to its un-clipped base mask.
            Self::refresh_moving_mask(instance, dock, active.edge);
            if !finished {
                continue;
            }
            self.slides.finish(key);
            if active.opening {
                // The full mask is visible only at the anchored, fully opened position.
                unsafe { instance.app.refresh_shape_mask(&instance.document) };
                raise_above_taskbars(display, window, raise_targets(instance.role, &taskbars));
            } else {
                // Never restore the full shape while still mapped: that exposes a final
                // un-clipped frame in front of the dock for one X server composite.
                // Finish the unmap at the server BEFORE restoring a full mask. The
                // earlier XFlush-only path could expose a final full-size composite
                // between the mask reset and the next paint/compositor turn.
                unsafe { XUnmapWindow(display, window) };
                unsafe { XSync(display, 0) };
                instance.shown = false;
                instance.slide_drawn = None;
                unsafe { instance.app.refresh_shape_mask(&instance.document) };
                instance.app.clear_surface_backdrop();
                instance.app.clear_node_backdrops();
                if landing_releases_grab(active.opening, instance.app.pointer_grabbed) {
                    drop_popup_grab(
                        display,
                        root,
                        &mut self.press_selection,
                        *id,
                        &mut instance.app,
                    );
                }
            }
        }
        unsafe { XFlush(display) };
    }
}
