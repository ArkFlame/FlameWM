use std::collections::VecDeque;
use std::io::Write;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::{Condvar, Mutex, MutexGuard};

use flamewm_api::ErrorCode;
use flamewm_api::system::SystemAction;

use super::{ControlError, ControlRequest, ControlResponse};

/// Lane preserves per-domain ordering: a single worker drains one global FIFO,
/// so submissions on the same lane always complete in submission order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationLane {
    Windows,
    Workspaces,
    Displays,
    Shortcuts,
    Panels,
    Session,
    System,
    Settings,
    Applications,
}

impl MutationLane {
    #[must_use]
    pub const fn for_request(request: &ControlRequest) -> Self {
        match request {
            ControlRequest::ActivateWindow(_)
            | ControlRequest::MinimizeWindow(_)
            | ControlRequest::RestoreWindow(_)
            | ControlRequest::CloseWindow(_) => Self::Windows,
            ControlRequest::ActivateWorkspace { .. }
            | ControlRequest::InsertWorkspaceAfter { .. }
            | ControlRequest::RemoveWorkspace { .. } => Self::Workspaces,
            ControlRequest::BeginDisplayMode { .. }
            | ControlRequest::KeepDisplayMode { .. }
            | ControlRequest::RevertDisplayMode { .. }
            | ControlRequest::SetShellScale { .. } => Self::Displays,
            ControlRequest::SetShortcut { .. }
            | ControlRequest::ClearShortcut { .. }
            | ControlRequest::ResetShortcut { .. }
            | ControlRequest::ApplyShortcuts { .. }
            | ControlRequest::BeginHotkeyCapture { .. }
            | ControlRequest::EndHotkeyCapture { .. } => Self::Shortcuts,
            ControlRequest::SetPanelEdge { .. }
            | ControlRequest::SetPanelSize { .. }
            | ControlRequest::PinApp { .. }
            | ControlRequest::UnpinApp { .. }
            | ControlRequest::ReorderTask { .. } => Self::Panels,
            ControlRequest::SessionAction(_) => Self::Session,
            ControlRequest::SystemAction { .. } => Self::System,
            ControlRequest::ApplySettings(_) => Self::Settings,
            ControlRequest::LaunchApplication { .. } => Self::Applications,
            _ => Self::Settings,
        }
    }
}

/// Rebuild guard for stale system-action retry (F20).
///
/// Returns the action to retry when the provider generation carried by
/// `action` still matches `snapshot` (or the action carries no provider
/// generation). Returns `None` when a provider generation moved: the
/// caller must return the original `StaleRevision` without retrying,
/// so a network secret or audio command is never replayed against a
/// provider that already rotated underneath it. The global revision is
/// applied by the caller from `snapshot.revision`; this helper never
/// invents generations.
#[must_use]
pub fn retryable_system_action(
    action: &SystemAction,
    snapshot: &flamewm_api::system::SystemSnapshot,
) -> Option<SystemAction> {
    let fresh = match action {
        SystemAction::SetVolume(volume) => {
            volume.generation == snapshot.audio.generation
                && volume.server_generation == snapshot.audio.server_generation
        }
        SystemAction::SetMute(mute) => {
            mute.generation == snapshot.audio.generation
                && mute.server_generation == snapshot.audio.server_generation
        }
        SystemAction::ConnectWifi { generation, .. }
        | SystemAction::SubmitNetworkSecret { generation, .. }
        | SystemAction::CancelNetworkSecret { generation, .. } => {
            *generation == snapshot.network.generation
        }
        _ => true,
    };
    fresh.then(|| action.clone())
}

#[derive(Debug, Clone)]
pub struct ControlMutation {
    pub id: u64,
    pub lane: MutationLane,
    pub request: ControlRequest,
}

#[derive(Debug, Clone)]
pub struct ControlMutationResult {
    pub id: u64,
    pub lane: MutationLane,
    pub outcome: Result<ControlResponse, ControlError>,
}

impl ControlMutationResult {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.outcome.is_ok()
    }
}

