use flamewm_api::system::{
    AudioEndpointKind, AudioMuteAction, AudioSnapshot, AudioTarget, AudioVolumeAction,
    MediaSnapshot, NetworkSnapshot, PowerSnapshot, ServiceAvailability, SystemAction,
    SystemSnapshot,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

pub type SystemActionHandler = Box<dyn FnMut(SystemAction, SystemSnapshot) -> FlameResult<()>>;

/// Volume change per media-key press, in percent.
pub const MEDIA_VOLUME_STEP: u8 = 5;
/// Highest volume the system action path accepts.
pub const MAX_VOLUME_PERCENT: u8 = 150;
/// Volume cap without "raise maximum volume" (over-amplification).
pub const BASE_VOLUME_PERCENT: u8 = 100;
/// Settings key of the "Raise maximum volume" toggle. The audio mixer, the volume OSD and
/// the WM media keys all read this one key; it is the only owner of the name.
pub const AUDIO_RAISE_MAXIMUM_KEY: &str = "audioRaiseMaximum";

/// Volume cap for the shared "raise maximum volume" value: 150 when raised, else 100.
#[must_use]
pub fn volume_cap_percent(raise_maximum: bool) -> u8 {
    if raise_maximum {
        MAX_VOLUME_PERCENT
    } else {
        BASE_VOLUME_PERCENT
    }
}

/// Media keys that adjust the default output endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAudioKey {
    VolumeUp,
    VolumeDown,
    VolumeMute,
}

/// Returns `current` moved one media-key step up or down, clamped to `0..=cap`.
#[must_use]
pub fn stepped_volume(current: u8, up: bool, cap: u8) -> u8 {
    let cap = cap.min(MAX_VOLUME_PERCENT);
    if up {
        current.saturating_add(MEDIA_VOLUME_STEP).min(cap)
    } else {
        current.saturating_sub(MEDIA_VOLUME_STEP).min(cap)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectBackoff {
    attempt: u8,
    initial_ms: u64,
    maximum_ms: u64,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            attempt: 0,
            initial_ms: 250,
            maximum_ms: 30_000,
        }
    }
}

