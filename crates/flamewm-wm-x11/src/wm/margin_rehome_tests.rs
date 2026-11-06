//! `windowMargin` changes re-home placed frames: a maximized frame follows the live margin, so
//! margin 0 yields the full work area and a change from 2 to 0 leaves no 2 px inset behind.

use super::{PlacementMode, rehome_outer};
use crate::geometry::Rect;

fn global() -> Rect {
    Rect::new(0, 0, 1920, 1080)
}

#[test]
fn maximized_frame_with_margin_zero_is_the_full_work_area() {
    let inset_by_two = Rect::new(2, 2, 1916, 1076);
    let next = rehome_outer(PlacementMode::Maximized, inset_by_two, &[], global(), 0);
    assert_eq!(next, global());
}

#[test]
fn changing_margin_from_two_to_zero_reapplies_to_a_maximized_frame() {
    let full = global();
    let two = rehome_outer(PlacementMode::Maximized, full, &[], global(), 2);
    assert_eq!(two, Rect::new(2, 2, 1916, 1076));
    let zero = rehome_outer(PlacementMode::Maximized, two, &[], global(), 0);
    assert_eq!(
        zero,
        global(),
        "stale 2 px inset must not survive a 0 margin"
    );
}
