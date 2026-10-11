use super::*;
impl X11App {
    pub(crate) fn update_cursor(&mut self, document: &RuntimeDocument, hover: Option<u32>) {
        let kind = hover
            .and_then(|index| {
                document
                    .document
                    .nodes
                    .get(index as usize)
                    .map(|node| (index, node))
            })
            .map(|(index, node)| self.interaction.style_for(index, node).cursor)
            .unwrap_or(CursorKind::Default);
        self.set_cursor(kind);
    }

    pub(crate) fn set_cursor(&mut self, kind: CursorKind) {
        if Some(kind) == self.current_cursor {
            return;
        }
        // Xcursor backend first (semantic names, cached); core font fallback
        // only when the Xcursor lookup fails.
        if let Some(xcursor) = self.xcursor.as_mut() {
            let window = self.window;
            let resolved = unsafe { xcursor.define(window, kind) };
            if resolved.is_some() {
                unsafe { XFlush(self.display) };
                self.current_cursor = Some(kind);
                return;
            }
        }
        let cursor = self
            .cursors
            .get(&kind)
            .copied()
            .or_else(|| self.cursors.get(&CursorKind::Default).copied());
        if let Some(cursor) = cursor {
            // X11App owns the window and cursor handles while its display is live.
            unsafe {
                XDefineCursor(self.display, self.window, cursor);
                XFlush(self.display);
            }
            self.current_cursor = Some(kind);
        }
    }
}
pub(crate) fn cursor_shape(kind: CursorKind) -> u32 {
    match kind {
        CursorKind::Default => XC_LEFT_PTR,
        CursorKind::Pointer => XC_HAND2,
        CursorKind::Text => XC_XTERM,
        CursorKind::Move => XC_FLEUR,
        CursorKind::ResizeHorizontal => XC_SB_H_DOUBLE_ARROW,
        CursorKind::ResizeVertical => XC_SB_V_DOUBLE_ARROW,
        CursorKind::ResizeNorthWestSouthEast => XC_BOTTOM_RIGHT_CORNER,
        CursorKind::ResizeNorthEastSouthWest => XC_BOTTOM_LEFT_CORNER,
    }
}
