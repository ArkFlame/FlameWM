use super::*;

impl<'a, C: Connection> Wm<'a, C> {
    pub(super) fn scan_existing(&mut self) -> Result<(), ReplyOrIdError> {
        let root = self.screen().root;
        let tree = self.conn.query_tree(root)?.reply()?;
        let mut pending = Vec::with_capacity(tree.children.len());
        for window in tree.children {
            if window == self.support_window {
                continue;
            }
            pending.push((
                window,
                self.conn.get_window_attributes(window)?,
                self.conn.get_geometry(window)?,
            ));
        }
        for (window, attrs, geometry) in pending {
            let Ok(attrs) = attrs.reply() else { continue };
            let Ok(geometry) = geometry.reply() else {
                continue;
            };
            if attrs.map_state != MapState::UNMAPPED {
                let kind = self.window_kind(window, attrs.override_redirect)?;
                match kind {
                    WindowKind::Popup => {
                        self.conn.map_window(window)?;
                    }
                    WindowKind::Desktop => self.map_unframed(window, StackMode::BELOW)?,
                    WindowKind::Dock => self.map_dock(window)?,
                    WindowKind::Normal | WindowKind::Utility => {
                        self.manage(window, &geometry, true, kind)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn window_kind(
        &self,
        window: Window,
        override_redirect: bool,
    ) -> Result<WindowKind, ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                window,
                self.atoms.net_wm_window_type,
                AtomEnum::ATOM,
                0,
                16,
            )?
            .reply()?;
        let types = reply.value32().map_or_else(Vec::new, Iterator::collect);
        Ok(classify(&self.atoms, &types, override_redirect))
    }

    /// Keeps every dock above a normal client that was just raised to the top of the stack.
    /// Fullscreen clients keep their existing stacking, so they are not raised over docks here.
    pub(super) fn raise_docks_above_normal(
        &self,
        client: Window,
        frame: Window,
    ) -> Result<(), ReplyError> {
        let fullscreen = match self.clients.get(&client) {
            Some(state) => state.is_fullscreen(),
            None => return Ok(()),
        };
        if fullscreen {
            return Ok(());
        }
        for dock in self.dock_struts.keys() {
            self.conn
                .configure_window(*dock, &dock_above_frame_aux(frame))?;
        }
        Ok(())
    }

    pub(super) fn map_unframed(
        &self,
        window: Window,
        stack_mode: StackMode,
    ) -> Result<(), ReplyError> {
        self.conn.map_window(window)?;
        self.conn
            .configure_window(window, &ConfigureWindowAux::new().stack_mode(stack_mode))?;
        Ok(())
    }

    pub(super) fn map_dock(&mut self, window: Window) -> Result<(), ReplyOrIdError> {
        self.dock_struts.insert(window, self.read_strut(window)?);
        self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        self.map_unframed(window, StackMode::ABOVE)?;
        self.changes.panels = true;
        self.mark_work_area();
        self.reconcile_work_area()?;
        self.publish_workspace_metadata()?;
        Ok(())
    }

    pub(super) fn read_strut(&self, window: Window) -> Result<[u32; 12], ReplyError> {
        let partial = self
            .conn
            .get_property(
                false,
                window,
                self.atoms.net_wm_strut_partial,
                AtomEnum::CARDINAL,
                0,
                12,
            )?
            .reply()?;
        if let Some(mut values) = partial.value32() {
            let mut result = [0; 12];
            for value in &mut result {
                *value = values.next().unwrap_or(0);
            }
            if result != [0; 12] {
                return Ok(result);
            }
        }
        let legacy = self
            .conn
            .get_property(
                false,
                window,
                self.atoms.net_wm_strut,
                AtomEnum::CARDINAL,
                0,
                4,
            )?
            .reply()?;
        let mut result = [0; 12];
        if let Some(mut values) = legacy.value32() {
            for value in result.iter_mut().take(4) {
                *value = values.next().unwrap_or(0);
            }
        }
        let (h, w) = (
            self.screen_rect.height.saturating_sub(1),
            self.screen_rect.width.saturating_sub(1),
        );
        result[4..].copy_from_slice(&[0, h, 0, h, 0, w, 0, w]);
        Ok(result)
    }

    pub(super) fn manage(
        &mut self,
        window: Window,
        geometry: &GetGeometryReply,
        adopt: bool,
        kind: WindowKind,
    ) -> Result<(), ReplyOrIdError> {
        if self.clients.contains_key(&window) {
            self.conn.map_window(window)?;
            return Ok(());
        }
        if kind == WindowKind::Popup {
            return Ok(());
        }
        if kind == WindowKind::Desktop {
            self.map_unframed(window, StackMode::BELOW)?;
            return Ok(());
        }
        if kind == WindowKind::Dock {
            self.map_dock(window)?;
            return Ok(());
        }

        // Initial-map order: read title/class/hints/icon/transient/type first.
        let title = self
            .read_title(window)
            .unwrap_or_else(|_| "Application".to_owned());
        let (wm_instance, wm_class) = self.wm_identity(window);
        let (icon, icon_fallback) = self.read_native_frame_icon(window);
        let transient_for = self.read_transient_for(window)?;
        let hints = self.read_hints(window);
        let workspace = transient_for
            .and_then(|owner| self.clients.get(&owner).map(|state| state.workspace))
            .unwrap_or(self.current_workspace);

        let screen = self.screen();
        let frame = self.conn.generate_id()?;
        let decoration = self.read_decoration(window, kind);
        let extents = self.extents_for_decoration(decoration);
        let work_root = self.work_root();
        let output_rects = self.output_root_rects();
        let start = crate::client::placement_floating(RootRect::new(0, 0, 1, 1));
        let initial_req = GeometryRequest {
            reason: GeometryReason::InitialMap,
            start: PlacementSnapshot {
                mode: start.mode,
                rect: start.current,
            },
            pointer_delta: (0, 0),
            edges: crate::frame::model::ResizeEdges::none(),
            work_area: work_root,
            outputs: &output_rects,
            hints,
            frame_extents: extents,
        };
        let origin = RootPoint::new(i32::from(geometry.x), i32::from(geometry.y));
        let size = (
            u32::from(geometry.width).max(1),
            u32::from(geometry.height).max(1),
        );
        let frame_rect = if adopt {
            crate::frame::geometry::plan_adopt(origin, size, extents)
        } else if crate::frame::geometry::can_place_near_pointer(
            adopt,
            kind == WindowKind::Normal,
            transient_for.is_some(),
            hints,
            origin,
        ) {
            // One best-effort pointer snapshot per eligible first map; never poll or
            // let a transient X11 failure prevent mapping the application.
            let pointer = self
                .conn
                .query_pointer(screen.root)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .filter(|reply| reply.same_screen);
            if let Some(pointer) = pointer {
                let point = RootPoint::new(i32::from(pointer.root_x), i32::from(pointer.root_y));
                let work_area = self
                    .output_for_point(point.x, point.y, Rect::new(point.x, point.y, 1, 1))
                    .map_or(work_root, |output| RootRect::from_legacy(output.work_area));
                crate::frame::geometry::plan_initial_map_near_pointer(
                    &initial_req,
                    point,
                    work_area,
                    size,
                )
                .frame
            } else {
                crate::frame::geometry::plan_initial_map(&initial_req, origin, size).frame
            }
        } else {
            crate::frame::geometry::plan_initial_map(&initial_req, origin, size).frame
        };
        let outer = crate::client::root_to_rect(frame_rect);

        // Create frame UNMAPPED with panel bg #1b1e20 (not black).
        self.conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            frame,
            screen.root,
            clamp_i16(outer.x),
            clamp_i16(outer.y),
            clamp_u16(outer.width),
            clamp_u16(outer.height),
            self.config.frame_border,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(0x1b1e20)
                .border_pixel(0x1b1e20)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::SUBSTRUCTURE_NOTIFY
                        | EventMask::SUBSTRUCTURE_REDIRECT,
                ),
        )?;

        // Grab-server only for save-set/reparent/structural-configure.
        // Reparent offset + client configure from the lifecycle contract
        // (single geometry source); this owner performs the X calls.
        self.conn.grab_server()?;
        self.conn.change_save_set(SetMode::INSERT, window)?;
        let (reparent_x, reparent_y) = lifecycle::reparent_offset(extents);
        self.conn
            .reparent_window(window, frame, reparent_x as i16, reparent_y as i16)?;
        self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
        )?;
        // Click-to-focus: a sync passive grab sees presses inside the client area;
        // handle_client_press focuses and then replays the press to the client.
        self.conn.grab_button(
            false,
            window,
            EventMask::BUTTON_PRESS,
            GrabMode::SYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            x11rb::NONE,
            ButtonIndex::ANY,
            ModMask::ANY,
        )?;
        let (client_x, client_y, client_w, client_h) =
            lifecycle::client_configure(crate::client::rect_to_root(outer), extents);
        self.conn.configure_window(
            window,
            &ConfigureWindowAux::new()
                .x(client_x)
                .y(client_y)
                .width(client_w)
                .height(client_h)
                .border_width(0),
        )?;

        // Create real 12 InputOnly children (8 resize + title-drag + 3 controls).
        let mut child_ids = [0; 12];
        for id in &mut child_ids {
            *id = self.conn.generate_id()?;
        }
        // Single lifecycle transaction: ids + placements from lifecycle
        // policy; this owner only performs the X create/configure loop.
        let lifecycle_ids = lifecycle::LifecycleIds::new(window, frame, child_ids);
        let resources = lifecycle_ids.resources();
        let placements = lifecycle::input_child_placements(
            outer.width.max(1) as i32,
            outer.height.max(1) as i32,
            extents.titlebar_h,
            lifecycle_ids,
        );
        for placement in &placements {
            self.conn.create_window(
                COPY_DEPTH_FROM_PARENT,
                placement.xid,
                frame,
                placement.x as i16,
                placement.y as i16,
                placement.w.max(1) as u16,
                placement.h.max(1) as u16,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::from(INPUT_CHILD_EVENT_MASK)),
            )?;
        }
        self.conn.ungrab_server()?;
        self.conn.flush()?;

