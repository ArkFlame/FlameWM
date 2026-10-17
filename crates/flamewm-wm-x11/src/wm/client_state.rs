use super::*;

impl<'a, C: Connection> Wm<'a, C> {
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
        let mode = self.clients.get(&client).map_or(PlacementMode::Floating, |state| state.placement.mode);
        let extents = match self.clients.get(&client).map(|state| state.decoration) {
            None | Some(Decoration::Server) => frame_extents_for_mode(
                mode,
                self.config.frame_border,
                self.config.titlebar_height,
            ),
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
        &self,
        frame: Window,
        outer: crate::geometry::Rect,
        floating: bool,
    ) -> Result<(), ReplyError> {
        use x11rb::protocol::shape::{SK as ShapeSk, SO as ShapeOp};
        let rects = crate::frame::geometry_shape::bounding_rectangles(
            outer,
            floating,
            self.decoration_tokens.style,
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

    pub(super) fn read_net_wm_icon(&self, client: Window) -> Result<Option<chrome::IconImage>, &'static str> {
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
        let utf8 = self
            .conn
            .get_property(
                false,
                client,
                self.atoms.net_wm_name,
                self.atoms.utf8_string,
                0,
                1024,
            )?
            .reply()?;
        if !utf8.value.is_empty() {
            return Ok(String::from_utf8_lossy(&utf8.value)
                .trim_matches('\0')
                .to_owned());
        }
        let legacy = self
            .conn
            .get_property(false, client, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?
            .reply()?;
        Ok(String::from_utf8_lossy(&legacy.value)
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
        } else if let Some(target) = self.registry.lookup_by_xid(window) {
            Some(target.client)
        } else {
            None
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
            .and_then(|reply| reply.value32().map(|iter| iter.collect::<Vec<_>>()))
            .map_or_else(ClientSizeHints::default, |values| {
                ClientSizeHints::parse(&values)
            })
    }
}
