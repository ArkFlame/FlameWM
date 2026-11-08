//! Live index updates. Every indexed folder, up to [`MAX_WATCHES`], gets one inotify watch. The
//! worker blocks in `read`, so no timer runs and an idle finder costs no wakeups. The events of
//! one read form one batch: they are applied to the shared index under a single write lock, and
//! `on_change` runs once for the batch.

use std::collections::{HashMap, HashSet};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};

use crate::index::{self, Entry, Shared};

/// Watches kept at once. Folders beyond this stay unwatched until the next full walk.
pub const MAX_WATCHES: usize = 4096;
const BUFFER_BYTES: usize = 16 * 1024;

fn watch_flags() -> WatchFlags {
    WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::DELETE_SELF
        | WatchFlags::MOVE_SELF
        | WatchFlags::ONLYDIR
        | WatchFlags::DONT_FOLLOW
}

/// True when `path` is `prefix` itself or lies below it, compared by whole path components.
fn is_under(path: &Path, prefix: &Path) -> bool {
    path.starts_with(prefix)
}

struct Event {
    wd: i32,
    flags: ReadFlags,
    name: Option<String>,
}

enum Op {
    Upsert(Entry),
    Remove,
}

/// Changes gathered from one read. The last operation on a path wins; removed folders take
/// their whole subtree.
#[derive(Default)]
struct Batch {
    ops: HashMap<String, Op>,
    trees: Vec<String>,
    overflow: bool,
    truncated: bool,
}

impl Batch {
    fn upsert(&mut self, entry: Entry) {
        self.ops.insert(entry.path.clone(), Op::Upsert(entry));
    }

    fn remove(&mut self, path: String) {
        self.ops.insert(path, Op::Remove);
    }

    fn remove_tree(&mut self, path: String) {
        let root = Path::new(&path);
        self.ops.retain(|key, _| !is_under(Path::new(key), root));
        self.ops.insert(path.clone(), Op::Remove);
        self.trees.push(path);
    }
}

#[derive(Default)]
struct Watcher {
    dirs: HashMap<i32, PathBuf>,
    warned: bool,
}

impl Watcher {
    /// Watches `dir` unless the budget is spent or the kernel refuses. Returns whether it is watched.
    fn watch(&mut self, fd: &OwnedFd, dir: &Path) -> bool {
        if self.dirs.len() >= MAX_WATCHES {
            if !self.warned {
                self.warned = true;
                eprintln!("flamewm-finder: watch budget of {MAX_WATCHES} folders reached");
            }
            return false;
        }
        match inotify::add_watch(fd.as_fd(), dir, watch_flags()) {
            Ok(wd) => {
                self.dirs.insert(wd, dir.to_path_buf());
                true
            }
            Err(_) => false,
        }
    }

    /// Adds every folder of `entries` (and the roots) to the watches.
    fn watch_all(&mut self, fd: &OwnedFd, roots: &[PathBuf], entries: &[Entry]) {
        for root in roots {
            self.watch(fd, root);
        }
        for entry in entries.iter().filter(|entry| entry.is_dir) {
            self.watch(fd, Path::new(&entry.path));
        }
    }

    /// Indexes a folder created while watching, and watches the folders under it.
    fn scan(&mut self, fd: &OwnedFd, dir: &Path, batch: &mut Batch, budget: usize) {
        let found = index::walk(
            &[dir.to_path_buf()],
            budget,
            index::MAX_DEPTH,
            index::WALK_BUDGET,
        );
        batch.truncated |= found.truncated;
        for entry in found.entries {
            if entry.is_dir {
                self.watch(fd, Path::new(&entry.path));
            }
            batch.upsert(entry);
        }
    }

