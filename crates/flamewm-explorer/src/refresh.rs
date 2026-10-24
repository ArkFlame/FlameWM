//! Live directory refresh: an inotify watch on the visible directory on a helper thread.
//! Bursts of events are coalesced; `notify` runs once the directory has been quiet for `QUIET`.
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use inotify::{Inotify, WatchMask};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;

pub const QUIET: Duration = Duration::from_millis(200);

const MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::ATTRIB)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/// Pure coalescing rule: fire once there is a pending change and the quiet period has elapsed.
pub fn should_fire(pending_since: Option<Instant>, now: Instant) -> bool {
    pending_since.is_some_and(|since| now.duration_since(since) >= QUIET)
}

/// Poll timeout in milliseconds: block indefinitely while idle, otherwise wake when the quiet period ends.
fn poll_timeout_ms(pending: Option<Instant>, now: Instant) -> Option<i64> {
    pending.map(|since| QUIET.saturating_sub(now.duration_since(since)).as_millis() as i64 + 1)
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
        let thread = thread::Builder::new().name("explorer-refresh".into()).spawn(move || {
            let mut buffer = [0_u8; 4096];
            let mut pending: Option<Instant> = None;
            loop {
                let timeout = poll_timeout_ms(pending, Instant::now()).map(|ms| Timespec {
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
                let stop_ready = fds[1].revents().intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR);
                let inotify_ready = fds[0].revents().intersects(PollFlags::IN);
                if stop_ready {
                    break;
                }
                if inotify_ready {
                    match inotify.read_events(&mut buffer) {
                        Ok(events) => {
                            if events.into_iter().next().is_some() {
                                pending.get_or_insert_with(Instant::now);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => break,
                    }
                }
                if should_fire(pending, Instant::now()) {
                    pending = None;
                    notify();
                }
            }
        })?;
        Ok(Self { stop_tx, thread: Some(thread) })
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        let _ = rustix::io::write(&self.stop_tx, &[1]);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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
        rx.recv_timeout(Duration::from_secs(5)).expect("refresh callback should fire after a file is created");
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
        assert_eq!(wakeups.load(Ordering::Relaxed), 0, "idle watch must block, not poll");
        assert_eq!(fired.load(Ordering::Relaxed), 0);
        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(1), "drop must wake the blocked thread");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_burst_of_writes_fires_one_callback() {
        let dir = std::env::temp_dir().join(format!("flamewm-refresh-burst-{}", std::process::id()));
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
        assert_eq!(fired.load(Ordering::Relaxed), 1, "a burst inside the quiet period is one refresh");
        drop(watch);
        let _ = fs::remove_dir_all(&dir);
    }
}
