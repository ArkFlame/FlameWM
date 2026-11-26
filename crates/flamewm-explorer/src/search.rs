//! Recursive filename search. A parallel walker runs on a small thread pool inside one worker thread,
//! streams hits to the UI in batches, and is cancelled per job. Symlinks are never traversed.
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::SystemTime;

/// Walker threads per search job.
const MAX_THREADS: usize = 4;
/// Hits sent to the UI per batch.
const BATCH: usize = 64;

pub const MAX_HITS: usize = 2_000;
pub const MAX_VISITS: usize = 200_000;
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

/// Lowercased character stream, so case folding never allocates a `String` for short-lived comparisons.
fn lower_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().flat_map(char::to_lowercase)
}

fn ci_eq(left: &str, right: &str) -> bool {
    if left.is_ascii() && right.is_ascii() {
        return left.eq_ignore_ascii_case(right);
    }
    lower_chars(left).eq(lower_chars(right))
}

fn ci_starts_with(text: &str, prefix: &str) -> bool {
    if text.is_ascii() && prefix.is_ascii() {
        return text.len() >= prefix.len()
            && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes());
    }
    let mut prefix = lower_chars(prefix);
    let mut text = lower_chars(text);
    loop {
        match prefix.next() {
            None => return true,
            Some(expected) => match text.next() {
                Some(actual) if actual == expected => {}
                _ => return false,
            },
        }
    }
}

/// ASCII-only substring test that skips starts whose first byte cannot match, instead of folding every
/// same-width window. A deep walk visits up to `MAX_VISITS` names, so the per-name cost is the dominant
/// non-filesystem share of a search; this compares one byte per candidate start and the needle only on a
/// first-byte hit.
fn ascii_contains_ci(text: &[u8], needle: &[u8]) -> bool {
    let Some(&first) = needle.first() else {
        return true;
    };
    if needle.len() > text.len() {
        return false;
    }
    let last = text.len() - needle.len();
    let mut start = 0;
    while start <= last {
        let Some(found) = text[start..=last]
            .iter()
            .position(|byte| byte.eq_ignore_ascii_case(&first))
        else {
            return false;
        };
        start += found;
        if text[start..start + needle.len()].eq_ignore_ascii_case(needle) {
            return true;
        }
        start += 1;
    }
    false
}

fn ci_contains(text: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if text.is_ascii() && needle.is_ascii() {
        return ascii_contains_ci(text.as_bytes(), needle.as_bytes());
    }
    // Non-ASCII names are rare; lowercasing both sides once keeps the same semantics as before.
    text.to_lowercase().contains(&needle.to_lowercase())
}

/// Match tier of `name` for `query`, case-insensitive: 0 exact, 1 prefix, 2 substring. `None` when no match.
/// Empty or whitespace-only queries match nothing. Allocation-free for ASCII names and queries.
#[must_use]
pub fn rank_match(query: &str, name: &str) -> Option<u8> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if ci_eq(name, query) {
        Some(0)
    } else if ci_starts_with(name, query) {
        Some(1)
    } else if ci_contains(name, query) {
        Some(2)
    } else {
        None
    }
}

/// Display order key: match tier, then shorter name, then alphabetical (case-insensitive).
#[must_use]
pub fn result_order(query: &str, name: &str) -> (u8, usize, String) {
    (
        rank_match(query, name).unwrap_or(u8::MAX),
        name.chars().count(),
        name.to_lowercase(),
    )
}

/// True when the hit cap was reached, so the result list is a prefix of all matches.
#[must_use]
pub fn hit_cap_reached(count: usize) -> bool {
    count >= MAX_HITS
}

/// One-line status shown when `hit_cap_reached`.
#[must_use]
pub fn cap_notice() -> String {
    format!("Showing first {MAX_HITS} matches — refine your search")
}

