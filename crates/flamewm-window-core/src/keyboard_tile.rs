//! Keyboard tiling transitions (Super+arrows). Pure: maps the current snap state and an arrow to
//! the next state. `None` as the result of [`Direction::Down`] means "restore the saved geometry".

use crate::SnapTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Next tile state for `direction` from `current`.
///
/// Quarters are reached from a half on the same side (Up/Down) or by moving sideways out of a
/// quarter. Left/Right from any other state lands on the half for that side. Down from a
/// maximized or untiled window returns `None`, meaning restore.
#[must_use]
pub fn next_tile(current: SnapTarget, direction: Direction) -> Option<SnapTarget> {
    use SnapTarget::{
        BottomHalf, BottomLeftQuarter, BottomRightQuarter, LeftHalf, Maximize, None, RightHalf,
        TopHalf, TopLeftQuarter, TopRightQuarter,
    };
    Some(match direction {
        Direction::Left => match current {
            TopRightQuarter => TopLeftQuarter,
            BottomRightQuarter => BottomLeftQuarter,
            _ => LeftHalf,
        },
        Direction::Right => match current {
            TopLeftQuarter => TopRightQuarter,
            BottomLeftQuarter => BottomRightQuarter,
            _ => RightHalf,
        },
        Direction::Up => match current {
            BottomLeftQuarter => TopLeftQuarter,
            BottomRightQuarter => TopRightQuarter,
            BottomHalf => TopHalf,
            LeftHalf => TopLeftQuarter,
            RightHalf => TopRightQuarter,
            _ => Maximize,
        },
        Direction::Down => match current {
            TopLeftQuarter | LeftHalf => BottomLeftQuarter,
            TopRightQuarter | RightHalf => BottomRightQuarter,
            TopHalf => BottomHalf,
            BottomLeftQuarter | BottomRightQuarter | BottomHalf => return Option::None,
            Maximize | None => return Option::None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap_geometry;
    use flamewm_api::{Rect, Size};

    #[test]
    fn left_and_right_from_plain_states_land_on_halves() {
        assert_eq!(
            next_tile(SnapTarget::None, Direction::Left),
            Some(SnapTarget::LeftHalf)
        );
        assert_eq!(
            next_tile(SnapTarget::Maximize, Direction::Right),
            Some(SnapTarget::RightHalf)
        );
        assert_eq!(
            next_tile(SnapTarget::TopHalf, Direction::Left),
            Some(SnapTarget::LeftHalf)
        );
    }

    #[test]
    fn sideways_moves_from_quarters_stay_on_the_same_row() {
        assert_eq!(
            next_tile(SnapTarget::TopRightQuarter, Direction::Left),
            Some(SnapTarget::TopLeftQuarter)
        );
        assert_eq!(
            next_tile(SnapTarget::BottomLeftQuarter, Direction::Right),
            Some(SnapTarget::BottomRightQuarter)
        );
    }

    #[test]
    fn up_and_down_move_between_rows_of_a_half() {
        assert_eq!(
            next_tile(SnapTarget::LeftHalf, Direction::Up),
            Some(SnapTarget::TopLeftQuarter)
        );
        assert_eq!(
            next_tile(SnapTarget::LeftHalf, Direction::Down),
            Some(SnapTarget::BottomLeftQuarter)
        );
        assert_eq!(
            next_tile(SnapTarget::TopHalf, Direction::Down),
            Some(SnapTarget::BottomHalf)
        );
        assert_eq!(
            next_tile(SnapTarget::BottomHalf, Direction::Up),
            Some(SnapTarget::TopHalf)
        );
        assert_eq!(
            next_tile(SnapTarget::None, Direction::Up),
            Some(SnapTarget::Maximize)
        );
    }

    #[test]
    fn down_from_untiled_or_bottom_restores() {
        assert_eq!(
            next_tile(SnapTarget::Maximize, Direction::Down),
            Option::None
        );
        assert_eq!(next_tile(SnapTarget::None, Direction::Down), Option::None);
        assert_eq!(
            next_tile(SnapTarget::BottomLeftQuarter, Direction::Down),
            Option::None
        );
    }

    fn quarter_tiles(work: Rect) -> [Rect; 4] {
        let current = Size::new(400, 300);
        [
            SnapTarget::TopLeftQuarter,
            SnapTarget::TopRightQuarter,
            SnapTarget::BottomLeftQuarter,
            SnapTarget::BottomRightQuarter,
        ]
        .map(|t| snap_geometry(t, work, current))
    }

    #[test]
    fn quarter_tiles_exactly_cover_an_even_work_area() {
        let work = Rect::new(4, 6, 1000, 800);
        let rects = quarter_tiles(work);
        let area: i64 = rects
            .iter()
            .map(|r| i64::from(r.width) * i64::from(r.height))
            .sum();
        assert_eq!(area, i64::from(work.width) * i64::from(work.height));
    }

    #[test]
    fn quarter_tiles_on_an_odd_work_area_do_not_overlap_and_leave_at_most_a_one_px_seam() {
        // Matches the committed geometry: the right/bottom tiles start at `size - floor(size/2)`,
        // so an odd extent leaves a single-pixel seam per axis rather than an overlap.
        let work = Rect::new(3, 5, 1001, 801);
        let rects = quarter_tiles(work);
        for (i, a) in rects.iter().enumerate() {
            assert!(a.x >= work.x && a.y >= work.y);
            assert!(a.right() <= work.right() && a.bottom() <= work.bottom());
            for b in &rects[i + 1..] {
                let overlap =
                    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom();
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
        let uncovered = i64::from(work.width) * i64::from(work.height)
            - rects
                .iter()
                .map(|r| i64::from(r.width) * i64::from(r.height))
                .sum::<i64>();
        assert_eq!(uncovered, 1001 + 801 - 1, "one column plus one row of seam");
    }

    #[test]
    fn arrow_cycle_reaches_every_quarter() {
        let mut state = SnapTarget::None;
        state = next_tile(state, Direction::Left).unwrap();
        assert_eq!(state, SnapTarget::LeftHalf);
        state = next_tile(state, Direction::Up).unwrap();
        assert_eq!(state, SnapTarget::TopLeftQuarter);
        state = next_tile(state, Direction::Right).unwrap();
        assert_eq!(state, SnapTarget::TopRightQuarter);
        state = next_tile(state, Direction::Down).unwrap();
        assert_eq!(state, SnapTarget::BottomRightQuarter);
        assert_eq!(
            next_tile(state, Direction::Left),
            Some(SnapTarget::BottomLeftQuarter)
        );
    }
}
