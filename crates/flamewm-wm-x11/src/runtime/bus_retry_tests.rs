use super::*;

#[test]
fn failed_connects_back_off_and_cap() {
    let start = Instant::now();
    let mut retry = BusRetry::new();
    retry.failed(start);
    assert!(!retry.due(start));
    assert!(retry.due(start + BUS_RETRY_MIN));
    let mut now = start;
    for _ in 0..10 {
        retry.failed(now);
        now += BUS_RETRY_MAX;
    }
    assert_eq!(retry.delay, BUS_RETRY_MAX);
}

#[test]
fn reset_after_connect_is_immediately_due_at_minimum_delay() {
    let start = Instant::now();
    let mut retry = BusRetry::new();
    retry.failed(start);
    retry.failed(start);
    retry.reset(start);
    assert_eq!(retry.delay, BUS_RETRY_MIN);
    assert!(retry.due(start));
}
