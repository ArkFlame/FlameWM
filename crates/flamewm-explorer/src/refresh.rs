//! Live directory refresh: an inotify watch on the visible directory on a helper thread.
//! Bursts of events are coalesced on the trailing edge; `notify` runs once, `QUIET` after the last
//! drained batch, so a burst (archive extraction, large copy) flushes its final event instead of
//! firing once per `QUIET` mid-burst. A burst that never goes quiet (continuous `MODIFY` events)
//! still repaints once per `MAX_WAIT` so the listing never freezes behind it.
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use flamewm_integrations_linux::device_watch::stop_and_join;
use inotify::{Inotify, WatchMask};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

pub const QUIET: Duration = Duration::from_millis(200);

/// Upper bound on how long a burst may defer its refresh. `MASK` includes `MODIFY`, so a large
/// copy into the visible directory emits events continuously; without this cap the listing would
/// not repaint until the copy ended. One refresh per second during such a burst keeps the view
/// live while still coalescing the hundreds of events behind it.
pub const MAX_WAIT: Duration = Duration::from_secs(1);

const MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::ATTRIB)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/// Pure coalescing rule: fire once there is a pending change and the quiet period has elapsed
/// since the last drained batch.
#[must_use]
pub fn should_fire(pending_since: Option<Instant>, now: Instant) -> bool {
    pending_since.is_some_and(|since| now.duration_since(since) >= QUIET)
}

/// Liveness bound for a burst that never goes quiet: fire once the burst has run for `MAX_WAIT`.
#[must_use]
pub fn burst_forced(burst_start: Option<Instant>, now: Instant) -> bool {
    burst_start.is_some_and(|since| now.duration_since(since) >= MAX_WAIT)
}

/// Poll timeout in milliseconds: block indefinitely while idle, otherwise wake at the earlier of
/// the quiet deadline (last batch + `QUIET`) and the burst's max-wait deadline.
fn poll_timeout_ms(
    pending: Option<Instant>,
    burst_start: Option<Instant>,
    now: Instant,
) -> Option<i64> {
    let next = pending
        .map(|since| since + QUIET)
        .into_iter()
        .chain(burst_start.map(|since| since + MAX_WAIT))
        .min()?;
    Some(next.saturating_duration_since(now).as_millis() as i64 + 1)
}