struct QueueInner {
    pending: VecDeque<ControlMutation>,
    results: VecDeque<ControlMutationResult>,
    next_id: u64,
    capacity: usize,
    closed: bool,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn busy(message: &str) -> ControlError {
    ControlError {
        name: super::error_name(ErrorCode::Busy),
        code: ErrorCode::Busy,
        message: message.to_owned(),
    }
}

/// Bounded FIFO mutation queue with a wake pipe.
///
/// `submit` returns an explicit `Busy` error when full; nothing is ever
/// silently dropped. The worker thread writes one byte per completed result so
/// the WM reactor can poll `wake_fd()` and drain with `try_recv_result`.
/// The queue owns no D-Bus or UI handles; the worker owns its own client
/// connection and never touches UI state.
pub struct ControlMutationQueue {
    inner: Mutex<QueueInner>,
    pending_cv: Condvar,
    result_cv: Condvar,
    wake_reader: Mutex<UnixStream>,
    wake_writer: Mutex<UnixStream>,
}

impl ControlMutationQueue {
    pub fn new(capacity: usize) -> Self {
        let (reader, writer) = UnixStream::pair().expect("mutation queue wake pipe must exist");
        reader
            .set_nonblocking(true)
            .expect("mutation wake reader must be nonblocking");
        writer
            .set_nonblocking(true)
            .expect("mutation wake writer must be nonblocking");
        Self {
            inner: Mutex::new(QueueInner {
                pending: VecDeque::new(),
                results: VecDeque::new(),
                next_id: 1,
                capacity: capacity.max(1),
                closed: false,
            }),
            pending_cv: Condvar::new(),
            result_cv: Condvar::new(),
            wake_reader: Mutex::new(reader),
            wake_writer: Mutex::new(writer),
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueInner> {
        lock_unpoisoned(&self.inner)
    }

    fn drain_wake_byte(&self) {
        use std::io::Read;
        let mut discard = [0_u8; 64];
        let _ = lock_unpoisoned(&self.wake_reader).read(&mut discard);
    }

    /// Submit a mutation; explicit `Busy` when the pending queue is full.
    /// High-value requests are never silently dropped: the caller owns retry.
    pub fn submit(&self, lane: MutationLane, request: ControlRequest) -> Result<u64, ControlError> {
        let mut inner = self.lock();
        if inner.closed {
            return Err(busy("mutation queue is shut down"));
        }
        if inner.pending.len() >= inner.capacity {
            return Err(busy("mutation queue is full"));
        }
        let id = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(1).max(1);
        inner
            .pending
            .push_back(ControlMutation { id, lane, request });
        self.pending_cv.notify_one();
        Ok(id)
    }

    /// Blocking pop for the worker thread. Returns `None` after `close()`
    /// once pending work is drained.
    pub fn take_pending(&self) -> Option<ControlMutation> {
        let mut inner = self.lock();
        loop {
            if let Some(mutation) = inner.pending.pop_front() {
                return Some(mutation);
            }
            if inner.closed {
                return None;
            }
            inner = self
                .pending_cv
                .wait(inner)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    /// Nonblocking pop for inline test drains; returns `None` when empty.
    pub fn try_take_for_test(&self) -> Option<ControlMutation> {
        self.lock().pending.pop_front()
    }

    /// Called only by the worker thread after executing a mutation.
    pub fn push_result(&self, result: ControlMutationResult) -> std::io::Result<()> {
        {
            let mut inner = self.lock();
            inner.results.push_back(result);
            self.result_cv.notify_all();
        }
        lock_unpoisoned(&self.wake_writer).write_all(&[1])
    }

    /// Nonblocking result drain for the reactor turn.
    pub fn try_recv_result(&self) -> Option<ControlMutationResult> {
        let result = self.lock().results.pop_front();
        if result.is_some() {
            self.drain_wake_byte();
        }
        result
    }

    /// Blocking wait used by tests; bounded by `timeout`, never a sleep loop.
    pub fn wait_for_result(&self, timeout: std::time::Duration) -> Option<ControlMutationResult> {
        let mut inner = self.lock();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(result) = inner.results.pop_front() {
                drop(inner);
                self.drain_wake_byte();
                return Some(result);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, timed_out) = self
                .result_cv
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(|poison| poison.into_inner());
            inner = guard;
            if timed_out.timed_out() && inner.results.is_empty() {
                return None;
            }
        }
    }

    /// Raw fd the WM reactor polls for result readiness.
    pub fn wake_fd(&self) -> RawFd {
        lock_unpoisoned(&self.wake_reader).as_raw_fd()
    }

    /// Sleep-free readiness probe: nonblocking read of one wake byte, then
    /// re-arm so the reactor pairing (one byte per queued result) is
    /// preserved. Returns true when a result wake was pending.
    pub fn wake_ready_probe(&self) -> std::io::Result<bool> {
        use std::io::Read;
        let mut byte = [0_u8; 1];
        let read = lock_unpoisoned(&self.wake_reader).read(&mut byte);
        let fired = match read {
            Ok(1) => true,
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "mutation queue wake reader reached EOF",
                ));
            }
            Ok(_) => unreachable!("one-byte wake probe cannot read more than one byte"),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => return Err(error),
        };
        if fired {
            lock_unpoisoned(&self.wake_writer).write_all(&byte)?;
        }
        Ok(fired)
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.lock().pending.len()
    }

    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        self.pending_cv.notify_all();
        self.result_cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_queue(capacity: usize) -> ControlMutationQueue {
        ControlMutationQueue::new(capacity)
    }

