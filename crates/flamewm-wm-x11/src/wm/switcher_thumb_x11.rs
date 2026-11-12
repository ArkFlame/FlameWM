//! X11 `ThumbSource`: `GetWindowAttributes` for state, pixels from the client's redirected frame
//! when it has one, otherwise from root-visible readback.
//!
//! Redirected frames (`XCompositeRedirectWindow`, see `manage.rs`) keep their own off-screen
//! contents, so `NameWindowPixmap` + `GetImage` returns what the client drew even when another
//! window covers it. Root readback is the fallback and returns cover pixels for occluded windows.

use x11rb::connection::Connection;
use x11rb::protocol::composite::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, MapState};

use super::switcher_thumb_provider::{Capture, ThumbSource, WindowState};

pub(crate) struct X11ThumbSource<'a, C: Connection> {
    pub(crate) conn: &'a C,
    /// WM-owned iconic state, which X does not expose directly.
    pub(crate) iconic: &'a dyn Fn(u32) -> bool,
    /// The redirected frame that contains this client, if the WM redirected it.
    pub(crate) redirected_frame: &'a dyn Fn(u32) -> Option<u32>,
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
        match (self.redirected_frame)(window) {
            // The client's geometry is relative to its frame, so the frame's pixmap is cropped to it.
            Some(frame) => self.capture_redirected(frame, geom.x, geom.y, geom.width, geom.height),
            None => self.capture_drawable(window, 0, 0, geom.width, geom.height),
        }
    }
}

impl<C: Connection> X11ThumbSource<'_, C> {
    /// Reads the frame's composite pixmap. `None` while the frame is unmapped (no pixmap exists).
    fn capture_redirected(&self, frame: u32, x: i16, y: i16, w: u16, h: u16) -> Option<Capture> {
        let pixmap = self.conn.generate_id().ok()?;
        self.conn
            .composite_name_window_pixmap(frame, pixmap)
            .ok()?
            .check()
            .ok()?;
        let capture = self.capture_drawable(pixmap, x, y, w, h);
        self.conn.free_pixmap(pixmap).ok()?.check().ok()?;
        capture
    }

    fn capture_drawable(&self, drawable: u32, x: i16, y: i16, w: u16, h: u16) -> Option<Capture> {
        let image = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, !0)
            .ok()?
            .reply()
            .ok()?;
        let stride = image.data.len().checked_div(usize::from(h))?;
        Some(Capture {
            width: u32::from(w),
            height: u32::from(h),
            stride,
            data: image.data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::COPY_FROM_PARENT;
    use x11rb::protocol::composite::Redirect;
    use x11rb::protocol::xproto::{CreateWindowAux, EventMask, WindowClass};

    /// Occluded-window proof on a private Xvfb with the Composite extension:
    /// `DISPLAY=:N cargo test -p flamewm-wm-x11 occluded_window -- --ignored --nocapture`.
    /// A green client inside a redirected frame is fully covered by a blue override-redirect
    /// window. The capture must be green (the drawn colour), not blue (what is on screen).
    #[test]
    #[ignore = "needs DISPLAY with the Composite extension (Xvfb +extension Composite)"]
    fn occluded_window_captures_its_drawn_colour() {
        let (conn, screen_num) = x11rb::connect(None).expect("DISPLAY");
        let screen = conn.setup().roots[screen_num].clone();
        let create = |parent, x, y, w, h, bg: u32, aux: CreateWindowAux| {
            let id = conn.generate_id().unwrap();
            conn.create_window(
                screen.root_depth,
                id,
                parent,
                x,
                y,
                w,
                h,
                0,
                WindowClass::INPUT_OUTPUT,
                COPY_FROM_PARENT,
                &aux.background_pixel(bg),
            )
            .unwrap();
            id
        };
        let frame = create(
            screen.root,
            100,
            100,
            300,
            200,
            0x0000_0000,
            CreateWindowAux::new(),
        );
        let client = create(frame, 40, 30, 200, 120, 0x0000_ff00, CreateWindowAux::new());
        conn.composite_redirect_window(frame, Redirect::AUTOMATIC)
            .unwrap();
        conn.map_window(client).unwrap();
        conn.map_window(frame).unwrap();
        let cover = create(
            screen.root,
            0,
            0,
            600,
            400,
            0x0000_00ff,
            CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::EXPOSURE),
        );
        conn.map_window(cover).unwrap();
        conn.get_input_focus().unwrap().reply().unwrap();

        let iconic = |_: u32| false;
        let redirected = |w: u32| (w == client).then_some(frame);
        let mut source = X11ThumbSource {
            conn: &conn,
            iconic: &iconic,
            redirected_frame: &redirected,
        };
        let cap = source.capture(client).expect("composite capture");
        assert_eq!((cap.width, cap.height), (200, 120));
        // 32-bit ZPixmap, little-endian BGRX: green is [B=0, G=255, R=0].
        for row in cap.data.chunks_exact(cap.stride).take(cap.height as usize) {
            for px in row[..cap.width as usize * 4].chunks_exact(4) {
                assert_eq!(
                    &px[..3],
                    &[0x00, 0xff, 0x00],
                    "covered pixel must be the drawn colour"
                );
            }
        }
        let thumb = crate::wm::switcher_thumbs::capture_to_thumb(
            &cap.data, cap.width, cap.height, cap.stride,
        );
        assert!(thumb.is_some_and(|(_, _, px)| px.iter().any(|&b| b != 0)));
    }
}
