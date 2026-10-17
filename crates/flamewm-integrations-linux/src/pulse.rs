use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use flamewm_api::system::{
    AudioEndpointKind, AudioEndpointSnapshot, AudioMuteAction, AudioSnapshot, AudioStreamSnapshot,
    AudioTarget, AudioVolumeAction, ServiceAvailability,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_integrations_core::pulse::{PulseController, PulseIntent};
use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::subscribe::{Facility, InterestMaskSet};
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::mainloop::api::Mainloop as PulseMainloop;
use pulse::mainloop::events::timer::TimeEvent;
use pulse::mainloop::threaded::Mainloop;
use pulse::time::MonotonicTs;
use pulse::volume::{ChannelVolumes, Volume};

const APPLICATION_NAME: &str = "FlameWM";
const RETRY_TIMER_SEED: Duration = Duration::from_secs(86_400);
const STREAM_TEXT_MAX_CHARS: usize = 128;

type SnapshotCallback = dyn FnMut(AudioSnapshot) + Send + 'static;
type PulseTimer = TimeEvent<<Mainloop as PulseMainloop>::MI>;
type ContextSlot = Rc<RefCell<Option<Context>>>;
type TimerSlot = Rc<RefCell<Option<PulseTimer>>>;

struct ReconcileState {
    default_sink_name: String,
    default_source_name: String,
    endpoints: Vec<AudioEndpointSnapshot>,
    streams: Vec<AudioStreamSnapshot>,
    pending: u8,
}

/// Event-driven PulseAudio/PipeWire-Pulse adapter.
///
/// Pulse owns one threaded mainloop and one context. Product state remains in
/// `PulseController`; consumers apply emitted snapshots to `SystemService`.
pub struct PulseAdapter {
    mainloop: Rc<RefCell<Mainloop>>,
    context: ContextSlot,
    controller: Arc<Mutex<PulseController>>,
    timer: TimerSlot,
    on_snapshot: Arc<Mutex<Box<SnapshotCallback>>>,
}

impl PulseAdapter {
    /// Starts the Pulse connection. Missing PulseAudio/PipeWire-Pulse remains an unavailable,
    /// reconnecting adapter instead of becoming a fatal product error.
    pub fn connect<F>(on_snapshot: F) -> FlameResult<Self>
    where
        F: FnMut(AudioSnapshot) + Send + 'static,
    {
        let mut mainloop = Mainloop::new()
            .ok_or_else(|| FlameError::unavailable("failed to create PulseAudio mainloop"))?;
        mainloop
            .start()
            .map_err(|error| pulse_error("failed to start PulseAudio mainloop", error))?;

        let mainloop = Rc::new(RefCell::new(mainloop));
        let context = Rc::new(RefCell::new(None));
        let controller = Arc::new(Mutex::new(PulseController::default()));
        let on_snapshot: Arc<Mutex<Box<SnapshotCallback>>> =
            Arc::new(Mutex::new(Box::new(on_snapshot)));
        let timer = Rc::new(RefCell::new(None));

        let timer_callback = {
            let mainloop = Rc::clone(&mainloop);
            let context = Rc::downgrade(&context);
            let controller = Arc::clone(&controller);
            let on_snapshot = Arc::clone(&on_snapshot);
            let timer = Rc::downgrade(&timer);
            Box::new(move |_| {
                retry_connection(&mainloop, &context, &controller, &on_snapshot, &timer);
            })
        };

        let timer_created = {
            let mut mainloop_ref = mainloop.borrow_mut();
            mainloop_ref.lock();
            let timer_created = mainloop_ref
                .new_timer_event_rt(MonotonicTs::now() + RETRY_TIMER_SEED, timer_callback);
            mainloop_ref.unlock();
            timer_created
        };
        let Some(timer_event) = timer_created else {
            mainloop.borrow_mut().stop();
            return Err(FlameError::new(
                ErrorCode::InternalFailure,
                "failed to create PulseAudio reconnect timer",
            ));
        };
        *timer.borrow_mut() = Some(timer_event);

        let adapter = Self {
            mainloop: Rc::clone(&mainloop),
            context: Rc::clone(&context),
            controller: Arc::clone(&controller),
            timer: Rc::clone(&timer),
            on_snapshot: Arc::clone(&on_snapshot),
        };

        let generation = {
            let mut controller = lock(&controller);
            controller.connect_started();
            controller.generation()
        };
        let timer_weak = Rc::downgrade(&timer);
        {
            let mut mainloop_ref = mainloop.borrow_mut();
            mainloop_ref.lock();
            if install_context_locked(
                &mut mainloop_ref,
                &context,
                &controller,
                &on_snapshot,
                &timer_weak,
                generation,
            )
            .is_err()
            {
                mark_disconnected(&controller, &on_snapshot, &timer_weak, generation);
            }
            mainloop_ref.unlock();
        }

        Ok(adapter)
    }

    #[must_use]
    pub fn snapshot(&self) -> AudioSnapshot {
        lock(&self.controller).snapshot()
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        lock(&self.controller).generation()
    }

    /// Queues a user-requested sink volume change. The resulting state arrives through the normal
    /// sink subscription; this method never synthesizes optimistic state.
    pub fn set_default_volume(&self, percent: u8, caller_generation: u64) -> FlameResult<()> {
        let snapshot = self.snapshot();
        self.set_volume_for_sink(
            &snapshot.sink_name,
            percent,
            caller_generation,
            snapshot.server_generation,
        )
    }

    /// Queues a volume change for the sink named by the system snapshot.
    pub fn set_volume_for_sink(
        &self,
        sink_name: &str,
        percent: u8,
        caller_generation: u64,
        caller_server_generation: u64,
    ) -> FlameResult<()> {
        let endpoint_id = self.sink_endpoint_id(sink_name, caller_server_generation)?;
        let intent = {
            let controller = lock(&self.controller);
            controller.set_volume(AudioVolumeAction {
                target: AudioTarget::Endpoint {
                    id: endpoint_id,
                    kind: AudioEndpointKind::Sink,
                },
                percent,
                generation: caller_generation,
                server_generation: caller_server_generation,
            })?
        };
        let PulseIntent::SetVolume(action) = intent else {
            return Err(FlameError::new(
                ErrorCode::InternalFailure,
                "unexpected PulseAudio volume intent",
            ));
        };
        if sink_name.is_empty() {
            return Err(FlameError::unavailable(
                "PulseAudio default sink is unavailable",
            ));
        }

        let context_slot = Rc::downgrade(&self.context);
        self.with_ready_context(|context| {
            let introspector = context.introspect();
            let mut seen = false;
            let volume = pulse_volume(action.percent);
            let _operation =
                introspector.get_sink_info_by_name(sink_name, move |result| match result {
                    ListResult::Item(info) if !seen => {
                        seen = true;
                        let channels = info.volume.len();
                        if channels == 0 {
                            return;
                        }
                        let mut target = ChannelVolumes::default();
                        target.set(channels, volume);
                        let Some(context_slot) = context_slot.upgrade() else {
                            return;
                        };
                        let context_ref = context_slot.borrow();
                        let Some(context) = context_ref.as_ref() else {
                            return;
                        };
                        let mut introspector = context.introspect();
                        let _ = introspector.set_sink_volume_by_index(endpoint_id, &target, None);
                    }
                    _ => {}
                });
            Ok(())
        })
    }

    /// Queues a user-requested sink mute change. Actual state remains subscription-driven.
    pub fn set_default_mute(&self, muted: bool, caller_generation: u64) -> FlameResult<()> {
        let snapshot = self.snapshot();
        self.set_mute_for_sink(
            &snapshot.sink_name,
            muted,
            caller_generation,
            snapshot.server_generation,
        )
    }

    /// Queues a mute change for the sink named by the system snapshot.
    pub fn set_mute_for_sink(
        &self,
        sink_name: &str,
        muted: bool,
        caller_generation: u64,
        caller_server_generation: u64,
    ) -> FlameResult<()> {
        let endpoint_id = self.sink_endpoint_id(sink_name, caller_server_generation)?;
        let intent = {
            let controller = lock(&self.controller);
            controller.set_mute(AudioMuteAction {
                target: AudioTarget::Endpoint {
                    id: endpoint_id,
                    kind: AudioEndpointKind::Sink,
                },
                muted,
                generation: caller_generation,
                server_generation: caller_server_generation,
            })?
        };
        let PulseIntent::SetMute(action) = intent else {
            return Err(FlameError::new(
                ErrorCode::InternalFailure,
                "unexpected PulseAudio mute intent",
            ));
        };
        if sink_name.is_empty() {
            return Err(FlameError::unavailable(
                "PulseAudio default sink is unavailable",
            ));
        }

        self.with_ready_context(|context| {
            let _operation = context.introspect().set_sink_mute_by_index(
                endpoint_id,
                action.muted,
                Some(self.write_failure_callback(action.generation, "sink mute")),
            );
            Ok(())
        })
    }

    /// Queue one real PulseAudio object mutation. Snapshot publication waits for the matching
    /// provider subscription/reconcile path rather than synthesizing optimistic state.
    pub fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<()> {
        let intent = lock(&self.controller).set_volume(action)?;
        let PulseIntent::SetVolume(action) = intent else {
            return Err(FlameError::new(
                ErrorCode::InternalFailure,
                "unexpected PulseAudio volume intent",
            ));
        };
        let mut target = ChannelVolumes::default();
        target.set(2, pulse_volume(action.percent));
        self.with_ready_context(|context| {
            let mut introspector = context.introspect();
            match action.target {
                AudioTarget::Endpoint {
                    id,
                    kind: AudioEndpointKind::Sink,
                } => {
                    let _operation = introspector.set_sink_volume_by_index(
                        id,
                        &target,
                        Some(self.write_failure_callback(action.generation, "sink volume")),
                    );
                }
                AudioTarget::Endpoint {
                    id,
                    kind: AudioEndpointKind::Source,
                } => {
                    let _operation = introspector.set_source_volume_by_index(
                        id,
                        &target,
                        Some(self.write_failure_callback(action.generation, "source volume")),
                    );
                }
                AudioTarget::Stream { id } => {
                    let _operation = introspector.set_sink_input_volume(
                        id,
                        &target,
                        Some(self.write_failure_callback(action.generation, "stream volume")),
                    );
                }
            }
            Ok(())
        })
    }

    pub fn set_mute(&self, action: AudioMuteAction) -> FlameResult<()> {
        let intent = lock(&self.controller).set_mute(action)?;
        let PulseIntent::SetMute(action) = intent else {
            return Err(FlameError::new(
                ErrorCode::InternalFailure,
                "unexpected PulseAudio mute intent",
            ));
        };
        self.with_ready_context(|context| {
            let mut introspector = context.introspect();
            match action.target {
                AudioTarget::Endpoint {
                    id,
                    kind: AudioEndpointKind::Sink,
                } => {
                    let _operation = introspector.set_sink_mute_by_index(
                        id,
                        action.muted,
                        Some(self.write_failure_callback(action.generation, "sink mute")),
                    );
                }
                AudioTarget::Endpoint {
                    id,
                    kind: AudioEndpointKind::Source,
                } => {
                    let _operation = introspector.set_source_mute_by_index(
                        id,
                        action.muted,
                        Some(self.write_failure_callback(action.generation, "source mute")),
                    );
                }
                AudioTarget::Stream { id } => {
                    let _operation = introspector.set_sink_input_mute(
                        id,
                        action.muted,
                        Some(self.write_failure_callback(action.generation, "stream mute")),
                    );
                }
            }
            Ok(())
        })
    }

    /// Resolves `sink_name` to its endpoint id in one snapshot, rejecting a stale server generation.
    fn sink_endpoint_id(&self, sink_name: &str, caller_server_generation: u64) -> FlameResult<u32> {
        let snapshot = self.snapshot();
        if snapshot.server_generation != caller_server_generation {
            return Err(FlameError::stale("stale PulseAudio server generation"));
        }
        snapshot
            .endpoints()
            .iter()
            .find(|endpoint| endpoint.kind == AudioEndpointKind::Sink && endpoint.name == sink_name)
            .map(|endpoint| endpoint.id)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "PulseAudio sink no longer exists"))
    }

    /// Runs one queued write against a ready context while holding the mainloop lock.
    fn with_ready_context(
        &self,
        write: impl FnOnce(&Context) -> FlameResult<()>,
    ) -> FlameResult<()> {
        let mut mainloop = self.mainloop.borrow_mut();
        mainloop.lock();
        let result = match self.context.borrow().as_ref() {
            None => Err(FlameError::unavailable("PulseAudio context is unavailable")),
            Some(context) if context.get_state() != ContextState::Ready => {
                Err(FlameError::unavailable("PulseAudio context is not ready"))
            }
            Some(context) => write(context),
        };
        mainloop.unlock();
        result
    }
}

