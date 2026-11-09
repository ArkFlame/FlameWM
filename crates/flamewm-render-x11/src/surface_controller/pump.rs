use super::*;

impl SurfaceController {
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
                && Some(unsafe { generic_cookie(&event) }.extension)
                    == self.press_selection.opcode()
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
                if is_root_geometry_configure(kind, unsafe { event.xconfigure.window }, root) {
                    self.root_geometry_changed = Some((
                        unsafe { event.xconfigure.width }.max(0) as u32,
                        unsafe { event.xconfigure.height }.max(0) as u32,
                    ));
                }
                let child = unsafe { event.xmap.window };
                self.root_changed |= matches!(
                    kind,
                    ROOT_CHILD_MAP_NOTIFY
                        | ROOT_CHILD_UNMAP_NOTIFY
                        | ROOT_CHILD_CONFIGURE_NOTIFY
                        | ROOT_CHILD_DESTROY_NOTIFY
                ) && !self.windows.contains_key(&child);
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
                        .map_err(mark_x_error)?;
                }
                // Drain the document-owned damage latch into the app latch so
                // `redraw_dirty` can coalesce; event_loop already merged its
                // own marks via redraw_if_dirty. A document-side Full only means an untracked
                // edit: the revision check repaints it and the command diff bounds it. Forwarding
                // it would latch an explicit Full and disable the region path for every edit.
                // Explicit Full requests reach the app through `mark_surface_dirty` itself.
                let pending = match instance.document.take_damage() {
                    flamewm_render_core::SurfaceDamage::Full => {
                        flamewm_render_core::SurfaceDamage::None
                    }
                    other => other,
                };
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

    /// Runs `f` with the surface's document while layout widths come from that surface's Xft
    /// backend, so layout measures exactly what the surface paints. Loads Xft on first use.
    pub fn with_document_text_measure<R>(
        &mut self,
        id: SurfaceId,
        f: impl FnOnce(&mut RuntimeDocument) -> R,
    ) -> Result<R, String> {
        let instance = self.instance_mut(id)?;
        instance.app.ensure_xft();
        let SurfaceInstance { app, document, .. } = instance;
        Ok(super::xft::scoped_text_measure(app.xft.as_mut(), || {
            f(document)
        }))
    }

    /// Native root-space origin of a surface; (0, 0) when the surface is unknown.
    pub(super) fn origin_of(&self, id: SurfaceId) -> (i32, i32) {
        self.surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0))
    }

    pub(super) fn retained(&self, id: SurfaceId) -> Result<(&X11App, &RuntimeDocument), String> {
        let instance = self.instance_ref(id)?;
        Ok((&instance.app, &instance.document))
    }

    /// Retained-live surface extent in root space: native origin + the
    /// retained window size. Reads retained state only; no LayoutEngine
    /// recompute, no headless fallback.
    pub fn surface_device_rect(&self, id: SurfaceId) -> Result<Rect, String> {
        let (app, _) = self.retained(id)?;
        let origin = self.origin_of(id);
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
        let origin = self.origin_of(id);
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
        let (_, document) = self.retained(id)?;
        let index = document
            .node_by_id(node)
            .ok_or_else(|| format!("no node with id '{node}'"))?;
        self.node_device_rect(id, index)
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
        Ok(self.instance_ref(id)?.role)
    }

    /// Overlay management policy: overlays stay raisable but are never
    /// WM-managed, never focusable, and never in the workarea/taskbar.
    pub fn is_wm_managed(&self, id: SurfaceId) -> Result<bool, String> {
        Ok(!overlay_excluded_from_wm(self.surface_role(id)?))
    }

    pub fn is_pointer_grabbed(&self, id: SurfaceId) -> Result<bool, String> {
        Ok(self.instance_ref(id)?.app.pointer_grabbed)
    }
}