/// Owns the watch thread. The thread blocks in `poll` on the inotify fd and a stop pipe, so an idle
/// watch costs no wakeups. Dropping it writes the stop pipe and joins the thread.
pub struct DirWatch {
    stop_tx: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl DirWatch {
    pub fn spawn(dir: &Path, notify: impl Fn() + Send + 'static) -> io::Result<Self> {
        Self::spawn_counting(dir, notify, None)
    }

    /// `wakeups` counts every return from `poll`; tests use it to prove an idle watch does not tick.
    fn spawn_counting(
        dir: &Path,
        notify: impl Fn() + Send + 'static,
        wakeups: Option<Arc<AtomicUsize>>,
    ) -> io::Result<Self> {
        let mut inotify = Inotify::init()?;
        inotify.watches().add(dir, MASK)?;
        let (stop_rx, stop_tx) = rustix::pipe::pipe().map_err(io::Error::from)?;
        let thread = thread::Builder::new()
            .name("explorer-refresh".into())
            .spawn(move || {
                flamewm_integrations_linux::background::lower_current_thread();
                let mut buffer = [0_u8; 4096];
                // Last drained batch: the quiet deadline counts from here.
                let mut pending: Option<Instant> = None;
                // First batch of the current burst: bounds how long continuous churn may defer.
                let mut burst_start: Option<Instant> = None;
                loop {
                    let now = Instant::now();
                    let timeout = poll_timeout_ms(pending, burst_start, now).map(|ms| Timespec {
                        tv_sec: (ms / 1000) as _,
                        tv_nsec: ((ms % 1000) * 1_000_000) as _,
                    });
                    let mut fds = [
                        PollFd::new(&inotify, PollFlags::IN),
                        PollFd::new(&stop_rx, PollFlags::IN),
                    ];
                    match poll(&mut fds, timeout.as_ref()) {
                        Ok(_) => {}
                        Err(Errno::INTR) => continue,
                        Err(_) => break,
                    }
                    if let Some(wakeups) = &wakeups {
                        wakeups.fetch_add(1, Ordering::Relaxed);
                    }
                    let stop_ready = fds[1]
                        .revents()
                        .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR);
                    let inotify_ready = fds[0].revents().intersects(PollFlags::IN);
                    if stop_ready {
                        break;
                    }
                    if inotify_ready {
                        // Drain the whole kernel queue per wakeup instead of one 4 KiB buffer:
                        // a burst larger than the buffer used to return to `poll` for each chunk,
                        // rebuilding the poll set and re-reading the clock every time.
                        let mut had_events = false;
                        loop {
                            match inotify.read_events(&mut buffer) {
                                Ok(events) => {
                                    if events.into_iter().next().is_some() {
                                        had_events = true;
                                    } else {
                                        break;
                                    }
                                }
                                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                                Err(_) => return,
                            }
                        }
                        if had_events {
                            // Trailing edge: restart the quiet window on every drained batch, so
                            // a burst still longer than `QUIET` settles into one refresh after it
                            // ends rather than firing every `QUIET` while it runs. Only drained
                            // batches move the deadline; the final event is never lost.
                            let now = Instant::now();
                            pending = Some(now);
                            burst_start.get_or_insert(now);
                        }
                    }
                    let now = Instant::now();
                    if should_fire(pending, now) || burst_forced(burst_start, now) {
                        pending = None;
                        burst_start = None;
                        notify();
                    }
                }
            })?;
        Ok(Self {
            stop_tx,
            thread: Some(thread),
        })
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        stop_and_join(&self.stop_tx, self.thread.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;

    #[test]
    fn coalescing_waits_for_quiet_period() {
        let t0 = Instant::now();
        assert!(!should_fire(None, t0 + Duration::from_secs(5)));
        assert!(!should_fire(Some(t0), t0 + Duration::from_millis(50)));
        assert!(should_fire(Some(t0), t0 + QUIET));
    }

    #[test]
    fn a_burst_that_never_goes_quiet_repaints_every_max_wait() {
        let t0 = Instant::now();
        assert!(!burst_forced(None, t0 + Duration::from_secs(5)));
        assert!(!burst_forced(
            Some(t0),
            t0 + MAX_WAIT - Duration::from_millis(1)
        ));
        assert!(burst_forced(Some(t0), t0 + MAX_WAIT));
        // Max-wait counts from the first batch of the burst, so continuous `MODIFY` events
        // (a large copy) still repaint instead of deferring until the copy ends.
        let last_batch = t0 + Duration::from_millis(900);
        assert_eq!(
            poll_timeout_ms(Some(last_batch), Some(t0), last_batch),
            Some(101),
            "quiet is 200 ms away but max-wait only 100 ms, so the earlier deadline wins"
        );
        assert_eq!(
            poll_timeout_ms(None, None, t0),
            None,
            "an idle watch blocks with no timeout"
        );
    }

    #[test]
    fn creating_a_file_triggers_one_refresh_callback() {
        let dir = std::env::temp_dir().join(format!("flamewm-refresh-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let watch = DirWatch::spawn(&dir, move || {
            let _ = tx.send(());
        })
        .unwrap();
        fs::write(dir.join("new.txt"), b"hello").unwrap();
        rx.recv_timeout(Duration::from_secs(5))
            .expect("refresh callback should fire after a file is created");
        drop(watch);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn idle_watch_does_not_wake_and_drop_returns_promptly() {
        let dir = std::env::temp_dir().join(format!("flamewm-refresh-idle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let wakeups = Arc::new(AtomicUsize::new(0));
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_in = Arc::clone(&fired);
        let watch = DirWatch::spawn_counting(
            &dir,
            move || {
                fired_in.fetch_add(1, Ordering::Relaxed);
            },
            Some(Arc::clone(&wakeups)),
        )
        .unwrap();
        // The old 50 ms poll loop woke about six times in this window; a blocked poll wakes zero times.
        thread::sleep(Duration::from_millis(300));
        assert_eq!(
            wakeups.load(Ordering::Relaxed),
            0,
            "idle watch must block, not poll"
        );
        assert_eq!(fired.load(Ordering::Relaxed), 0);
        let started = Instant::now();
        drop(watch);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "drop must wake the blocked thread"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_burst_of_writes_fires_one_callback() {
        let dir =
            std::env::temp_dir().join(format!("flamewm-refresh-burst-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_in = Arc::clone(&fired);
        let watch = DirWatch::spawn(&dir, move || {
            fired_in.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        for index in 0..20 {
            fs::write(dir.join(format!("f{index}.txt")), b"x").unwrap();
        }
        thread::sleep(QUIET * 4);
        assert_eq!(
            fired.load(Ordering::Relaxed),
            1,
            "a burst inside the quiet period is one refresh"
        );
        drop(watch);
        let _ = fs::remove_dir_all(&dir);
    }
}