impl PulseAdapter {
    /// Success callback for one queued write. PulseAudio reports the async
    /// outcome only here, so a rejected write is diagnosed and reconciled
    /// against the server instead of being silently treated as applied.
    fn write_failure_callback(
        &self,
        generation: u64,
        what: &'static str,
    ) -> Box<dyn FnMut(bool) + 'static> {
        let context = Rc::downgrade(&self.context);
        let controller = Arc::clone(&self.controller);
        let on_snapshot = Arc::clone(&self.on_snapshot);
        let timer = Rc::downgrade(&self.timer);
        Box::new(move |succeeded| {
            if succeeded {
                return;
            }
            eprintln!("flamewm-integrations-linux: PulseAudio {what} write rejected; reconciling");
            request_reconcile(&context, &controller, &on_snapshot, &timer, generation);
        })
    }
}

impl Drop for PulseAdapter {
    fn drop(&mut self) {
        let mut mainloop = self.mainloop.borrow_mut();
        mainloop.lock();
        if let Some(mut context) = self.context.borrow_mut().take() {
            context.set_state_callback(None);
            context.set_subscribe_callback(None);
            context.disconnect();
        }
        self.timer.borrow_mut().take();
        mainloop.unlock();
        mainloop.stop();
    }
}

fn install_context_locked(
    mainloop: &mut Mainloop,
    context_slot: &ContextSlot,
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    timer: &Weak<RefCell<Option<PulseTimer>>>,
    generation: u64,
) -> FlameResult<()> {
    if let Some(mut context) = context_slot.borrow_mut().take() {
        context.set_state_callback(None);
        context.set_subscribe_callback(None);
        context.disconnect();
    }

    let mut context = Context::new(&*mainloop, APPLICATION_NAME)
        .ok_or_else(|| FlameError::unavailable("failed to create PulseAudio context"))?;

    let weak_context = Rc::downgrade(context_slot);
    let state_controller = Arc::clone(controller);
    let state_snapshot = Arc::clone(on_snapshot);
    let state_timer = timer.clone();
    context.set_state_callback(Some(Box::new(move || {
        context_state_changed(
            &weak_context,
            &state_controller,
            &state_snapshot,
            &state_timer,
            generation,
        );
    })));

    let subscription_context = Rc::downgrade(context_slot);
    let subscription_controller = Arc::clone(controller);
    let subscription_snapshot = Arc::clone(on_snapshot);
    let subscription_timer = timer.clone();
    context.set_subscribe_callback(Some(Box::new(move |facility, _operation, _index| {
        if !matches!(
            facility,
            Some(Facility::Sink | Facility::Source | Facility::SinkInput | Facility::Server)
        ) {
            return;
        }
        let should_reconcile = {
            let mut controller = lock(&subscription_controller);
            controller.subscription_event(generation)
                && controller.take_reconcile_intent(generation).is_some()
        };
        if should_reconcile {
            request_reconcile(
                &subscription_context,
                &subscription_controller,
                &subscription_snapshot,
                &subscription_timer,
                generation,
            );
        }
    })));

    context
        .connect(None, ContextFlagSet::NOAUTOSPAWN, None)
        .map_err(|error| pulse_error("failed to connect PulseAudio context", error))?;
    *context_slot.borrow_mut() = Some(context);
    Ok(())
}

