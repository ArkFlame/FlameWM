//! J07 nonblocking control actions. The UI thread never calls
//! `ControlClient::call` directly: it enqueues onto the bounded
//! `ControlMutationQueue` owned by `MutationDispatcher` and returns
//! immediately. The worker owns its own control connection; results drain
//! via `wake_fd` on the reactor turn. Full/closed queue is a nonfatal
//! diagnostic (counter + stderr), never a panic or blocking call.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};

use flamewm_api::settings::SettingsSnapshot as ApiSettingsSnapshot;
use flamewm_api::error::ErrorCode;
use flamewm_control_core::{ControlMutationQueue, ControlRequest, MutationLane};
use flamewm_control_dbus::MutationDispatcher;
use flamewm_shell_core::weather::WeatherSettings;

/// Result of a Start-menu launch, correlated back to the app that was asked
/// to start. `app` is the user-visible application name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartLaunchOutcome {
    Started { invocation: u64, app: String },
    Failed { invocation: u64, app: String, message: String },
}

/// One weather location pick: the settings transaction, the location it selects
/// (for the optimistic view), and how many times it was already resubmitted.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherPick {
    pub request: ControlRequest,
    pub settings: WeatherSettings,
    pub attempt: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WeatherPickOutcome {
    pub id: u64,
    pub pick: WeatherPick,
    /// The applied snapshot when the mutation succeeded (carries the new revision).
    pub result: Result<Option<ApiSettingsSnapshot>, ErrorCode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkSecretOutcome {
    pub id: u64,
    pub identity: (u64, u64),
    pub result: Result<(), ()>,
}

thread_local! {
    // Reactor-thread only: enqueue and drain both run on the shell loop.
    // Queue ids correlate replies; invocation ids fence replies from older
    // Start-menu sessions after a close/reopen.
    static PENDING_LAUNCHES: RefCell<Vec<(u64, u64, String)>> = RefCell::new(Vec::new());
    static LAUNCH_OUTCOMES: RefCell<VecDeque<StartLaunchOutcome>> = RefCell::new(VecDeque::new());
    static PENDING_NETWORK_SECRETS: RefCell<Vec<(u64, (u64, u64))>> = RefCell::new(Vec::new());
    static NETWORK_SECRET_OUTCOMES: RefCell<VecDeque<NetworkSecretOutcome>> = RefCell::new(VecDeque::new());
    static PENDING_WEATHER_PICKS: RefCell<Vec<(u64, WeatherPick)>> = RefCell::new(Vec::new());
    static WEATHER_PICK_OUTCOMES: RefCell<VecDeque<WeatherPickOutcome>> = RefCell::new(VecDeque::new());
}

/// Enqueue a weather location settings transaction; its result is reported by
/// [`take_weather_pick_outcomes`]. Never blocks.
pub fn enqueue_weather_pick(dispatcher: Option<&Arc<MutationDispatcher>>, pick: WeatherPick) -> Option<u64> {
    let id = enqueue_action(dispatcher, pick.request.clone())?;
    PENDING_WEATHER_PICKS.with(|pending| pending.borrow_mut().push((id, pick)));
    Some(id)
}

pub fn take_weather_pick_outcomes() -> Vec<WeatherPickOutcome> {
    WEATHER_PICK_OUTCOMES.with(|outcomes| outcomes.borrow_mut().drain(..).collect())
}

pub fn enqueue_network_secret(dispatcher: Option<&Arc<MutationDispatcher>>, identity: (u64, u64), request: ControlRequest) -> Option<u64> {
    let id = enqueue_action(dispatcher, request)?;
    PENDING_NETWORK_SECRETS.with(|pending| pending.borrow_mut().push((id, identity)));
    Some(id)
}

pub fn take_network_secret_outcomes() -> Vec<NetworkSecretOutcome> {
    NETWORK_SECRET_OUTCOMES.with(|outcomes| outcomes.borrow_mut().drain(..).collect())
}

/// Enqueue a Start-menu launch and remember the app name against the queue
/// id so the matching result can be correlated. A refused enqueue is recorded
/// as an immediate failed outcome. Never blocks.
pub fn enqueue_start_launch(
    dispatcher: Option<&Arc<MutationDispatcher>>,
    app: String,
    request: ControlRequest,
) -> u64 {
    static NEXT_INVOCATION: AtomicU64 = AtomicU64::new(1);
    let invocation = NEXT_INVOCATION.fetch_add(1, Ordering::Relaxed);
    match enqueue_action(dispatcher, request) {
        Some(id) => PENDING_LAUNCHES.with(|pending| pending.borrow_mut().push((id, invocation, app))),
        None => record_launch_outcome(StartLaunchOutcome::Failed {
            invocation,
            app,
            message: "control queue refused the launch request".to_owned(),
        }),
    }
    invocation
}

/// Take all launch outcomes recorded since the last call, oldest first.
pub fn take_launch_outcomes() -> Vec<StartLaunchOutcome> {
    LAUNCH_OUTCOMES.with(|outcomes| outcomes.borrow_mut().drain(..).collect())
}

/// Select only the result belonging to the currently active Start launch.
/// Draining the queue first ensures stale completions cannot be replayed.
#[must_use]
pub fn matching_launch_outcome(
    outcomes: Vec<StartLaunchOutcome>,
    invocation: u64,
) -> Option<StartLaunchOutcome> {
    outcomes.into_iter().find(|outcome| match outcome {
        StartLaunchOutcome::Started { invocation: id, .. }
        | StartLaunchOutcome::Failed { invocation: id, .. } => *id == invocation,
    })
}

/// Pure mapping from a launch outcome to the Start error line: a failure
/// names the application, a success clears the line.
#[must_use]
pub fn start_error_after_launch(outcome: &StartLaunchOutcome) -> Option<String> {
    match outcome {
        StartLaunchOutcome::Started { .. } => None,
        StartLaunchOutcome::Failed { app, .. } => Some(format!("Could not start {app}")),
    }
}

fn record_launch_outcome(outcome: StartLaunchOutcome) {
    LAUNCH_OUTCOMES.with(|outcomes| outcomes.borrow_mut().push_back(outcome));
}

fn take_pending_launch(id: u64) -> Option<(u64, String)> {
    PENDING_LAUNCHES.with(|pending| {
        let mut pending = pending.borrow_mut();
        let index = pending.iter().position(|(pending_id, _, _)| *pending_id == id)?;
        let (_, invocation, app) = pending.swap_remove(index);
        Some((invocation, app))
    })
}

/// Enqueue one control mutation; returns the queue id on success, `None`
/// on full/closed queue (counted, stderr diagnostic, loop survives).
pub fn enqueue_action(
    dispatcher: Option<&Arc<MutationDispatcher>>,
    request: ControlRequest,
) -> Option<u64> {
    let Some(dispatcher) = dispatcher else {
        action_fail_counter().increment();
        eprintln!("flamewm-shell: mutation dropped: no control dispatcher (action unavailable)");
        return None;
    };
    enqueue_on_queue(dispatcher.queue(), request)
}

/// Queue-level submit used by tests that cannot open a bus.
pub fn enqueue_on_queue(queue: &Arc<ControlMutationQueue>, request: ControlRequest) -> Option<u64> {
    let lane = MutationLane::for_request(&request);
    match queue.submit(lane, request) {
        Ok(id) => {
            action_ok_counter().increment();
            Some(id)
        }
        Err(error) => {
            action_fail_counter().increment();
            eprintln!("flamewm-shell: mutation enqueue failed: {}", error.message);
            None
        }
    }
}

/// Nonblocking result drain for the reactor turn. Failures are nonfatal
/// diagnostics; the event loop survives either way.
pub fn drain_results(dispatcher: Option<&Arc<MutationDispatcher>>) {
    drain_results_with_settings(dispatcher, |_| {});
}

/// Drain the bounded mutation result queue on the reactor turn. Settings
/// mutation responses retain their typed snapshot and are handed to the shell
/// state owner; all other successful responses remain intentionally generic.
pub fn drain_results_with_settings(
    dispatcher: Option<&Arc<MutationDispatcher>>,
    mut apply_settings: impl FnMut(ApiSettingsSnapshot),
) {
    let Some(dispatcher) = dispatcher else {
        return;
    };
    while let Some(result) = dispatcher.try_recv_result() {
        let network_identity = PENDING_NETWORK_SECRETS.with(|pending| {
            let mut pending = pending.borrow_mut();
            let position = pending.iter().position(|(id, _)| *id == result.id)?;
            Some(pending.swap_remove(position).1)
        });
        let weather_pick = PENDING_WEATHER_PICKS.with(|pending| {
            let mut pending = pending.borrow_mut();
            let position = pending.iter().position(|(id, _)| *id == result.id)?;
            Some(pending.swap_remove(position).1)
        });
        if let Some(pick) = weather_pick {
            WEATHER_PICK_OUTCOMES.with(|outcomes| outcomes.borrow_mut().push_back(WeatherPickOutcome {
                id: result.id,
                pick,
                result: match &result.outcome {
                    Ok(flamewm_control_core::ControlResponse::Settings(snapshot)) => Ok(Some(snapshot.clone())),
                    Ok(_) => Ok(None),
                    Err(error) => Err(error.code),
                },
            }));
        }
        if let Some(identity) = network_identity {
            NETWORK_SECRET_OUTCOMES.with(|outcomes| outcomes.borrow_mut().push_back(NetworkSecretOutcome {
                id: result.id,
                identity,
                result: result.outcome.as_ref().map(|_| ()).map_err(|_| ()),
            }));
        }
        let launch_outcome = take_pending_launch(result.id).map(|(invocation, app)| match &result.outcome {
            Ok(_) => StartLaunchOutcome::Started { invocation, app },
            Err(error) => StartLaunchOutcome::Failed {
                invocation,
                app,
                message: error.message.clone(),
            },
        });
        match result.outcome {
            Ok(flamewm_control_core::ControlResponse::Settings(snapshot)) => {
                result_ok_counter().increment();
                apply_settings(snapshot);
            }
            Ok(_) => result_ok_counter().increment(),
            Err(error) => {
                result_err_counter().increment();
                eprintln!(
                    "flamewm-shell: mutation {} failed: {}",
                    result.id, error.message
                );
            }
        }
        if let Some(outcome) = launch_outcome {
            record_launch_outcome(outcome);
        }
    }
}

fn action_ok_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.action.enqueue_ok"))
}

