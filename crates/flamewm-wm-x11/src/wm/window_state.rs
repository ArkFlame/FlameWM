//! Per-window decoration, extents, and move/resize state reads.
use super::*;

impl<C: Connection> Wm<'_, C> {
    pub(super) fn extents(&self) -> FrameExtents {
        FrameExtents::new(i32::from(self.titlebar_height()), 1)
    }

    /// Effective Server titlebar band. The `titlebars` setting is the only authority;
    /// every geometry, shape and chrome read goes through it.
    pub(super) fn titlebar_height(&self) -> u16 {
        if crate::live_settings::titlebars() {
            self.config.titlebar_height
        } else {
            0
        }
    }

    /// Frame extents for a decoration owner. Only `Server` gets a Flame
    /// titlebar and border; `ClientSide`/`None` frames equal the client.
    pub(super) fn extents_for_decoration(&self, decoration: Decoration) -> FrameExtents {
        match decoration {
            Decoration::Server => self.extents(),
            Decoration::ClientSide | Decoration::None => FrameExtents::new(0, 0),
        }
    }

    pub(super) fn extents_for(&self, client: Window) -> FrameExtents {
        let decoration = self
            .clients
            .get(&client)
            .map_or(Decoration::Server, |state| state.decoration);
        self.extents_for_decoration(decoration)
    }

    /// Reads `_MOTIF_WM_HINTS` and `_GTK_FRAME_EXTENTS` and decides who draws
    /// the frame. Conventions: Motif (Qt frameless, `OnlyOffice`, Electron) and
    /// GTK3/4 CSD (Firefox, GNOME apps) report client-drawn frames; Flame adds
    /// no titlebar, border or resize handles for them.
    pub(super) fn read_decoration(&self, window: Window, kind: WindowKind) -> Decoration {
        let motif = self.read_cardinals(window, self.atoms.motif_wm_hints, 5);
        let gtk = self.read_cardinals(window, self.atoms.gtk_frame_extents, 4);
        decoration_policy(&DecorationHints {
            override_redirect: false,
            kind,
            motif_decorations: motif.as_deref().and_then(motif_decorations_field),
            gtk_frame_extents: gtk.as_deref().and_then(gtk_frame_extents),
        })
    }

    pub(super) fn read_cardinals(&self, window: Window, atom: Atom, len: u32) -> Option<Vec<u32>> {
        self.conn
            .get_property(false, window, atom, AtomEnum::ANY, 0, len)
            .ok()?
            .reply()
            .ok()?
            .value32()
            .map(std::iter::Iterator::collect)
    }

    /// `PropertyNotify` on Motif/GTK hints: re-evaluate and rebuild in place.
    /// The client area stays fixed; the frame grows or shrinks by the
    /// titlebar, so nothing is unmapped or remapped.
    pub(super) fn refresh_decoration(&mut self, client: Window) -> Result<(), ReplyError> {
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        let (old, kind) = (state.decoration, state.kind);
        let new = self.read_decoration(client, kind);
        if new == old {
            return Ok(());
        }
        let old_ext = self.extents_for_decoration(old);
        let new_ext = self.extents_for_decoration(new);
        let Some(state) = self.clients.get_mut(&client) else {
            return Ok(());
        };
        let outer = state.outer;
        let y = outer.y + old_ext.titlebar_h - new_ext.titlebar_h;
        let height = (outer.height as i32 - old_ext.titlebar_h + new_ext.titlebar_h).max(1);
        state.decoration = new;
        state.set_outer(Rect::new(outer.x, y, outer.width, height as u32));
        if let Some(resources) = self.registry.lookup_by_client(client) {
            for xid in resources.children() {
                if input_child_mapped(
                    new,
                    resources.region_of(xid),
                    crate::live_settings::titlebars(),
                ) {
                    self.conn.map_window(xid)?;
                } else {
                    self.conn.unmap_window(xid)?;
                }
            }
        }
        self.apply_frame_geometry(client)?;
        self.layout_input_children(client)?;
        self.set_frame_extents(client)?;
        self.paint_chrome(client)?;
        self.mark_windows();
        Ok(())
    }

    /// `_NET_WM_MOVERESIZE` from the client's own header bar or edges.
    /// Directions 0-7 are the edges/corners, 8 is move; keyboard (9-10) and
    /// cancel (11) are not honoured. Starts the same frame reducer press that
    /// a real button press would, so the existing pointer grab drives it.
    pub(super) fn begin_client_moveresize(
        &mut self,
        client: Window,
        pointer: RootPoint,
        direction: u32,
    ) -> Result<(), ReplyError> {
        const MOVERESIZE_MOVE: u32 = 8;
        let want = match direction {
            MOVERESIZE_MOVE => FrameRegion::TitleDrag,
            0..=7 => FrameRegion::Resize(resize_edges_for_direction(direction)),
            _ => return Ok(()),
        };
        if self
            .sessions
            .get(&client)
            .is_some_and(|session| !session.session.is_idle())
        {
            return Ok(());
        }
        let Some(resources) = self.registry.lookup_by_client(client) else {
            return Ok(());
        };
        let Some(source) = resources.children().into_iter().find(|xid| {
            self.registry
                .lookup_by_xid(*xid)
                .is_some_and(|target| target.region == want)
        }) else {
            return Ok(());
        };
        self.focus(client)?;
        self.reduce_and_execute(client, FrameEvent::Press { source, pointer })
    }
}
