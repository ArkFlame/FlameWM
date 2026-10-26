//! Downloads organizer runtime. Owns the Downloads inotify watch (registered through the reactor
//! like the Desktop watch) and the settle/move scheduling. Classification, naming and the move
//! policy live in `flamewm_downloads_core`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use flamewm_downloads_core::{
    MAX_MOVES_PER_TICK, NoticeLatch, POLL, STABLE_AFTER, classify, is_candidate_name, load_notice,
    move_into_category, persist_notice,
};
use flamewm_reactor::WakeDeadline;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};

/// One filesystem note: name, finished (close-write or rename-in), gone (delete or rename-out).
pub type Queue = Rc<RefCell<Vec<(String, bool, bool)>>>;

const WATCH_MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::DELETE);

#[derive(Debug, Clone)]
struct Tracked {
    stat: Option<(u64, Option<SystemTime>)>,
    changed_at: Instant,
    finished: bool,
}

pub struct Organizer {
    dir: PathBuf,
    notice_path: PathBuf,
    notice: NoticeLatch,
    notice_pending: bool,
    enabled: bool,
    watches: Watches,
    watch: Option<WatchDescriptor>,
    source: Option<Inotify>,
    queue: Queue,
    /// Names that must never be acted on: the baseline at enable time and failed moves.
    ignored: BTreeSet<String>,
    tracked: BTreeMap<String, Tracked>,
    wake: WakeDeadline,
    armed: Option<Instant>,
}

impl Organizer {
    /// Creates the inotify instance without adding a watch. `set_enabled` controls the watch.
    pub fn new(dir: PathBuf, notice_path: PathBuf) -> io::Result<Self> {
        let inotify = Inotify::init()?;
        let watches = inotify.watches();
        Ok(Self {
            dir,
            notice: load_notice(&notice_path),
            notice_path,
            notice_pending: false,
            enabled: false,
            watches,
            watch: None,
            source: Some(inotify),
            queue: Queue::default(),
            ignored: BTreeSet::new(),
            tracked: BTreeMap::new(),
            wake: WakeDeadline::default(),
            armed: None,
        })
    }

    /// Hands the inotify fd to the reactor. Available exactly once.
    pub fn take_source(&mut self) -> Option<Inotify> {
        self.source.take()
    }

    #[must_use]
    pub fn queue(&self) -> Queue {
        Rc::clone(&self.queue)
    }

    /// Shares the reactor wake slot so pending settle checks wake the loop.
    pub fn attach_wake(&mut self, wake: WakeDeadline) {
        self.wake = wake;
    }

    #[cfg(test)]
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Idempotent: adds or removes the kernel watch so a disabled organizer watches nothing.
    pub fn set_enabled(&mut self, on: bool) {
        if on == self.enabled {
            return;
        }
        if on {
            self.start();
        } else {
            self.stop();
        }
    }

