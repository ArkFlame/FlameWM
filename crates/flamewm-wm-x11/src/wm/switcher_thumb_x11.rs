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

    /// Live refresh and last-frame proof on a private Xvfb with Composite and DAMAGE:
    /// `DISPLAY=:N cargo test -p flamewm-wm-x11 damaged_window -- --ignored --nocapture`.
    /// Production `ThumbProvider` over `X11ThumbSource`, with a real DAMAGE object on the client.
    /// The client repaints green to red; its `DamageNotify` drives `provider.damage` (the
    /// `switcher_damaged` path) and the next pump captures red. Once minimised, the client is
    /// never read again and its request serves the last red frame.
    #[test]
    #[ignore = "needs DISPLAY with Composite (Xvfb +extension Composite)"]
    fn damaged_window_recaptures_and_minimised_window_keeps_last_frame() {
        use crate::wm::switcher_thumb_provider::{CaptureBudget, ThumbProvider};
        use crate::wm::switcher_thumbs::ThumbKey;
        use std::cell::Cell;
        use std::time::Duration;
        use x11rb::protocol::Event;
        use x11rb::protocol::damage::{self, ReportLevel};
        use x11rb::protocol::xproto::{CreateGCAux, Rectangle};

        const GREEN: [u8; 3] = [0x00, 0xff, 0x00];
        const RED: [u8; 3] = [0x00, 0x00, 0xff];
        let (conn, screen_num) = x11rb::connect(None).expect("DISPLAY");
        let screen = conn.setup().roots[screen_num].clone();
        let frame = conn.generate_id().unwrap();
        conn.create_window(
            screen.root_depth,
            frame,
            screen.root,
            100,
            100,
            300,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().background_pixel(0),
        )
        .unwrap();
        let client = conn.generate_id().unwrap();
        conn.create_window(
            screen.root_depth,
            client,
            frame,
            40,
            30,
            200,
            120,
            0,
            WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().background_pixel(0x0000_ff00),
        )
        .unwrap();
        conn.composite_redirect_window(frame, Redirect::AUTOMATIC)
            .unwrap();
        conn.map_window(client).unwrap();
        conn.map_window(frame).unwrap();
        damage::query_version(&conn, 1, 1)
            .unwrap()
            .reply()
            .expect("DAMAGE extension");
        let damage_id = conn.generate_id().unwrap();
        damage::create(&conn, damage_id, client, ReportLevel::NON_EMPTY)
            .unwrap()
            .check()
            .unwrap();
        // Same reset as `watch_switcher_damage`: clear the paints from the map before watching.
        damage::subtract(&conn, damage_id, 0u32, 0u32)
            .unwrap()
            .check()
            .unwrap();
        conn.get_input_focus().unwrap().reply().unwrap();
        while conn.poll_for_event().unwrap().is_some() {}

        let minimised = Cell::new(false);
        let iconic = |_: u32| minimised.get();
        let redirected = |window: u32| (window == client).then_some(frame);
        let key = ThumbKey {
            window: client,
            src_w: 200,
            src_h: 120,
        };
        let mut provider = ThumbProvider::default();
        let mut budget = CaptureBudget::new(4);
        let pump = |provider: &mut ThumbProvider, budget: &mut CaptureBudget| {
            budget.begin_turn();
            let mut source = X11ThumbSource {
                conn: &conn,
                iconic: &iconic,
                redirected_frame: &redirected,
            };
            provider.pump(&mut source, budget)
        };
        let solid = |px: &[u8]| -> Option<[u8; 3]> {
            let first = [px[0], px[1], px[2]];
            px.chunks_exact(4).all(|p| p[..3] == first).then_some(first)
        };

        provider.request(key);
        assert_eq!(pump(&mut provider, &mut budget), 1, "first capture");
        let first = provider.request(key).expect("cached").2.clone();
        assert_eq!(
            solid(&first),
            Some(GREEN),
            "first capture shows the drawn colour"
        );

        // The client repaints by drawing into itself, which is what reports DAMAGE.
        let gc = conn.generate_id().unwrap();
        conn.create_gc(gc, client, &CreateGCAux::new().foreground(0x00ff_0000))
            .unwrap()
            .check()
            .unwrap();
        conn.poly_fill_rectangle(
            client,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: 200,
                height: 120,
            }],
        )
        .unwrap()
        .check()
        .unwrap();
        let mut damaged = false;
        for _ in 0..300 {
            while let Some(event) = conn.poll_for_event().unwrap() {
                damaged |= matches!(event, Event::DamageNotify(e) if e.damage == damage_id);
            }
            if damaged {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(damaged, "DamageNotify after the client repainted");
        // Production path: handle_damage_notify -> switcher_damaged -> provider.damage.
        provider.damage(client);
        assert!(
            provider.request(key).is_none(),
            "damaged preview is not served"
        );
        assert_eq!(
            pump(&mut provider, &mut budget),
            1,
            "recapture after damage"
        );
        let repainted = provider.request(key).expect("recaptured").2.clone();
        assert_eq!(
            solid(&repainted),
            Some(RED),
            "thumbnail follows the repaint"
        );

        minimised.set(true);
        provider.end_session();
        provider.begin_session(|window| window == client);
        let kept = provider
            .request(key)
            .expect("minimised window keeps its last frame")
            .2
            .clone();
        assert_eq!(solid(&kept), Some(RED), "last frame is the red repaint");
        assert_eq!(
            pump(&mut provider, &mut budget),
            0,
            "minimised window is never read"
        );
    }
}
