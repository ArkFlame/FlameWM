//! Bounded background icon resolution with priority-aware admission.
//!
//! The service owns one shared generation-scoped [`IconLookupIndex`], one
//! shared result cache, exactly [`ICON_RESOLVE_CONCURRENCY`] workers, a
//! priority queue with pending-dedupe, and a wake FD signalled whenever a
//! result completes. Workers perform zero FS rescan: they fork from the
//! base resolver and share the immutable index + theme config with fresh
//! small private caches. No UI/X11 calls happen in workers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crate::icon_theme::IconLookupIndex;
use crate::icons::{IconError, IconKey, IconResolver, Rgba8Raster};

/// Resolution urgency. Required jobs are never discarded by the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconPriority {
    High,
    Low,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconJob {
    pub key: IconKey,
    pub priority: IconPriority,
}

#[derive(Clone, Debug)]
pub struct IconResult {
    pub key: Arc<IconKey>,
    pub result: Result<Rgba8Raster, IconError>,
    /// Worker completion envelope timestamp. `drain_ready`/`take_ready`
    /// record `icon.result.wake_to_apply` from this instant to apply.
    pub completed_at: std::time::Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconSubmit {
    Enqueued,
    Coalesced,
    DroppedLow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconSubmitError {
    HighQueueFull(IconJob),
    Stopped(IconJob),
}

/// Worker seam for deterministic service tests and alternate resolver setup.
pub trait IconResolve: Send + 'static {
    fn resolve(&mut self, key: &IconKey) -> Result<Rgba8Raster, IconError>;

    /// Adopts a refreshed theme index before the next resolve. Default is a
    /// no-op for resolvers without theme state.
    fn apply_theme(&mut self, _index: Arc<IconLookupIndex>) {}
}

impl IconResolve for IconResolver {
    fn resolve(&mut self, key: &IconKey) -> Result<Rgba8Raster, IconError> {
        self.prepare_key(key)
    }

    fn apply_theme(&mut self, index: Arc<IconLookupIndex>) {
        let generation = index.generation;
        self.set_shared_index(index);
        // Clears this worker's theme caches (generation-scoped entries).
        self.notify_theme_changed(generation);
    }
}

struct Queue {
    high: VecDeque<Queued>,
    low: VecDeque<Queued>,
    pending: HashSet<Arc<IconKey>>,
    capacity: usize,
    stopped: bool,
}

#[derive(Clone, Debug)]
struct Queued {
    key: Arc<IconKey>,
    submitted_at: std::time::Instant,
}

impl Queue {
    fn len(&self) -> usize {
        self.high.len() + self.low.len()
    }

    fn submit(&mut self, job: IconJob) -> Result<IconSubmit, IconSubmitError> {
        if self.stopped {
            return Err(IconSubmitError::Stopped(job));
        }
        if self.pending.contains(&job.key) {
            return Ok(IconSubmit::Coalesced);
        }
        if self.len() == self.capacity {
            match job.priority {
                IconPriority::High => {
                    if let Some(replaced) = self.low.pop_front() {
                        self.pending.remove(&replaced.key);
                    } else {
                        return Err(IconSubmitError::HighQueueFull(job));
                    }
                }
                IconPriority::Low => return Ok(IconSubmit::DroppedLow),
            }
        }
        let priority = job.priority;
        let key = Arc::new(job.key);
        self.pending.insert(Arc::clone(&key));
        let queued = Queued {
            key,
            submitted_at: std::time::Instant::now(),
        };
        match priority {
            IconPriority::High => self.high.push_back(queued),
            IconPriority::Low => self.low.push_back(queued),
        }
        Ok(IconSubmit::Enqueued)
    }
}

struct SharedQueue {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// Current shared lookup index. Held only for an Arc clone/swap.
    theme: Mutex<Arc<IconLookupIndex>>,
    /// Bumped after every `theme` swap. Workers compare it to their applied
    /// epoch before each resolve and adopt the new index when it differs.
    theme_epoch: AtomicU64,
}

/// Exact worker count: one-time index build shared by all workers.
pub const ICON_RESOLVE_CONCURRENCY: usize = 20;
/// Bounded pending queue depth (HIGH retry ownership on full, LOW may drop).
pub const ICON_QUEUE_CAPACITY: usize = 256;
/// Final shared result/raster cache bounds.
pub const RESULT_CACHE_ENTRIES: usize = 2048;
const RESULT_CACHE_BYTES: usize = 512 * 1024 * 4;

/// Clamp a requested worker count into 1..=20 (>20 clamps to 20).
#[must_use]
pub fn clamp_worker_count(requested: usize) -> usize {
    requested.clamp(1, ICON_RESOLVE_CONCURRENCY)
}

/// Priority icon service. Results are keyed only by [`IconKey`], never UI slots.
///
/// Owns one shared [`IconLookupIndex`], one shared result/raster cache
/// (workers resolve through the shared index and post results into the
/// shared result map), exactly [`ICON_RESOLVE_CONCURRENCY`] workers, a
/// priority queue with pending-dedupe, and a wake FD signalled on every
/// completed result. Workers perform no UI/X11 calls.
pub struct IconService<R = IconResolver> {
    worker_count: usize,
    queue_capacity: usize,
    shared: Arc<SharedQueue>,
    results: Option<Receiver<IconResult>>,
    workers: Vec<JoinHandle<()>>,
    resolver: std::marker::PhantomData<R>,
    /// One shared result cache (key -> resolved raster) fed by workers.
    result_cache: Arc<Mutex<ResultCache>>,
    /// Wake FD: worker writes one byte per completed result; drained by poller.
    wake_reader: Option<UnixStream>,
    submitted: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    coalesced: Arc<AtomicU64>,
    dropped_low: Arc<AtomicU64>,
    /// Off-thread theme watch/rebuild owner (one thread, joined on drop).
    theme_rebuilder: ThemeRebuilder,
}

impl<R: IconResolve> IconService<R> {
    /// Starts `worker_count` resolver workers clamped to 1..=20
    /// (>20 clamps to 20). Queue capacity is clamped to 1..=256 and
    /// excludes active jobs.
    ///
    /// Workers share one generation-scoped index; pass the service index into
    /// each worker factory via [`IconService::shared_index`] when the worker
    /// resolver supports it (see `IconResolver::set_shared_index`).
    pub fn new<F>(worker_count: usize, queue_capacity: usize, factory: F) -> Self
    where
        F: Fn() -> R + Send + Sync + 'static,
    {
        Self::with_index(
            worker_count,
            queue_capacity,
            factory,
            Arc::new(empty_index()),
        )
    }