fn introspector(context_slot: &ContextSlot) -> Option<pulse::context::introspect::Introspector> {
    context_slot.borrow().as_ref().map(Context::introspect)
}

fn context_state_changed(
    context_slot: &Weak<RefCell<Option<Context>>>,
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    timer: &Weak<RefCell<Option<PulseTimer>>>,
    generation: u64,
) {
    if lock(controller).generation() != generation {
        return;
    }
    let Some(context_slot) = context_slot.upgrade() else {
        return;
    };
    let state = context_slot.borrow().as_ref().map(Context::get_state);
    match state {
        Some(ContextState::Ready) => {
            let became_ready = lock(controller).ready(generation);
            if !became_ready {
                return;
            }
            if let Some(context) = context_slot.borrow_mut().as_mut() {
                let _ = context.subscribe(
                    InterestMaskSet::SINK
                        | InterestMaskSet::SOURCE
                        | InterestMaskSet::SINK_INPUT
                        | InterestMaskSet::SERVER,
                    |_| {},
                );
            }
            restart_timer(timer, RETRY_TIMER_SEED);
            request_reconcile(
                &Rc::downgrade(&context_slot),
                controller,
                on_snapshot,
                timer,
                generation,
            );
        }
        Some(ContextState::Failed | ContextState::Terminated) => {
            mark_disconnected(controller, on_snapshot, timer, generation);
        }
        _ => {}
    }
}

