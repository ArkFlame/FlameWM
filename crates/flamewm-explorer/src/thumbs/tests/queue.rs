//! Queue ordering and pass-start cancellation for the thumbnail handoff.
use super::*;

fn key_at(index: usize) -> ThumbKey {
    key(&format!("img-{index:03}.png"), index as u64)
}

#[test]
fn pop_is_fifo_for_one_pass() {
    let queue = ThumbQueue::default();
    for index in 0..4 {
        queue.push(key_at(index));
    }
    let order: Vec<PathBuf> = (0..4).map(|_| queue.pop().path).collect();
    assert_eq!(order, (0..4).map(|i| PathBuf::from(format!("img-{i:03}.png"))).collect::<Vec<_>>());
}

#[test]
fn pass_start_drops_queued_jobs_and_frees_their_keys() {
    let queue = ThumbQueue::default();
    let mut cache = Thumbnails::default();
    // Pass 1 shows rows 0..3; nothing has decoded yet, so all four are queued.
    for index in 0..4 {
        let key = key_at(index);
        assert!(cache.begin(&key));
        queue.push(key);
    }
    // Scroll: pass 2 shows rows 2..5. Rows 0 and 1 leave the view; their queued jobs must not decode.
    assert_eq!(cache.start_pass(&queue), 4);
    // The queue is empty and no stale key is pending, so a visible row can request again.
    assert!(queue.retain(|_| true).is_empty());
    assert!(cache.begin(&key_at(0)), "a dropped key is requestable again");
    assert!(cache.begin(&key_at(2)), "pass 2 re-requests visible row 2");
}

#[test]
fn in_flight_decode_keeps_its_pending_mark_until_complete() {
    let queue = ThumbQueue::default();
    let mut cache = Thumbnails::default();
    let key = key_at(7);
    assert!(cache.begin(&key));
    queue.push(key.clone());
    // The decoder takes the job: it is no longer queued, so a pass start does not touch it.
    assert_eq!(queue.pop(), key);
    assert_eq!(cache.start_pass(&queue), 0);
    assert!(!cache.begin(&key), "an in-flight decode is not requested twice");
    cache.complete(key.clone(), None);
    assert!(matches!(cache.lookup(&key), Lookup::Failed));
}

#[test]
fn retain_keeps_matching_jobs_in_order() {
    let queue = ThumbQueue::default();
    for index in 0..6 {
        queue.push(key_at(index));
    }
    let dropped = queue.retain(|key| key.path.to_string_lossy().ends_with("0.png") || key.path.to_string_lossy().ends_with("2.png"));
    assert_eq!(dropped.len(), 4);
    assert_eq!(queue.pop().path, PathBuf::from("img-000.png"));
    assert_eq!(queue.pop().path, PathBuf::from("img-002.png"));
}