    /// Starts the service around an existing shared lookup index.
    pub fn with_index<F>(
        worker_count: usize,
        queue_capacity: usize,
        factory: F,
        index: Arc<IconLookupIndex>,
    ) -> Self
    where
        F: Fn() -> R + Send + Sync + 'static,
    {
        let workers_n = clamp_worker_count(worker_count);
        let capacity = queue_capacity.clamp(1, ICON_QUEUE_CAPACITY);
        let shared = Arc::new(SharedQueue {
            queue: Mutex::new(Queue {
                high: VecDeque::new(),
                low: VecDeque::new(),
                pending: HashSet::new(),
                capacity,
                stopped: false,
            }),
            ready: Condvar::new(),
            theme: Mutex::new(Arc::clone(&index)),
            theme_epoch: AtomicU64::new(0),
        });
        let (result_tx, results) = mpsc::sync_channel(capacity);
        let wake_pair: Option<(UnixStream, UnixStream)> = UnixStream::pair().ok();
        let (wake_reader, wake_writer_opt): (Option<UnixStream>, Option<UnixStream>) =
            match wake_pair {
                Some((reader, writer)) => (Some(reader), Some(writer)),
                None => (None, None),
            };
        if let (Some(reader), Some(writer)) = (wake_reader.as_ref(), wake_writer_opt.as_ref()) {
            let _ = reader.set_nonblocking(true);
            let _ = writer.set_nonblocking(true);
        }
        let wake_tx: Option<Arc<Mutex<UnixStream>>> = wake_writer_opt
            .as_ref()
            .and_then(|stream: &UnixStream| stream.try_clone().ok())
            .map(|stream| Arc::new(Mutex::new(stream)));
        let factory = Arc::new(factory);
        let result_cache = Arc::new(Mutex::new(ResultCache::default()));
        let submitted = Arc::new(AtomicU64::new(0));
        let completed = Arc::new(AtomicU64::new(0));
        let coalesced = Arc::new(AtomicU64::new(0));
        let dropped_low = Arc::new(AtomicU64::new(0));
        let mut workers = Vec::new();
        for number in 0..workers_n {
            let shared = Arc::clone(&shared);
            let result_tx = result_tx.clone();
            let wake_tx = wake_tx.clone();
            let cache = Arc::clone(&result_cache);
            let completed = Arc::clone(&completed);
            let factory = Arc::clone(&factory);
            workers.push(
                thread::Builder::new()
                    .name(format!("icon-resolver-{number}"))
                    .spawn(move || {
                        worker_loop(shared, result_tx, wake_tx, cache, completed, factory());
                    })
                    .expect("spawn icon resolver worker"),
            );
        }
        Self {
            worker_count: workers_n,
            queue_capacity: capacity,
            shared,
            results: Some(results),
            workers,
            resolver: std::marker::PhantomData,
            result_cache,
            wake_reader,
            submitted,
            completed,
            coalesced,
            dropped_low,
            theme_rebuilder: ThemeRebuilder::with_wake(wake_tx),
        }
    }

    /// Queues a non-blocking theme watch check on the rebuild thread against
    /// the currently installed index. Returns `false` (no-op) when a check is
    /// already in flight or a finished index is still waiting in
    /// [`IconService::poll_theme_rebuild`]. Never performs stat or build I/O
    /// on the calling thread.
    #[allow(clippy::must_use_candidate)]
    pub fn request_theme_rebuild(&self) -> bool {
        self.theme_rebuilder.request(self.shared_index())
    }

    /// Queues an off-thread build of the index for `theme_names` (a resolved theme chain).
    /// Returns `false` while a job is in flight or a finished index is unpolled.
    #[allow(clippy::must_use_candidate)]
    pub fn request_theme_change(&self, theme_names: Vec<String>) -> bool {
        self.theme_rebuilder
            .request_theme(self.shared_index(), theme_names)
    }

    /// Non-blocking: returns the finished replacement index, if the rebuild
    /// thread produced one since the last poll. The caller installs it with
    /// [`IconService::install_theme_index`]. Returns `None` when the theme is
    /// unchanged or the check is still running.
    #[must_use]
    pub fn poll_theme_rebuild(&self) -> Option<Arc<IconLookupIndex>> {
        self.theme_rebuilder.take()
    }

    /// `true` when no theme watch check is queued or running.
    #[must_use]
    pub fn theme_rebuild_idle(&self) -> bool {
        self.theme_rebuilder.is_idle()
    }

    /// The current shared generation-scoped lookup index (Arc clone).
    #[must_use]
    pub fn shared_index(&self) -> Arc<IconLookupIndex> {
        self.shared.theme.lock().map_or_else(
            |poisoned| Arc::clone(&poisoned.into_inner()),
            |slot| Arc::clone(&slot),
        )
    }

    /// Generation of the current shared index; keys built for new requests
    /// should carry this value.
    #[must_use]
    pub fn theme_generation(&self) -> u64 {
        self.shared_index().generation
    }