fn request_reconcile(
    context_slot: &Weak<RefCell<Option<Context>>>,
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    timer: &Weak<RefCell<Option<PulseTimer>>>,
    generation: u64,
) {
    if lock(controller).generation() != generation {
        return;
    }
    let Some(context_slot) = context_slot.upgrade() else {
        return;
    };
    let Some(introspector) = introspector(&context_slot) else {
        return;
    };
    let sink_context = Rc::downgrade(&context_slot);
    let sink_controller = Arc::clone(controller);
    let sink_snapshot = Arc::clone(on_snapshot);
    let sink_timer = timer.clone();
    let _operation = introspector.get_server_info(move |server| {
        let Some(sink_name) = server.default_sink_name.as_ref() else {
            mark_disconnected(&sink_controller, &sink_snapshot, &sink_timer, generation);
            return;
        };
        request_audio_lists(
            &sink_context,
            &sink_controller,
            &sink_snapshot,
            generation,
            sink_name.to_string(),
            server
                .default_source_name
                .as_deref()
                .unwrap_or_default()
                .to_owned(),
        );
    });
}

fn request_audio_lists(
    context_slot: &Weak<RefCell<Option<Context>>>,
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    generation: u64,
    default_sink_name: String,
    default_source_name: String,
) {
    if lock(controller).generation() != generation {
        return;
    }
    let Some(context_slot) = context_slot.upgrade() else {
        return;
    };
    let Some(introspector) = introspector(&context_slot) else {
        return;
    };
    let state = Arc::new(Mutex::new(ReconcileState {
        default_sink_name,
        default_source_name,
        endpoints: Vec::new(),
        streams: Vec::new(),
        pending: 3,
    }));
    request_sink_list(
        &introspector,
        Arc::clone(&state),
        Arc::clone(controller),
        Arc::clone(on_snapshot),
        generation,
    );
    request_source_list(
        &introspector,
        Arc::clone(&state),
        Arc::clone(controller),
        Arc::clone(on_snapshot),
        generation,
    );
    request_stream_list(
        &introspector,
        state,
        Arc::clone(controller),
        Arc::clone(on_snapshot),
        generation,
    );
}

