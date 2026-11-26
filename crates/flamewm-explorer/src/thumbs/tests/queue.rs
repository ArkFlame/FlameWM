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
    assert_eq!(
        order,
        (0..4)
            .map(|i| PathBuf::from(format!("img-{i:03}.png")))
            .collect::<Vec<_>>()
    );
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
    assert!(
        cache.begin(&key_at(0)),
        "a dropped key is requestable again"
    );
    assert!(cache.begin(&key_at(2)), "pass 2 re-requests visible row 2");
}

#[test]
fn in_flight_decode_keeps_its_pending_mark_while_its_row_stays_shown() {
    let queue = ThumbQueue::default();
    let mut cache = Thumbnails::default();
    let key = key_at(7);
    assert!(cache.begin(&key));
    queue.push(key.clone());
    // The decoder takes the job: it is no longer queued, so a pass that keeps its row leaves it alone.
    assert_eq!(queue.pop(), key);
    assert_eq!(
        cache.start_pass_keeping(&queue, |candidate| candidate.path == key.path),
        0
    );
    assert!(
        !cache.begin(&key),
        "an in-flight decode is not requested twice"
    );
    cache.complete(key.clone(), None);
    assert!(matches!(cache.lookup(&key), Lookup::Failed));
}

#[test]
fn in_flight_decode_dropped_by_a_pass_frees_its_pending_mark() {
    let queue = ThumbQueue::default();
    let mut cache = Thumbnails::default();
    let key = key_at(8);
    assert!(cache.begin(&key));
    queue.push(key.clone());
    assert_eq!(queue.pop(), key);
    // The row left the view: the worker abandons it and the key is requestable again when it returns.
    assert_eq!(cache.start_pass(&queue), 1);
    assert!(cache.begin(&key), "the cancelled row is requestable again");
}

#[test]
fn pass_start_keeps_queued_jobs_that_stay_visible() {
    let queue = ThumbQueue::default();
    let mut cache = Thumbnails::default();
    for index in 0..4 {
        let key = key_at(index);
        assert!(cache.begin(&key));
        queue.push(key);
    }
    // Pass 2 still shows rows 2 and 3: those jobs stay queued and keep their pending mark, so the
    // pass neither clones their keys again nor requests them twice.
    let kept = [PathBuf::from("img-002.png"), PathBuf::from("img-003.png")];
    assert_eq!(
        cache.start_pass_keeping(&queue, |key| kept.contains(&key.path)),
        2
    );
    assert!(!cache.begin(&key_at(2)), "a kept job stays pending");
    assert_eq!(queue.pop().path, kept[0]);
    assert_eq!(queue.pop().path, kept[1]);
}

#[test]
fn retain_keeps_matching_jobs_in_order() {
    let queue = ThumbQueue::default();
    for index in 0..6 {
        queue.push(key_at(index));
    }
    let dropped = queue.retain(|key| {
        key.path.to_string_lossy().ends_with("0.png")
            || key.path.to_string_lossy().ends_with("2.png")
    });
    assert_eq!(dropped.len(), 4);
    assert_eq!(queue.pop().path, PathBuf::from("img-000.png"));
    assert_eq!(queue.pop().path, PathBuf::from("img-002.png"));
}

#[test]
fn wants_tracks_queued_and_in_flight_jobs() {
    let queue = ThumbQueue::default();
    let (first, second) = (key_at(1), key_at(2));
    queue.push(first.clone());
    queue.push(second.clone());
    assert!(
        queue.wants(&first) && queue.wants(&second),
        "queued jobs are wanted"
    );
    let popped = queue.pop();
    assert!(queue.wants(&popped), "the in-flight job stays wanted");
    queue.release(&popped);
    assert!(!queue.wants(&popped), "a released job is no longer wanted");
    assert!(
        queue.wants(&second),
        "the remaining queued job is still wanted"
    );
}

#[test]
fn pass_cancels_an_in_flight_decode_only_when_its_row_left_the_view() {
    let queue = ThumbQueue::default();
    let key = key_at(3);
    queue.push(key.clone());
    assert_eq!(queue.pop(), key);
    // A later pass still shows the row: the decode keeps running.
    assert_eq!(queue.retain(|candidate| candidate.path == key.path).len(), 0);
    assert!(queue.wants(&key));
    // The row scrolled away: the worker sees the cancel at its next stage check, and the pass
    // reports the abandoned key so its pending mark is freed for a later re-request.
    assert_eq!(queue.retain(|_| false).len(), 1);
    assert!(!queue.wants(&key), "a scrolled-away decode is abandoned");
    queue.release(&key);
}