    /// Installs `next` as the shared index for subsequent requests and returns
    /// its generation. O(1) on the caller: swaps an Arc under a short lock and
    /// bumps the epoch. Workers adopt it (set index + `notify_theme_changed`)
    /// right before their next resolve, so no worker is interrupted mid-job
    /// and no caller thread waits on a worker.
    pub fn install_theme_index(&self, next: Arc<IconLookupIndex>) -> u64 {
        let mut installed = false;
        let generation;
        {
            let mut slot = self
                .shared
                .theme
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A rebuild is based on a snapshot captured when it was queued.
            // Do not let a late result from that snapshot roll the service
            // back after a newer generation has already been installed.
            if next.generation > slot.generation {
                *slot = next;
                installed = true;
            }
            generation = slot.generation;
        }
        if installed {
            self.shared.theme_epoch.fetch_add(1, Ordering::Release);
        }
        generation
    }

    /// Live worker count (clamped 1..=20 at construction).
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.worker_count
    }

    /// Live queue capacity (clamped 1..=256 at construction).
    #[must_use]
    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    /// Live shared result cache length (bounded by [`RESULT_CACHE_ENTRIES`]).
    #[must_use]
    pub fn result_cache_len(&self) -> usize {
        self.result_cache
            .lock()
            .map_or(0, |cache| cache.entries.len())
    }

    /// Live shared result cache byte total (bounded by `RESULT_CACHE_BYTES`).
    #[must_use]
    pub fn result_cache_bytes(&self) -> usize {
        self.result_cache.lock().map_or(0, |cache| cache.bytes)
    }

    /// Raw FD readable whenever a result completes (edge: drain `wake_drain`).
    #[must_use]
    pub fn wake_fd(&self) -> Option<RawFd> {
        self.wake_reader.as_ref().map(AsRawFd::as_raw_fd)
    }

    /// Non-blocking drain of pending wake bytes (call after `wake_fd` readable).
    pub fn wake_drain(&mut self) {
        if let Some(reader) = &mut self.wake_reader {
            let mut buf = [0_u8; 64];
            while reader.read(&mut buf).is_ok_and(|n| n > 0) {}
        }
    }

    /// Cached result for `key` from the single shared result cache.
    #[must_use]
    pub fn cached(&self, key: &IconKey) -> Option<Result<Rgba8Raster, IconError>> {
        self.result_cache.lock().ok()?.entries.get(key).cloned()
    }
}

impl<R: IconResolve> IconService<R> {
    /// Emits a 5000ms-cooldown service summary from existing counters.
    fn emit_service_summary(&self, context: &str) {
        let (submitted, completed, coalesced, dropped_low) = self.counters();
        let queue_len = self.shared.queue.lock().map_or(0, |queue| queue.len());
        flamewm_debug::emit(
            flamewm_debug::ICON_SERVICE_SUMMARY,
            flamewm_debug::ICON_SERVICE_SUMMARY_COOLDOWN,
            || {
                format!(
                    "{context} submitted={submitted} completed={completed} coalesced={coalesced} dropped_low={dropped_low} queue_len={queue_len}"
                )
            },
        );
    }

    /// `(submitted, completed, coalesced, dropped_low)` service counters.
    #[must_use]
    pub fn counters(&self) -> (u64, u64, u64, u64) {
        (
            self.submitted.load(Ordering::Relaxed),
            self.completed.load(Ordering::Relaxed),
            self.coalesced.load(Ordering::Relaxed),
            self.dropped_low.load(Ordering::Relaxed),
        )
    }

    /// Attempts non-blocking submission. A full high-priority queue returns
    /// ownership to caller; it is never silently dropped.
    pub fn submit(&self, job: IconJob) -> Result<IconSubmit, IconSubmitError> {
        let submitted = self
            .shared
            .queue
            .lock()
            .expect("icon queue poisoned")
            .submit(job);
        match &submitted {
            Ok(IconSubmit::Enqueued) => {
                note_icon_request();
                self.submitted.fetch_add(1, Ordering::Relaxed);
                self.shared.ready.notify_one();
            }
            Ok(IconSubmit::Coalesced) => {
                self.coalesced.fetch_add(1, Ordering::Relaxed);
            }
            Ok(IconSubmit::DroppedLow) => {
                self.dropped_low.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {}
        }
        submitted
    }

    /// Drains completed jobs and releases their dedupe keys for future work.
    /// `&mut self` because the result receiver is consumed through the
    /// service's own forward path; callers hold the service behind `Arc`
    /// and forward from a single owned task.
    pub fn drain_ready(&mut self) -> Vec<IconResult> {
        let mut ready = Vec::new();
        let Some(results) = &self.results else {
            return ready;
        };
        while let Ok(result) = results.try_recv() {
            // Wake-to-apply: worker completion envelope -> applied here.
            let wake_to_apply = result.completed_at.elapsed();
            flamewm_profiler::record_elapsed("icon.result.wake_to_apply", wake_to_apply);
            observe_icon_raster(is_non_fallback(&result.result));
            self.finish(&result);
            emit_apply(&result, wake_to_apply);
            ready.push(result);
        }
        self.emit_service_summary("drain");
        ready
    }

    /// Takes one completed icon job, releasing its dedupe key.
    pub fn take_ready(&mut self) -> Option<IconResult> {
        let result = self.results.as_ref()?.recv().ok()?;
        let wake_to_apply = result.completed_at.elapsed();
        flamewm_profiler::record_elapsed("icon.result.wake_to_apply", wake_to_apply);
        observe_icon_raster(is_non_fallback(&result.result));
        self.finish(&result);
        emit_apply(&result, wake_to_apply);
        self.emit_service_summary("take");
        Some(result)
    }

    fn finish(&self, result: &IconResult) {
        self.shared
            .queue
            .lock()
            .expect("icon queue poisoned")
            .pending
            .remove(&result.key);
        if let Ok(mut cache) = self.result_cache.lock() {
            // Final shared result cache: bounded by entries and bytes.
            let cost = result_memory(&result.result);
            while (cache.entries.len() >= RESULT_CACHE_ENTRIES
                || cache.bytes + cost > RESULT_CACHE_BYTES)
                && !cache.entries.is_empty()
            {
                let Some(first) = cache.entries.keys().next().cloned() else {
                    break;
                };
                cache.remove(&first);
            }
            cache.insert(result.key.clone(), result.result.clone());
        }
    }
}

/// Shared result cache map plus the running sum of its [`result_memory`] costs,
/// kept in step on every insert and remove so eviction never rescans the map.
#[derive(Default)]
struct ResultCache {
    entries: HashMap<Arc<IconKey>, Result<Rgba8Raster, IconError>>,
    bytes: usize,
}

impl ResultCache {
    fn insert(&mut self, key: Arc<IconKey>, value: Result<Rgba8Raster, IconError>) {
        self.bytes += result_memory(&value);
        if let Some(replaced) = self.entries.insert(key, value) {
            self.bytes -= result_memory(&replaced);
        }
    }