fn request_sink_list(
    introspector: &pulse::context::introspect::Introspector,
    state: Arc<Mutex<ReconcileState>>,
    controller: Arc<Mutex<PulseController>>,
    on_snapshot: Arc<Mutex<Box<SnapshotCallback>>>,
    generation: u64,
) {
    let _operation = introspector.get_sink_info_list(move |result| match result {
        ListResult::Item(info) => {
            let name = info.name.as_deref().unwrap_or_default().to_owned();
            let description = info.description.as_deref().unwrap_or_default().to_owned();
            let volume_percent = volume_percent(info.volume.avg());
            let muted = info.mute;
            let index = info.index;
            let is_default = lock(&state).default_sink_name == name;
            lock(&state).endpoints.push(AudioEndpointSnapshot {
                id: index,
                kind: AudioEndpointKind::Sink,
                name,
                description,
                volume_percent,
                muted,
                is_default,
            });
        }
        _ => complete_audio_list(&state, &controller, &on_snapshot, generation),
    });
}

fn request_source_list(
    introspector: &pulse::context::introspect::Introspector,
    state: Arc<Mutex<ReconcileState>>,
    controller: Arc<Mutex<PulseController>>,
    on_snapshot: Arc<Mutex<Box<SnapshotCallback>>>,
    generation: u64,
) {
    let _operation = introspector.get_source_info_list(move |result| match result {
        ListResult::Item(info) => {
            let name = info.name.as_deref().unwrap_or_default().to_owned();
            let description = info.description.as_deref().unwrap_or_default().to_owned();
            let volume_percent = volume_percent(info.volume.avg());
            let muted = info.mute;
            let index = info.index;
            let is_default = lock(&state).default_source_name == name;
            lock(&state).endpoints.push(AudioEndpointSnapshot {
                id: index,
                kind: AudioEndpointKind::Source,
                name,
                description,
                volume_percent,
                muted,
                is_default,
            });
        }
        _ => complete_audio_list(&state, &controller, &on_snapshot, generation),
    });
}

