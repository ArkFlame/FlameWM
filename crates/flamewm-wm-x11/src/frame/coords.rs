//! Frame coordinate domains: root (screen) vs frame-local vs client-local.
//!
//! Pure value objects. No X calls, no x11rb.
//!
//! Domains:
//! - Root: screen coordinates (frame outer top-left, client root origin).
//! - Frame-local: origin = frame outer top-left.
//! - Client-local: origin = reparented client window origin inside the frame.
//!   Old-good model: client local x=0, y=titlebar; size = (frame.w,
//!   frame.h - titlebar). X border is an outer frame property only.

/// Point in root (screen) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RootPoint {
    pub x: i32,
    pub y: i32,
}

/// Point in frame-local coordinates (origin = frame outer top-left).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FramePoint {
    pub x: i32,
    pub y: i32,
}

/// Rectangle in root (screen) coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RootRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Rectangle in frame-local coordinates (origin = frame outer top-left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Rectangle in client-local coordinates (origin = client window origin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientLocalRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Client rectangle in root (screen) coordinates.
///
/// Distinct from [`RootRect`] (which may describe a frame outer rect) so
/// callers cannot silently mix frame-root and client-root geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientRootRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RootPoint {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Explicit domain crossing: root -> frame given the frame origin in root coords.
    #[cfg(test)]
    #[must_use]
    pub fn to_frame(self, frame_origin: RootPoint) -> FramePoint {
        FramePoint {
            x: self.x.saturating_sub(frame_origin.x),
            y: self.y.saturating_sub(frame_origin.y),
        }
    }
}

#[cfg(test)]
impl FramePoint {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Explicit domain crossing: frame -> root given the frame origin in root coords.
    #[must_use]
    pub fn to_root(self, frame_origin: RootPoint) -> RootPoint {
        RootPoint {
            x: self.x.saturating_add(frame_origin.x),
            y: self.y.saturating_add(frame_origin.y),
        }
    }
}

impl ClientLocalRect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}

impl ClientRootRect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub const fn origin(self) -> RootPoint {
        RootPoint::new(self.x, self.y)
    }

    #[must_use]
    pub fn size_u32(self) -> (u32, u32) {
        (self.w.max(1) as u32, self.h.max(1) as u32)
    }
}

impl RootRect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Legacy frame `Rect` (u32 size) -> placement `RootRect`.
    #[must_use]
    pub fn from_legacy(rect: crate::geometry::Rect) -> Self {
        Self::new(
            rect.x,
            rect.y,
            rect.width.min(i32::MAX as u32) as i32,
            rect.height.min(i32::MAX as u32) as i32,
        )
    }

    /// Placement `RootRect` -> legacy frame `Rect`.
    #[must_use]
    pub fn to_legacy(self) -> crate::geometry::Rect {
        crate::geometry::Rect::new(self.x, self.y, self.w.max(1) as u32, self.h.max(1) as u32)
    }

    #[cfg(test)]
    #[must_use]
    pub const fn origin(self) -> RootPoint {
        RootPoint::new(self.x, self.y)
    }

    /// Explicit domain crossing: root -> frame given the frame origin in root coords.
    #[cfg(test)]
    #[must_use]
    pub fn to_frame(self, frame_origin: RootPoint) -> FrameRect {
        let p = self.origin().to_frame(frame_origin);
        FrameRect::new(p.x, p.y, self.w, self.h)
    }
}

impl FrameRect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Explicit domain crossing: frame -> root given the frame origin in root coords.
    #[cfg(test)]
    #[must_use]
    pub fn to_root(self, frame_origin: RootPoint) -> RootRect {
        let p = FramePoint::new(self.x, self.y).to_root(frame_origin);
        RootRect::new(p.x, p.y, self.w, self.h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_roundtrip_nonzero_origin() {
        let origin = RootPoint::new(500, 200);
        let root = RootPoint::new(520, 230);
        let frame = root.to_frame(origin);
        assert_eq!(frame, FramePoint::new(20, 30));
        assert_eq!(frame.to_root(origin), root);
    }

    #[test]
    fn rect_crossing_nonzero_origin() {
        let origin = RootPoint::new(500, 200);
        let root = RootRect::new(500, 200, 400, 300);
        let frame = root.to_frame(origin);
        assert_eq!(frame, FrameRect::new(0, 0, 400, 300));
        assert_eq!(frame.to_root(origin), root);
    }

    #[test]
    fn origin_rect_and_negative_offsets() {
        let origin = RootPoint::new(500, 200);
        let root = RootRect::new(480, 190, 100, 50);
        let frame = root.to_frame(origin);
        assert_eq!(frame, FrameRect::new(-20, -10, 100, 50));
        assert_eq!(frame.to_root(origin), root);
        assert_eq!(root.origin(), RootPoint::new(480, 190));
        assert_eq!(FramePoint::new(0, 0).to_root(origin), origin);
    }

    #[test]
    fn client_local_types_constructible() {
        let r = ClientLocalRect::new(0, 31, 400, 269);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 31, 400, 269));
    }

    #[test]
    fn client_root_rect_origin_and_size() {
        let r = ClientRootRect::new(400, 231, 400, 269);
        assert_eq!(r.origin(), RootPoint::new(400, 231));
        assert_eq!(r.size_u32(), (400, 269));
    }

    #[test]
    fn synthetic_root_differs_from_local_at_nonzero_origin() {
        // Frame at (400,200): client local origin is (0,titlebar) while the
        // synthetic client-root rect carries the frame offset.
        let local = ClientLocalRect::new(0, 31, 400, 269);
        let root = ClientRootRect::new(400, 231, 400, 269);
        assert_ne!((local.x, local.y), (root.x, root.y));
        assert_eq!((local.w, local.h), (root.w, root.h));
        assert_eq!(root.origin(), RootPoint::new(400, 231));
    }
}
