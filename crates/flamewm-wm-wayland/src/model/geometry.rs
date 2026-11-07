use super::*;

/// Rectangle centre, the anchor the X11 directional search compares window positions by.
pub(super) fn centre(rect: Rect) -> (i32, i32) {
    (rect.x + rect.w / 2, rect.y + rect.h / 2)
}

/// Converts one shared frame rectangle to the model's client rectangle exactly once.
pub(super) fn client_from_frame(frame: flamewm_api::Rect, titlebars: bool) -> Rect {
    let band = if titlebars { TITLEBAR_HEIGHT } else { 0 };
    Rect::new(frame.x, frame.y + band, frame.width, frame.height - band)
}

/// Moves `rect` fully inside `output`, shrinking it only when it is larger than the output.
///
/// A zero or negative output has no usable area, so the rect is returned unchanged rather than
/// clamped into an empty range.
pub(super) fn clamp_into_output(rect: Rect, output: Rect) -> Rect {
    if output.w <= 0 || output.h <= 0 {
        return rect;
    }
    let w = rect.w.clamp(0, output.w);
    let h = rect.h.clamp(0, output.h);
    let x = rect.x.clamp(output.x, output.x + output.w - w);
    let y = rect.y.clamp(output.y, output.y + output.h - h);
    Rect::new(x, y, w, h)
}

/// Keeps a pointer position on the last pixel row and column of `output`.
///
/// An output with no usable area leaves the point unchanged, like `clamp_into_output`.
pub(crate) fn clamp_point_into(x: f64, y: f64, output: Rect) -> (f64, f64) {
    if output.w <= 0 || output.h <= 0 {
        return (x, y);
    }
    let (min_x, min_y) = (f64::from(output.x), f64::from(output.y));
    let max_x = f64::from(output.x + output.w - 1);
    let max_y = f64::from(output.y + output.h - 1);
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

/// Client rect of a window maximized into `work`: the frame fills the shared-margin work area.
pub(super) fn maximized_client(work: Rect, titlebars: bool, margin: i32) -> Rect {
    let work = api_rect(work);
    let tile = snap_geometry(
        SnapTarget::Maximize,
        work,
        flamewm_api::Size::new(work.width, work.height),
    );
    client_from_frame(apply_window_margin(tile, work, margin), titlebars)
}

/// Client rect for a snapped placement. Preview and commit both call this one conversion path.
pub(super) fn snapped_client(
    target: SnapTarget,
    work: Rect,
    floating_restore: Rect,
    titlebars: bool,
    margin: i32,
) -> Rect {
    let work = api_rect(work);
    let tile = snap_geometry(
        target,
        work,
        flamewm_api::Size::new(floating_restore.w, floating_restore.h),
    );
    client_from_frame(apply_window_margin(tile, work, margin), titlebars)
}

pub(super) fn layout_rect(
    mode: Mode,
    saved_rect: Rect,
    floating_restore: Rect,
    work: Rect,
    titlebars: bool,
    margin: i32,
) -> Rect {
    match mode {
        Mode::Normal | Mode::Fullscreen => saved_rect,
        Mode::Snapped(target) => snapped_client(target, work, floating_restore, titlebars, margin),
        Mode::Maximized => maximized_client(work, titlebars, margin),
    }
}

/// Bridges the model rectangle to the shared geometry type the window-core policy speaks.
pub(super) const fn api_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(rect.x, rect.y, rect.w, rect.h)
}

/// Titlebar button rectangle, right-aligned inside the frame: close, maximize, minimize.
pub fn button_rect(frame: Rect, button: Button) -> Rect {
    let slot = BUTTON_SIZE + BUTTON_GAP;
    let right = frame.x + frame.w - BUTTON_GAP;
    let x = match button {
        Button::Close => right - slot,
        Button::Maximize => right - 2 * slot,
        Button::Minimize => right - 3 * slot,
    };
    Rect::new(x, frame.y, BUTTON_SIZE, TITLEBAR_HEIGHT)
}

pub(super) fn button_at(frame: Rect, px: f64, py: f64) -> Option<Button> {
    [Button::Close, Button::Maximize, Button::Minimize]
        .into_iter()
        .find(|button| button_rect(frame, *button).contains(px, py))
}

