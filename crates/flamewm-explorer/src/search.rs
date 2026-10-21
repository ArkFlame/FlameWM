//! Recursive filename search. Pure walker plus a single cancellable worker thread.
//! The walker never follows symlinks, is bounded by hit count and visited entries, and checks a cancel flag.
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

/// Breadth-first walk from `root`. Symlinks are reported but never traversed.
pub fn search_tree(root: &Path, query: &str, show_hidden: bool, cancel: &AtomicBool) -> SearchOutcome {
    let query: String = query.trim().chars().take(MAX_QUERY_CHARS).collect();
    let mut outcome = SearchOutcome::default();
    if query.is_empty() {
        return outcome;
    }
    let mut queue = VecDeque::from([root.to_path_buf()]);
    let mut visits = 0_usize;
    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            return outcome;
        }
        let Ok(read) = fs::read_dir(&dir) else { continue };
        for item in read {
            let Ok(item) = item else { continue };
            visits += 1;
            if visits > MAX_VISITS {
                outcome.truncated = true;
                return outcome;
            }
            if visits % 512 == 0 && cancel.load(Ordering::Relaxed) {
                outcome.cancelled = true;
                return outcome;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let path = item.path();
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            let directory = meta.is_dir();
            if name_matches(&name, &query) {
                if outcome.hits.len() >= MAX_HITS {
                    outcome.truncated = true;
                    return outcome;
                }
                outcome.hits.push(SearchHit { parent: dir.clone(), path: path.clone(), name, directory });
            }
            if directory {
                queue.push_back(path);
            }
        }
    }
    outcome
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
        self.last.is_some_and(|last| now.duration_since(last) >= DEBOUNCE)
    }
    pub fn clear(&mut self) {
        self.last = None;
    }
}

pub struct SearchJob {
    pub id: u64,
    pub root: PathBuf,
    pub query: String,
    pub show_hidden: bool,
}

pub struct SearchResult {
    pub id: u64,
    pub query: String,
    pub outcome: SearchOutcome,
}

/// One worker thread. Starting a new job cancels the previous one; stale results are filtered by `id`.
pub struct SearchWorker {
    jobs: Sender<SearchJob>,
    results: Receiver<SearchResult>,
    cancel: Arc<AtomicBool>,
    next_id: u64,
}

impl SearchWorker {
    pub fn spawn(wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (jobs_tx, jobs_rx) = mpsc::channel::<SearchJob>();
        let (results_tx, results_rx) = mpsc::channel::<SearchResult>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        thread::Builder::new().name("explorer-search".into()).spawn(move || {
            while let Ok(job) = jobs_rx.recv() {
                worker_cancel.store(false, Ordering::Relaxed);
                let outcome = search_tree(&job.root, &job.query, job.show_hidden, &worker_cancel);
                if results_tx.send(SearchResult { id: job.id, query: job.query, outcome }).is_err() {
                    break;
                }
                wake();
            }
        })?;
        Ok(Self { jobs: jobs_tx, results: results_rx, cancel, next_id: 0 })
    }

    /// Cancels any running search and queues a new one. Returns the job id.
    pub fn start(&mut self, root: PathBuf, query: String, show_hidden: bool) -> Result<u64, String> {
        self.cancel.store(true, Ordering::Relaxed);
        self.next_id += 1;
        let id = self.next_id;
        self.jobs
            .send(SearchJob { id, root, query, show_hidden })
            .map(|()| id)
            .map_err(|_| "Search worker unavailable".to_owned())
    }

    /// Cancels the running search and marks it stale so its result is ignored.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.next_id += 1;
    }

    /// Returns the newest result for the active job, dropping stale ones.
    pub fn poll(&mut self) -> Option<SearchResult> {
        let mut latest = None;
        while let Ok(result) = self.results.try_recv() {
            if result.id == self.next_id {
                latest = Some(result);
            }
        }
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("flamewm-search-test-{tag}-{}", std::process::id()));
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
        assert_eq!(names, vec!["Report".to_owned(), "report-final.txt".to_owned()]);
        let hit = outcome.hits.iter().find(|h| h.name == "report-final.txt").unwrap();
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
        worker.start(root.clone(), "zzz-none".into(), false).unwrap();
        let id = worker.start(root.clone(), "notes".into(), false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let result = loop {
            if let Some(result) = worker.poll() {
                break result;
            }
            assert!(Instant::now() < deadline, "search worker timed out");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(result.id, id);
        assert_eq!(result.outcome.hits.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
