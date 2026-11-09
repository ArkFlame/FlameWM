use super::*;

impl SurfaceController {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let display = XOpenDisplay(ptr::null());
            if display.is_null() {
                return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
            }
            install_x_io_error_handler();
            install_x_protocol_error_handler();
            crate::xlib::return_large_frees_to_system();
            let press_opcode = xi_raw_press_opcode(display);
            if press_opcode.is_none() {
                eprintln!(
                    "flamewm: XInput2 2.0 unavailable; popups dismiss on focus loss and Escape only"
                );
            }
            Ok(Self {
                screen: XDefaultScreen(display),
                display,
                next_id: 1,
                node_wheel_targets: false,
                surfaces: HashMap::new(),
                windows: HashMap::new(),
                deferred_native_libraries: Vec::new(),
                icon_geometry_atom: 0,
                slides: Timeline::new(),
                external_dock: None,
                root_changed: false,
                root_geometry_changed: None,
                press_selection: RawPressSelection::new(press_opcode),
                press_observer_armed: false,
                raw_press_observations: Vec::new(),
                desktop: None,
            })
        }
    }

    pub fn connection_fd(&self) -> RawFd {
        unsafe { XConnectionNumber(self.display) as RawFd }
    }

    pub fn root_geometry(&self) -> (u32, u32) {
        let (mut root_return, mut x, mut y) = (0, 0, 0);
        let (mut width, mut height, mut border, mut depth) = (0, 0, 0, 0);
        let status = unsafe {
            crate::xlib_ext::XGetGeometry(
                self.display,
                self.root_window(),
                &mut root_return,
                &mut x,
                &mut y,
                &mut width,
                &mut height,
                &mut border,
                &mut depth,
            )
        };
        if status != 0 {
            (width, height)
        } else {
            unsafe {
                (
                    XDisplayWidth(self.display, self.screen).max(0) as u32,
                    XDisplayHeight(self.display, self.screen).max(0) as u32,
                )
            }
        }
    }

    /// Opt-in for surfaces created afterwards: wheel ticks over a node with an action also
    /// reach that node as a `Motion`-phase event carrying the wheel button and `dx,dy` text.
    /// Off by default so other apps keep only the `scroll` action.
    pub fn set_node_wheel_targets(&mut self, enabled: bool) {
        self.node_wheel_targets = enabled;
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
        let mut app =
            unsafe { X11App::new_surface(self.display, &x11_config, config.x, config.y, false)? };
        app.node_wheel_targets = self.node_wheel_targets;
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
            dock_bar: None,
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
            instance
                .app
                .redraw(&instance.document)
                .map_err(mark_x_error)?;
        }
        unsafe {
            XMapWindow(self.display, window);
            if surface_role_needs_popup_raise(role) {
                raise_above_taskbars(
                    self.display,
                    window,
                    raise_targets(role, &self.taskbar_windows()),
                );
            }
            instance.app.present_scene().map_err(mark_x_error)?;
            XFlush(self.display);
        }
        self.windows.insert(window, id);
        self.surfaces.insert(id, instance);
        Ok(id)
    }

    /// Override the XShape bounding silhouette of one overlay with the alpha pixels of its icon.
    /// Default paint coverage uses the image's whole rectangular bounds, exposing an opaque
    /// black box on a 24-bit visual. `None` restores the normal coverage-derived shape.
    pub fn set_surface_shape_mask(
        &mut self,
        id: SurfaceId,
        spans: Option<Vec<(u32, u32, u32)>>,
    ) -> Result<(), String> {
        let instance = self.instance_mut(id)?;
        instance.app.shape_override = spans;
        instance.app.shape_applied.set(None);
        instance.app.mark_full();
        instance.document.mark_dirty(SurfaceDamage::Full);
        Ok(())
    }

    pub fn destroy_surface(&mut self, id: SurfaceId) -> Result<(), String> {
        if let Some(mut instance) = self.surfaces.remove(&id) {
            self.slides.cancel(id.get());
            self.windows.remove(&instance.app.window);
            drop_popup_grab(
                self.display,
                self.root_window(),
                &mut self.press_selection,
                id,
                &mut instance.app,
            );
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
        drop_popup_grab(
            display,
            root,
            &mut self.press_selection,
            id,
            &mut instance.app,
        );
        unsafe {
            XUnmapWindow(display, instance.app.window);
            instance.app.present_scene().map_err(mark_x_error)?;
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
}