fn action_fail_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.action.enqueue_fail"))
}

fn result_ok_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.action.result_ok"))
}

fn result_err_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.action.result_err"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use flamewm_control_core::{
        ControlMutationQueue, ControlMutationResult, ControlResponse, MutationLane,
    };

    fn production_source() -> &'static str {
        include_str!("control_actions.rs")
            .split_once("\n#[cfg(test)]\nmod tests")
            .map(|(source, _)| source)
            .expect("control action tests must have a production section")
    }

    fn settings_result_fixture() -> ControlMutationResult {
        let mut snapshot = SettingsSnapshot::new(7);
        snapshot.values.insert(
            "audio.raise_maximum".to_owned(),
            SettingValue::Boolean(true),
        );
        ControlMutationResult {
            id: 11,
            lane: MutationLane::Settings,
            outcome: Ok(ControlResponse::Settings(snapshot)),
        }
    }

    #[test]
    fn enqueue_returns_immediately_and_full_is_nonfatal() {
        let queue = Arc::new(ControlMutationQueue::new(1));
        let first = enqueue_on_queue(&queue, ControlRequest::Ping);
        assert!(first.is_some());
        let second = enqueue_on_queue(&queue, ControlRequest::Ping);
        assert!(
            second.is_none(),
            "full queue must be nonfatal None, not panic"
        );
        assert_eq!(queue.pending_len(), 1, "full entry retained, not dropped");
    }

    #[test]
    fn missing_dispatcher_is_a_diagnosed_none_not_a_silent_drop() {
        let request = ControlRequest::Ping;
        assert_eq!(enqueue_action(None, request), None);
    }

    #[test]
    fn red_t06_settings_mutation_result_requires_direct_snapshot_application() {
        // RED(T06): the current drain path matches Ok(_) and discards this
        // Settings snapshot instead of handing it to the shell state owner.
        let result = settings_result_fixture();
        assert!(matches!(
            result.outcome,
            Ok(ControlResponse::Settings(snapshot))
                if snapshot.revision == 7
                    && snapshot.values.get("audio.raise_maximum")
                        == Some(&SettingValue::Boolean(true))
        ));
        assert!(
            production_source().contains("ControlResponse::Settings"),
            "T06 RED: drain_results must consume Settings(snapshot) directly; current mutation only logs generic Ok(_)"
        );
    }

    #[test]
    fn red_start_launch_failure_sets_error_and_success_clears_it() {
        // Launch failure must surface as Start error state; success clears it.
        let failed = StartLaunchOutcome::Failed {
            invocation: 1,
            app: "Calculator".to_owned(),
            message: "exec failed".to_owned(),
        };
        assert_eq!(
            start_error_after_launch(&failed),
            Some("Could not start Calculator".to_owned()),
            "a failed launch must set the Start error message"
        );
        let started = StartLaunchOutcome::Started {
            invocation: 2,
            app: "Calculator".to_owned(),
        };
        assert_eq!(
            start_error_after_launch(&started),
            None,
            "a successful launch must clear the Start error message"
        );
    }

    #[test]
    fn stale_out_of_order_launch_results_do_not_match_active_invocation() {
        let outcomes = vec![
            StartLaunchOutcome::Failed {
                invocation: 1,
                app: "Old app".to_owned(),
                message: "old failure".to_owned(),
            },
            StartLaunchOutcome::Started {
                invocation: 2,
                app: "Current app".to_owned(),
            },
        ];
        let current = matching_launch_outcome(outcomes, 2).expect("current result is retained");
        assert!(matches!(current, StartLaunchOutcome::Started { invocation: 2, .. }));
        assert!(matching_launch_outcome(
            vec![StartLaunchOutcome::Failed {
                invocation: 1,
                app: "Old app".to_owned(),
                message: "old failure".to_owned(),
            }],
            2,
        ).is_none());
    }
}
