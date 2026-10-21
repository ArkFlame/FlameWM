//! Live directory refresh: an inotify watch on the visible directory on a helper thread.
//! Bursts of events are coalesced; `notify` runs once the directory has been quiet for `QUIET`.
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use inotify::{Inotify, WatchMask};

pub const QUIET: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(50);

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

/// Owns the watch thread. Dropping it stops the thread within one poll interval.
pub struct DirWatch {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DirWatch {
    pub fn spawn(dir: &Path, notify: impl Fn() + Send + 'static) -> io::Result<Self> {
        let mut inotify = Inotify::init()?;
        inotify.watches().add(dir, MASK)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new().name("explorer-refresh".into()).spawn(move || {
            let mut buffer = [0_u8; 4096];
            let mut pending: Option<Instant> = None;
            while !thread_stop.load(Ordering::Relaxed) {
                match inotify.read_events(&mut buffer) {
                    Ok(events) => {
                        if events.into_iter().next().is_some() {
                            pending.get_or_insert_with(Instant::now);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => break,
                }
                if should_fire(pending, Instant::now()) {
                    pending = None;
                    notify();
                }
                thread::sleep(POLL);
            }
        })?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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
}