        // Insert FrameResources + ManagedClient.
        let _ = self.registry.register(resources);
        let title_width = flamewm_render_x11::external_text_measure(&title, 12.0)
            .0
            .round() as i32;
        let client = ManagedClient {
            client: window,
            frame,
            placement: crate::client::placement_floating(crate::client::rect_to_root(outer)),
            hints,
            outer,
            workspace,
            title,
            title_text_width: title_width,
            icon,
            icon_fallback,
            icon_request_token: self.next_client_lifetime,
            catalog_icon_key: None,
            wm_instance,
            wm_class,
            transient_for,
            minimized: false,
            sticky: false,
            ignore_unmap: 1,
            kind,
            hover_control: None,
            decoration,
        };
        self.frame_to_client.insert(frame, window);
        for xid in resources.children() {
            self.frame_to_client.insert(xid, window);
        }
        self.clients.insert(window, client);
        self.next_client_lifetime = self.next_client_lifetime.saturating_add(1);
        self.chrome_runtimes
            .insert(window, chrome_runtime::ChromeRuntime::new());
        if self
            .clients
            .get(&window)
            .is_some_and(|client| client.icon.is_none())
        {
            self.request_catalog_icon(window);
        }
        self.managed_order.push(window);
        let controller = self.controller_for(window).unwrap_or_else(|| {
            ControllerState::new(
                window,
                self.screen().root,
                crate::client::placement_floating(crate::client::rect_to_root(outer)),
                self.work_root(),
                hints,
                extents,
            )
            .with_screen_rect(crate::client::rect_to_root(Rect::new(
                self.screen_rect.x,
                self.screen_rect.y,
                self.screen_rect.width,
                self.screen_rect.height,
            )))
        });
        self.sessions.insert(window, controller);

