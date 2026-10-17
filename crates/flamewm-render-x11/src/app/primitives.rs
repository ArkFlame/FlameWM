use super::*;
impl X11App {
    /// Fills one device rect on the backbuffer.
    unsafe fn fill_box(&self, x: i32, y: i32, width: u32, height: u32) {
        // SAFETY: App owns live Xlib display, backbuffer, and graphics context.
        unsafe { XFillRectangle(self.display, self.backbuffer, self.gc, x, y, width, height) };
    }

    /// Fills the 90-degree corner arc of a `diameter` square at `(x, y)` from `start` degrees.
    unsafe fn fill_corner(&self, x: i32, y: i32, diameter: u32, start: i32) {
        // SAFETY: App owns live Xlib display, backbuffer, and graphics context.
        unsafe {
            XFillArc(
                self.display,
                self.backbuffer,
                self.gc,
                x,
                y,
                diameter,
                diameter,
                start * 64,
                90 * 64,
            )
        };
    }

    /// Strokes the 90-degree corner arc of a `diameter` square at `(x, y)` from `start` degrees.
    unsafe fn stroke_corner(&self, x: i32, y: i32, diameter: u32, start: i32) {
        // SAFETY: App owns live Xlib display, backbuffer, and graphics context.
        unsafe {
            XDrawArc(
                self.display,
                self.backbuffer,
                self.gc,
                x,
                y,
                diameter,
                diameter,
                start * 64,
                90 * 64,
            )
        };
    }

    /// Strokes one device line segment on the backbuffer.
    unsafe fn stroke_segment(&self, x1: i32, y1: i32, x2: i32, y2: i32) {
        // SAFETY: App owns live Xlib display, backbuffer, and graphics context.
        unsafe { XDrawLine(self.display, self.backbuffer, self.gc, x1, y1, x2, y2) };
    }

    pub(crate) unsafe fn fill_rounded_rect(&self, rect: Rect, radius: f32) {
        let x = rect.x.round() as i32;
        let y = rect.y.round() as i32;
        let width = rect.width.round().max(0.0) as u32;
        let height = rect.height.round().max(0.0) as u32;
        if width == 0 || height == 0 {
            return;
        }
        let radius = radius.round().max(0.0).min((width.min(height) / 2) as f32) as u32;
        if radius == 0 {
            unsafe { self.fill_box(x, y, width, height) };
            return;
        }
        let diameter = radius * 2;
        let right = x + width as i32 - diameter as i32;
        let bottom = y + height as i32 - diameter as i32;
        unsafe {
            self.fill_box(x + radius as i32, y, width.saturating_sub(diameter), height);
            self.fill_box(x, y + radius as i32, width, height.saturating_sub(diameter));
            self.fill_corner(x, y, diameter, 90);
            self.fill_corner(right, y, diameter, 0);
            self.fill_corner(x, bottom, diameter, 180);
            self.fill_corner(right, bottom, diameter, 270);
        }
    }

    pub(crate) unsafe fn stroke_rounded_rect(&self, rect: Rect, radius: f32, inset: i32) {
        let x = rect.x.round() as i32 + inset;
        let y = rect.y.round() as i32 + inset;
        let width = (rect.width.round() as i32 - 1 - inset * 2).max(0) as u32;
        let height = (rect.height.round() as i32 - 1 - inset * 2).max(0) as u32;
        if width == 0 || height == 0 {
            return;
        }
        let radius = (radius.round() as i32 - inset)
            .max(0)
            .min((width.min(height) / 2) as i32) as u32;
        if radius <= 1 {
            // SAFETY: App owns live Xlib display, backbuffer, and graphics context.
            unsafe { XDrawRectangle(self.display, self.backbuffer, self.gc, x, y, width, height) };
            return;
        }
        let diameter = radius * 2;
        let right = x + width as i32 - diameter as i32;
        let bottom = y + height as i32 - diameter as i32;
        let (x_end, y_end) = (x + width as i32, y + height as i32);
        let r = radius as i32;
        unsafe {
            self.stroke_segment(x + r, y, x_end - r, y);
            self.stroke_segment(x + r, y_end, x_end - r, y_end);
            self.stroke_segment(x, y + r, x, y_end - r);
            self.stroke_segment(x_end, y + r, x_end, y_end - r);
            self.stroke_corner(x, y, diameter, 90);
            self.stroke_corner(right, y, diameter, 0);
            self.stroke_corner(x, bottom, diameter, 180);
            self.stroke_corner(right, bottom, diameter, 270);
        }
    }
}

pub(crate) fn scale_rect(rect: Rect, scale: f32) -> Rect {
    Rect {
        x: rect.x * scale,
        y: rect.y * scale,
        width: rect.width * scale,
        height: rect.height * scale,
    }
}
