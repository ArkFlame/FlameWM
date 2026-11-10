use super::*;

impl SurfaceController {
    /// Releases this popup's pointer grab and restores focus. Idempotent.
    pub(super) fn release_grab(&mut self, id: SurfaceId) -> Result<(), String> {
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
        Ok(())
    }

    pub fn hide(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let now = Instant::now();
        let key = id.get();
        let (slide, shown) = {
            let instance = self.instance_mut(id)?;
            (instance.slide, instance.shown)
        };
        // A slide-out keeps the grab so an outside press during the close is still captured;
        // advance_motion releases it when the slide lands.
        if !slide_out_keeps_grab(slide, shown) {
            self.release_grab(id)?;
        }
        if let Some(config) = slide.filter(|config| config.transition != PopupTransition::Slide)
            && self.begin_overlay_hide(id, config, shown, now)?
        {
            unsafe { XFlush(display) };
            return Ok(());
        }
        let slide = slide.filter(|config| config.transition == PopupTransition::Slide);
        if let (Some(config), true) = (slide, shown) {
            // Already closing: keep the running transition.
            if self.slides.kind(key).is_some_and(|active| !active.opening) {
                unsafe { XFlush(display) };
                return Ok(());
            }
            self.slides.set_speed(config.speed);
            let closing = SlideKind {
                edge: config.edge,
                opening: false,
            };
            // Do not raise a closing popup over the taskbar. Its existing Z-order is
            // already sufficient, and the bounding shape clips anything under the dock.
            // The first close frame is clipped too, not only frames after the first tick.
            let dock = self.dock_rect();
            let instance = self.instance_mut(id)?;
            let origin = instance.slide_drawn.unwrap_or(instance.origin);
            let clip = panel_clip(
                dock,
                origin,
                (instance.app.width, instance.app.height),
                config.edge,
            );
            unsafe {
                instance
                    .app
                    .refresh_shape_mask_clipped(&instance.document, clip)
            };
            // A new close starts fully shown; a reversed open continues from its progress.
            if self.slides.start_towards(
                key,
                closing,
                now,
                SLIDE_OPEN_BASE_MS,
                Easing::EaseInCubic,
                (1.0, 0.0),
            ) == Start::Started
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
        self.forget_live_composition();
        Ok(())
    }

    /// Paints pending popup content into its backbuffer and captures it without mapping the window.
    pub(super) fn capture_popup(&mut self, id: SurfaceId) -> Result<CapturedImage, String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (width, height) = (instance.app.width, instance.app.height);
        // SAFETY: the app owns its backbuffer and document; both are live for this call.
        unsafe {
            let painted = instance
                .app
                .redraw_if_dirty(&mut instance.document)
                .map_err(mark_x_error)?;
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
        let taskbars = self.taskbar_windows();
        self.with_window(id, |display, window| {
            raise_above_taskbars(display, window, &taskbars)
        })
    }

    /// Windows of every taskbar (Dock role) this controller owns.
    pub(super) fn taskbar_windows(&self) -> Vec<Window> {
        self.surfaces
            .values()
            .filter(|instance| instance.role == SurfaceRole::Dock)
            .map(|instance| instance.app.window)
            .collect()
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
        self.apply_icon_geometry(window, Some(rect.map(u64::from)));
        Ok(())
    }

    /// Deletes `_NET_WM_ICON_GEOMETRY` from a client window.
    pub fn clear_window_icon_geometry(&mut self, window: u64) -> Result<(), String> {
        self.apply_icon_geometry(window, None);
        Ok(())
    }

    pub(super) fn apply_icon_geometry(&mut self, window: u64, rect: Option<[u64; 4]>) {
        let property = self.icon_geometry_atom();
        crate::app::apply_window_icon_geometry(self.display, window as Window, property, rect);
        unsafe { XFlush(self.display) };
    }

    pub(super) fn icon_geometry_atom(&mut self) -> Atom {
        if self.icon_geometry_atom == 0 {
            let name = std::ffi::CString::new("_NET_WM_ICON_GEOMETRY")
                .expect("static atom contains no NUL");
            self.icon_geometry_atom = unsafe { XInternAtom(self.display, name.as_ptr(), 0) };
        }
        self.icon_geometry_atom
    }

    pub fn grab_pointer(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if !instance.input.allows_grab() {
            return Err(format!(
                "surface {} is pass-through; pointer grab refused",
                id.0
            ));
        }
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
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Subscribes to root child map/unmap/configure/destroy (SubstructureNotify is shared, so the
    /// window manager's redirect is unaffected). Idempotent.
    pub fn watch_root_changes(&self) {
        // SAFETY: the display is live; the root window is the screen's root.
        unsafe {
            XSelectInput(
                self.display,
                XRootWindow(self.display, self.screen),
                ROOT_SUBSTRUCTURE_NOTIFY_MASK | ROOT_STRUCTURE_NOTIFY_MASK,
            );
        }
    }

    /// Whether root children changed since the last call, then clears the flag.
    pub fn take_root_changed(&mut self) -> bool {
        std::mem::take(&mut self.root_changed)
    }

    pub fn take_root_geometry_changed(&mut self) -> Option<(u32, u32)> {
        std::mem::take(&mut self.root_geometry_changed)
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
        let instance = self.instance_ref(id)?;
        Ok(instance
            .app
            .pending_damage
            .merge(instance.document.peek_damage()))
    }
}