    fn collect(&mut self, fd: &OwnedFd, batch: &mut Batch, budget: usize, event: Event) {
        if event.flags.contains(ReadFlags::QUEUE_OVERFLOW) {
            batch.overflow = true;
            return;
        }
        if event.flags.contains(ReadFlags::IGNORED) {
            self.dirs.remove(&event.wd);
            return;
        }
        let Some(dir) = self.dirs.get(&event.wd).cloned() else {
            return;
        };
        if event
            .flags
            .intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF)
        {
            if let Some(text) = dir.to_str() {
                batch.remove_tree(text.to_owned());
            }
            return;
        }
        let Some(name) = event.name else {
            return;
        };
        if !index::accept_name(&name) {
            return;
        }
        let path = dir.join(&name);
        let Some(text) = path.to_str().map(str::to_owned) else {
            return;
        };
        if event
            .flags
            .intersects(ReadFlags::DELETE | ReadFlags::MOVED_FROM)
        {
            batch.remove_tree(text.clone());
        }
        if event
            .flags
            .intersects(ReadFlags::CREATE | ReadFlags::MOVED_TO | ReadFlags::CLOSE_WRITE)
        {
            match index::stat(&path) {
                Some(entry) if entry.is_dir => {
                    self.watch(fd, &path);
                    batch.upsert(entry);
                    self.scan(fd, &path, batch, budget);
                }
                Some(entry) => batch.upsert(entry),
                None => batch.remove(text),
            }
        }
    }

    /// Reads one batch of events, applies it, and reports whether the index changed.
    fn step<Fd: AsFd>(
        &mut self,
        fd: &OwnedFd,
        reader: &mut inotify::Reader<'_, Fd>,
        shared: &Shared,
        roots: &[PathBuf],
    ) -> io::Result<bool> {
        let mut events = vec![next_event(reader)?];
        while !reader.is_buffer_empty() {
            events.push(next_event(reader)?);
        }
        let budget = index::MAX_ENTRIES.saturating_sub(shared.read().len());
        let mut batch = Batch::default();
        for event in events {
            self.collect(fd, &mut batch, budget, event);
        }
        if batch.overflow {
            // The kernel dropped events, so the index is rebuilt from a fresh walk.
            let docs = shared.read().doc_paths.clone();
            let mut fresh = index::walk(
                roots,
                index::MAX_ENTRIES,
                index::MAX_DEPTH,
                index::WALK_BUDGET,
            );
            fresh.set_docs(docs);
            self.watch_all(fd, roots, &fresh.entries);
            *shared.write() = fresh;
            return Ok(true);
        }
        Ok(commit(shared, batch))
    }
}

fn next_event<Fd: AsFd>(reader: &mut inotify::Reader<'_, Fd>) -> io::Result<Event> {
    let event = reader.next()?;
    Ok(Event {
        wd: event.wd(),
        flags: event.events(),
        name: event
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    })
}

/// Applies a batch to the shared index. Returns false when nothing changed.
fn commit(shared: &Shared, batch: Batch) -> bool {
    if batch.ops.is_empty() && batch.trees.is_empty() && !batch.truncated {
        return false;
    }
    let mut upserts = Vec::new();
    let mut touched: HashSet<String> = HashSet::new();
    for (path, op) in batch.ops {
        match op {
            Op::Upsert(entry) => {
                touched.insert(path);
                upserts.push(entry);
            }
            Op::Remove => {
                touched.insert(path);
            }
        }
    }
    let trees: Vec<PathBuf> = batch.trees.iter().map(PathBuf::from).collect();
    let mut index = shared.write();
    if !touched.is_empty() || !trees.is_empty() {
        index.entries.retain(|entry| {
            !touched.contains(&entry.path)
                && !trees
                    .iter()
                    .any(|root| is_under(Path::new(&entry.path), root))
        });
    }
    let room = index::MAX_ENTRIES.saturating_sub(index.len());
    if upserts.len() > room {
        upserts.truncate(room);
        index.truncated = true;
    }
    index.entries.extend(upserts);
    index.truncated |= batch.truncated;
    index.finish();
    true
}