        self.set_frame_extents(window)?;
        self.set_client_workspace(window, self.current_workspace)?;
        self.set_wm_state(window, WM_STATE_NORMAL)?;

        // Retarget + paint chrome + shape + cursors.
        self.paint_chrome(window)?;
        let _ = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.borrow().flush());
        let floating = self
            .clients
            .get(&window)
            .is_some_and(|state| state.is_floating());
        self.clear_frame_shape(frame, outer, floating)?;
        self.bind_child_cursors(window);

        // Map client + input children + frame (renderer flush above).
        self.conn.map_window(window)?;
        if decoration == Decoration::Server {
            for xid in resources.children() {
                self.conn.map_window(xid)?;
            }
        }
        self.conn.map_window(frame)?;
        self.mark_windows();
        self.publish_client_list()?;
        self.focus(window)?;
        self.raise_docks_above_normal(window, frame)?;
        // Window-open effect: only a new, non-adopted normal top-level. It starts on a later
        // Expose, so arming never delays the map or the focus above.
        if !adopt && kind == WindowKind::Normal && self.animator.wants(Effect::Open) {
            self.open_gate.arm(window);
        }
        Ok(())
    }

    pub(super) fn unmanage(
        &mut self,
        window: Window,
        restore_to_root: bool,
    ) -> Result<(), ReplyError> {
        self.snap_preview.hide();
        if let Some(frame) = self.clients.get(&window).map(|state| state.frame) {
            crate::frame::titlebar_backdrop::forget(u64::from(frame));
        }
        // The client is leaving: drop its flight and overlay without applying an end state. A
        // running close keeps its afterimage, which finishes on its own timeline.
        if !animation::survives_unmanage(self.animator.running(window)) {
            self.animator.cancel(window);
            self.overlays.remove(&window);
        }
        self.linger.remove(window);
        self.release_damage_watch(window);
        self.minimized_captures.remove(window);
        self.open_gate.clear(window);
        // Abort any in-progress interaction through the reducer so session
        // teardown, ungrab, and snap-preview hide stay in one owner. Runs
        // before `clients.remove` because `controller_for` reads it.
        if let Some(controller) = self.controller_for(window) {
            if !controller.session.is_idle() {
                let (next, commit) = interaction::step(&controller, FrameEvent::Cancel);
                self.store_controller(window, &next);
                self.execute_effects(window, &commit)?;
            }
        }
        let Some(client) = self.clients.remove(&window) else {
            return Ok(());
        };
        self.deferred_icon_jobs.remove(&window);
        self.frame_extents_published.remove(&window);
        for requesters in self.icon_requests.values_mut() {
            requesters.retain(|(id, _)| *id != window);
        }
        self.icon_requests
            .retain(|_, requesters| !requesters.is_empty());
        self.managed_order.retain(|managed| *managed != window);
        self.frame_to_client.remove(&client.frame);
        self.chrome_runtimes.remove(&window);
        let removed = self.registry.remove_client(window);
        if let Some(renderer) = self.renderer.as_ref() {
            let mut renderer = renderer.borrow_mut();
            for xid in removed.iter().copied().chain(std::iter::once(client.frame)) {
                renderer.forget_cursor(u64::from(xid));
            }
        }
        for xid in &removed {
            let _ = self.conn.destroy_window(*xid);
            self.frame_to_client.remove(xid);
        }
        self.release_pointer();
        self.sessions.remove(&window);
        if self.active == Some(window) {
            self.active = None;
        }
        if restore_to_root {
            let _ = self.conn.grab_server();
            let _ = self.conn.change_save_set(SetMode::DELETE, window);
            let _ = self.conn.reparent_window(
                window,
                self.screen().root,
                clamp_i16(client.outer.x),
                clamp_i16(client.outer.y),
            );
            let _ = self.conn.ungrab_server();
        }
        let _ = self.conn.destroy_window(client.frame);
        let _ = self.set_wm_state(window, WM_STATE_WITHDRAWN);
        self.mark_windows();
        self.publish_client_list()?;
        self.publish_active()?;
        Ok(())
    }
}