impl ReconnectBackoff {
    #[must_use]
    pub fn next_delay_ms(&mut self) -> u64 {
        let shift = u32::from(self.attempt.min(7));
        let delay = self
            .initial_ms
            .saturating_mul(1_u64 << shift)
            .min(self.maximum_ms);
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[derive(Default)]
pub struct SystemService {
    snapshot: SystemSnapshot,
    pulse_backoff: ReconnectBackoff,
    action_handler: Option<SystemActionHandler>,
}

impl SystemService {
    #[must_use]
    pub fn snapshot(&self) -> &SystemSnapshot {
        &self.snapshot
    }

    pub fn update_network(&mut self, update: NetworkSnapshot) -> bool {
        if update.generation < self.snapshot.network.generation {
            return false;
        }
        self.apply(|s| &mut s.network, update)
    }

    pub fn update_media(&mut self, update: MediaSnapshot) -> bool {
        if update.generation < self.snapshot.media.generation {
            return false;
        }
        self.apply(|s| &mut s.media, update)
    }

    pub fn update_power(&mut self, update: PowerSnapshot) -> bool {
        self.apply(|s| &mut s.power, update)
    }

    pub fn update_audio(&mut self, update: AudioSnapshot) -> bool {
        if update.generation < self.snapshot.audio.generation
            || update.server_generation < self.snapshot.audio.server_generation
        {
            return false;
        }
        if update.availability == ServiceAvailability::Available {
            self.pulse_backoff.reset();
        }
        self.apply(|s| &mut s.audio, update)
    }

    pub fn pulse_disconnected(&mut self) -> u64 {
        let generation = self.snapshot.audio.generation.saturating_add(1);
        let server_generation = self.snapshot.audio.server_generation.saturating_add(1);
        let update = AudioSnapshot {
            availability: ServiceAvailability::Unavailable,
            generation,
            server_generation,
            ..AudioSnapshot::default()
        };
        self.apply(|s| &mut s.audio, update);
        self.pulse_backoff.next_delay_ms()
    }

    pub fn set_action_handler(&mut self, handler: SystemActionHandler) {
        self.action_handler = Some(handler);
    }

    pub fn perform_action(
        &mut self,
        action: SystemAction,
        expected_revision: u64,
    ) -> FlameResult<()> {
        // Audio volume/mute writes are fenced by PulseAudio generation (checked
        // below), so repeated clicks before the next snapshot signal must not be
        // rejected by the whole-snapshot revision. Other actions stay revision-fenced.
        if !bypasses_revision_fence(&action) && expected_revision != self.snapshot.revision {
            return Err(FlameError::stale("stale system snapshot revision"));
        }
        match &action {
            SystemAction::SetVolume(action) => {
                if action.percent > MAX_VOLUME_PERCENT {
                    return Err(FlameError::new(
                        ErrorCode::InvalidArgument,
                        "audio volume must be in 0..=150",
                    ));
                }
                self.validate_audio_generation(action.generation, action.server_generation)?;
            }
            SystemAction::SetMute(action) => {
                self.validate_audio_generation(action.generation, action.server_generation)?;
            }
            SystemAction::ConnectWifi { generation, .. }
            | SystemAction::SubmitNetworkSecret { generation, .. }
            | SystemAction::CancelNetworkSecret { generation, .. } => {
                if *generation != self.snapshot.network.generation {
                    return Err(FlameError::stale("stale NetworkManager service generation"));
                }
            }
            _ => {}
        }
        let snapshot = self.snapshot.clone();
        let Some(handler) = self.action_handler.as_mut() else {
            return Err(FlameError::new(
                ErrorCode::Unsupported,
                "system action provider is unavailable",
            ));
        };
        handler(action, snapshot)
    }

    /// Applies a media key to the default output sink through `perform_action`,
    /// the same typed path the UI uses. The write is fenced by the current audio
    /// generation. With no default sink there is nothing to adjust and this is a no-op.
    /// `cap` is the live volume cap (`volume_cap_percent` of the shared setting).
    pub fn media_audio_key(&mut self, key: MediaAudioKey, cap: u8) -> FlameResult<()> {
        let revision = self.snapshot.revision;
        let generation = self.snapshot.audio.generation;
        let server_generation = self.snapshot.audio.server_generation;
        let Some(endpoint) = self
            .snapshot
            .audio
            .endpoints
            .iter()
            .find(|endpoint| endpoint.is_default && endpoint.kind == AudioEndpointKind::Sink)
        else {
            return Ok(());
        };
        let target = AudioTarget::Endpoint {
            id: endpoint.id,
            kind: endpoint.kind,
        };
        let action = match key {
            MediaAudioKey::VolumeUp | MediaAudioKey::VolumeDown => {
                SystemAction::SetVolume(AudioVolumeAction {
                    target,
                    percent: stepped_volume(
                        endpoint.volume_percent,
                        key == MediaAudioKey::VolumeUp,
                        cap,
                    ),
                    generation,
                    server_generation,
                })
            }
            MediaAudioKey::VolumeMute => SystemAction::SetMute(AudioMuteAction {
                target,
                muted: !endpoint.muted,
                generation,
                server_generation,
            }),
        };
        self.perform_action(action, revision)
    }

    /// Stores `update` into the field picked by `slot`; bumps the revision and returns true only on change.
    fn apply<T: PartialEq>(
        &mut self,
        slot: impl FnOnce(&mut SystemSnapshot) -> &mut T,
        update: T,
    ) -> bool {
        let field = slot(&mut self.snapshot);
        if *field == update {
            return false;
        }
        *field = update;
        self.snapshot.revision = self.snapshot.revision.saturating_add(1).max(1);
        true
    }

    fn validate_audio_generation(
        &self,
        generation: u64,
        server_generation: u64,
    ) -> FlameResult<()> {
        if generation != self.snapshot.audio.generation
            || server_generation != self.snapshot.audio.server_generation
        {
            return Err(FlameError::stale("stale PulseAudio generation"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_audio_callback_from_old_server_generation_is_ignored() {
        let mut service = SystemService::default();
        let _ = service.pulse_disconnected();
        let accepted = service.update_audio(AudioSnapshot {
            availability: ServiceAvailability::Available,
            generation: 0,
            server_generation: 0,
            volume_percent: 90,
            ..AudioSnapshot::default()
        });
        assert!(!accepted);
    }

    #[test]
    fn volume_and_mute_bypass_revision_fence_but_other_actions_do_not() {
        let volume = SystemAction::SetVolume(flamewm_api::system::AudioVolumeAction {
            target: flamewm_api::system::AudioTarget::Stream { id: 1 },
            percent: 40,
            generation: 0,
            server_generation: 0,
        });
        let mute = SystemAction::SetMute(flamewm_api::system::AudioMuteAction {
            target: flamewm_api::system::AudioTarget::Stream { id: 1 },
            muted: true,
            generation: 0,
            server_generation: 0,
        });
        assert!(bypasses_revision_fence(&volume));
        assert!(bypasses_revision_fence(&mute));
        assert!(!bypasses_revision_fence(&SystemAction::SetWifiEnabled(
            true
        )));
    }

    #[test]
    fn power_update_bumps_revision_only_on_change() {
        let mut service = SystemService::default();
        let update = PowerSnapshot {
            available: ServiceAvailability::Available,
            present: true,
            percent: 87,
            ..PowerSnapshot::default()
        };
        assert!(service.update_power(update));
        assert_eq!(service.snapshot().revision, 1);
        assert!(!service.update_power(update));
        assert_eq!(service.snapshot().revision, 1);
    }

    #[test]
    fn battery_menu_actions_bypass_revision_fence_but_still_reach_handler() {
        let mut service = SystemService::default();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = std::rc::Rc::clone(&seen);
        service.set_action_handler(Box::new(move |action, _| {
            sink.borrow_mut().push(action);
            Ok(())
        }));
        let _ = service.update_power(PowerSnapshot {
            percent: 50,
            ..PowerSnapshot::default()
        });
        let stale = service.snapshot().revision.wrapping_add(7);
        service
            .perform_action(SystemAction::BatteryMenuOpened, stale)
            .expect("menu open is not revision-fenced");
        assert_eq!(seen.borrow().as_slice(), [SystemAction::BatteryMenuOpened]);
    }

    #[test]
    fn reconnect_backoff_is_bounded() {
        let mut backoff = ReconnectBackoff::default();
        let mut last = 0;
        for _ in 0..32 {
            last = backoff.next_delay_ms();
        }
        assert_eq!(last, 30_000);
    }

    #[test]
    fn revision_changes_only_for_logical_snapshot_changes() {
        let mut service = SystemService::default();
        let update = NetworkSnapshot {
            generation: 1,
            ..NetworkSnapshot::default()
        };
        assert!(service.update_network(update.clone()));
        assert_eq!(service.snapshot().revision, 1);
        assert!(!service.update_network(update));
        assert_eq!(service.snapshot().revision, 1);
    }

    #[test]
    fn action_rejects_stale_revision_before_missing_provider() {
        let mut service = SystemService::default();
        let error = service
            .perform_action(SystemAction::Scan, 1)
            .expect_err("stale action must fail");
        assert_eq!(error.code, ErrorCode::StaleRevision);
    }
}

/// Volume and mute writes carry their own `PulseAudio` generation fences, so the
/// snapshot-wide revision is not required for them. Battery-menu visibility toggles
/// are idempotent and must not be rejected because power or CPU samples moved the revision.
#[must_use]
fn bypasses_revision_fence(action: &SystemAction) -> bool {
    matches!(
        action,
        SystemAction::SetVolume(_)
            | SystemAction::SetMute(_)
            | SystemAction::BatteryMenuOpened
            | SystemAction::BatteryMenuClosed
    )
}

#[cfg(test)]
mod media_key_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use flamewm_api::system::{AudioEndpointSnapshot, ServiceAvailability};

    use super::*;

    fn sink(id: u32, is_default: bool, volume: u8, muted: bool) -> AudioEndpointSnapshot {
        AudioEndpointSnapshot {
            id,
            kind: AudioEndpointKind::Sink,
            name: format!("sink{id}"),
            description: String::new(),
            volume_percent: volume,
            muted,
            is_default,
        }
    }

    fn service_with(
        endpoints: Vec<AudioEndpointSnapshot>,
    ) -> (SystemService, Rc<RefCell<Vec<SystemAction>>>) {
        let mut service = SystemService::default();
        let update = AudioSnapshot {
            availability: ServiceAvailability::Available,
            generation: 4,
            server_generation: 9,
            endpoints,
            ..AudioSnapshot::default()
        };
        assert!(service.update_audio(update));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        service.set_action_handler(Box::new(move |action, _snapshot| {
            sink.borrow_mut().push(action);
            Ok(())
        }));
        (service, seen)
    }

    #[test]
    fn step_moves_five_percent_and_clamps_to_0_through_150() {
        assert_eq!(stepped_volume(40, true, 150), 45);
        assert_eq!(stepped_volume(40, false, 150), 35);
        assert_eq!(stepped_volume(148, true, 150), 150);
        assert_eq!(stepped_volume(150, true, 150), 150);
        assert_eq!(stepped_volume(3, false, 150), 0);
        assert_eq!(stepped_volume(0, false, 150), 0);
    }

    #[test]
    fn step_clamps_to_the_live_cap_when_raise_maximum_is_off() {
        let cap = volume_cap_percent(false);
        assert_eq!(stepped_volume(98, true, cap), 100);
        assert_eq!(stepped_volume(100, true, cap), 100);
        assert_eq!(stepped_volume(120, true, cap), 100);
        assert_eq!(stepped_volume(120, false, cap), 100);
        assert_eq!(stepped_volume(60, false, cap), 55);
    }

    #[test]
    fn volume_cap_follows_the_shared_raise_maximum_value() {
        assert_eq!(AUDIO_RAISE_MAXIMUM_KEY, "audioRaiseMaximum");
        assert_eq!(volume_cap_percent(false), 100);
        assert_eq!(volume_cap_percent(true), 150);
    }

    #[test]
    fn volume_up_is_capped_at_100_without_raise_maximum() {
        let (mut service, seen) = service_with(vec![sink(2, true, 98, false)]);
        service
            .media_audio_key(MediaAudioKey::VolumeUp, volume_cap_percent(false))
            .expect("dispatch");
        assert!(matches!(
            seen.borrow().as_slice(),
            [SystemAction::SetVolume(AudioVolumeAction { percent: 100, .. })]
        ));
    }

    #[test]
    fn volume_up_reaches_150_with_raise_maximum() {
        let (mut service, seen) = service_with(vec![sink(2, true, 148, false)]);
        service
            .media_audio_key(MediaAudioKey::VolumeUp, volume_cap_percent(true))
            .expect("dispatch");
        assert!(matches!(
            seen.borrow().as_slice(),
            [SystemAction::SetVolume(AudioVolumeAction { percent: 150, .. })]
        ));
    }

    #[test]
    fn volume_up_dispatches_set_volume_with_generation_fence_on_default_sink() {
        let (mut service, seen) =
            service_with(vec![sink(1, false, 10, false), sink(2, true, 40, false)]);
        service
            .media_audio_key(MediaAudioKey::VolumeUp, MAX_VOLUME_PERCENT)
            .expect("dispatch");
        assert_eq!(
            seen.borrow().as_slice(),
            [SystemAction::SetVolume(AudioVolumeAction {
                target: AudioTarget::Endpoint {
                    id: 2,
                    kind: AudioEndpointKind::Sink
                },
                percent: 45,
                generation: 4,
                server_generation: 9,
            })]
        );
    }

    #[test]
    fn volume_down_at_zero_dispatches_zero_not_underflow() {
        let (mut service, seen) = service_with(vec![sink(2, true, 0, false)]);
        service
            .media_audio_key(MediaAudioKey::VolumeDown, MAX_VOLUME_PERCENT)
            .expect("dispatch");
        assert!(matches!(
            seen.borrow().as_slice(),
            [SystemAction::SetVolume(AudioVolumeAction {
                percent: 0,
                ..
            })]
        ));
    }

    #[test]
    fn mute_key_toggles_the_current_default_sink_state() {
        let (mut service, seen) = service_with(vec![sink(2, true, 40, true)]);
        service
            .media_audio_key(MediaAudioKey::VolumeMute, MAX_VOLUME_PERCENT)
            .expect("dispatch");
        assert!(matches!(
            seen.borrow().as_slice(),
            [SystemAction::SetMute(AudioMuteAction {
                muted: false,
                generation: 4,
                server_generation: 9,
                ..
            })]
        ));
    }

    #[test]
    fn no_default_sink_is_a_noop_without_handler_call() {
        let (mut service, seen) = service_with(vec![sink(1, false, 40, false)]);
        service
            .media_audio_key(MediaAudioKey::VolumeUp, MAX_VOLUME_PERCENT)
            .expect("no-op");
        service
            .media_audio_key(MediaAudioKey::VolumeMute, MAX_VOLUME_PERCENT)
            .expect("no-op");
        assert!(seen.borrow().is_empty());
    }
}
