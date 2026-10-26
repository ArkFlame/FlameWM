//! Recursive filename search. A parallel walker runs on a small thread pool inside one worker thread,
//! streams hits to the UI in batches, and is cancelled per job. Symlinks are never traversed.
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

/// Walker threads per search job.
const MAX_THREADS: usize = 4;
/// Hits sent to the UI per batch.
const BATCH: usize = 64;

pub const MAX_HITS: usize = 2_000;
pub const MAX_VISITS: usize = 200_000;
pub const DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_QUERY_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub name: String,
    pub parent: PathBuf,
    pub directory: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// Set when `MAX_HITS` or `MAX_VISITS` stopped the walk early.
    pub truncated: bool,
    pub cancelled: bool,
}

/// Case-insensitive substring match on the file name. Empty or whitespace-only queries match nothing.
pub fn name_matches(name: &str, query: &str) -> bool {
    let query = query.trim();
    !query.is_empty() && name.to_lowercase().contains(&query.to_lowercase())
}

/// Shared directory queue: `active` counts directories being read, so idle threads wait for them to publish subdirectories.
struct Queue {
    dirs: VecDeque<PathBuf>,
    active: usize,
}

/// Parallel breadth-first walk from `root`. Symlinks are reported but never traversed.
/// Hits reach `sink` in batches from any walker thread. Returns `(truncated, cancelled)`.
fn walk(
    root: &Path,
    query: &str,
    show_hidden: bool,
    cancel: &AtomicBool,
    threads: usize,
    sink: &(dyn Fn(Vec<SearchHit>) + Sync),
) -> (bool, bool) {
    let query: String = query.trim().chars().take(MAX_QUERY_CHARS).collect();
    if query.is_empty() {
        return (false, false);
    }
    let queue = Mutex::new(Queue {
        dirs: VecDeque::from([root.to_path_buf()]),
        active: 0,
    });
    let ready = Condvar::new();
    let visits = AtomicUsize::new(0);
    let hits = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    let stop = || cancel.load(Ordering::Relaxed) || truncated.load(Ordering::Relaxed);
    thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                let mut batch = Vec::new();
                loop {
                    let dir = {
                        let mut state = queue.lock().unwrap_or_else(|poison| poison.into_inner());
                        loop {
                            if stop() {
                                state.dirs.clear();
                                break None;
                            }
                            if let Some(dir) = state.dirs.pop_front() {
                                state.active += 1;
                                break Some(dir);
                            }
                            if state.active == 0 {
                                break None;
                            }
                            state = ready
                                .wait(state)
                                .unwrap_or_else(|poison| poison.into_inner());
                        }
                    };
                    let Some(dir) = dir else {
                        ready.notify_all();
                        break;
                    };
                    let mut subdirs = Vec::new();
                    if let Ok(read) = fs::read_dir(&dir) {
                        for item in read {
                            if stop() {
                                break;
                            }
                            let Ok(item) = item else { continue };
                            if visits.fetch_add(1, Ordering::Relaxed) + 1 > MAX_VISITS {
                                truncated.store(true, Ordering::Relaxed);
                                break;
                            }
                            let name = item.file_name().to_string_lossy().into_owned();
                            if !show_hidden && name.starts_with('.') {
                                continue;
                            }
                            let path = item.path();
                            let Ok(meta) = fs::symlink_metadata(&path) else {
                                continue;
                            };
                            let directory = meta.is_dir();
                            if name_matches(&name, &query) {
                                if hits.fetch_add(1, Ordering::Relaxed) >= MAX_HITS {
                                    truncated.store(true, Ordering::Relaxed);
                                    break;
                                }
                                batch.push(SearchHit {
                                    parent: dir.clone(),
                                    path: path.clone(),
                                    name,
                                    directory,
                                    size: meta.len(),
                                    modified: meta.modified().ok(),
                                });
                                if batch.len() >= BATCH {
                                    sink(std::mem::take(&mut batch));
                                }
                            }
                            if directory {
                                subdirs.push(path);
                            }
                        }
                    }
                    {
                        let mut state = queue.lock().unwrap_or_else(|poison| poison.into_inner());
                        state.dirs.extend(subdirs);
                        state.active -= 1;
                    }
                    ready.notify_all();
                }
                if !batch.is_empty() {
                    sink(batch);
                }
            });
        }
    });
    (
        truncated.load(Ordering::Relaxed),
        cancel.load(Ordering::Relaxed),
    )
}

