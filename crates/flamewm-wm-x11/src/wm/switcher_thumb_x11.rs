//! X11 `ThumbSource`: `GetWindowAttributes` for state, `GetGeometry` + `GetImage` for pixels.
//!
//! Root-visible readback only: an occluded window returns whatever covers it. Correct occlusion
//! needs composite redirection (see docs/backlog/alttab.md, option C), not measured yet.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, MapState};

use super::switcher_thumb_provider::{Capture, ThumbSource, WindowState};

pub(crate) struct X11ThumbSource<'a, C: Connection> {
    pub(crate) conn: &'a C,
    /// WM-owned iconic state, which X does not expose directly.
    pub(crate) iconic: &'a dyn Fn(u32) -> bool,
}

impl<C: Connection> ThumbSource for X11ThumbSource<'_, C> {
    fn state(&mut self, window: u32) -> Option<WindowState> {
        let attrs = self.conn.get_window_attributes(window).ok()?.reply().ok()?;
        Some(WindowState {
            mapped: attrs.map_state != MapState::UNMAPPED,
            viewable: attrs.map_state == MapState::VIEWABLE,
            iconic: (self.iconic)(window),
        })
    }

    fn capture(&mut self, window: u32) -> Option<Capture> {
        let geom = self.conn.get_geometry(window).ok()?.reply().ok()?;
        if geom.width == 0 || geom.height == 0 {
            return None;
        }
        let image = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                window,
                0,
                0,
                geom.width,
                geom.height,
                !0,
            )
            .ok()?
            .reply()
            .ok()?;
        let stride = image.data.len().checked_div(usize::from(geom.height))?;
        Some(Capture {
            width: u32::from(geom.width),
            height: u32::from(geom.height),
            stride,
            data: image.data,
        })
    }
}