fn request_stream_list(
    introspector: &pulse::context::introspect::Introspector,
    state: Arc<Mutex<ReconcileState>>,
    controller: Arc<Mutex<PulseController>>,
    on_snapshot: Arc<Mutex<Box<SnapshotCallback>>>,
    generation: u64,
) {
    let _operation = introspector.get_sink_input_info_list(move |result| match result {
        ListResult::Item(info) => lock(&state).streams.push(stream_snapshot_from_proplist(
            info.index,
            info.sink,
            info.name.as_deref().unwrap_or_default().to_owned(),
            volume_percent(info.volume.avg()),
            info.mute,
            |key| info.proplist.get_str(key),
        )),
        _ => complete_audio_list(&state, &controller, &on_snapshot, generation),
    });
}

fn complete_audio_list(
    state: &Arc<Mutex<ReconcileState>>,
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    generation: u64,
) {
    let completed = {
        let mut state = lock(state);
        state.pending = state.pending.saturating_sub(1);
        (state.pending == 0).then(|| {
            (
                std::mem::take(&mut state.endpoints),
                std::mem::take(&mut state.streams),
            )
        })
    };
    if let Some((endpoints, streams)) = completed {
        let server_generation = lock(controller).server_generation();
        if lock(controller).reconcile(endpoints, streams, generation, server_generation) {
            emit_snapshot(controller, on_snapshot);
        }
    }
}

fn retry_connection(
    mainloop: &Rc<RefCell<Mainloop>>,
    context_slot: &Weak<RefCell<Option<Context>>>,
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    timer: &Weak<RefCell<Option<PulseTimer>>>,
) {
    if lock(controller).snapshot().availability == ServiceAvailability::Available {
        restart_timer(timer, RETRY_TIMER_SEED);
        return;
    }
    let generation = {
        let mut controller = lock(controller);
        controller.connect_started();
        controller.generation()
    };
    let Some(context_slot) = context_slot.upgrade() else {
        return;
    };
    let result = {
        let mut mainloop = mainloop.borrow_mut();
        // Timer callbacks run on Pulse's event-loop thread, which already owns the
        // threaded-mainloop lock. `Mainloop::lock()` is forbidden on that thread.
        install_context_locked(
            &mut mainloop,
            &context_slot,
            controller,
            on_snapshot,
            timer,
            generation,
        )
    };
    if result.is_err() {
        mark_disconnected(controller, on_snapshot, timer, generation);
    }
}

