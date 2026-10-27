//! Clip-rect math for the paint stack, in device pixels.
//!
//! The XRender and Xft draws do not inherit the GC clip, so each primitive is
//! clipped here against the retained `PushClip` stack before it is drawn.

use flamewm_render_core::Rect;

/// Visible part of one primitive under the active clip stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Visible {
    /// Entirely inside every clip: draw unchanged.
    Whole,
    /// Entirely outside at least one clip: draw nothing.
    Hidden,
    /// Cut by a clip: draw only this device rect.
    Part(Rect),
}

/// Overlap of two rects, or `None` when they do not share area.
pub(crate) fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.width).min(b.x + b.width);
    let y2 = (a.y + a.height).min(b.y + b.height);
    (x2 > x1 && y2 > y1).then_some(Rect {
        x: x1,
        y: y1,
        width: x2 - x1,
        height: y2 - y1,
    })
}

/// Intersection of every clip in the stack; `None` when the stack is empty
/// or the clips share no area.
pub(crate) fn stack_bounds(stack: &[Rect]) -> Option<Rect> {
    let mut iter = stack.iter().copied();
    let first = iter.next()?;
    iter.try_fold(first, intersect)
}

/// Classifies `rect` against the clip stack. An empty stack leaves it whole.
pub(crate) fn visible_under(rect: Rect, stack: &[Rect]) -> Visible {
    if stack.is_empty() {
        return Visible::Whole;
    }
    let Some(bounds) = stack_bounds(stack) else {
        return Visible::Hidden;
    };
    match intersect(rect, bounds) {
        None => Visible::Hidden,
        Some(part) if part == rect => Visible::Whole,
        Some(part) => Visible::Part(part),
    }
}

/// The four edge bands of a `width`-thick border inside `rect`: top, bottom, left, right.
/// Bands overlap at the corners only when `width` exceeds half the smaller side.
#[cfg(test)]
pub(crate) fn border_bands(rect: Rect, width: f32) -> [Rect; 4] {
    let w = width.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    let inner_h = (rect.height - 2.0 * w).max(0.0);
    [
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: w,
        },
        Rect {
            x: rect.x,
            y: rect.y + rect.height - w,
            width: rect.width,
            height: w,
        },
        Rect {
            x: rect.x,
            y: rect.y + w,
            width: w,
            height: inner_h,
        },
        Rect {
            x: rect.x + rect.width - w,
            y: rect.y + w,
            width: w,
            height: inner_h,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn intersect_returns_overlap_and_none_for_disjoint_or_touching() {
        assert_eq!(
            intersect(r(0.0, 0.0, 10.0, 10.0), r(5.0, 2.0, 10.0, 4.0)),
            Some(r(5.0, 2.0, 5.0, 4.0))
        );
        assert_eq!(
            intersect(r(0.0, 0.0, 10.0, 10.0), r(10.0, 0.0, 5.0, 5.0)),
            None,
            "edge contact has no area"
        );
        assert_eq!(
            intersect(r(0.0, 0.0, 10.0, 10.0), r(20.0, 20.0, 1.0, 1.0)),
            None
        );
    }

    #[test]
    fn stack_bounds_is_the_nested_intersection() {
        let stack = [
            r(0.0, 0.0, 100.0, 100.0),
            r(10.0, 10.0, 50.0, 80.0),
            r(0.0, 40.0, 200.0, 200.0),
        ];
        assert_eq!(stack_bounds(&stack), Some(r(10.0, 40.0, 50.0, 50.0)));
        assert_eq!(stack_bounds(&[]), None);
        assert_eq!(
            stack_bounds(&[r(0.0, 0.0, 5.0, 5.0), r(9.0, 9.0, 5.0, 5.0)]),
            None
        );
    }

    #[test]
    fn visible_under_classifies_whole_part_and_hidden() {
        let clip = [r(0.0, 0.0, 100.0, 100.0), r(0.0, 0.0, 100.0, 90.0)];
        assert_eq!(
            visible_under(r(10.0, 10.0, 20.0, 20.0), &clip),
            Visible::Whole
        );
        assert_eq!(
            visible_under(r(10.0, 85.0, 20.0, 20.0), &clip),
            Visible::Part(r(10.0, 85.0, 20.0, 5.0))
        );
        assert_eq!(
            visible_under(r(10.0, 95.0, 20.0, 5.0), &clip),
            Visible::Hidden
        );
        assert_eq!(
            visible_under(r(10.0, 95.0, 20.0, 5.0), &[]),
            Visible::Whole,
            "no clip leaves it whole"
        );
    }

    #[test]
    fn border_bands_tile_the_ring_without_covering_the_interior() {
        let bands = border_bands(r(0.0, 0.0, 10.0, 8.0), 1.0);
        assert_eq!(bands[0], r(0.0, 0.0, 10.0, 1.0));
        assert_eq!(bands[1], r(0.0, 7.0, 10.0, 1.0));
        assert_eq!(bands[2], r(0.0, 1.0, 1.0, 6.0));
        assert_eq!(bands[3], r(9.0, 1.0, 1.0, 6.0));
        let area: f32 = bands.iter().map(|b| b.width * b.height).sum();
        assert_eq!(
            area,
            10.0 * 8.0 - 8.0 * 6.0,
            "ring area equals outer minus inner"
        );
    }
}