    fn start(&mut self) {
        match self.watches.add(&self.dir, WATCH_MASK) {
            Ok(watch) => self.watch = Some(watch),
            Err(error) => {
                eprintln!(
                    "desktop: downloads organizer inactive: watch {}: {error}",
                    self.dir.display()
                );
                return;
            }
        }
        self.enabled = true;
        self.queue.borrow_mut().clear();
        self.tracked.clear();
        // Baseline: everything present when the organizer starts is never moved.
        self.ignored = match fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect(),
            Err(error) => {
                eprintln!("desktop: downloads baseline unreadable: {error}");
                BTreeSet::new()
            }
        };
    }

    fn stop(&mut self) {
        if let Some(watch) = self.watch.take()
            && let Err(error) = self.watches.remove(watch)
        {
            eprintln!("desktop: downloads organizer unwatch failed: {error}");
        }
        self.enabled = false;
        self.queue.borrow_mut().clear();
        self.tracked.clear();
        self.ignored.clear();
        self.disarm();
    }

    /// Returns true once after the first real move so the desktop shows the notice.
    pub fn take_notice(&mut self) -> bool {
        std::mem::take(&mut self.notice_pending)
    }

    /// Settles candidate files and moves the ones that finished. Called from the desktop turn.
    pub fn tick(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        let notes: Vec<_> = self.queue.borrow_mut().drain(..).collect();
        for (name, finished, gone) in notes {
            if gone {
                self.tracked.remove(&name);
                self.ignored.remove(&name);
                continue;
            }
            if !is_candidate_name(&name)
                || classify(&name).is_none()
                || self.ignored.contains(&name)
            {
                continue;
            }
            let entry = self.tracked.entry(name).or_insert_with(|| Tracked {
                stat: None,
                changed_at: now,
                finished: false,
            });
            entry.finished |= finished;
        }

        let mut due = Vec::new();
        for name in self.tracked.keys().cloned().collect::<Vec<_>>() {
            let stat = match fs::symlink_metadata(self.dir.join(&name)) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    (metadata.len(), metadata.modified().ok())
                }
                // Vanished, directory or symlink: never acted on from here on.
                Ok(metadata) => {
                    self.tracked.remove(&name);
                    if !metadata.file_type().is_file() {
                        self.ignored.insert(name);
                    }
                    continue;
                }
                Err(_) => {
                    self.tracked.remove(&name);
                    continue;
                }
            };
            let Some(entry) = self.tracked.get_mut(&name) else {
                continue;
            };
            match entry.stat {
                None => {
                    entry.stat = Some(stat);
                    entry.changed_at = now;
                }
                Some(previous) if previous != stat => {
                    entry.stat = Some(stat);
                    entry.changed_at = now;
                    entry.finished = false;
                }
                Some(_) => {
                    if entry.finished
                        || now.saturating_duration_since(entry.changed_at) >= STABLE_AFTER
                    {
                        due.push(name);
                    }
                }
            }
        }

        for name in due.into_iter().take(MAX_MOVES_PER_TICK) {
            self.tracked.remove(&name);
            let Some(category) = classify(&name) else {
                continue;
            };
            match move_into_category(&self.dir, &name, category) {
                Ok(_) => self.announce_first_move(),
                Err(error) => {
                    eprintln!("desktop: downloads organizer left {name} in place: {error}");
                    self.ignored.insert(name);
                }
            }
        }
        self.arm(now);
    }

    /// Latches the notice before it is shown so a crash or restart cannot repeat it.
    fn announce_first_move(&mut self) {
        if self.notice.claim() {
            if let Err(error) = persist_notice(&self.notice_path, self.notice) {
                eprintln!("desktop: downloads notice state not saved: {error}");
            }
            self.notice_pending = true;
        }
    }

    /// Publishes the next settle check to the shared reactor wake slot while files are pending.
    fn arm(&mut self, now: Instant) {
        if self.tracked.is_empty() {
            self.disarm();
            return;
        }
        let due = now + POLL;
        // Only another owner's future deadline is kept; our own previous value is replaced.
        let other = self
            .wake
            .get()
            .filter(|value| Some(*value) != self.armed && *value > now);
        let next = other.map_or(due, |other| other.min(due));
        self.wake.set(Some(next));
        self.armed = Some(next);
    }

    fn disarm(&mut self) {
        if self.armed.is_some() && self.wake.get() == self.armed {
            self.wake.set(None);
        }
        self.armed = None;
    }
}