/// Case-insensitive substring match on the file name. Empty or whitespace-only queries match nothing.
/// One pass: the walker only needs to know whether a visited name matches at all, so this skips the
/// separate exact and prefix compares `rank_match` needs for the result ordering.
#[must_use]
pub fn name_matches(name: &str, query: &str) -> bool {
    let query = query.trim();
    !query.is_empty() && ci_contains(name, query)
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
                        let mut state = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                            let os_name = item.file_name();
                            let name = os_name.to_string_lossy();
                            if !show_hidden && name.starts_with('.') {
                                continue;
                            }
                            // `file_type` answers from the readdir record (d_type) on normal filesystems,
                            // so non-matching files no longer cost an lstat and a `PathBuf` each.
                            let Ok(file_type) = item.file_type() else {
                                continue;
                            };
                            let directory = file_type.is_dir();
                            let matched = name_matches(&name, &query);
                            if !matched && !directory {
                                continue;
                            }
                            let path = dir.join(&os_name);
                            if matched {
                                let Ok(meta) = fs::symlink_metadata(&path) else {
                                    continue;
                                };
                                if hits.fetch_add(1, Ordering::Relaxed) >= MAX_HITS {
                                    truncated.store(true, Ordering::Relaxed);
                                    break;
                                }
                                batch.push(SearchHit {
                                    parent: dir.clone(),
                                    path: path.clone(),
                                    name: name.into_owned(),
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
                        let mut state = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(batch);
    });
    SearchOutcome {
        hits: collected
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        truncated,
        cancelled,
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
    /// Set by the walker when a wake is already queued, so a fast walk that publishes many batches in a
    /// burst costs the UI one repaint instead of one per 64 hits. Cleared at the start of `poll`.
    wake_pending: Arc<AtomicBool>,
    next_id: u64,
}

impl SearchWorker {
    /// `wake` is called from walker threads after each batch, so it must be cheap and thread-safe.
    pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Self> {
        let (jobs_tx, jobs_rx) = mpsc::channel::<SearchJob>();
        let (events_tx, events_rx) = mpsc::channel::<SearchEvent>();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let pending = Arc::clone(&wake_pending);
        thread::Builder::new()
            .name("explorer-search".into())
            .spawn(move || {
                flamewm_integrations_linux::background::lower_current_thread();
                // The event is sent before the pending flag is flipped: a batch always sits in the channel
                // before its wake is dropped as already-queued, so `poll` cannot miss a batch it drained.
                let wake_once = || {
                    if !pending.swap(true, Ordering::Relaxed) {
                        wake();
                    }
                };
                while let Ok(job) = jobs_rx.recv() {
                    let id = job.id;
                    let batches = events_tx.clone();
                    let sink = |hits: Vec<SearchHit>| {
                        if batches.send(SearchEvent::Batch { id, hits }).is_ok() {
                            wake_once();
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
                    wake_once();
                }
            })?;
        Ok(Self {
            jobs: jobs_tx,
            events: events_rx,
            current: None,
            wake_pending,
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
        // Clear before draining: a batch sent after this store sees `false` and queues its own wake, while
        // one already in the channel is drained below. That keeps a burst of batches to one UI wake.
        self.wake_pending.store(false, Ordering::Relaxed);
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
    use std::time::{Duration, Instant};

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
    fn rank_match_orders_exact_then_prefix_then_substring() {
        assert_eq!(rank_match("Notes", "notes"), Some(0));
        assert_eq!(rank_match("note", "Notes.md"), Some(1));
        assert_eq!(rank_match("ote", "notes.md"), Some(2));
        assert_eq!(rank_match("zzz", "notes.md"), None);
        assert_eq!(rank_match("  ", "notes.md"), None);
    }

    #[test]
    fn rank_match_is_unicode_case_insensitive() {
        assert_eq!(rank_match("ÉCOLE", "école.txt"), Some(1));
        assert_eq!(rank_match("école", "ÉCOLE"), Some(0));
        assert_eq!(rank_match("Ü", "müller"), Some(2));
        assert!(name_matches("ÉTÉ.md", "été"));
    }

    #[test]
    fn result_order_sorts_by_tier_then_length_then_name() {
        let names = [
            "my-report.txt",
            "report-zeta.txt",
            "Report",
            "report-alpha.txt",
            "report.md",
            "a-report-b",
            "Reports",
        ];
        let mut sorted = names.to_vec();
        sorted.sort_by_cached_key(|name| result_order("report", name));
        assert_eq!(
            sorted,
            vec![
                "Report",
                "Reports",
                "report.md",
                "report-zeta.txt",
                "report-alpha.txt",
                "a-report-b",
                "my-report.txt",
            ]
        );
    }

    #[test]
    fn hit_cap_flag_is_set_exactly_at_the_cap() {
        assert!(!hit_cap_reached(MAX_HITS - 1));
        assert!(hit_cap_reached(MAX_HITS));
        assert_eq!(
            cap_notice(),
            "Showing first 2000 matches — refine your search"
        );
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

    #[test]
    fn name_matches_agrees_with_rank_match() {
        let queries = ["report", "REPORT", "  epo  ", " ", "", "école", "É", "zzz"];
        let names = [
            "Report",
            "report-final.txt",
            "a-report-b",
            "notes.md",
            "ÉCOLE.TXT",
            "müller",
        ];
        for query in queries {
            for name in names {
                assert_eq!(
                    name_matches(name, query),
                    rank_match(query, name).is_some(),
                    "query {query:?} name {name:?}"
                );
            }
        }
    }

    #[test]
    fn ascii_contains_ci_matches_the_folded_substring() {
        let cases = [
            ("report-final.txt", "REPORT"),
            ("report-final.txt", "final"),
            ("report-final.txt", "t-f"),
            ("report-final.txt", "report-final.txt"),
            ("report-final.txt", "zzz"),
            ("abc", "abcd"),
            ("abc", "c"),
            ("ABC", "b"),
            ("", "a"),
            ("anything", ""),
        ];
        for (text, needle) in cases {
            assert_eq!(
                ascii_contains_ci(text.as_bytes(), needle.as_bytes()),
                text.to_lowercase().contains(&needle.to_lowercase()),
                "text {text:?} needle {needle:?}"
            );
        }
    }

    #[test]
    fn a_burst_of_batches_costs_one_wake_until_polled() {
        let root = tree("wake");
        // Three full batches plus a remainder: enough sink calls that uncoalesced wakes would be several.
        for index in 0..200 {
            fs::write(root.join(format!("report-{index:03}.txt")), b"x").unwrap();
        }
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let mut worker =
            SearchWorker::spawn(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            })
            .unwrap();
        let id = worker.start(root.clone(), "report".into(), false).unwrap();
        // Let the walk publish every batch without any poll: the pending flag stays set, so only the
        // first batch wakes the UI. The events themselves all wait in the channel.
        thread::sleep(Duration::from_millis(200));
        assert_eq!(wakes.load(Ordering::Relaxed), 1, "batches must coalesce");

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
                        assert_eq!(got, id);
                        hits += batch.len();
                    }
                    SearchEvent::Done { id: got, .. } => {
                        assert_eq!(got, id);
                        done = true;
                    }
                }
            }
            assert!(Instant::now() < deadline, "search worker timed out");
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(hits, 202, "every match still streams");
        let _ = fs::remove_dir_all(&root);
    }
}