fn mark_disconnected(
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
    timer: &Weak<RefCell<Option<PulseTimer>>>,
    generation: u64,
) {
    let retry_delay = {
        let mut controller = lock(controller);
        if controller.generation() != generation {
            return;
        }
        controller.server_restarted();
        controller.next_reconnect_delay_ms()
    };
    emit_snapshot(controller, on_snapshot);
    let Some(delay) = retry_delay else {
        return;
    };
    restart_timer(timer, Duration::from_millis(delay));
}

fn restart_timer(timer: &Weak<RefCell<Option<PulseTimer>>>, delay: Duration) {
    let Some(timer) = timer.upgrade() else {
        return;
    };
    if let Some(timer) = timer.borrow_mut().as_mut() {
        timer.restart_rt(MonotonicTs::now() + delay);
    }
}

fn emit_snapshot(
    controller: &Arc<Mutex<PulseController>>,
    on_snapshot: &Arc<Mutex<Box<SnapshotCallback>>>,
) {
    let snapshot = lock(controller).snapshot();
    let mut callback = lock(on_snapshot);
    callback(snapshot);
}

/// Builds a stream snapshot; identity keys come from the sink-input proplist via `lookup`.
fn stream_snapshot_from_proplist(
    id: u32,
    endpoint_id: u32,
    name: String,
    volume_percent: u8,
    muted: bool,
    lookup: impl Fn(&str) -> Option<String>,
) -> AudioStreamSnapshot {
    AudioStreamSnapshot {
        id,
        endpoint_id,
        name,
        volume_percent,
        muted,
        app_name: sanitize_stream_text(lookup("application.name")),
        app_id: sanitize_stream_text(lookup("application.id")),
        icon_name: sanitize_stream_text(lookup("application.icon_name")),
        process_binary: sanitize_stream_text(lookup("application.process.binary")),
        media_name: sanitize_stream_text(lookup("media.name")),
    }
}

/// Drops control characters, trims, and bounds proplist text; empty text becomes `None`.
fn sanitize_stream_text(raw: Option<String>) -> Option<String> {
    let cleaned: String = raw?.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(STREAM_TEXT_MAX_CHARS).collect())
}

fn pulse_volume(percent: u8) -> Volume {
    let normal = u64::from(Volume::NORMAL.0);
    let raw = (normal
        .saturating_mul(u64::from(percent.min(150)))
        .saturating_add(50))
        / 100;
    Volume(raw.min(u64::from(Volume::MAX.0)) as u32)
}

fn volume_percent(volume: Volume) -> u8 {
    let normal = u64::from(Volume::NORMAL.0);
    ((u64::from(volume.0)
        .saturating_mul(100)
        .saturating_add(normal / 2))
        / normal)
        .min(150) as u8
}