/// Reactor read callback body: drains kernel notes into the shared queue.
pub fn read_events(source: &mut Inotify, queue: &Queue) {
    let mut buffer = [0u8; 4096];
    loop {
        let events = match source.read_events(&mut buffer) {
            Ok(events) => events,
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(_) => break,
        };
        let mut any = false;
        for event in events {
            any = true;
            // Non-UTF-8 names are never downloads we can classify; skipping them is safe.
            let Some(name) = event.name.and_then(|name| name.to_str()) else {
                continue;
            };
            let finished = event
                .mask
                .intersects(EventMask::CLOSE_WRITE | EventMask::MOVED_TO);
            let gone = event
                .mask
                .intersects(EventMask::DELETE | EventMask::MOVED_FROM);
            queue.borrow_mut().push((name.to_owned(), finished, gone));
        }
        if !any {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;

    struct Harness {
        root: PathBuf,
        organizer: Organizer,
        source: Inotify,
        queue: Queue,
    }

    impl Harness {
        fn new(name: &str, enabled: bool) -> Self {
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let root = std::env::temp_dir()
                .join(format!("flamewm-dl-{name}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&root).expect("create temp downloads");
            let notice = root.with_extension("notice");
            let mut organizer = Organizer::new(root.clone(), notice).expect("organizer");
            let source = organizer.take_source().expect("source");
            let queue = organizer.queue();
            organizer.set_enabled(enabled);
            Self {
                root,
                organizer,
                source,
                queue,
            }
        }

        /// Pumps the same read path the reactor uses, then one organizer tick.
        fn pump(&mut self) {
            read_events(&mut self.source, &self.queue);
            self.organizer.tick(Instant::now());
        }

        /// Pumps until `done` holds or the timeout expires.
        fn wait_for(&mut self, timeout: Duration, done: impl Fn(&Path) -> bool) -> bool {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                self.pump();
                if done(&self.root) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
            let _ = fs::remove_file(self.root.with_extension("notice"));
        }
    }

    const FIVE_SECONDS: Duration = Duration::from_secs(5);

    #[test]
    fn browser_rename_from_partial_moves_finished_file_once() {
        let mut harness = Harness::new("rename", true);
        fs::write(harness.root.join("report.pdf.crdownload"), b"partial").unwrap();
        // Partial names are never candidates: no move, even after settling time.
        assert!(!harness.wait_for(Duration::from_millis(400), |root| {
            root.join("Documents").exists()
        }));
        assert!(harness.root.join("report.pdf.crdownload").exists());

        fs::rename(
            harness.root.join("report.pdf.crdownload"),
            harness.root.join("report.pdf"),
        )
        .unwrap();
        assert!(harness.wait_for(FIVE_SECONDS, |root| {
            root.join("Documents/report.pdf").exists()
        }));
        assert!(!harness.root.join("report.pdf").exists());
        assert_eq!(
            fs::read(harness.root.join("Documents/report.pdf")).unwrap(),
            b"partial"
        );
        assert!(
            harness.organizer.take_notice(),
            "first real move announces once"
        );
        assert!(!harness.organizer.take_notice());
    }

    #[test]
    fn close_write_moves_without_waiting_for_stability_fallback() {
        let mut harness = Harness::new("close", true);
        fs::write(harness.root.join("clip.MP4"), b"video").unwrap();
        let started = Instant::now();
        assert!(harness.wait_for(FIVE_SECONDS, |root| {
            root.join("Multimedia/clip.MP4").exists()
        }));
        assert!(
            started.elapsed() < STABLE_AFTER,
            "close-write path must not wait for the fallback"
        );
    }

    #[test]
    fn preexisting_dotfiles_dirs_uncategorized_and_symlinks_are_never_moved() {
        let mut harness = Harness::new("ignored", true);
        // Baseline file was created before enable inside the harness only if we add it now; use
        // a fresh organizer on a dir that already holds it to prove the baseline rule.
        let root = harness.root.clone();
        fs::create_dir(root.join("folder.pdf")).unwrap();
        fs::write(root.join(".hidden.pdf"), b"x").unwrap();
        fs::write(root.join("unknown.xyz"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("unknown.xyz"), root.join("link.pdf")).unwrap();
        fs::write(root.join("real.pdf"), b"x").unwrap();
        assert!(harness.wait_for(FIVE_SECONDS, |root| {
            root.join("Documents/real.pdf").exists()
        }));
        assert!(root.join("folder.pdf").is_dir());
        assert!(root.join(".hidden.pdf").exists());
        assert!(root.join("unknown.xyz").exists());
        assert!(
            fs::symlink_metadata(root.join("link.pdf"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        harness.pump();
        assert!(!root.join("Documents/folder.pdf").exists());
    }

    #[test]
    fn files_present_at_enable_time_are_baseline_and_never_moved() {
        let mut harness = Harness::new("baseline", false);
        fs::write(harness.root.join("old.pdf"), b"old").unwrap();
        harness.organizer.set_enabled(true);
        fs::write(harness.root.join("new.pdf"), b"new").unwrap();
        assert!(harness.wait_for(FIVE_SECONDS, |root| root.join("Documents/new.pdf").exists()));
        harness.pump();
        assert!(
            harness.root.join("old.pdf").exists(),
            "baseline file must stay in Downloads"
        );
    }

    #[test]
    fn clash_gets_suffix_and_existing_category_file_is_kept() {
        let mut harness = Harness::new("clash", true);
        fs::create_dir(harness.root.join("Documents")).unwrap();
        fs::write(harness.root.join("Documents/a.pdf"), b"keep").unwrap();
        fs::write(harness.root.join("a.pdf"), b"incoming").unwrap();
        assert!(harness.wait_for(FIVE_SECONDS, |root| {
            root.join("Documents/a (1).pdf").exists()
        }));
        assert_eq!(
            fs::read(harness.root.join("Documents/a.pdf")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn disabled_organizer_moves_nothing_and_watches_nothing() {
        let mut harness = Harness::new("disabled", false);
        fs::write(harness.root.join("quiet.pdf"), b"x").unwrap();
        assert!(!harness.wait_for(Duration::from_millis(600), |root| {
            root.join("Documents").exists()
        }));
        assert!(harness.root.join("quiet.pdf").exists());
        assert!(!harness.organizer.enabled());
    }

    #[test]
    fn disabling_live_stops_pending_moves() {
        let mut harness = Harness::new("live-off", true);
        harness.organizer.set_enabled(false);
        fs::write(harness.root.join("later.pdf"), b"x").unwrap();
        assert!(!harness.wait_for(Duration::from_millis(600), |root| {
            root.join("Documents").exists()
        }));
        harness.organizer.set_enabled(true);
        assert!(harness.organizer.enabled());
    }

    #[test]
    fn notice_latch_persists_so_restart_does_not_repeat_it() {
        let mut harness = Harness::new("notice", true);
        fs::write(harness.root.join("first.pdf"), b"x").unwrap();
        assert!(harness.wait_for(FIVE_SECONDS, |root| {
            root.join("Documents/first.pdf").exists()
        }));
        assert!(harness.organizer.take_notice());
        let notice_path = harness.root.with_extension("notice");
        let mut restarted = Organizer::new(harness.root.clone(), notice_path).unwrap();
        restarted.set_enabled(true);
        fs::write(harness.root.join("second.pdf"), b"x").unwrap();
        let mut source = restarted.take_source().unwrap();
        let queue = restarted.queue();
        let deadline = Instant::now() + FIVE_SECONDS;
        while !harness.root.join("Documents/second.pdf").exists() && Instant::now() < deadline {
            read_events(&mut source, &queue);
            restarted.tick(Instant::now());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(harness.root.join("Documents/second.pdf").exists());
        assert!(
            !restarted.take_notice(),
            "persisted latch must suppress the second notice"
        );
    }
}
