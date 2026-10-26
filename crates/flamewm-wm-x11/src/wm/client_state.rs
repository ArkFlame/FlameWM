use super::*;

impl<C: Connection> Wm<'_, C> {
    pub(super) fn set_wm_state(&self, client: Window, state: u32) -> Result<(), ReplyError> {
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.wm_state,
            self.atoms.wm_state,
            &[state, 0],
        )?;
        Ok(())
    }

    pub(super) fn publish_window_state(&self, client: Window) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        let mut values = Vec::with_capacity(3);
        if state.minimized {
            values.push(self.atoms.net_wm_state_hidden);
        }
        if state.is_maximized() {
            values.push(self.atoms.net_wm_state_maximized_vert);
            values.push(self.atoms.net_wm_state_maximized_horz);
        }
        if state.is_fullscreen() {
            values.push(self.atoms.net_wm_state_fullscreen);
        }
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_wm_state,
            AtomEnum::ATOM,
            &values,
        )?;
        Ok(())
    }

    /// `_NET_WM_ALLOWED_ACTIONS`: the EWMH actions this WM honours. GTK (Firefox) reads it
    /// before offering maximize/resize affordances; it was never published, so those were hidden.
    pub(super) fn publish_allowed_actions(&self, client: Window) -> Result<(), ReplyError> {
        let values = [
            self.atoms.net_wm_action_move,
            self.atoms.net_wm_action_resize,
            self.atoms.net_wm_action_minimize,
            self.atoms.net_wm_action_maximize_horz,
            self.atoms.net_wm_action_maximize_vert,
            self.atoms.net_wm_action_fullscreen,
            self.atoms.net_wm_action_close,
            self.atoms.net_wm_action_change_desktop,
        ];
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_wm_allowed_actions,
            AtomEnum::ATOM,
            &values,
        )?;
        Ok(())
    }

    /// `_NET_FRAME_EXTENTS` (EWMH: left, right, top, bottom). Client-drawn
    /// frames publish zeros: Flame adds no frame, so the window is its own
    /// visible area and GTK's `_GTK_FRAME_EXTENTS` stays client-owned.
    /// Effective X border for `client`'s frame: the configured border only
    /// while floating, zero for snapped/maximized/fullscreen frames.
    pub(super) fn effective_frame_border(&self, client: Window) -> u16 {
        let configured = self.config.frame_border;
        self.clients.get(&client).map_or(configured, |state| {
            frame_border_for_mode(state.placement.mode, configured)
        })
    }

    pub(super) fn set_frame_extents(&mut self, client: Window) -> Result<(), ReplyError> {
        let mode = self
            .clients
            .get(&client)
            .map_or(PlacementMode::Floating, |state| state.placement.mode);
        let extents = match self.clients.get(&client).map(|state| state.decoration) {
            None | Some(Decoration::Server) => {
                frame_extents_for_mode(mode, self.config.frame_border, self.titlebar_height())
            }
            Some(Decoration::ClientSide | Decoration::None) => [0; 4],
        };
        if !frame_extents_needs_publish(&self.frame_extents_published, client, extents) {
            return Ok(());
        }
        self.conn.change_property32(
            PropMode::REPLACE,
            client,
            self.atoms.net_frame_extents,
            AtomEnum::CARDINAL,
            &extents,
        )?;
        self.frame_extents_published.insert(client, extents);
        Ok(())
    }

    pub(super) fn send_configure_notify(&self, client: Window) -> Result<(), ReplyError> {
        configure_bump!(synthetic, synthetic_total);
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        // Synthetic contract owned by configure policy: current client-root
        // geometry; the frame is never moved by a notify.
        let notify = configure::synthetic_notify_for(
            state.outer.x,
            state.outer.y,
            state.outer.width.max(1) as i32,
            state.outer.height.max(1) as i32,
            self.extents(),
        );
        self.send_notify_event(
            client,
            clamp_i16(notify.x),
            clamp_i16(notify.y),
            clamp_u16(notify.w),
            clamp_u16(notify.h),
        )
    }

    pub(super) fn send_notify_event(
        &self,
        client: Window,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    ) -> Result<(), ReplyError> {
        let event = ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: client,
            window: client,
            above_sibling: 0,
            x,
            y,
            width,
            height,
            border_width: 0,
            override_redirect: false,
        };
        self.conn
            .send_event(false, client, EventMask::STRUCTURE_NOTIFY, event)?;
        Ok(())
    }

    pub(super) fn clear_frame_shape(
        &mut self,
        frame: Window,
        outer: crate::geometry::Rect,
        floating: bool,
    ) -> Result<(), ReplyError> {
        use x11rb::protocol::shape::{SK as ShapeSk, SO as ShapeOp};
        // Every shape writer (ApplyShape, manage, decoration change) comes through
        // here, so this one check covers all emitters. Only the state of the
        // ManagedClient owning `frame` is consulted and updated.
        let key = crate::client::ShapeKey {
            frame,
            width: outer.width,
            height: outer.height,
            floating,
            tokens: self.decoration_tokens,
        };
        let owner = self.clients.values().find(|c| c.frame == frame);
        if owner.is_some_and(|c| c.shaped == Some(key)) {
            return Ok(());
        }
        let owner_client = owner.map(|c| c.client);
        let radius = u32::from(crate::frame::geometry_shape::scaled_corner_radius(
            floating,
            self.decoration_tokens.style,
            self.titlebar_height(),
        ));
        let rects = crate::frame::geometry_shape::bounding_rectangles_with_titlebar(
            outer,
            floating,
            self.decoration_tokens.style,
            self.titlebar_height(),
        );
        self.conn.shape_rectangles(
            ShapeOp::SET,
            ShapeSk::BOUNDING,
            ClipOrdering::UNSORTED,
            frame,
            0,
            0,
            &rects,
        )?;
        // The child client is not clipped by the frame's bounding shape alone;
        // set the clip region to the same rounded spans so client pixels cannot
        // fill the transparent corner notches (and hide the shadow there).
        self.conn.shape_rectangles(
            ShapeOp::SET,
            ShapeSk::CLIP,
            ClipOrdering::UNSORTED,
            frame,
            0,
            0,
            &rects,
        )?;
        // The client is a child window with its own square pixels. Intersect
        // its shape with the frame's bottom-corner arcs so the rounded corner
        // and the shadow in the notch are actually visible. The client shape is
        // reset to the full client rect first, so a grown frame is not left with
        // the smaller shape from the previous mask; any app-set client shape is
        // replaced by this reset.
        let Some(client) = owner_client else {
            return Ok(());
        };
        let cw = outer.width.saturating_sub(2);
        let ch = outer
            .height
            .saturating_sub(u32::from(self.titlebar_height()) + 1);
        // Reset to the full client rect first: intersecting with the previous mask can only
        // shrink, so a grown frame would keep the old, smaller client shape. Also used to
        // restore an unshaped client below.
        let full = [x11rb::protocol::xproto::Rectangle {
            x: 0,
            y: 0,
            width: cw.min(u32::from(u16::MAX)) as u16,
            height: ch.min(u32::from(u16::MAX)) as u16,
        }];
        if radius > 0 && cw > 0 && ch > 0 {
            let mask =
                crate::frame::geometry_shape::client_bottom_rounded_rectangles(cw, ch, radius);
            self.conn.shape_rectangles(
                ShapeOp::SET,
                ShapeSk::BOUNDING,
                ClipOrdering::UNSORTED,
                client,
                0,
                0,
                &full,
            )?;
            self.conn.shape_rectangles(
                ShapeOp::INTERSECT,
                ShapeSk::BOUNDING,
                ClipOrdering::UNSORTED,
                client,
                0,
                0,
                &mask,
            )?;
            self.rounded_clients.insert(client);
        } else if self.rounded_clients.remove(&client) {
            // Restore the unshaped client when the decoration turns square.
            self.conn.shape_rectangles(
                ShapeOp::SET,
                ShapeSk::BOUNDING,
                ClipOrdering::UNSORTED,
                client,
                0,
                0,
                &full,
            )?;
        }
        if let Some(state) = self.clients.values_mut().find(|c| c.frame == frame) {
            state.shaped = Some(key);
        }
        Ok(())
    }

    /// Native EWMH icon only. A missing/invalid native property leaves an
    /// empty slot while the catalog fallback is resolved asynchronously.
    pub(super) fn read_native_frame_icon(
        &self,
        client: Window,
    ) -> (Option<chrome::IconImage>, Option<String>) {
        match self.read_net_wm_icon(client) {
            Ok(Some(icon)) => (Some(icon), None),
            Ok(None) => (None, Some("native-icon-absent".to_owned())),
            Err(reason) => (None, Some(reason.to_owned())),
        }
    }

    pub(super) fn read_net_wm_icon(
        &self,
        client: Window,
    ) -> Result<Option<chrome::IconImage>, &'static str> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.net_wm_icon,
                AtomEnum::CARDINAL,
                0,
                u32::MAX,
            )
            .map_err(|_| "get-property-failed")?
            .reply()
            .map_err(|_| "get-property-reply-failed")?;
        let cardinals = reply.value32().map_or_else(Vec::new, Iterator::collect);
        if cardinals.is_empty() {
            return Ok(None);
        }
        chrome::parse_net_wm_icon(&cardinals)
            .map(Some)
            .ok_or("invalid-payload")
    }

    /// Parsed `WM_CLASS` identity (instance, class) for one client. The raw
    /// property holds two NUL-separated Latin-1 strings: instance first,
    /// class second. Cached on the owner at manage time; paint never reads
    /// the property.
    pub(super) fn wm_identity(&self, client: Window) -> (String, String) {
        let raw = self
            .conn
            .get_property(false, client, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.value)
            .unwrap_or_default();
        parse_wm_class(&raw)
    }

    pub(super) fn read_title(&self, client: Window) -> Result<String, ReplyError> {
        let mut value = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.net_wm_name,
                self.atoms.utf8_string,
                0,
                1024,
            )?
            .reply()?
            .value;
        if value.is_empty() {
            value = self
                .conn
                .get_property(false, client, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?
                .reply()?
                .value;
        }
        Ok(String::from_utf8_lossy(&value)
            .trim_matches('\0')
            .to_owned())
    }

    pub(super) fn read_transient_for(&self, client: Window) -> Result<Option<Window>, ReplyError> {
        let reply = self
            .conn
            .get_property(
                false,
                client,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                0,
                1,
            )?
            .reply()?;
        Ok(reply
            .value32()
            .and_then(|mut values| values.next())
            .filter(|window| *window != 0))
    }

    pub(super) fn client_for(&self, window: Window) -> Option<Window> {
        if self.clients.contains_key(&window) {
            Some(window)
        } else if let Some(client) = self.frame_to_client.get(&window).copied() {
            Some(client)
        } else {
            self.registry
                .lookup_by_xid(window)
                .map(|target| target.client)
        }
    }

    pub(super) fn live_pointer_grab_target(&self, requested: Window) -> Option<Window> {
        let client = self.frame_to_client.get(&requested).copied()?;
        let state = self.clients.get(&client)?;
        let resources = self.registry.lookup_by_client(client)?;
        (state.frame == resources.frame && pointer_grab_target(requested, resources))
            .then_some(requested)
    }

    pub(super) fn read_hints(&self, client: Window) -> ClientSizeHints {
        self.conn
            .get_property(
                false,
                client,
                AtomEnum::WM_NORMAL_HINTS,
                AtomEnum::ANY,
                0,
                18,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(std::iter::Iterator::collect::<Vec<_>>))
            .map_or_else(ClientSizeHints::default, |values| {
                ClientSizeHints::parse(&values)
            })
    }
}