/// Synchronous search built on the parallel walker (tests and one-shot callers).
pub fn search_tree(
    root: &Path,
    query: &str,
    show_hidden: bool,
    cancel: &AtomicBool,
) -> SearchOutcome {
    let collected = Mutex::new(Vec::new());
    let (truncated, cancelled) = walk(root, query, show_hidden, cancel, MAX_THREADS, &|batch| {
        collected
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .extend(batch);
    });
    SearchOutcome {
        hits: collected
            .into_inner()
            .unwrap_or_else(|poison| poison.into_inner()),
        truncated,
        cancelled,
    }
}

/// Pure debounce: a request is due once `DEBOUNCE` has elapsed since the last keystroke.
#[derive(Debug, Default)]
pub struct Debounce {
    last: Option<std::time::Instant>,
}

impl Debounce {
    pub fn touch(&mut self, now: std::time::Instant) {
        self.last = Some(now);
    }
    pub fn due(&self, now: std::time::Instant) -> bool {
        self.last
            .is_some_and(|last| now.duration_since(last) >= DEBOUNCE)
    }
    pub fn clear(&mut self) {
        self.last = None;
    }
}

struct SearchJob {
    id: u64,
    root: PathBuf,
    query: String,
    show_hidden: bool,
    cancel: Arc<AtomicBool>,
}

/// Streamed progress of the active search job. `Done` ends it.
#[derive(Debug, PartialEq, Eq)]
pub enum SearchEvent {
    Batch {
        id: u64,
        hits: Vec<SearchHit>,
    },
    Done {
        id: u64,
        truncated: bool,
        cancelled: bool,
    },
}

/// One worker thread that runs each job's parallel walk. Starting or cancelling a job sets its cancel flag;
/// events from stale jobs are filtered by `id` in `poll`.
pub struct SearchWorker {
    jobs: Sender<SearchJob>,
    events: Receiver<SearchEvent>,
    current: Option<Arc<AtomicBool>>,
    next_id: u64,
}

