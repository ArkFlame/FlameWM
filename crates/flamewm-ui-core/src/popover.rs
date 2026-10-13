//! Pure popover placement: pick an on-screen origin for a popup of
//! `popup_size` anchored to `source_rect`, preferring `edge`, clamped to
//! `work_area` with `offset` gap. No rendering here.

use flamewm_api::{Point, Rect, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverEdge {
    Below,
    Above,
    Right,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverAlign {
    Start,
    End,
    Center,
    /// Right edge pinned to the bounds' right edge (`work_area` passed to
    /// `place_aligned`); used for output-anchored quick controls.
    OutputEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopoverGeometry {
    pub origin: Point,
    pub edge: PopoverEdge,
}

impl PopoverGeometry {
    #[must_use]
    pub fn place(
        source_rect: Rect,
        popup_size: Size,
        edge: PopoverEdge,
        work_area: Rect,
        offset: i32,
    ) -> Self {
        Self::place_aligned(
            source_rect,
            popup_size,
            edge,
            PopoverAlign::Start,
            work_area,
            offset,
        )
    }

    #[must_use]
    pub fn place_aligned(
        source_rect: Rect,
        popup_size: Size,
        edge: PopoverEdge,
        align: PopoverAlign,
        work_area: Rect,
        offset: i32,
    ) -> Self {
        let size = Size::new(popup_size.width.max(0), popup_size.height.max(0));
        let candidates = [
            edge,
            Self::opposite(edge),
            PopoverEdge::Below,
            PopoverEdge::Above,
        ];
        for candidate in candidates {
            let origin = Self::origin_for(source_rect, size, candidate, align, offset, work_area);
            if Self::fits(origin, size, work_area) {
                return Self {
                    origin: Self::clamp_to_work_area(origin, size, work_area),
                    edge: candidate,
                };
            }
        }
        // Nothing fits: clamp preferred origin into the work area.
        let origin = Self::clamp_to_work_area(
            Self::origin_for(source_rect, size, edge, align, offset, work_area),
            size,
            work_area,
        );
        Self { origin, edge }
    }

    const fn opposite(edge: PopoverEdge) -> PopoverEdge {
        match edge {
            PopoverEdge::Below => PopoverEdge::Above,
            PopoverEdge::Above => PopoverEdge::Below,
            PopoverEdge::Right => PopoverEdge::Left,
            PopoverEdge::Left => PopoverEdge::Right,
        }
    }

    fn origin_for(
        source: Rect,
        size: Size,
        edge: PopoverEdge,
        align: PopoverAlign,
        offset: i32,
        bounds: Rect,
    ) -> Point {
        match edge {
            PopoverEdge::Below => Point::new(
                Self::align_x(source, size, align, bounds),
                source.bottom() + offset,
            ),
            PopoverEdge::Above => Point::new(
                Self::align_x(source, size, align, bounds),
                source.y - size.height - offset,
            ),
            PopoverEdge::Right => {
                Point::new(source.right() + offset, Self::align_y(source, size, align))
            }
            PopoverEdge::Left => Point::new(
                source.x - size.width - offset,
                Self::align_y(source, size, align),
            ),
        }
    }

    const fn align_x(source: Rect, size: Size, align: PopoverAlign, bounds: Rect) -> i32 {
        match align {
            PopoverAlign::Start => source.x,
            PopoverAlign::End => source.right() - size.width,
            PopoverAlign::Center => source.x + (source.width - size.width) / 2,
            PopoverAlign::OutputEnd => bounds.right() - size.width,
        }
    }

    const fn align_y(source: Rect, size: Size, align: PopoverAlign) -> i32 {
        match align {
            PopoverAlign::Start => source.y,
            PopoverAlign::End => source.bottom() - size.height,
            PopoverAlign::Center => source.y + (source.height - size.height) / 2,
            PopoverAlign::OutputEnd => source.y,
        }
    }

    fn fits(origin: Point, size: Size, work_area: Rect) -> bool {
        let rect = Rect::from_parts(origin, size);
        origin.x >= work_area.x
            && origin.y >= work_area.y
            && rect.right() <= work_area.right()
            && rect.bottom() <= work_area.bottom()
    }

    fn clamp_to_work_area(origin: Point, size: Size, work_area: Rect) -> Point {
        Rect::from_parts(origin, size)
            .clamp_inside(work_area)
            .origin()
    }
}
