//! X side of the frame shadow (pure shading lives in `frame::shadow`).
//!
//! Per floating frame: three override-redirect, input-transparent windows
//! stacked directly below the frame. The server holds each strip's shaded
//! pixels as the window background, so exposes repaint without WM work.
//! Pixels are refreshed only on map, resize, focus change and hide. A move
//! just reconfigures the three strips.

use super::*;
use crate::frame::shadow::{self, STRIPS};
use crate::geometry::Rect;
use x11rb::protocol::shape::{SK as ShapeSk, SO as ShapeOp};

/// Strip windows of one frame and what they currently show.
pub(super) struct FrameShadow {
    wins: [Window; 3],
    rects: [Rect; 3],
    focused: bool,
    size: (u32, u32),
}

fn strip_aux(rect: Rect, frame: Window) -> ConfigureWindowAux {
    ConfigureWindowAux::new()
        .x(rect.x)
        .y(rect.y)
        .width(rect.width.max(1))
        .height(rect.height.max(1))
        .sibling(frame)
        .stack_mode(StackMode::BELOW)
}

impl<'a, C: Connection> Wm<'a, C> {
    /// Bring shadow windows in line with the current frames. Called once per
    /// reactor turn; cheap when nothing changed.
    pub(super) fn reconcile_frame_shadows(&mut self) -> Result<(), ReplyOrIdError> {
        let screen = self.screen();
        let usable = self.decoration_tokens.window_shadows
            && screen.root_depth == 24
            && self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let wanted: Vec<(Window, Rect, bool)> = if usable {
            self.clients
                .values()
                .filter(|c| c.is_floating() && c.visible_on(self.current_workspace))
                .map(|c| (c.frame, c.outer, self.active == Some(c.client)))
                .collect()
        } else {
            Vec::new()
        };
        let gone: Vec<Window> = self
            .frame_shadows
            .keys()
            .copied()
            .filter(|frame| !wanted.iter().any(|w| w.0 == *frame))
            .collect();
        for frame in gone {
            if let Some(shadow) = self.frame_shadows.remove(&frame) {
                for win in shadow.wins {
                    self.conn.destroy_window(win)?;
                }
            }
        }
        for (frame, outer, focused) in wanted {
            self.sync_frame_shadow(frame, outer, focused)?;
        }
        Ok(())
    }

    fn sync_frame_shadow(
        &mut self,
        frame: Window,
        outer: Rect,
        focused: bool,
    ) -> Result<(), ReplyOrIdError> {
        let rects = STRIPS.map(|strip| shadow::strip_rect(strip, outer));
        let size = (outer.width, outer.height);
        let Some(prev) = self.frame_shadows.get(&frame) else {
            let wins = self.create_shadow_windows()?;
            return self.refresh_frame_shadow(frame, outer, focused, wins);
        };
        if prev.size != size || prev.focused != focused {
            let wins = prev.wins;
            return self.refresh_frame_shadow(frame, outer, focused, wins);
        }
        if prev.rects == rects {
            return Ok(());
        }
        // Move only: reconfigure each strip, keeping it directly under the frame.
        let wins = prev.wins;
        for (win, rect) in wins.iter().zip(rects) {
            self.conn.configure_window(*win, &strip_aux(rect, frame))?;
        }
        if let Some(shadow) = self.frame_shadows.get_mut(&frame) {
            shadow.rects = rects;
        }
        Ok(())
    }

