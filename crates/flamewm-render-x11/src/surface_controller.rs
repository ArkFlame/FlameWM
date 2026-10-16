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
mod present;
mod raw_press;
mod slide;
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
            x,
            y,
            inside: false,
            time_ms,
            text: None,
        },
    }
}

pub fn run_with_surface_controller_events<F>(
    mut document: RuntimeDocument,
    config: X11Config,
    mut on_event: F,
) -> Result<(), String>
where
    F: FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    unsafe {
        let display = XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
        }
        let mut app = match X11App::new(display, &config) {
            Ok(app) => app,
            Err(error) => {
                XCloseDisplay(display);
                return Err(error);
            }
        };
        let surface = SurfaceId(1);
        let result = app.event_loop(&mut document, &mut |event, document| {
            on_event(
                &SurfaceControllerEvent {
                    surface,
                    event: event.clone(),
                },
                document,
            )
        });
        let deferred_native = unsafe { app.take_deferred_native_libraries() };
        drop(app);
        XCloseDisplay(display);
        drop(deferred_native);
        result
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

/// DEBUG-ONLY pure stack-order predicate (no X): bottom-to-top roles must
/// satisfy desktop < normal < dock < popup (popup covers menu/dropdown/
/// overlay). Test/canary use only.
pub fn stack_order_ok(bottom_to_top: &[SurfaceRole]) -> bool {
    fn rank(role: SurfaceRole) -> u8 {
        match role {
            SurfaceRole::Desktop => 0,
            SurfaceRole::Normal => 1,
            SurfaceRole::Dock => 2,
            SurfaceRole::PopupMenu | SurfaceRole::DropdownMenu | SurfaceRole::Overlay => 3,
        }
    }
    let mut last: Option<u8> = None;
    for role in bottom_to_top {
        let rank = rank(*role);
        // Non-decreasing rank keeps equal-class siblings total without
        // imposing an order inside one class.
        if let Some(prev) = last {
            if rank < prev {
                return false;
            }
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

pub struct SurfaceController {
    display: *mut Display,
    screen: i32,
    next_id: u64,
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
    /// A root child was mapped, unmapped, configured or destroyed since the last `take_root_changed`.
    root_changed: bool,
    /// XInput2 raw-press selection that dismisses popups on outside presses.
    press_selection: RawPressSelection,
    /// Set while the quick-popup press observer holds its root selection.
    press_observer_armed: bool,
    /// Root presses observed while armed, drained by the owner.
    raw_press_observations: Vec<RawPressObservation>,
}


impl SurfaceController {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let display = XOpenDisplay(ptr::null());
            if display.is_null() {
                return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
            }
            install_x_io_error_handler();
            install_x_protocol_error_handler();
            let press_opcode = xi_raw_press_opcode(display);
            if press_opcode.is_none() {
                eprintln!("flamewm: XInput2 2.0 unavailable; popups dismiss on focus loss and Escape only");
            }
            Ok(Self {
                screen: XDefaultScreen(display),
                display,
                next_id: 1,
                surfaces: HashMap::new(),
                windows: HashMap::new(),
                deferred_native_libraries: Vec::new(),
                icon_geometry_atom: 0,
                slides: Timeline::new(),
                root_changed: false,
                press_selection: RawPressSelection::new(press_opcode),
                press_observer_armed: false,
                raw_press_observations: Vec::new(),
            })
        }
    }

    pub fn connection_fd(&self) -> RawFd {
        unsafe { XConnectionNumber(self.display) as RawFd }
    }

    pub fn root_geometry(&self) -> (u32, u32) {
        unsafe {
            (
                XDisplayWidth(self.display, self.screen).max(0) as u32,
                XDisplayHeight(self.display, self.screen).max(0) as u32,
            )
        }
    }

    pub fn create_surface(
        &mut self,
        document: RuntimeDocument,
        config: SurfaceConfig,
    ) -> Result<SurfaceId, String> {
        let id = SurfaceId(self.next_id);
        self.next_id = self.next_id.checked_add(1).ok_or("surface id exhausted")?;
        let (role, input) = (config.role, config.input);
        let x11_config = X11Config::from(config.clone());
        let app =
            unsafe { X11App::new_surface(self.display, &x11_config, config.x, config.y, false)? };
        let window = app.window;
        let mut instance = SurfaceInstance {
            app,
            document,
            role,
            input,
            origin: (config.x, config.y),
            slide: None,
            shown: config.initially_visible,
            slide_drawn: None,
            overlay: None,
            grab_pending: false,
            dock_edge: None,
            dock_radius: 0,
            popup_backdrop: None,
        };
        // Overlay/passthrough surfaces are excluded from WM client
        // management: override-redirect + empty input region + no grab/focus.
        if overlay_excluded_from_wm(role) {
            unsafe {
                let mut attributes = XSetWindowAttributes {
                    background_pixmap: 0,
                    background_pixel: TRANSPARENT_CLEAR_PIXEL,
                    border_pixmap: 0,
                    border_pixel: 0,
                    bit_gravity: 0,
                    win_gravity: 0,
                    backing_store: 0,
                    backing_planes: 0,
                    backing_pixel: 0,
                    save_under: 0,
                    event_mask: 0,
                    do_not_propagate_mask: 0,
                    override_redirect: 1,
                    colormap: 0,
                    cursor: 0,
                };
                XChangeWindowAttributes(
                    self.display,
                    window,
                    CW_OVERRIDE_REDIRECT,
                    &mut attributes,
                );
            }
        }
        if input == SurfaceInputMode::PassThrough {
            if let Some(bridge) = instance.app.xshape.as_ref() {
                // Empty input region: pointer falls through the surface.
                if let Err(error) = unsafe { bridge.apply_input_mask(window, &[]) } {
                    mark_x_error_seen();
                    return Err(error);
                }
            } else {
                return Err(
                    "passthrough surface requires XShape input region (unavailable)".to_string(),
                );
            }
        }
        // Hidden create: mark Full dirty, no layout/paint/present/map/shape.
        // First show renders retained offscreen, then maps (popup map+raise).
        if !config.initially_visible {
            instance.app.mark_full();
            instance.document.mark_dirty(SurfaceDamage::Full);
            self.windows.insert(window, id);
            self.surfaces.insert(id, instance);
            return Ok(id);
        }
        // Visible create: paint retained offscreen first (redraw refreshes
        // shape once), then map (popup map+raise) + present/flush.
        unsafe {
            instance.app.redraw(&instance.document).map_err(|error| {
                mark_x_error_seen();
                error
            })?;
        }
        unsafe {
            if surface_role_needs_popup_raise(role) {
                XMapWindow(self.display, window);
                XRaiseWindow(self.display, window);
            } else {
                XMapWindow(self.display, window);
            }
            instance.app.present_scene().map_err(|error| {
                mark_x_error_seen();
                error
            })?;
            XFlush(self.display);
        }
        self.windows.insert(window, id);
        self.surfaces.insert(id, instance);
        Ok(id)
    }

    pub fn destroy_surface(&mut self, id: SurfaceId) -> Result<(), String> {
        if let Some(mut instance) = self.surfaces.remove(&id) {
            self.slides.cancel(id.get());
            self.windows.remove(&instance.app.window);
            if instance.app.pointer_grabbed {
                unsafe {
                    release_popup_presses(self.display, self.root_window(), &mut self.press_selection, id);
                    popup_restore_focus(self.display, &mut instance.app);
                }
                instance.app.pointer_grabbed = false;
            }
            if x_io_broken() || x_error_seen() || !display_fd_alive(self.display) {
                // No backend may call into a broken display. Preserve the
                // complete app until process teardown rather than unloading a
                // library whose close-display hook may still be referenced.
                std::mem::forget(instance);
            } else {
                // SAFETY: this controller still owns the live display.
                self.deferred_native_libraries
                    .push(unsafe { instance.app.take_deferred_native_libraries() });
            }
            Ok(())
        } else {
            Err(format!("unknown surface {}", id.0))
        }
    }

    /// Close one surface in the same turn: ungrab-once (only when grabbed),
    /// then unmap + Expose-present (retained re-blit) + flush. No repaint.
    pub fn close_surface(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if instance.app.pointer_grabbed {
            unsafe {
                release_popup_presses(display, root, &mut self.press_selection, id);
                popup_restore_focus(display, &mut instance.app);
            }
            instance.app.pointer_grabbed = false;
        }
        unsafe {
            XUnmapWindow(display, instance.app.window);
            instance.app.present_scene().map_err(|error| {
                mark_x_error_seen();
                error
            })?;
            XFlush(display);
        }
        // The popup is unmapped here without a slide-out: drop slide bookkeeping so the next show
        // starts a fresh slide instead of taking the "already shown" path with no animation.
        instance.shown = false;
        instance.slide_drawn = None;
        // SAFETY: display/window live; a closed popup must not keep a slide clip for its next show.
        unsafe { instance.app.refresh_shape_mask(&instance.document) };
        // Immediate close drops any Fade/Lamp overlay without playing it.
        instance.overlay = None;
        self.slides.cancel(id.get());
        Ok(())
    }

    pub fn show(&mut self, id: SurfaceId) -> Result<(), String> {
        // First-show single present: project->measure->anchor->prepare->
        // paint->shape->commit->map+raise->present->grab->flush in one
        // ordered cycle, no feature redraw. Second show sees clean damage
        // and maps/presents without repainting.
        let role = self.surface_role(id)?;
        let display = self.display;
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
        if !shown {
            // Before any paint or map: the root under the unmapped popup is still clean.
            if let Err(error) = self.install_popup_backdrop(id) {
                eprintln!("flamewm-render-x11: popup backdrop skipped (nonfatal): {error}");
            }
        }
        if let Some(config) = slide.filter(|config| config.transition != PopupTransition::Slide) {
            if self.begin_overlay_show(id, config, shown, now)? {
                return Ok(());
            }
        }
        let mut emerging: Option<SlideEdge> = None;
        if let Some(config) = slide.filter(|config| config.transition == PopupTransition::Slide) {
            self.slides.set_speed(config.speed);
            let opening = SlideKind { edge: config.edge, opening: true };
            if shown {
                match self.slides.kind(key) {
                    // Double open while opening: already converging on the final frame.
                    Some(active) if active.opening => return Ok(()),
                    // Reverse from the current progress; the window is still mapped.
                    Some(_) => {
                        self.slides.start_towards(key, opening, now, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0));
                        return Ok(());
                    }
                    None => {}
                }
            } else if self
                .slides
                .start_towards(key, opening, now, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0))
                == Start::Started
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
                .map_err(|error| {
                    mark_x_error_seen();
                    error
                })?;
        }
        cycle.advance("paint")?;
        // SAFETY: display/window live; shape follows retained repaint. A slide clips to the
        // panel side before the window maps, so its first visible frame is already clipped.
        match emerging {
            Some(edge) => unsafe {
                let start = slide_origin(origin, size, edge, 0.0);
                instance
                    .app
                    .refresh_shape_mask_clipped(&instance.document, panel_clip(dock, start, size, edge));
            },
            None => unsafe { instance.app.refresh_shape_mask(&instance.document) },
        }
        cycle.advance("shape")?;
        cycle.advance("commit")?;
        unsafe {
            XMapWindow(display, window);
            // Slides stack above application windows; the panel clip keeps the emerging part hidden.
            if emerging.is_some() || surface_role_needs_popup_raise(role) {
                XRaiseWindow(display, window);
            }
        }
        cycle.advance("map_raise")?;
        let instance = self.instance_mut(id)?;
        unsafe {
            instance.app.present_scene().map_err(|error| {
                mark_x_error_seen();
                error
            })?;
            XFlush(display);
        }
        cycle.advance("present")?;
        cycle.advance("grab_flush")?;
        debug_assert!(cycle.is_complete());
        let instance = self.instance_mut(id)?;
        instance.shown = true;
        Ok(())
    }

    /// Releases this popup's pointer grab and restores focus. Idempotent.
    fn release_grab(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if instance.app.pointer_grabbed {
            unsafe {
                release_popup_presses(display, root, &mut self.press_selection, id);
                popup_restore_focus(display, &mut instance.app);
            }
            instance.app.pointer_grabbed = false;
        }
        Ok(())
    }

    pub fn hide(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let now = Instant::now();
        let key = id.get();
        let (slide, shown) = {
            let instance = self
                .surfaces
                .get_mut(&id)
                .ok_or_else(|| format!("unknown surface {}", id.0))?;
            (instance.slide, instance.shown)
        };
        // A slide-out keeps the grab so an outside press during the close is still captured;
        // advance_motion releases it when the slide lands.
        if !slide_out_keeps_grab(slide, shown) {
            self.release_grab(id)?;
        }
        if let Some(config) = slide.filter(|config| config.transition != PopupTransition::Slide) {
            if self.begin_overlay_hide(id, config, shown, now)? {
                unsafe { XFlush(display) };
                return Ok(());
            }
        }
        let slide = slide.filter(|config| config.transition == PopupTransition::Slide);
        if let (Some(config), true) = (slide, shown) {
            // Already closing: keep the running transition.
            if self.slides.kind(key).is_some_and(|active| !active.opening) {
                unsafe { XFlush(display) };
                return Ok(());
            }
            self.slides.set_speed(config.speed);
            let closing = SlideKind { edge: config.edge, opening: false };
            let window = self.instance_mut(id)?.app.window;
            // Slide-out starts above application windows; the panel clip hides the sinking part.
            unsafe { XRaiseWindow(display, window) };
            // A new close starts fully shown; a reversed open continues from its progress.
            if self.slides.start_towards(key, closing, now, SLIDE_CLOSE_BASE_MS, Easing::EaseOutCubic, (1.0, 0.0))
                == Start::Started
            {
                unsafe { XFlush(display) };
                return Ok(());
            }
        }
        // No slide, or the timeline refused the transition: unmap immediately.
        self.release_grab(id)?;
        self.slides.cancel(key);
        let instance = self.instance_mut(id)?;
        unsafe {
            XUnmapWindow(display, instance.app.window);
            XFlush(display);
        }
        instance.shown = false;
        instance.slide_drawn = None;
        // The snapshot is recaptured on the next show, so a closed popup keeps no stale pixels.
        instance.app.clear_surface_backdrop();
        instance.app.clear_node_backdrops();
        Ok(())
    }

    /// Paints pending popup content into its backbuffer and captures it without mapping the window.
    fn capture_popup(&mut self, id: SurfaceId) -> Result<CapturedImage, String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (width, height) = (instance.app.width, instance.app.height);
        // SAFETY: the app owns its backbuffer and document; both are live for this call.
        unsafe {
            let painted = instance
                .app
                .redraw_if_dirty(&mut instance.document)
                .map_err(|error| {
                    mark_x_error_seen();
                    error
                })?;
            if painted {
                instance.app.refresh_shape_mask(&instance.document);
            }
        }
        let backdrop = crate::backdrop::capture_drawable_rect(
            display,
            instance.app.backbuffer as u64,
            (0, 0, width, height),
        )?;
        crate::xcomposite::rgba_to_argb_owned(backdrop.width, backdrop.height, backdrop.pixels)
    }

    pub fn raise(&self, id: SurfaceId) -> Result<(), String> {
        self.with_window(id, |display, window| unsafe {
            XRaiseWindow(display, window)
        })
    }

    /// Publishes window-relative pager cell rects on a surface's X window
    /// (`FLAMEWM_PAGER_CELLS`). Callers pass rects derived from the retained
    /// layout of that same surface.
    pub fn set_pager_cells(&mut self, id: SurfaceId, cells: &[[u64; 4]]) -> Result<(), String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        crate::app::apply_pager_cells(instance.app.display, instance.app.window, cells);
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Sets `_NET_WM_ICON_GEOMETRY` on a client window (not a surface this
    /// controller owns) to a root-space rect. Errors from a vanished client
    /// are ignored by the protocol handler; the caller never aborts.
    pub fn set_window_icon_geometry(&mut self, window: u64, rect: [u32; 4]) -> Result<(), String> {
        let property = self.icon_geometry_atom();
        crate::app::apply_window_icon_geometry(
            self.display,
            window as Window,
            property,
            Some(rect.map(u64::from)),
        );
        unsafe { XFlush(self.display) };
        Ok(())
    }

    /// Deletes `_NET_WM_ICON_GEOMETRY` from a client window.
    pub fn clear_window_icon_geometry(&mut self, window: u64) -> Result<(), String> {
        let property = self.icon_geometry_atom();
        crate::app::apply_window_icon_geometry(self.display, window as Window, property, None);
        unsafe { XFlush(self.display) };
        Ok(())
    }

    fn icon_geometry_atom(&mut self) -> Atom {
        if self.icon_geometry_atom == 0 {
            let name = std::ffi::CString::new("_NET_WM_ICON_GEOMETRY")
                .expect("static atom contains no NUL");
            self.icon_geometry_atom = unsafe { XInternAtom(self.display, name.as_ptr(), 0) };
        }
        self.icon_geometry_atom
    }

    /// Retained native origin update (root-space device pixels). Called
    /// after a successful move so geometry queries track the window.
    pub fn set_origin(&mut self, id: SurfaceId, x: i32, y: i32) -> Result<(), String> {
        self.instance_mut(id)?.origin = (x, y);
        Ok(())
    }

    pub fn grab_pointer(&mut self, id: SurfaceId) -> Result<(), String> {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if !instance.input.allows_grab() {
            return Err(format!(
                "surface {} is pass-through; pointer grab refused",
                id.0
            ));
        }
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if instance.overlay.is_some() && !instance.shown {
            // The real window maps when the opening flight lands; grab then.
            instance.grab_pending = true;
            return Ok(());
        }
        // SAFETY: live connection; raw press selection only (see grab_popup_presses).
        unsafe { grab_popup_presses(display, root, &mut self.press_selection, id) };
        instance.app.pointer_grabbed = true;
        unsafe {
            popup_take_focus(display, &mut instance.app);
            XFlush(display);
        }
        Ok(())
    }

    pub fn ungrab_pointer(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        unsafe {
            release_popup_presses(display, root, &mut self.press_selection, id);
            popup_restore_focus(display, &mut instance.app);
        }
        instance.app.pointer_grabbed = false;
        unsafe {
            XFlush(display);
        }
        Ok(())
    }

    /// Subscribes to root child map/unmap/configure/destroy (SubstructureNotify is shared, so the
    /// window manager's redirect is unaffected). Idempotent.
    pub fn watch_root_changes(&self) {
        // SAFETY: the display is live; the root window is the screen's root.
        unsafe {
            XSelectInput(self.display, XRootWindow(self.display, self.screen), ROOT_SUBSTRUCTURE_NOTIFY_MASK);
        }
    }

    /// Whether root children changed since the last call, then clears the flag.
    pub fn take_root_changed(&mut self) -> bool {
        std::mem::take(&mut self.root_changed)
    }

    /// Mark damage on one surface without painting (merged max coverage).
    pub fn mark_surface_dirty(
        &mut self,
        id: SurfaceId,
        damage: SurfaceDamage,
    ) -> Result<(), String> {
        self.instance_mut(id)?.app.mark_dirty(damage);
        self.instance_mut(id)?.document.mark_dirty(damage);
        Ok(())
    }

    /// Peek the merged damage latch for one surface (no consume).
    pub fn peek_surface_damage(&self, id: SurfaceId) -> Result<SurfaceDamage, String> {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        Ok(instance
            .app
            .pending_damage
            .merge(instance.document.peek_damage()))
    }

    pub fn pump_events<F>(&mut self, mut on_action: F) -> Result<usize, String>
    where
        F: FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        let mut handled = 0;
        loop {
            let queued = unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) };
            if queued <= 0 {
                break;
            }
            let mut event = MaybeUninit::<XEvent>::uninit();
            unsafe {
                XNextEvent(self.display, event.as_mut_ptr());
            }
            let event = unsafe { event.assume_init() };
            let root = unsafe { XRootWindow(self.display, self.screen) };
            if unsafe { event.type_ } == GENERIC_EVENT
                && Some(unsafe { generic_cookie(&event) }.extension) == self.press_selection.opcode()
            {
                // Raw press from the popup selection: observe only, never grabbed or replayed.
                self.dismiss_on_raw_press(&event, &mut on_action)?;
                handled += 1;
                continue;
            }
            if unsafe { event.xany.window } == root {
                // Other root children coming and going change what lies under a pseudo-transparency
                // bar. Our own surfaces are excluded: a capture unmaps and remaps them itself.
                let kind = unsafe { event.type_ };
                let child = unsafe { event.xmap.window };
                self.root_changed |= matches!(kind, ROOT_CHILD_MAP_NOTIFY | ROOT_CHILD_UNMAP_NOTIFY | ROOT_CHILD_CONFIGURE_NOTIFY | ROOT_CHILD_DESTROY_NOTIFY)
                    && !self.windows.contains_key(&child);
                continue;
            }
            let Some(id) = self.windows.get(&unsafe { event.xany.window }).copied() else {
                continue;
            };
            let event_type = unsafe { event.type_ };
            // Pointer input is ignored while the popup is moving: the menu is not hit-testable
            // until its final frame. Keyboard input still reaches the final layout.
            let moving = self.slides.kind(id.get()).is_some();
            if moving && matches!(event_type, BUTTON_PRESS | BUTTON_RELEASE | MOTION_NOTIFY) {
                continue;
            }
            // Outside presses arrive as raw root presses (dismiss_on_raw_press).
            // A release that ends outside the popup (e.g. a slider drag) never dismisses.
            unsafe {
                XPutBackEvent(self.display, &event as *const XEvent as *mut XEvent);
            }
            let closed = {
                let instance = self.instance_mut(id)?;
                let mut callback = |event: &ActionEvent, document: &mut RuntimeDocument| {
                    on_action(
                        &SurfaceControllerEvent {
                            surface: id,
                            event: event.clone(),
                        },
                        document,
                    )
                };
                unsafe {
                    instance
                        .app
                        .event_loop_once(&mut instance.document, &mut callback)
                        .map_err(|error| {
                            mark_x_error_seen();
                            error
                        })?;
                }
                // Drain the document-owned damage latch into the app latch so
                // `redraw_dirty` can coalesce; event_loop already merged its
                // own marks via redraw_if_dirty.
                let pending = instance.document.take_damage();
                instance.app.mark_dirty(pending);
                instance.app.close_requested
            };
            handled += 1;
            if closed {
                self.destroy_surface(id)?;
            }
        }
        Ok(handled)
    }

    pub fn document_mut(&mut self, id: SurfaceId) -> Result<&mut RuntimeDocument, String> {
        Ok(&mut self.instance_mut(id)?.document)
    }

    fn retained(&self, id: SurfaceId) -> Result<(&X11App, &RuntimeDocument), String> {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        Ok((&instance.app, &instance.document))
    }

    /// Retained-live surface extent in root space: native origin + the
    /// retained window size. Reads retained state only; no LayoutEngine
    /// recompute, no headless fallback.
    pub fn surface_device_rect(&self, id: SurfaceId) -> Result<Rect, String> {
        let (app, _) = self.retained(id)?;
        let origin = self
            .surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0));
        Ok(Rect {
            x: origin.0 as f32,
            y: origin.1 as f32,
            width: app.width as f32,
            height: app.height as f32,
        })
    }

    /// Retained-live node rect in root space: native origin + the retained
    /// layout box scaled by ui_scale. None-shaped as Err when no retained
    /// layout exists (mapped-surface queries never use 1350x641 here).
    pub fn node_device_rect(&self, id: SurfaceId, index: u32) -> Result<Rect, String> {
        let (app, document) = self.retained(id)?;
        let origin = self
            .surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0));
        let local = app
            .node_global_rect(document, index)
            .ok_or_else(|| format!("surface {} has no retained layout", id.0))?;
        Ok(Rect {
            x: local.x + origin.0 as f32,
            y: local.y + origin.1 as f32,
            width: local.width,
            height: local.height,
        })
    }

    /// Node rect by string id from retained layout (root-space device rect).
    pub fn node_device_rect_by_id(&self, id: SurfaceId, node: &str) -> Result<Rect, String> {
        let (app, document) = self.retained(id)?;
        let index = document
            .node_by_id(node)
            .ok_or_else(|| format!("no node with id '{node}'"))?;
        let origin = self
            .surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0));
        let local = app
            .node_global_rect(document, index)
            .ok_or_else(|| format!("surface {} has no retained layout", id.0))?;
        Ok(Rect {
            x: local.x + origin.0 as f32,
            y: local.y + origin.1 as f32,
            width: local.width,
            height: local.height,
        })
    }

    /// Retained intrinsic content size of the document root in device
    /// pixels. Reads the retained layout; no LayoutEngine recompute.
    pub fn document_intrinsic_device_size(&self, id: SurfaceId) -> Result<(f32, f32), String> {
        let (app, document) = self.retained(id)?;
        app.document_intrinsic_size(document)
            .ok_or_else(|| format!("surface {} has no retained layout", id.0))
    }

    pub fn controller_event(&self, id: SurfaceId, event: ActionEvent) -> SurfaceControllerEvent {
        SurfaceControllerEvent { surface: id, event }
    }

    pub fn surface_role(&self, id: SurfaceId) -> Result<SurfaceRole, String> {
        Ok(self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?
            .role)
    }

    pub fn surface_input_mode(&self, id: SurfaceId) -> Result<SurfaceInputMode, String> {
        Ok(self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?
            .input)
    }

    /// Overlay management policy: overlays stay raisable but are never
    /// WM-managed, never focusable, and never in the workarea/taskbar.
    pub fn is_wm_managed(&self, id: SurfaceId) -> Result<bool, String> {
        Ok(!overlay_excluded_from_wm(self.surface_role(id)?))
    }

    pub fn is_pointer_grabbed(&self, id: SurfaceId) -> Result<bool, String> {
        Ok(self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?
            .app
            .pointer_grabbed)
    }

    pub fn run_with_reactor<F, R>(
        &mut self,
        reactor: &mut Reactor,
        mut on_action: F,
        mut on_tick: R,
    ) -> Result<(), String>
    where
        F: FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
        R: FnMut(&mut Self) -> Result<(), String>,
    {
        let token = reactor
            .register_fd(
                ReactorPebble(self.connection_fd()),
                calloop::Interest::READ,
                move |_, _| {
                    // X readiness only wakes dispatch; pump happens below.
                },
            )
            .map_err(|error| error.to_string())?;
        loop {
            reactor
                .dispatch(self.motion_frame_timeout())
                .map_err(|error| error.to_string())?;
            let drained = self.pump_events(&mut on_action)?;
            self.advance_motion(Instant::now());
            if self.surfaces.is_empty() {
                break;
            }
            // Timer sources registered by the caller also wake dispatch, so
            // tick callbacks always run; redraws follow drained X events.
            on_tick(self)?;
            if drained > 0 {
                self.redraw_dirty()?;
            }
        }
        reactor.remove(token).map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Dispatch one reactor cycle and return typed events after document callbacks finish.
    ///
    /// The UI bridge uses this boundary to apply surface operations without exposing the
    /// borrowed runtime document or holding the controller borrow across those operations.
    pub fn run_with_reactor_step(
        &mut self,
        reactor: &mut Reactor,
    ) -> Result<(Vec<SurfaceControllerEvent>, usize), String> {
        let token = reactor
            .register_fd(
                ReactorPebble(self.connection_fd()),
                calloop::Interest::READ,
                move |_, _| {
                    // X readiness only wakes dispatch; pump happens below.
                },
            )
            .map_err(|error| error.to_string())?;
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

    fn instance_mut(&mut self, id: SurfaceId) -> Result<&mut SurfaceInstance, String> {
        self.surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))
    }

    fn with_window<F>(&self, id: SurfaceId, operation: F) -> Result<(), String>
    where
        F: FnOnce(*mut Display, Window) -> i32,
    {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
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
            let instance = self
                .surfaces
                .get(&id)
                .ok_or_else(|| format!("unknown surface {}", id.0))?;
            width.max(1) != instance.app.width || height.max(1) != instance.app.height
        };
        let role_label = self
            .surfaces
            .get(&id)
            .map(|instance| surface_role_label(instance.role))
            .unwrap_or("unknown");
        let span_trace = trace
            .map(|t| t.for_surface(id.get()))
            .unwrap_or_else(|| GeometryTrace {
                txn: next_geometry_txn(),
                surface: id.get(),
                role: role_label,
            });
        span_trace.native_request((x, y, width.max(1) as i32, height.max(1) as i32));
        if size_changed {
            let display = self.display;
            let instance = self.instance_mut(id)?;
            unsafe {
                instance
                    .app
                    .commit_geometry(&instance.document, width, height)
                    .map_err(|error| {
                        mark_x_error_seen();
                        error
                    })?
            };
            unsafe {
                XMoveResizeWindow(
                    display,
                    instance.app.window,
                    x,
                    y,
                    instance.app.width,
                    instance.app.height,
                )
            };
            instance.origin = (x, y);
            instance.slide_drawn = None;
            span_trace.retained((x, y, instance.app.width as i32, instance.app.height as i32));
            if instance.role == SurfaceRole::Dock {
                let root_w = unsafe { XDisplayWidth(display, instance.app.screen).max(1) as u32 };
                let root_h = unsafe { XDisplayHeight(display, instance.app.screen).max(1) as u32 };
                let (ox, oy) = instance.origin;
                let (w, h) = (instance.app.width, instance.app.height);
                crate::app::apply_dock_strut(
                    instance.app.display,
                    instance.app.window,
                    root_w,
                    root_h,
                    instance.dock_edge,
                    ox,
                    oy,
                    w,
                    h,
                );
                apply_dock_mask(instance, w, h, dock_mask_plan(0, instance.dock_radius));
            }
            unsafe { XFlush(display) };
            return Ok(());
        }
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (w, h) = (instance.app.width, instance.app.height);
        unsafe { XMoveResizeWindow(display, instance.app.window, x, y, w, h) };
        instance.origin = (x, y);
        instance.slide_drawn = None;
        if instance.role == SurfaceRole::Dock {
            let root_w = unsafe { XDisplayWidth(display, instance.app.screen).max(1) as u32 };
            let root_h = unsafe { XDisplayHeight(display, instance.app.screen).max(1) as u32 };
            crate::app::apply_dock_strut(
                instance.app.display,
                instance.app.window,
                root_w,
                root_h,
                instance.dock_edge,
                x,
                y,
                w,
                h,
            );
            apply_dock_mask(instance, w, h, dock_mask_plan(0, instance.dock_radius));
        }
        span_trace.retained((x, y, w as i32, h as i32));
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
    pub(super) fn advance_motion(&mut self, now: Instant) {
        let display = self.display;
        let root = self.root_window();
        let dock = self.dock_rect();
        for (id, instance) in self.surfaces.iter_mut() {
            let key = id.get();
            let (Some(active), Some(progress)) = (self.slides.kind(key), self.slides.progress(key, now)) else {
                continue;
            };
            let finished = self.slides.finished(key, now) == Some(true);
            if let Some((transition, overlay)) = instance.overlay.as_mut() {
                let landed = finished
                    || overlay.paint(overlay_paint_progress(*transition, progress)).is_err();
                if !landed {
                    continue;
                }
                self.slides.finish(key);
                if active.opening {
                    // Map and present the real popup, then drop the overlay: same turn, no gap.
                    // SAFETY: the window is this controller's live child of the root.
                    unsafe {
                        XMapWindow(display, instance.app.window);
                        XRaiseWindow(display, instance.app.window);
                    }
                    let _ = unsafe { instance.app.present_scene() };
                    instance.shown = true;
                    if std::mem::take(&mut instance.grab_pending) {
                        // SAFETY: the window was just mapped; display/root are live.
                        unsafe { grab_popup_presses(display, root, &mut self.press_selection, *id) };
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
                // Keep the popup on the screen side of the panel edge while it moves.
                let clip = panel_clip(dock, (x, y), size, active.edge);
                unsafe { instance.app.refresh_shape_mask_clipped(&instance.document, clip) };
            }
            if !finished {
                continue;
            }
            // Slide landed: the base rounded mask is exact again.
            unsafe { instance.app.refresh_shape_mask(&instance.document) };
            self.slides.finish(key);
            if active.opening {
                // Back on top: the popup sits against the panel, so the raise is invisible.
                unsafe { XRaiseWindow(display, window) };
            } else {
                // Slide-out landed: release the grab held since hide() started the close.
                if landing_releases_grab(active.opening, instance.app.pointer_grabbed) {
                    unsafe {
                        release_popup_presses(display, root, &mut self.press_selection, *id);
                        popup_restore_focus(display, &mut instance.app);
                    }
                    instance.app.pointer_grabbed = false;
                }
                unsafe { XUnmapWindow(display, window) };
                instance.shown = false;
                instance.slide_drawn = None;
            }
        }
        unsafe { XFlush(display) };
    }

}
/// Borrowed X connection fd wrapper for reactor registration.
/// The display outlives the registration; calloop never owns the fd.
struct ReactorPebble(RawFd);

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

/// True while the X connection fd still exists. After an X IO error the
/// fd is gone; probing via /proc needs std only (no new deps).
fn display_fd_alive(display: *mut Display) -> bool {
    let fd = unsafe { XConnectionNumber(display) };
    if fd < 0 {
        return false;
    }
    std::path::Path::new(&format!("/proc/self/fd/{fd}")).exists()
}