    fn execute_inline(queue: &ControlMutationQueue, mutation: ControlMutation) {
        queue
            .push_result(ControlMutationResult {
                id: mutation.id,
                lane: mutation.lane,
                outcome: Ok(ControlResponse::Unit),
            })
            .expect("write mutation wake byte");
    }

    #[test]
    fn single_mutation_completes_with_result() {
        let queue = test_queue(8);
        let id = queue
            .submit(MutationLane::Windows, ControlRequest::Ping)
            .expect("submit");
        let pending = queue.take_pending().expect("pending");
        assert_eq!(pending.id, id);
        execute_inline(&queue, pending);
        let result = queue
            .wait_for_result(std::time::Duration::from_secs(5))
            .expect("result");
        assert_eq!(result.id, id);
        assert!(result.is_ok());
    }

    #[test]
    fn same_lane_mutations_complete_in_order() {
        let queue = test_queue(8);
        let first = queue
            .submit(
                MutationLane::Windows,
                ControlRequest::RestoreWindow(flamewm_api::WindowRef::new(7, 1)),
            )
            .expect("first");
        let second = queue
            .submit(
                MutationLane::Windows,
                ControlRequest::ActivateWindow(flamewm_api::WindowRef::new(7, 1)),
            )
            .expect("second");
        let mut order = Vec::new();
        while let Some(mutation) = queue.try_take_for_test() {
            order.push(mutation.id);
            execute_inline(&queue, mutation);
            if order.len() == 2 {
                break;
            }
        }
        assert_eq!(order, vec![first, second]);
        let first_result = queue
            .wait_for_result(std::time::Duration::from_secs(5))
            .expect("first result");
        let second_result = queue
            .wait_for_result(std::time::Duration::from_secs(5))
            .expect("second result");
        assert_eq!(first_result.id, first);
        assert_eq!(second_result.id, second);
    }

    #[test]
    fn queue_full_returns_explicit_busy() {
        let queue = test_queue(1);
        queue
            .submit(MutationLane::Panels, ControlRequest::Ping)
            .expect("first fits");
        let error = queue
            .submit(MutationLane::Panels, ControlRequest::Ping)
            .expect_err("second must be Busy");
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(
            queue.pending_len(),
            1,
            "full entry is retained, not dropped"
        );
    }

    #[test]
    fn wake_fd_fires_on_result() {
        let queue = test_queue(8);
        let fd = queue.wake_fd();
        assert!(fd >= 0);
        let id = queue
            .submit(MutationLane::System, ControlRequest::Ping)
            .expect("submit");
        let pending = queue.take_pending().expect("pending");
        execute_inline(&queue, pending);
        assert!(
            queue.wake_ready_probe().expect("read and re-arm wake byte"),
            "wake fd must fire without sleep"
        );
        let result = queue.try_recv_result().expect("drain");
        assert_eq!(result.id, id);
    }

    #[test]
    fn close_unblocks_worker_and_joins() {
        use std::sync::Arc;
        let queue = Arc::new(test_queue(8));
        let worker_queue = Arc::clone(&queue);
        let handle = std::thread::spawn(move || {
            let mut drained = 0;
            while worker_queue.take_pending().is_some() {
                drained += 1;
            }
            drained
        });
        queue.close();
        let drained = handle.join().expect("worker joins after close");
        assert_eq!(drained, 0);
        assert!(
            queue
                .submit(MutationLane::Windows, ControlRequest::Ping)
                .is_err(),
            "closed queue must reject new submissions"
        );
    }
}