fn pulse_error(context: &str, error: impl std::fmt::Display) -> FlameError {
    FlameError::new(ErrorCode::Unavailable, format!("{context}: {error}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_integrations_core::pulse::BackoffConfig;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn stream_identity_maps_proplist_keys_and_sanitizes_text() {
        let mut proplist = pulse::proplist::Proplist::new().expect("proplist must allocate");
        for (key, value) in [
            ("application.name", "  Spotify\t"),
            ("application.id", "com.spotify.Client"),
            ("application.icon_name", "spotify-client"),
            ("application.process.binary", "spotify"),
            ("media.name", "\u{7}\n"),
            ("media.title", "ignored"),
        ] {
            proplist.set_str(key, value).expect("set proplist key");
        }
        let stream = stream_snapshot_from_proplist(4, 9, "Playback".to_owned(), 55, true, |key| {
            proplist.get_str(key)
        });
        assert_eq!(stream.id, 4);
        assert_eq!(stream.endpoint_id, 9);
        assert_eq!(stream.name, "Playback");
        assert_eq!(stream.volume_percent, 55);
        assert!(stream.muted);
        assert_eq!(stream.app_name.as_deref(), Some("Spotify"));
        assert_eq!(stream.app_id.as_deref(), Some("com.spotify.Client"));
        assert_eq!(stream.icon_name.as_deref(), Some("spotify-client"));
        assert_eq!(stream.process_binary.as_deref(), Some("spotify"));
        assert_eq!(
            stream.media_name, None,
            "control-only media.name must drop to None"
        );
    }

    #[test]
    fn stream_identity_absent_keys_are_none_and_long_text_is_bounded() {
        let absent = stream_snapshot_from_proplist(1, 2, String::new(), 0, false, |_| None);
        assert_eq!(
            absent,
            AudioStreamSnapshot {
                id: 1,
                endpoint_id: 2,
                ..AudioStreamSnapshot::default()
            }
        );
        let long = "é".repeat(STREAM_TEXT_MAX_CHARS + 40);
        let bounded =
            stream_snapshot_from_proplist(1, 2, String::new(), 0, false, |_| Some(long.clone()));
        let expected: String = "é".repeat(STREAM_TEXT_MAX_CHARS);
        assert_eq!(bounded.app_name.as_deref(), Some(expected.as_str()));
        assert_eq!(bounded.media_name.as_deref(), Some(expected.as_str()));
        let blank = stream_snapshot_from_proplist(1, 2, String::new(), 0, false, |_| {
            Some(" \t\n ".to_owned())
        });
        assert_eq!(blank.app_name, None);
    }

    #[test]
    fn unavailable_retry_timer_returns_from_pulse_callback_thread() {
        let mut initial_mainloop = Mainloop::new().expect("Pulse mainloop must be constructible");
        initial_mainloop
            .start()
            .expect("Pulse mainloop must start for the callback probe");
        let mainloop = Rc::new(RefCell::new(initial_mainloop));
        let context = Rc::new(RefCell::new(None));
        let controller = Arc::new(Mutex::new(PulseController::default()));
        let on_snapshot: Arc<Mutex<Box<SnapshotCallback>>> = Arc::new(Mutex::new(Box::new(|_| {})));
        let timer = Rc::new(RefCell::new(None));
        let context_weak = Rc::downgrade(&context);
        let timer_weak = Rc::downgrade(&timer);
        let callback_mainloop = Rc::clone(&mainloop);
        let retry_mainloop = Rc::clone(&mainloop);
        let callback_controller = Arc::clone(&controller);
        let callback_snapshot = Arc::clone(&on_snapshot);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (completed_tx, completed_rx) = mpsc::channel();

        let timer_callback = Box::new(move |_| {
            let callback_thread = thread::current().id();
            let in_pulse_thread = callback_mainloop.borrow().in_thread();
            entered_tx
                .send((callback_thread, in_pulse_thread))
                .expect("callback probe receiver must remain available");
            retry_connection(
                &retry_mainloop,
                &context_weak,
                &callback_controller,
                &callback_snapshot,
                &timer_weak,
            );
            completed_tx
                .send(())
                .expect("callback probe receiver must remain available");
        });

        let timer_event = {
            let mut mainloop_ref = mainloop.borrow_mut();
            mainloop_ref.lock();
            let event = mainloop_ref.new_timer_event_rt(
                MonotonicTs::now()
                    + Duration::from_millis(BackoffConfig::default().initial_ms + 50),
                timer_callback,
            );
            mainloop_ref.unlock();
            event
        };
        let Some(timer_event) = timer_event else {
            mainloop.borrow_mut().stop();
            panic!("Pulse callback probe timer must be created");
        };
        *timer.borrow_mut() = Some(timer_event);

        let wait = Duration::from_millis(BackoffConfig::default().initial_ms + 1_000);
        let entered = entered_rx.recv_timeout(wait);
        let completed = if entered.is_ok() {
            completed_rx.recv_timeout(wait)
        } else {
            Err(mpsc::RecvTimeoutError::Disconnected)
        };
        mainloop.borrow_mut().stop();

        let (callback_thread, in_pulse_thread) =
            entered.expect("unavailable retry timer must execute its callback");
        assert_ne!(
            callback_thread,
            thread::current().id(),
            "retry callback must run on the Pulse threaded-mainloop thread"
        );
        assert!(
            in_pulse_thread,
            "retry callback must report Pulse threaded-mainloop affinity"
        );
        assert!(
            completed.is_ok(),
            "unavailable retry callback must return without calling Mainloop::lock from the callback thread"
        );
    }
}
