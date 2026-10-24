use super::newest_configure;
use std::collections::VecDeque;

#[test]
fn drag_run_adopts_only_the_newest_size() {
    let mut queue: VecDeque<(u32, u32)> = VecDeque::from([(501, 400), (502, 400), (560, 430)]);
    let newest = newest_configure((500, 400), || queue.pop_front());
    assert_eq!(newest, (560, 430));
    assert!(queue.is_empty(), "every queued configure is consumed");
}

#[test]
fn empty_queue_keeps_the_first_event() {
    let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
    assert_eq!(newest_configure((500, 400), || queue.pop_front()), (500, 400));
}

#[test]
fn run_stops_at_first_foreign_event_and_leaves_the_rest_queued() {
    // `None` models a queue head that is not a ConfigureNotify for this window.
    let mut queue: VecDeque<Option<(u32, u32)>> =
        VecDeque::from([Some((510, 400)), None, Some((999, 999))]);
    let newest = newest_configure((500, 400), || queue.pop_front().flatten());
    assert_eq!(newest, (510, 400));
    assert_eq!(queue.len(), 1, "events after the foreign one are not consumed");
}
