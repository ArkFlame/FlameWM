use super::{WAKE_EVENT_BUDGET, wake_paint_due};

#[test]
fn empty_queue_paints_on_the_wake() {
    assert!(wake_paint_due(false, 0, WAKE_EVENT_BUDGET));
}

#[test]
fn queued_events_within_budget_are_drained_before_paint() {
    assert!(!wake_paint_due(true, 0, WAKE_EVENT_BUDGET));
    assert!(!wake_paint_due(true, WAKE_EVENT_BUDGET - 1, WAKE_EVENT_BUDGET));
}

#[test]
fn foreign_event_flood_still_paints_once_budget_is_spent() {
    // A permanently non-empty queue (root substructure events) must not defer the repaint.
    assert!(wake_paint_due(true, WAKE_EVENT_BUDGET, WAKE_EVENT_BUDGET));
    assert!(wake_paint_due(true, WAKE_EVENT_BUDGET + 5, WAKE_EVENT_BUDGET));
}