    fn create_shadow_windows(&self) -> Result<[Window; 3], ReplyOrIdError> {
        let root = self.screen().root;
        let mut wins = [0; 3];
        for win in &mut wins {
            *win = self.conn.generate_id()?;
            self.conn.create_window(
                COPY_DEPTH_FROM_PARENT,
                *win,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::NO_EVENT),
            )?;
            // Empty input shape: the strip never takes pointer input.
            self.conn.shape_rectangles(
                ShapeOp::SET,
                ShapeSk::INPUT,
                ClipOrdering::UNSORTED,
                *win,
                0,
                0,
                &[],
            )?;
        }
        Ok(wins)
    }

    /// Shade all three strips from a root snapshot, under one server grab.
    /// The strips are unmapped during the read so they never shade themselves.
    fn refresh_frame_shadow(
        &mut self,
        frame: Window,
        outer: Rect,
        focused: bool,
        wins: [Window; 3],
    ) -> Result<(), ReplyOrIdError> {
        let _guard = flamewm_profiler::start("wm.shadow.refresh");
        let rects = STRIPS.map(|strip| shadow::strip_rect(strip, outer));
        self.conn.grab_server()?;
        let result = self.refresh_strips(frame, outer, focused, wins, rects);
        self.conn.ungrab_server()?;
        let shown = result?;
        if shown {
            self.frame_shadows.insert(
                frame,
                FrameShadow {
                    wins,
                    rects,
                    focused,
                    size: (outer.width, outer.height),
                },
            );
        } else {
            for win in wins {
                self.conn.destroy_window(win)?;
            }
            self.frame_shadows.remove(&frame);
        }
        Ok(())
    }

    /// Returns `false` when a strip could not be shaded (no snapshot, request
    /// too large); the caller then hides the shadow for this frame.
    fn refresh_strips(
        &self,
        frame: Window,
        outer: Rect,
        focused: bool,
        wins: [Window; 3],
        rects: [Rect; 3],
    ) -> Result<bool, ReplyOrIdError> {
        let root = self.screen().root;
        for win in wins {
            self.conn.unmap_window(win)?;
        }
        let gc = self.conn.generate_id()?;
        self.conn
            .create_gc(gc, root, &CreateGCAux::new())?;
        let mut shown = true;
        for (i, strip) in STRIPS.iter().enumerate() {
            let rect = rects[i];
            let Some(mut pixels) = self.root_snapshot(rect)? else {
                shown = false;
                break;
            };
            shadow::shade_strip(*strip, outer, focused, &mut pixels);
            if pixels.len() + 32 > self.conn.maximum_request_bytes() {
                shown = false;
                break;
            }
            let pixmap = self.conn.generate_id()?;
            let (w, h) = (rect.width.max(1) as u16, rect.height.max(1) as u16);
            self.conn.create_pixmap(24, pixmap, root, w, h)?;
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                gc,
                w,
                h,
                0,
                0,
                0,
                24,
                &pixels,
            )?;
            // The server keeps a pixmap alive while a window uses it as background.
            self.conn.change_window_attributes(
                wins[i],
                &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
            )?;
            self.conn.free_pixmap(pixmap)?;
            self.conn.configure_window(wins[i], &strip_aux(rect, frame))?;
        }
        self.conn.free_gc(gc)?;
        if shown {
            for win in wins {
                self.conn.map_window(win)?;
            }
        }
        Ok(shown)
    }

    /// BGRX pixels of `rect`, clipped to the screen. Off-screen parts are zero
    /// (never visible). Returns `None` when nothing of the rect is on screen.
    fn root_snapshot(&self, rect: Rect) -> Result<Option<Vec<u8>>, ReplyOrIdError> {
        let screen = self.screen();
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.width as i32).min(i32::from(screen.width_in_pixels));
        let y1 = (rect.y + rect.height as i32).min(i32::from(screen.height_in_pixels));
        if x1 <= x0 || y1 <= y0 {
            return Ok(None);
        }
        let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                screen.root,
                x0 as i16,
                y0 as i16,
                cw as u16,
                ch as u16,
                u32::MAX,
            )?
            .reply()?;
        if reply.depth != 24 || reply.data.len() != cw * ch * 4 {
            return Ok(None);
        }
        let (w, h) = (rect.width.max(1) as usize, rect.height.max(1) as usize);
        let mut pixels = vec![0u8; w * h * 4];
        let (ox, oy) = ((x0 - rect.x) as usize, (y0 - rect.y) as usize);
        for row in 0..ch {
            let src = &reply.data[row * cw * 4..(row + 1) * cw * 4];
            let dst = ((oy + row) * w + ox) * 4;
            pixels[dst..dst + cw * 4].copy_from_slice(src);
        }
        Ok(Some(pixels))
    }
}