    fn remove(&mut self, key: &IconKey) {
        if let Some(removed) = self.entries.remove(key) {
            self.bytes -= result_memory(&removed);
        }
    }
}

fn is_non_fallback(result: &Result<Rgba8Raster, IconError>) -> bool {
    result
        .as_ref()
        .is_ok_and(|raster| raster.fallback_reason.is_none())
}

fn result_memory(result: &Result<Rgba8Raster, IconError>) -> usize {
    match result {
        Ok(raster) => raster.pixels.len() + 96,
        Err(_) => 96,
    }
}

/// Empty generation-0 index used when no theme state is provided yet.
fn empty_index() -> IconLookupIndex {
    IconLookupIndex::new(
        Vec::new(),
        vec!["hicolor".to_owned()],
        HashMap::new(),
        HashMap::new(),
        0,
    )
}

/// Shared state between a [`ThemeRebuilder`] and its rebuild thread.
struct RebuildState {
    /// Set by `request` before sending; cleared by the thread after the
    /// check (and after publishing any result into `ready`).
    in_flight: AtomicBool,
    /// Finished replacement index, waiting for the owner to poll it.
    ready: Mutex<Option<Arc<IconLookupIndex>>>,
}

/// Off-thread theme watch and rebuild owner.
///
/// [`IconLookupIndex::needs_rebuild`] stats the watch set and, on change, does
/// a full index build (directory walks). That work runs on one dedicated
/// thread here; callers only submit the current index and poll for the
/// result, so no UI or caller thread blocks on it. At most one check is in
/// flight; a finished result is held until polled. The thread exits and is
/// joined when the rebuilder is dropped.
/// Work for the rebuild thread: a throttled watch check of `current`, or a full build of
/// `current`'s search roots under an explicit theme chain.
enum ThemeJob {
    Watch(Arc<IconLookupIndex>),
    Theme {
        current: Arc<IconLookupIndex>,
        theme_names: Vec<String>,
    },
}

pub struct ThemeRebuilder {
    tx: Option<mpsc::Sender<ThemeJob>>,
    state: Arc<RebuildState>,
    handle: Option<JoinHandle<()>>,
}

impl ThemeRebuilder {
    /// Spawns the rebuild thread (idle until a request arrives).
    #[must_use]
    pub fn new() -> Self {
        Self::with_wake(None)
    }

    /// Like [`ThemeRebuilder::new`], but a published result also writes one byte to
    /// `wake`, the owner's reactor wake pipe, so an idle event loop picks it up.
    #[must_use]
    pub fn with_wake(wake: Option<Arc<Mutex<UnixStream>>>) -> Self {
        let (tx, rx) = mpsc::channel::<ThemeJob>();
        let state = Arc::new(RebuildState {
            in_flight: AtomicBool::new(false),
            ready: Mutex::new(None),
        });
        let thread_state = Arc::clone(&state);
        let handle = thread::Builder::new()
            .name("icon-theme-rebuild".to_owned())
            .spawn(move || rebuild_loop(&rx, &thread_state, wake.as_ref()))
            .expect("spawn icon theme rebuild thread");
        Self {
            tx: Some(tx),
            state,
            handle: Some(handle),
        }
    }

    /// Queues a watch check of `current`. Non-blocking. Returns `true` when a
    /// check was queued, `false` when one is already in flight or an
    /// unpolled result is waiting.
    pub fn request(&self, current: Arc<IconLookupIndex>) -> bool {
        self.submit(ThemeJob::Watch(current))
    }

    /// Queues a full build of `current`'s roots under `theme_names`. Same gating as
    /// [`ThemeRebuilder::request`].
    pub fn request_theme(&self, current: Arc<IconLookupIndex>, theme_names: Vec<String>) -> bool {
        self.submit(ThemeJob::Theme {
            current,
            theme_names,
        })
    }

