use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    /// Captures the root once (24-bit LSB only; otherwise the wallpaper is used) and freezes
    /// the backdrop for the dragged frame.
    pub(super) fn begin_drag_backdrop(&self, client: Window) {
        let Some((frame, outer)) = self.clients.get(&client).map(|state| (state.frame, state.outer))
        else {
            return;
        };
        let screen = self.screen();
        let root = (
            u32::from(screen.width_in_pixels),
            u32::from(screen.height_in_pixels),
        );
        let raw_root = (screen.root_depth == 24
            && self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST)
            .then(|| {
                self.conn
                    .get_image(ImageFormat::Z_PIXMAP,
                        screen.root,
                        0,
                        0,
                        screen.width_in_pixels,
                        screen.height_in_pixels,
                        u32::MAX,
                    )
                    .ok()?
                    .reply()
                    .ok()
            })
            .flatten()
            .filter(|reply| reply.depth == 24)
            .map(|reply| reply.data);
        crate::frame::titlebar_backdrop::begin_drag(
            u64::from(frame),
            root,
            raw_root,
            (outer.x, outer.y, outer.width, outer.height),
        );
    }

    /// Apply live chrome tokens from the settings sync point. A change
    /// repaints every frame and reapplies its shape (radius/glass).
    pub(crate) fn apply_decoration_tokens(
        &mut self,
        tokens: flamewm_skin::recipes::decoration::ChromeTokens,
    ) -> Result<(), ReplyError> {
        if tokens == self.decoration_tokens {
            return Ok(());
        }
        self.decoration_tokens = tokens;
        let clients: Vec<Window> = self.clients.keys().copied().collect();
        for client in clients {
            let Some((frame, outer, floating)) = self
                .clients
                .get(&client)
                .map(|state| (state.frame, state.outer, state.is_floating()))
            else {
                continue;
            };
            self.clear_frame_shape(frame, outer, floating)?;
            self.paint_chrome(client)?;
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.borrow().flush();
        }
        Ok(())
    }

    pub(super) fn paint_chrome(&mut self, client: Window) -> Result<(), ReplyError> {
        if self
            .clients
            .get(&client)
            .is_some_and(|state| state.decoration != Decoration::Server)
        {
            return Ok(());
        }
        // Single chrome paint path: delegate scene build to chrome_runtime
        // (J08 paint_plan seam); this owner only bridges cached rasters.
        let Some((chrome_state, identity, icon, frame)) = self.clients.get(&client).map(|state| {
            (
                chrome_runtime::ChromeState {
                    frame_w: state.outer.width.max(1),
                    frame_h: state.outer.height.max(1),
                    active: self.active.is_none_or(|current| current == client),
                    hover: state.hover_control,
                    pressed: self.pressed_control(client),
                    maximized: state.is_maximized(),
                    fullscreen: state.is_fullscreen(),
                    floating: state.is_floating(),
                    tokens: self.decoration_tokens,
                },
                chrome_runtime::ChromeIdentity {
                    title: state.title.clone(),
                    wm_instance: state.wm_instance.clone(),
                    wm_class: state.wm_class.clone(),
                },
                state.icon.clone(),
                state.frame,
            )
        }) else {
            return Ok(());
        };
        let runtime = self.chrome_runtimes.entry(client).or_default();
        runtime.scene_for(&identity, chrome_state, icon);
        // The renderer reads the frame origin on its own Xlib connection; sync so the
        // queued MoveFrameRoot on self.conn is applied before the backdrop samples it.
        self.conn.sync()?;
        if let Some(renderer) = self.renderer.as_ref() {
            let mut renderer = renderer.borrow_mut();
            let _ = runtime.paint_cached(&mut renderer, u64::from(frame));
        }
        Ok(())
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
        for placement in &lifecycle::input_child_placements(
            state.outer.width.max(1) as i32,
            state.outer.height.max(1) as i32,
            self.extents_for(client).titlebar_h,
            ids,
        ) {
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
                format!(
                    "effect=LayoutInput rect=({},{},{},{})",
                    placement.x, placement.y, placement.w, placement.h
                ),
                &result,
            );
            result?;
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