impl SearchWorker {
    /// `wake` is called from walker threads after each batch, so it must be cheap and thread-safe.
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        let (jobs_tx, jobs_rx) = mpsc::channel::<SearchJob>();
        let (events_tx, events_rx) = mpsc::channel::<SearchEvent>();
        thread::Builder::new()
            .name("explorer-search".into())
            .spawn(move || {
                while let Ok(job) = jobs_rx.recv() {
                    let id = job.id;
                    let batches = events_tx.clone();
                    let sink = |hits: Vec<SearchHit>| {
                        if batches.send(SearchEvent::Batch { id, hits }).is_ok() {
                            wake();
                        }
                    };
                    let (truncated, cancelled) = walk(
                        &job.root,
                        &job.query,
                        job.show_hidden,
                        &job.cancel,
                        MAX_THREADS,
                        &sink,
                    );
                    if events_tx
                        .send(SearchEvent::Done {
                            id,
                            truncated,
                            cancelled,
                        })
                        .is_err()
                    {
                        break;
                    }
                    wake();
                }
            })?;
        Ok(Self {
            jobs: jobs_tx,
            events: events_rx,
            current: None,
            next_id: 0,
        })
    }

    /// Cancels any running search and queues a new one. Returns the job id.
    pub fn start(
        &mut self,
        root: PathBuf,
        query: String,
        show_hidden: bool,
    ) -> Result<u64, String> {
        self.cancel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.current = Some(Arc::clone(&cancel));
        let id = self.next_id;
        self.jobs
            .send(SearchJob {
                id,
                root,
                query,
                show_hidden,
                cancel,
            })
            .map(|()| id)
            .map_err(|_| "Search worker unavailable".to_owned())
    }

    /// Cancels the running search and marks it stale so its events are ignored.
    pub fn cancel(&mut self) {
        if let Some(flag) = self.current.take() {
            flag.store(true, Ordering::Relaxed);
        }
        self.next_id += 1;
    }

    /// Drains events for the active job; events of stale jobs are dropped.
    pub fn poll(&mut self) -> Vec<SearchEvent> {
        let active = self.next_id;
        self.events
            .try_iter()
            .filter(|event| match event {
                SearchEvent::Batch { id, .. } | SearchEvent::Done { id, .. } => *id == active,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn tree(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("flamewm-search-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/deep/Report")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("a/deep/report-final.txt"), b"x").unwrap();
        fs::write(root.join("a/notes.md"), b"x").unwrap();
        fs::write(root.join(".hidden/report-secret.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("a"), root.join("loop")).unwrap();
        root
    }

    #[test]
    fn matches_case_insensitively_and_ignores_blank_queries() {
        assert!(name_matches("Report-Final.TXT", " report "));
        assert!(!name_matches("notes.md", "   "));
        assert!(!name_matches("notes.md", ""));
    }

    #[test]
    fn walks_recursively_with_parent_paths_skipping_hidden_and_symlink_loops() {
        let root = tree("walk");
        let cancel = AtomicBool::new(false);
        let outcome = search_tree(&root, "report", false, &cancel);
        let mut names: Vec<_> = outcome.hits.iter().map(|h| h.name.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["Report".to_owned(), "report-final.txt".to_owned()]
        );
        let hit = outcome
            .hits
            .iter()
            .find(|h| h.name == "report-final.txt")
            .unwrap();
        assert_eq!(hit.parent, root.join("a/deep"));
        assert!(!outcome.truncated && !outcome.cancelled);
        let with_hidden = search_tree(&root, "secret", true, &cancel);
        assert_eq!(with_hidden.hits.len(), 1);
        assert!(search_tree(&root, "secret", false, &cancel).hits.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancellation_stops_walk_immediately() {
        let root = tree("cancel");
        let cancel = AtomicBool::new(true);
        let outcome = search_tree(&root, "report", false, &cancel);
        assert!(outcome.cancelled);
        assert!(outcome.hits.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn debounce_waits_for_quiet_period() {
        let mut debounce = Debounce::default();
        let t0 = Instant::now();
        assert!(!debounce.due(t0));
        debounce.touch(t0);
        assert!(!debounce.due(t0 + Duration::from_millis(100)));
        assert!(debounce.due(t0 + DEBOUNCE));
        debounce.clear();
        assert!(!debounce.due(t0 + DEBOUNCE));
    }

    #[test]
    fn worker_returns_only_latest_job_result() {
        let root = tree("worker");
        let mut worker = SearchWorker::spawn(|| {}).unwrap();
        worker
            .start(root.clone(), "zzz-none".into(), false)
            .unwrap();
        let id = worker.start(root.clone(), "notes".into(), false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut hits = 0;
        let mut done = false;
        while !done {
            for event in worker.poll() {
                match event {
                    SearchEvent::Batch {
                        id: got,
                        hits: batch,
                    } => {
                        assert_eq!(got, id, "only the latest job may report");
                        hits += batch.len();
                    }
                    SearchEvent::Done {
                        id: got, cancelled, ..
                    } => {
                        assert_eq!(got, id);
                        assert!(!cancelled);
                        done = true;
                    }
                }
            }
            assert!(Instant::now() < deadline, "search worker timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(hits, 1);
        let _ = fs::remove_dir_all(&root);
    }
}
