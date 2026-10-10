use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_control_core::mutations::retryable_system_action;
use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
use flamewm_control_core::{ControlMutationQueue, ControlMutationResult, MutationLane};
use flamewm_dbus_reactor::BusKind;

use super::ControlClient;

/// Counter labels for the stale-revision bounded retry (F20).
pub const STALE_REVISION_COUNTER: &str = "shell.action.stale_revision";
pub const RETRY_COUNTER: &str = "shell.action.retry";
pub const RETRY_SUCCESS_COUNTER: &str = "shell.action.retry_success";
pub const RETRY_REJECTED_GENERATION_COUNTER: &str = "shell.action.retry_rejected_generation";

static STALE_REVISION_COUNT: AtomicU64 = AtomicU64::new(0);
static RETRY_COUNT: AtomicU64 = AtomicU64::new(0);
static RETRY_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static RETRY_REJECTED_GENERATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Snapshot of the stale-revision retry counters, in
/// `(stale_revision, retry, retry_success, retry_rejected_generation)` order.
#[must_use]
pub fn stale_retry_stats() -> (u64, u64, u64, u64) {
    (
        STALE_REVISION_COUNT.load(Ordering::Relaxed),
        RETRY_COUNT.load(Ordering::Relaxed),
        RETRY_SUCCESS_COUNT.load(Ordering::Relaxed),
        RETRY_REJECTED_GENERATION_COUNT.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
fn reset_stale_retry_stats_for_test() {
    STALE_REVISION_COUNT.store(0, Ordering::Relaxed);
    RETRY_COUNT.store(0, Ordering::Relaxed);
    RETRY_SUCCESS_COUNT.store(0, Ordering::Relaxed);
    RETRY_REJECTED_GENERATION_COUNT.store(0, Ordering::Relaxed);
}

fn is_stale_revision(outcome: &Result<ControlResponse, ControlError>) -> bool {
    matches!(
        outcome,
        Err(error) if error.code == ErrorCode::StaleRevision
    )
}

/// Scriptable core of the mutation call path so tests can drive the exact
/// F20 retry sequence without a bus. `call` performs one request over the
/// worker's connection.
fn execute_with_calls(
    request: &ControlRequest,
    call: &mut dyn FnMut(&ControlRequest) -> Result<ControlResponse, ControlError>,
) -> Result<ControlResponse, ControlError> {
    let first = call(request);
    let action = match (request, &first) {
        (ControlRequest::SystemAction { action, .. }, outcome) if is_stale_revision(outcome) => {
            action.clone()
        }
        _ => return first,
    };
    STALE_REVISION_COUNT.fetch_add(1, Ordering::Relaxed);
    let snapshot = match call(&ControlRequest::GetSystem) {
        Ok(ControlResponse::System(snapshot)) => snapshot,
        _ => return first,
    };
    let Some(fresh_action) = retryable_system_action(&action, &snapshot) else {
        RETRY_REJECTED_GENERATION_COUNT.fetch_add(1, Ordering::Relaxed);
        return first;
    };
    let rebuilt = ControlRequest::SystemAction {
        action: fresh_action,
        expected_revision: snapshot.revision,
    };
    RETRY_COUNT.fetch_add(1, Ordering::Relaxed);
    let second = call(&rebuilt);
    if second.is_ok() {
        RETRY_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    second
}

fn execute_mutation(
    client: &ControlClient,
    request: &ControlRequest,
) -> Result<ControlResponse, ControlError> {
    execute_with_calls(request, &mut |req| client.call(req))
}

/// Thread-confined mutation dispatcher.
///
/// The worker owns one `ControlClient` connection and never touches UI state:
/// it pops queued mutations, executes them over its own connection in FIFO
/// order (same-lane order falls out of the single global FIFO), and pushes
/// results with a wake-FD byte so the WM reactor can drain without polling.
/// `shutdown` closes the queue and joins the worker thread.
pub struct MutationDispatcher {
    queue: Arc<ControlMutationQueue>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl MutationDispatcher {
    fn spawn_worker(queue: Arc<ControlMutationQueue>, bus: BusKind) -> JoinHandle<()> {
        std::thread::spawn(move || {
            let client = ControlClient::connect(bus).ok();
            loop {
                let Some(mutation) = queue.take_pending() else {
                    break;
                };
                let outcome = match &client {
                    Some(client) => execute_mutation(client, &mutation.request),
                    None => Err(ControlError {
                        name: flamewm_control_core::error_name(ErrorCode::Unavailable),
                        code: ErrorCode::Unavailable,
                        message: "mutation worker bus unavailable".to_owned(),
                    }),
                };
                let _ = queue.push_result(ControlMutationResult {
                    id: mutation.id,
                    lane: mutation.lane,
                    outcome,
                });
            }
        })
    }

    pub fn start(bus: BusKind, capacity: usize) -> FlameResult<Arc<Self>> {
        let queue = Arc::new(ControlMutationQueue::new(capacity));
        let worker = Self::spawn_worker(Arc::clone(&queue), bus);
        Ok(Arc::new(Self {
            queue,
            worker: Mutex::new(Some(worker)),
        }))
    }

    #[must_use]
    pub fn queue(&self) -> &Arc<ControlMutationQueue> {
        &self.queue
    }

    /// Submit a mutation; explicit `Busy` when the bounded queue is full.
    pub fn submit(self: &Arc<Self>, request: ControlRequest) -> Result<u64, ControlError> {
        let lane = MutationLane::for_request(&request);
        self.queue.submit(lane, request)
    }

    /// Nonblocking result drain for the reactor turn.
    pub fn try_recv_result(&self) -> Option<ControlMutationResult> {
        self.queue.try_recv_result()
    }

    /// Blocking wait used by tests; bounded by `timeout`, never a sleep loop.
    pub fn wait_for_result(&self, timeout: std::time::Duration) -> Option<ControlMutationResult> {
        self.queue.wait_for_result(timeout)
    }

    /// Raw wake fd the WM reactor polls.
    #[must_use]
    pub fn wake_fd(&self) -> i32 {
        self.queue.wake_fd()
    }

    /// Close the queue and join the worker. Idempotent; second call is a
    /// no-op. Returns an error only if the worker thread panicked.
    pub fn shutdown(&self) -> FlameResult<()> {
        self.queue.close();
        let handle = self
            .worker
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(handle) = handle {
            handle.join().map_err(|_| {
                FlameError::new(ErrorCode::InternalFailure, "mutation worker panicked")
            })?;
        }
        Ok(())
    }
}

impl Drop for MutationDispatcher {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Pure FIFO ordering oracle used by tests that cannot open a bus: drains the
/// queue in submission order and records `Unit` results without sleeping.
pub fn drain_queue_inline(queue: &ControlMutationQueue) -> Vec<u64> {
    let mut order = Vec::new();
    while let Some(mutation) = queue.try_take_for_test() {
        let id = mutation.id;
        let lane = mutation.lane;
        queue.push_result(ControlMutationResult {
            id,
            lane,
            outcome: Ok(ControlResponse::Unit),
        })
        .expect("write mutation wake byte");
        order.push(id);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn inline_queue(capacity: usize) -> Arc<ControlMutationQueue> {
        Arc::new(ControlMutationQueue::new(capacity))
    }

    #[test]
    fn dispatcher_queue_single_mutation_result_round_trips_inline() {
        let queue = inline_queue(8);
        let id = queue
            .submit(MutationLane::Windows, ControlRequest::Ping)
            .expect("submit");
        let order = drain_queue_inline(&queue);
        assert_eq!(order, vec![id]);
        let result = queue
            .wait_for_result(Duration::from_secs(5))
            .expect("result without sleep");
        assert_eq!(result.id, id);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatcher_queue_preserves_restore_then_activate_order() {
        use flamewm_api::WindowRef;
        let queue = inline_queue(8);
        let restore = queue
            .submit(
                MutationLane::Windows,
                ControlRequest::RestoreWindow(WindowRef::new(7, 1)),
            )
            .expect("restore");
        let activate = queue
            .submit(
                MutationLane::Windows,
                ControlRequest::ActivateWindow(WindowRef::new(7, 1)),
            )
            .expect("activate");
        let order = drain_queue_inline(&queue);
        assert_eq!(order, vec![restore, activate]);
        let first = queue
            .wait_for_result(Duration::from_secs(5))
            .expect("first");
        let second = queue
            .wait_for_result(Duration::from_secs(5))
            .expect("second");
        assert_eq!((first.id, second.id), (restore, activate));
    }

    #[test]
    fn dispatcher_queue_full_reports_busy_without_dropping() {
        let queue = inline_queue(1);
        queue
            .submit(MutationLane::Panels, ControlRequest::Ping)
            .expect("first fits");
        let error = queue
            .submit(MutationLane::Panels, ControlRequest::Ping)
            .expect_err("full queue must report Busy");
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(queue.pending_len(), 1);
    }

    #[test]
    fn dispatcher_queue_wake_fd_fires_without_sleep() {
        let queue = inline_queue(8);
        let fd = queue.wake_fd();
        assert!(fd >= 0);
        queue
            .submit(MutationLane::System, ControlRequest::Ping)
            .expect("submit");
        drain_queue_inline(&queue);
        assert!(
            queue
                .wake_ready_probe()
                .expect("read and re-arm wake byte"),
            "wake fd must fire without sleep"
        );
        assert!(queue.try_recv_result().is_some());
    }

    #[test]
    fn dispatcher_shutdown_joins_worker_thread() {
        let queue = inline_queue(8);
        let worker_queue = Arc::clone(&queue);
        let handle = std::thread::spawn(move || {
            let mut drained = 0;
            while worker_queue.take_pending().is_some() {
                drained += 1;
            }
            drained
        });
        queue.close();
        let drained = handle.join().expect("worker joins on shutdown");
        assert_eq!(drained, 0);
    }

    fn stale_error() -> ControlError {
        ControlError {
            name: flamewm_control_core::error_name(ErrorCode::StaleRevision),
            code: ErrorCode::StaleRevision,
            message: "stale".to_owned(),
        }
    }

    fn retry_test_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::OnceLock;
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn system_snapshot(
        revision: u64,
        network_generation: u64,
        audio: (u64, u64),
    ) -> ControlResponse {
        ControlResponse::System(flamewm_api::system::SystemSnapshot {
            revision,
            network: flamewm_api::system::NetworkSnapshot {
                generation: network_generation,
                ..flamewm_api::system::NetworkSnapshot::default()
            },
            audio: flamewm_api::system::AudioSnapshot {
                generation: audio.0,
                server_generation: audio.1,
                ..flamewm_api::system::AudioSnapshot::default()
            },
            ..flamewm_api::system::SystemSnapshot::default()
        })
    }

    fn scan_request(revision: u64) -> ControlRequest {
        ControlRequest::SystemAction {
            action: flamewm_api::system::SystemAction::Scan,
            expected_revision: revision,
        }
    }

    #[test]
    fn stale_global_revision_retries_once_with_latest_revision() {
        let _guard = retry_test_lock();
        reset_stale_retry_stats_for_test();
        let calls = std::cell::RefCell::new(Vec::new());
        let mut call = |req: &ControlRequest| {
            calls.borrow_mut().push(req.clone());
            match calls.borrow().len() {
                1 => Err(stale_error()),
                2 => Ok(system_snapshot(9, 1, (1, 1))),
                _ => Ok(ControlResponse::Unit),
            }
        };
        let outcome = execute_with_calls(&scan_request(1), &mut call);
        assert!(outcome.is_ok());
        assert_eq!(calls.borrow().len(), 3);
        match &calls.borrow()[2] {
            ControlRequest::SystemAction {
                action,
                expected_revision,
            } => {
                assert_eq!(*action, flamewm_api::system::SystemAction::Scan);
                assert_eq!(*expected_revision, 9);
            }
            other => panic!("retry must rebuild system action, got {other:?}"),
        }
        assert_eq!(stale_retry_stats(), (1, 1, 1, 0));
    }

    #[test]
    fn network_generation_change_does_not_retry_secret() {
        let _guard = retry_test_lock();
        reset_stale_retry_stats_for_test();
        let request = ControlRequest::SystemAction {
            action: flamewm_api::system::SystemAction::SubmitNetworkSecret {
                request_id: 7,
                generation: 1,
                secret: "s3cr3t".to_owned(),
            },
            expected_revision: 1,
        };
        let calls = std::cell::RefCell::new(Vec::new());
        let mut call = |req: &ControlRequest| {
            calls.borrow_mut().push(req.clone());
            match calls.borrow().len() {
                1 => Err(stale_error()),
                _ => Ok(system_snapshot(9, 2, (1, 1))),
            }
        };
        let outcome = execute_with_calls(&request, &mut call);
        assert_eq!(outcome, Err(stale_error()));
        assert_eq!(calls.borrow().len(), 2, "secret must not be replayed");
        assert!(!format!("{:?}", calls.borrow()).contains("s3cr3t"));
        assert_eq!(stale_retry_stats(), (1, 0, 0, 1));
    }

    #[test]
    fn audio_generation_change_does_not_retry() {
        let _guard = retry_test_lock();
        reset_stale_retry_stats_for_test();
        let request = ControlRequest::SystemAction {
            action: flamewm_api::system::SystemAction::SetVolume(
                flamewm_api::system::AudioVolumeAction {
                    target: flamewm_api::system::AudioTarget::Stream { id: 1 },
                    percent: 50,
                    generation: 1,
                    server_generation: 1,
                },
            ),
            expected_revision: 1,
        };
        let mut call = |req: &ControlRequest| {
            if *req == ControlRequest::GetSystem {
                return Ok(system_snapshot(9, 1, (1, 2)));
            }
            Err(stale_error())
        };
        let outcome = execute_with_calls(&request, &mut call);
        assert_eq!(outcome, Err(stale_error()));
        assert_eq!(stale_retry_stats(), (1, 0, 0, 1));
    }

    #[test]
    fn second_stale_returns_without_loop() {
        let _guard = retry_test_lock();
        reset_stale_retry_stats_for_test();
        let calls = std::cell::RefCell::new(0);
        let mut call = |req: &ControlRequest| {
            *calls.borrow_mut() += 1;
            if *req == ControlRequest::GetSystem {
                return Ok(system_snapshot(9, 1, (1, 1)));
            }
            Err(stale_error())
        };
        let outcome = execute_with_calls(&scan_request(1), &mut call);
        assert_eq!(outcome, Err(stale_error()));
        assert_eq!(*calls.borrow(), 3, "exactly one retry, never a loop");
        assert_eq!(stale_retry_stats(), (1, 1, 0, 0));
    }

    #[test]
    fn non_system_mutation_never_retries() {
        let _guard = retry_test_lock();
        reset_stale_retry_stats_for_test();
        let calls = std::cell::RefCell::new(0);
        let mut call = |_: &ControlRequest| {
            *calls.borrow_mut() += 1;
            Err(stale_error())
        };
        let outcome = execute_with_calls(&ControlRequest::Ping, &mut call);
        assert_eq!(outcome, Err(stale_error()));
        assert_eq!(*calls.borrow(), 1);
        assert_eq!(stale_retry_stats(), (0, 0, 0, 0));
    }

    #[test]
    fn secret_debug_stays_redacted() {
        let action = flamewm_api::system::SystemAction::SubmitNetworkSecret {
            request_id: 7,
            generation: 3,
            secret: "not-for-logs".to_owned(),
        };
        let debug = format!("{action:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("not-for-logs"));
    }
}