    fn submit(&self, job: ThemeJob) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        if self.state.in_flight.swap(true, Ordering::AcqRel) {
            return false;
        }
        let has_ready = self.state.ready.lock().map_or(true, |slot| slot.is_some());
        if has_ready || tx.send(job).is_err() {
            self.state.in_flight.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// Takes the finished replacement index, if any. Never blocks on the
    /// rebuild thread (a short mutex guards only the slot swap).
    #[must_use]
    pub fn take(&self) -> Option<Arc<IconLookupIndex>> {
        self.state.ready.lock().ok()?.take()
    }

    /// `true` when no check is queued or running on the rebuild thread.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        !self.state.in_flight.load(Ordering::Acquire)
    }
}

impl Default for ThemeRebuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ThemeRebuilder {
    fn drop(&mut self) {
        // Closing the sender ends the thread's `recv` loop.
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn rebuild_loop(
    rx: &Receiver<ThemeJob>,
    state: &RebuildState,
    wake: Option<&Arc<Mutex<UnixStream>>>,
) {
    while let Ok(job) = rx.recv() {
        let next = match job {
            // Throttled (5s) stat; a full build only when watched mtimes changed.
            ThemeJob::Watch(current) => current.needs_rebuild(),
            ThemeJob::Theme {
                current,
                theme_names,
            } => Some(IconLookupIndex::build(
                &theme_names,
                &current.data_dirs,
                current.generation.saturating_add(1),
            )),
        };
        if let Some(next) = next {
            if let Ok(mut slot) = state.ready.lock() {
                *slot = Some(Arc::new(next));
            }
            if let Some(wake) = wake {
                if let Ok(mut stream) = wake.lock() {
                    let _ = stream.write_all(&[1]);
                }
            }
        }
        // Cleared after publishing so `is_idle` implies the result is visible.
        state.in_flight.store(false, Ordering::Release);
    }
}

impl<R> Drop for IconService<R> {
    fn drop(&mut self) {
        self.shared
            .queue
            .lock()
            .expect("icon queue poisoned")
            .stopped = true;
        self.shared.ready.notify_all();
        // Wake blocked result sends before waiting for worker termination.
        self.results.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop<R: IconResolve>(
    shared: Arc<SharedQueue>,
    result_tx: SyncSender<IconResult>,
    wake_tx: Option<Arc<Mutex<UnixStream>>>,
    result_cache: Arc<Mutex<ResultCache>>,
    completed: Arc<AtomicU64>,
    mut resolver: R,
) {
    // Epoch of the theme index this worker last adopted (0 = construction).
    let mut applied_epoch: u64 = 0;
    loop {
        // Peek the next key; serve from the shared result cache when a prior
        // worker already completed it (still forwards so waiter observes it).
        enum Next {
            Cached(IconResult),
            Resolve(Arc<IconKey>),
        }
        let next: Option<Next> = {
            let mut queue = shared.queue.lock().expect("icon queue poisoned");
            while !queue.stopped && queue.high.is_empty() && queue.low.is_empty() {
                queue = shared.ready.wait(queue).expect("icon queue poisoned");
            }
            if queue.stopped {
                return;
            }
            let front_key = queue
                .high
                .front()
                .map(|q| q.key.clone())
                .or_else(|| queue.low.front().map(|q| q.key.clone()))
                .expect("queued icon");
            if let Some(hit) = result_cache
                .lock()
                .ok()
                .and_then(|c| c.entries.get(&front_key).cloned())
            {
                let queued = if queue.high.front().is_some_and(|q| q.key == front_key) {
                    queue.high.pop_front().expect("queued icon")
                } else {
                    queue.low.pop_front().expect("queued icon")
                };
                // Queue wait: envelope submitted_at -> dequeue, not Condvar idle.
                flamewm_profiler::record_elapsed("icon.queue.wait", queued.submitted_at.elapsed());
                Some(Next::Cached(IconResult {
                    key: queued.key,
                    result: hit,
                    completed_at: std::time::Instant::now(),
                }))
            } else {
                let queued = queue
                    .high
                    .pop_front()
                    .or_else(|| queue.low.pop_front())
                    .expect("queued icon");
                flamewm_profiler::record_elapsed("icon.queue.wait", queued.submitted_at.elapsed());
                Some(Next::Resolve(queued.key))
            }
        };
        match next {
            Some(Next::Cached(done)) => {
                completed.fetch_add(1, Ordering::Relaxed);
                if result_tx.send(done).is_err() {
                    return;
                }
            }
            Some(Next::Resolve(key)) => {
                // Adopt a theme swap before resolving: the epoch check is one
                // atomic load; the index Arc is cloned under a short lock that
                // the UI thread also holds only for the swap itself.
                let epoch = shared.theme_epoch.load(Ordering::Acquire);
                if epoch != applied_epoch {
                    applied_epoch = epoch;
                    let index = shared.theme.lock().map_or_else(
                        |poisoned| Arc::clone(&poisoned.into_inner()),
                        |slot| Arc::clone(&slot),
                    );
                    resolver.apply_theme(index);
                }
                // Resolution runs without holding the queue lock.
                let result = resolver.resolve(&key);
                let ok = result.is_ok();
                let fallback = result
                    .as_ref()
                    .ok()
                    .and_then(|raster| raster.fallback_reason.as_ref());
                flamewm_debug::emit(
                    flamewm_debug::ICON_RESOLVE_RESULT,
                    flamewm_debug::ICON_RESOLVE_RESULT_COOLDOWN,
                    || format!("key={key:?} ok={ok}"),
                );
                if let Some(reason) = fallback {
                    flamewm_debug::emit(
                        flamewm_debug::ICON_FALLBACK,
                        flamewm_debug::ICON_FALLBACK_COOLDOWN,
                        || format!("key={key:?} reason={reason}"),
                    );
                }
                completed.fetch_add(1, Ordering::Relaxed);
                if result_tx
                    .send(IconResult {
                        result,
                        key,
                        completed_at: std::time::Instant::now(),
                    })
                    .is_err()
                {
                    return;
                }
            }
            None => return,
        }
        // Wake FD: one byte per completed result.
        if let Some(wake) = &wake_tx {
            if let Ok(mut stream) = wake.lock() {
                let _ = stream.write_all(&[1]);
            }
        }
    }
}

/// Process-wide origin for the first-icon milestone: the first icon request.
static FIRST_ICON_REQUEST: OnceLock<std::time::Instant> = OnceLock::new();
/// Set once per process when the first non-fallback raster is observed.
static FIRST_NON_FALLBACK_RECORDED: AtomicBool = AtomicBool::new(false);

/// Profiler label for the one-shot time-to-first-non-fallback-icon milestone.
pub const FIRST_RASTER_LABEL: &str = "icon.first_raster";

/// Marks the first icon request of this process (first call wins).
pub fn note_icon_request() {
    let _ = FIRST_ICON_REQUEST.get_or_init(std::time::Instant::now);
}

/// Pure once-only milestone: returns the elapsed time from `origin` to `now`
/// on the first call with `non_fallback == true`, and `None` afterwards.
pub fn first_non_fallback_elapsed(
    origin: std::time::Instant,
    now: std::time::Instant,
    non_fallback: bool,
    recorded: &AtomicBool,
) -> Option<std::time::Duration> {
    if !non_fallback {
        return None;
    }
    recorded
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()?;
    Some(now.saturating_duration_since(origin))
}

/// Records the one-per-process first-non-fallback-icon event through the
/// profiler. Shared by the service and the shell icon loader.
pub fn observe_icon_raster(non_fallback: bool) {
    let Some(origin) = FIRST_ICON_REQUEST.get() else {
        return;
    };
    if let Some(elapsed) = first_non_fallback_elapsed(
        *origin,
        std::time::Instant::now(),
        non_fallback,
        &FIRST_NON_FALLBACK_RECORDED,
    ) {
        flamewm_profiler::record_elapsed(FIRST_RASTER_LABEL, elapsed);
    }
}

/// Per-icon apply summary at 5000ms cooldown from the finished result.
fn emit_apply(result: &IconResult, wake_to_apply: std::time::Duration) {
    let ok = result.result.is_ok();
    let fallback = result
        .result
        .as_ref()
        .ok()
        .and_then(|raster| raster.fallback_reason.as_ref());
    if fallback.is_some() {
        flamewm_debug::emit(
            flamewm_debug::ICON_FALLBACK,
            flamewm_debug::ICON_FALLBACK_COOLDOWN,
            || format!("apply key={:?} reason={}", result.key, fallback.unwrap()),
        );
    }
    flamewm_debug::emit(
        flamewm_debug::ICON_APPLY_SUMMARY,
        flamewm_debug::ICON_APPLY_SUMMARY_COOLDOWN,
        || {
            format!(
                "key={:?} ok={ok} wake_to_apply_ms={:.3}",
                result.key,
                wake_to_apply.as_secs_f64() * 1000.0
            )
        },
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::icons::{IconLookupPurpose, IconOrigin, IconRequest, IconSize};

    struct FakeResolver(Arc<AtomicUsize>);

    impl IconResolve for FakeResolver {
        fn resolve(&mut self, _: &IconKey) -> Result<Rgba8Raster, IconError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(IconError::InvalidSize {
                logical: 0,
                physical: 0,
            })
        }
    }

    fn fake_service(workers: usize) -> (IconService<FakeResolver>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = IconService::new(workers, 8, {
            let calls = Arc::clone(&calls);
            move || FakeResolver(Arc::clone(&calls))
        });
        (service, calls)
    }

    fn key(name: &str) -> IconKey {
        IconKey::new(
            IconLookupPurpose::Application,
            IconRequest::name(name),
            IconSize::new(16, 16),
            1,
            1,
        )
    }

    #[test]
    fn core_spawns_exactly_twenty_workers_with_index_built_once() {
        use crate::icon_theme::INDEX_BUILD_COUNT;
        use std::sync::atomic::Ordering;
        // One-build proof: hermetic empty data dirs keep the build cheap
        // (no live Breeze/hicolor tree walk). `INDEX_BUILD_COUNT` is
        // process-global and shared with parallel lib tests, so exact
        // absolute counts are racy; the race-free invariant is `Arc::ptr_eq`
        // (workers literally share the one built allocation, zero rebuilds
        // by construction: `from_shared_index`/`fork_worker` never call
        // `IconLookupIndex::build`). Single pass: no retry loop over full
        // index builds (that loop was the workspace-run CPU hog).
        let before = INDEX_BUILD_COUNT.load(Ordering::SeqCst);
        let base = IconResolver::new(
            std::path::PathBuf::from("/tmp"),
            vec!["hicolor".to_owned()],
            Vec::new(),
            9001,
        );
        assert!(
            INDEX_BUILD_COUNT.load(Ordering::SeqCst) - before >= 1,
            "base performs the one IconLookupIndex::build for its generation"
        );
        let shared = base.shared_index().clone();
        let workers: Vec<IconResolver> = (0..20)
            .map(|_| {
                IconResolver::from_shared_index(
                    std::path::PathBuf::from("/tmp"),
                    vec!["hicolor".to_owned()],
                    Vec::new(),
                    9001,
                    Arc::clone(&shared),
                )
            })
            .collect();
        assert_eq!(workers.len(), 20, "exactly 20 forked workers");
        for worker in &workers {
            assert!(
                Arc::ptr_eq(worker.shared_index(), &shared),
                "fork shares the one built index with zero rebuilds"
            );
        }
        let base41 = Arc::new(base);
        let shared_index = Arc::clone(base41.shared_index());
        let mut service = IconService::with_index(
            20,
            256,
            move || base41.fork_worker(),
            Arc::clone(&shared_index),
        );
        assert!(
            Arc::ptr_eq(&service.shared_index(), &shared_index),
            "service shares the one built index with zero rebuilds"
        );
        assert_eq!(service.worker_count(), 20);
        assert_eq!(service.queue_capacity(), 256);
        // clamp: >20 clamps to 20, 0 clamps to 1; queue clamps to 256.
        assert_eq!(crate::icon_service::clamp_worker_count(99), 20);
        assert_eq!(crate::icon_service::clamp_worker_count(0), 1);
        assert_eq!(crate::icon_service::ICON_RESOLVE_CONCURRENCY, 20);
        assert_eq!(crate::icon_service::ICON_QUEUE_CAPACITY, 256);
        // 20 unique HIGH requests complete; duplicate coalesces in pending.
        for index in 0..20 {
            let job = IconJob {
                key: key(&format!("core-{index}")),
                priority: IconPriority::High,
            };
            assert_eq!(service.submit(job), Ok(IconSubmit::Enqueued));
        }
        assert_eq!(
            service.submit(IconJob {
                key: key("core-0"),
                priority: IconPriority::High,
            }),
            Ok(IconSubmit::Coalesced)
        );
        let mut got = 0;
        for _ in 0..20 {
            if service.take_ready().is_some() {
                got += 1;
            }
        }
        assert_eq!(got, 20, "20 unique requests complete");
        // Cache bounds: repeated misses never exceed final shared bounds.
        assert!(service.result_cache_len() <= crate::icon_service::RESULT_CACHE_ENTRIES);
        // Workers only call `IconResolve::resolve` -> `prepare_key`
        // (FS + decode); assert the worker fn has no UI/X11 surface calls.
        let src = include_str!("icon_service.rs");
        let start = src.find("fn worker_loop").expect("worker_loop");
        let tail = &src[start..];
        let end = tail.find("\nfn emit_apply").expect("worker end");
        let worker = &tail[..end];
        assert!(!worker.contains("x11"), "no X11 in workers");
        assert!(!worker.contains("X11"), "no X11 in workers");
        assert!(!worker.contains("UiDocument"), "no UI in workers");
    }

    #[test]
    fn duplicate_key_resolves_once_and_result_has_no_target() {
        let (mut service, calls) = fake_service(2);
        let job = IconJob {
            key: key("same"),
            priority: IconPriority::High,
        };
        assert_eq!(service.submit(job.clone()), Ok(IconSubmit::Enqueued));
        assert_eq!(service.submit(job), Ok(IconSubmit::Coalesced));
        let result = service.take_ready().expect("one completed result");
        assert_eq!(*result.key, key("same"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn installed_theme_index_bumps_generation_and_old_generation_keys_miss() {
        let (mut service, _calls) = fake_service(1);
        assert_eq!(service.theme_generation(), 0);
        let next = Arc::new(IconLookupIndex::new(
            Vec::new(),
            vec!["hicolor".to_owned()],
            HashMap::new(),
            HashMap::new(),
            1,
        ));
        assert_eq!(service.install_theme_index(next), 1);
        assert_eq!(service.theme_generation(), 1);
        assert_eq!(service.shared_index().generation, 1);

        let old_key = IconKey::new(
            IconLookupPurpose::Application,
            IconRequest::name("app"),
            IconSize::new(16, 16),
            0,
            0,
        );
        let new_key = IconKey::new(
            IconLookupPurpose::Application,
            IconRequest::name("app"),
            IconSize::new(16, 16),
            service.theme_generation(),
            0,
        );
        assert_ne!(old_key, new_key, "generation is part of key identity");

        // A result completed under the old generation is cached under the old
        // key only; the new generation key misses it and must re-resolve.
        assert_eq!(
            service.submit(IconJob {
                key: old_key.clone(),
                priority: IconPriority::High,
            }),
            Ok(IconSubmit::Enqueued)
        );
        service.take_ready().expect("old-generation job completes");
        assert!(service.cached(&old_key).is_some());
        assert!(service.cached(&new_key).is_none());
    }

    #[test]
    fn late_theme_rebuild_result_cannot_replace_newer_installed_generation() {
        let (service, _calls) = fake_service(1);
        let index = |generation| {
            Arc::new(IconLookupIndex::new(
                Vec::new(),
                vec!["hicolor".to_owned()],
                HashMap::new(),
                HashMap::new(),
                generation,
            ))
        };

        assert_eq!(service.install_theme_index(index(8)), 8);
        // This models an asynchronous rebuild result based on a prior
        // generation arriving after a distinct newer refresh was installed.
        assert_eq!(service.install_theme_index(index(7)), 8);
        assert_eq!(service.theme_generation(), 8);
        assert_eq!(service.shared_index().generation, 8);
    }

    fn wait_until_idle<R: IconResolve>(service: &IconService<R>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !service.theme_rebuild_idle() {
            assert!(
                std::time::Instant::now() < deadline,
                "theme rebuild thread did not finish within deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Fresh watch root plus a generation-3 index whose epoch mtime forces a rebuild check.
    fn stale_watch_index(tag: &str) -> (std::path::PathBuf, IconLookupIndex) {
        let root =
            std::env::temp_dir().join(format!("flamewm-service-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("watch root");
        let mut stale = IconLookupIndex::new(
            vec![root.clone()],
            Vec::new(),
            HashMap::new(),
            HashMap::new(),
            3,
        );
        // Epoch mtime never matches the real root, so the check must rebuild.
        stale.watch_mtime = Some(std::time::UNIX_EPOCH);
        (root, stale)
    }

    /// Polls the rebuild slot until a finished index arrives or the deadline passes.
    fn poll_theme_index<R: IconResolve>(
        service: &IconService<R>,
        what: &str,
    ) -> Arc<IconLookupIndex> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(index) = service.poll_theme_rebuild() {
                return index;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{what} must arrive within deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn submitted_theme_rebuild_completes_off_thread_with_next_generation() {
        let (service, _calls) = fake_service(1);
        let (root, stale) = stale_watch_index("rebuild");
        service.install_theme_index(Arc::new(stale));

        assert!(service.request_theme_rebuild(), "check must queue");
        let rebuilt = poll_theme_index(&service, "rebuilt index");
        wait_until_idle(&service);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(rebuilt.generation, 4, "rebuilt generation is installed + 1");
        assert_eq!(service.install_theme_index(rebuilt), 4);
        assert_eq!(service.theme_generation(), 4);
        assert!(service.poll_theme_rebuild().is_none(), "result taken once");
    }

    #[test]
    fn requested_theme_chain_builds_off_thread_with_next_generation() {
        let (service, _calls) = fake_service(1);
        service.install_theme_index(Arc::new(IconLookupIndex::build(&[], &[], 5)));

        let chain = vec!["flame-no-such-theme".to_owned(), "hicolor".to_owned()];
        assert!(
            service.request_theme_change(chain.clone()),
            "theme build must queue"
        );
        let built = poll_theme_index(&service, "requested theme index");
        wait_until_idle(&service);
        assert_eq!(
            built.theme_names, chain,
            "built index uses the requested chain"
        );
        assert_eq!(built.generation, 6, "built generation is installed + 1");
        assert_eq!(service.install_theme_index(built), 6);
    }

    #[test]
    fn published_theme_rebuild_wakes_reactor_fd() {
        let (service, _calls) = fake_service(1);
        let (root, stale) = stale_watch_index("wake");
        service.install_theme_index(Arc::new(stale));
        let mut reader = service
            .wake_reader
            .as_ref()
            .expect("wake pipe")
            .try_clone()
            .expect("clone wake reader");

        assert!(service.request_theme_rebuild(), "check must queue");
        wait_until_idle(&service);
        let mut byte = [0_u8; 1];
        let woke = reader
            .read(&mut byte)
            .expect("wake pipe readable after publish");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            woke, 1,
            "a published theme rebuild must write the reactor wake fd"
        );
        assert!(
            service.poll_theme_rebuild().is_some(),
            "result is held for polling"
        );
    }

    #[test]
    fn unchanged_theme_queues_check_but_yields_no_new_index() {
        let (service, _calls) = fake_service(1);
        // Built index records the live watch mtime, so the check finds no change.
        let current = IconLookupIndex::build(&[], &[], 5);
        service.install_theme_index(Arc::new(current));

        assert!(service.request_theme_rebuild(), "check must queue");
        wait_until_idle(&service);
        assert!(
            service.poll_theme_rebuild().is_none(),
            "unchanged theme must not produce a replacement index"
        );
        assert_eq!(service.theme_generation(), 5, "generation unchanged");
        assert!(
            service.request_theme_rebuild(),
            "idle service accepts a new check"
        );
        wait_until_idle(&service);
        assert!(service.poll_theme_rebuild().is_none());
    }

    #[test]
    fn high_replaces_queued_low_but_full_high_queue_returns_job() {
        let mut queue = Queue {
            high: VecDeque::new(),
            low: VecDeque::new(),
            pending: HashSet::new(),
            capacity: 1,
            stopped: false,
        };
        assert_eq!(
            queue.submit(IconJob {
                key: key("first"),
                priority: IconPriority::Low
            }),
            Ok(IconSubmit::Enqueued)
        );
        let high = IconJob {
            key: key("high"),
            priority: IconPriority::High,
        };
        assert_eq!(queue.submit(high), Ok(IconSubmit::Enqueued));
        let required = IconJob {
            key: key("required"),
            priority: IconPriority::High,
        };
        assert_eq!(
            queue.submit(required.clone()),
            Err(IconSubmitError::HighQueueFull(required))
        );
        assert_eq!(
            queue.submit(IconJob {
                key: key("low"),
                priority: IconPriority::Low
            }),
            Ok(IconSubmit::DroppedLow)
        );
    }

    #[test]
    fn result_cache_eviction_honours_byte_and_entry_limits() {
        use std::time::Instant;

        let service = IconService::new(1, 8, || FakeResolver(Arc::new(AtomicUsize::new(0))));
        let finish_raster = |name: String, pixels: usize| IconResult {
            key: Arc::new(key(&name)),
            result: Ok(Rgba8Raster {
                source: name,
                width: 1,
                height: 1,
                pixels: vec![0; pixels],
                origin: IconOrigin::PackagedFlame,
                fallback_reason: None,
            }),
            completed_at: Instant::now(),
        };
        // Large rasters: the byte bound binds before the entry bound.
        for index in 0..16 {
            service.finish(&finish_raster(format!("big-{index}"), 256 * 1024));
        }
        {
            let cache = service.result_cache.lock().expect("cache");
            assert!(cache.bytes <= RESULT_CACHE_BYTES, "byte bound honoured");
            assert!(cache.entries.len() < 16, "byte bound evicted entries");
            let recomputed: usize = cache.entries.values().map(result_memory).sum();
            assert_eq!(cache.bytes, recomputed, "running total matches map");
        }
        // Tiny rasters on a fresh service: the entry bound binds exactly.
        let small = IconService::new(1, 8, || FakeResolver(Arc::new(AtomicUsize::new(0))));
        for index in 0..(RESULT_CACHE_ENTRIES + 10) {
            small.finish(&finish_raster(format!("small-{index}"), 0));
        }
        assert_eq!(small.result_cache_len(), RESULT_CACHE_ENTRIES);
        let cache = small.result_cache.lock().expect("cache");
        let recomputed: usize = cache.entries.values().map(result_memory).sum();
        assert_eq!(cache.bytes, recomputed, "running total matches map");
    }

    #[test]
    fn first_non_fallback_elapsed_fires_once_and_skips_fallbacks() {
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant};

        let origin = Instant::now();
        let recorded = AtomicBool::new(false);
        let fallback_at = origin + Duration::from_millis(5);
        assert_eq!(
            first_non_fallback_elapsed(origin, fallback_at, false, &recorded),
            None
        );
        let first_at = origin + Duration::from_millis(12);
        assert_eq!(
            first_non_fallback_elapsed(origin, first_at, true, &recorded),
            Some(Duration::from_millis(12))
        );
        let second_at = origin + Duration::from_millis(40);
        assert_eq!(
            first_non_fallback_elapsed(origin, second_at, true, &recorded),
            None
        );
    }
}