/// Client rect inside a grip band along any edge (client corners included) starts a resize.
pub(super) fn in_resize_grip(rect: Rect, px: f64, py: f64) -> bool {
    if !rect.contains(px, py) {
        return false;
    }
    let grip = f64::from(RESIZE_GRIP);
    px < f64::from(rect.x) + grip
        || px >= f64::from(rect.x + rect.w) - grip
        || py < f64::from(rect.y) + grip
        || py >= f64::from(rect.y + rect.h) - grip
}

/// Two-axis resize edges when `(px, py)` is in the [`CORNER_GRIP`] square at a `frame` corner.
///
/// The square belongs to the frame, not the client: a decorated window's top corners are in the
/// titlebar band, outside the client rect [`in_resize_grip`] inspects. The close button starts at
/// the same frame corner, so only the square it shares with that handle resizes.
pub fn corner_at(frame: Rect, px: f64, py: f64) -> Option<ResizeEdge> {
    if !frame.contains(px, py) {
        return None;
    }
    let grip = f64::from(CORNER_GRIP);
    let left = px < f64::from(frame.x) + grip;
    let right = px >= f64::from(frame.x + frame.w) - grip;
    let top = py < f64::from(frame.y) + grip;
    let bottom = py >= f64::from(frame.y + frame.h) - grip;
    ((left || right) && (top || bottom)).then_some(ResizeEdge {
        left,
        right,
        top,
        bottom,
    })
}

/// Resize edges for a press on `frame`, whose client rect is `rect`: a frame corner square wins
/// over the client grip band, so a corner drag from the titlebar resizes both axes.
pub fn resize_edge_at(frame: Rect, rect: Rect, px: f64, py: f64) -> ResizeEdge {
    corner_at(frame, px, py).unwrap_or_else(|| ResizeEdge::at(rect, px, py))
}

/// Edges a resize grab moves: `left`/`top` move the origin, `right`/`bottom` move the far side.
/// A corner sets two flags. X11 parity: edge handles and Ctrl+Super+right use the same edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeEdge {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl ResizeEdge {
    /// Edges under `(px, py)` inside the grip band of `rect`.
    pub fn at(rect: Rect, px: f64, py: f64) -> Self {
        let grip = f64::from(RESIZE_GRIP);
        Self {
            left: px < f64::from(rect.x) + grip,
            right: px >= f64::from(rect.x + rect.w) - grip,
            top: py < f64::from(rect.y) + grip,
            bottom: py >= f64::from(rect.y + rect.h) - grip,
        }
    }

    /// Quadrant of `rect` holding `(px, py)`: Ctrl+Super+right resizes the corner nearest the grab.
    pub fn quadrant(rect: Rect, px: f64, py: f64) -> Self {
        let left = px < f64::from(rect.x) + f64::from(rect.w) / 2.0;
        let top = py < f64::from(rect.y) + f64::from(rect.h) / 2.0;
        Self {
            left,
            right: !left,
            top,
            bottom: !top,
        }
    }

    /// Geometry after moving this edge by `(dx, dy)` from `start`, never below the minimum size.
    /// A left or top edge that hits the minimum stays on the opposite side (the origin follows).
    pub fn resized(self, start: Rect, dx: i32, dy: i32) -> Rect {
        let (mut x, mut y, mut w, mut h) = (start.x, start.y, start.w, start.h);
        if self.left {
            x = start.x + dx;
            w = start.w - dx;
        } else if self.right {
            w = start.w + dx;
        }
        if self.top {
            y = start.y + dy;
            h = start.h - dy;
        } else if self.bottom {
            h = start.h + dy;
        }
        if w < MIN_CLIENT_W {
            if self.left {
                x = start.x + start.w - MIN_CLIENT_W;
            }
            w = MIN_CLIENT_W;
        }
        if h < MIN_CLIENT_H {
            if self.top {
                y = start.y + start.h - MIN_CLIENT_H;
            }
            h = MIN_CLIENT_H;
        }
        Rect::new(x, y, w, h)
    }
}