/// Runs the watcher until the kernel fails the descriptor. `on_change` runs after each batch that
/// changed the index. Blocks the calling thread, which is meant to be the index worker.
pub fn run(shared: Shared, roots: Vec<PathBuf>, mut on_change: impl FnMut()) -> io::Result<()> {
    let fd = inotify::init(CreateFlags::CLOEXEC)?;
    let mut watcher = Watcher::default();
    let entries = shared.read().entries.clone();
    watcher.watch_all(&fd, &roots, &entries);
    drop(entries);
    let mut buffer = [MaybeUninit::<u8>::uninit(); BUFFER_BYTES];
    let mut reader = inotify::Reader::new(&fd, &mut buffer);
    loop {
        if watcher.step(&fd, &mut reader, &shared, &roots)? {
            on_change();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("finder-watch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs")).unwrap();
        root
    }

    /// Runs the watcher over `root` with one indexed folder, returning the reader's parts.
    fn setup(root: &Path) -> (OwnedFd, Watcher, Shared, Vec<PathBuf>) {
        let roots = vec![root.to_path_buf()];
        let shared = Shared::default();
        *shared.write() = index::walk(
            &roots,
            index::MAX_ENTRIES,
            index::MAX_DEPTH,
            index::WALK_BUDGET,
        );
        let fd = inotify::init(CreateFlags::CLOEXEC).unwrap();
        let mut watcher = Watcher::default();
        let entries = shared.read().entries.clone();
        watcher.watch_all(&fd, &roots, &entries);
        (fd, watcher, shared, roots)
    }

    fn paths(shared: &Shared) -> Vec<String> {
        let mut paths: Vec<String> = shared
            .read()
            .entries
            .iter()
            .map(|e| e.path.clone())
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn created_and_removed_files_reach_the_index_in_one_step() {
        let root = scratch("files");
        let (fd, mut watcher, shared, roots) = setup(&root);
        let mut buffer = [MaybeUninit::<u8>::uninit(); BUFFER_BYTES];
        let mut reader = inotify::Reader::new(&fd, &mut buffer);

        let made = root.join("notes.txt");
        fs::write(&made, "hello").unwrap();
        assert!(watcher.step(&fd, &mut reader, &shared, &roots).unwrap());
        assert!(paths(&shared).contains(&made.to_string_lossy().into_owned()));

        fs::remove_file(&made).unwrap();
        assert!(watcher.step(&fd, &mut reader, &shared, &roots).unwrap());
        assert!(!paths(&shared).contains(&made.to_string_lossy().into_owned()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_folders_are_scanned_and_removed_with_their_contents() {
        let root = scratch("tree");
        let (fd, mut watcher, shared, roots) = setup(&root);
        let mut buffer = [MaybeUninit::<u8>::uninit(); BUFFER_BYTES];
        let mut reader = inotify::Reader::new(&fd, &mut buffer);

        let nested = root.join("project/src");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("main.rs"), "fn main() {}").unwrap();
        // The folder appears first; the file may land in the same or a later batch.
        while !paths(&shared).iter().any(|p| p.ends_with("main.rs")) {
            assert!(watcher.step(&fd, &mut reader, &shared, &roots).unwrap());
        }

        fs::remove_dir_all(root.join("project")).unwrap();
        while paths(&shared).iter().any(|p| p.contains("/project")) {
            assert!(watcher.step(&fd, &mut reader, &shared, &roots).unwrap());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hidden_names_are_never_added() {
        let root = scratch("hidden");
        let (fd, mut watcher, shared, roots) = setup(&root);
        let mut buffer = [MaybeUninit::<u8>::uninit(); BUFFER_BYTES];
        let mut reader = inotify::Reader::new(&fd, &mut buffer);

        fs::write(root.join(".secret"), "x").unwrap();
        fs::write(root.join("visible.txt"), "x").unwrap();
        while !paths(&shared).iter().any(|p| p.ends_with("visible.txt")) {
            assert!(watcher.step(&fd, &mut reader, &shared, &roots).unwrap());
        }
        assert!(!paths(&shared).iter().any(|p| p.ends_with(".secret")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_empty_batch_reports_no_change() {
        let shared = Shared::default();
        assert!(!commit(&shared, Batch::default()));
    }

    #[test]
    fn watch_budget_is_enforced() {
        let root = scratch("budget");
        let fd = inotify::init(CreateFlags::CLOEXEC).unwrap();
        let mut watcher = Watcher::default();
        for i in 0..MAX_WATCHES {
            watcher
                .dirs
                .insert(i as i32 + 1_000_000, PathBuf::from(i.to_string()));
        }
        assert!(
            !watcher.watch(&fd, &root.join("docs")),
            "a full budget refuses new watches"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
