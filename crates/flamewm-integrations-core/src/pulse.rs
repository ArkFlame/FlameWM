use flamewm_api::system::{
    AudioEndpointSnapshot, AudioMuteAction, AudioSnapshot, AudioStreamSnapshot, AudioTarget,
    AudioVolumeAction, ServiceAvailability,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::backoff::next_reconnect_delay_ms;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PulseState {
    #[default]
    Unavailable,
    Connecting,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseStatus {
    pub state: PulseState,
    pub endpoints: Vec<AudioEndpointSnapshot>,
    pub streams: Vec<AudioStreamSnapshot>,
}

impl Default for PulseStatus {
    fn default() -> Self {
        Self {
            state: PulseState::Unavailable,
            endpoints: Vec::new(),
            streams: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PulseIntent {
    SetVolume(AudioVolumeAction),
    SetMute(AudioMuteAction),
    Reconcile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffConfig {
    pub initial_ms: u64,
    pub maximum_ms: u64,
    pub maximum_attempts: u8,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_ms: 300,
            maximum_ms: 8_000,
            maximum_attempts: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseController {
    generation: u64,
    server_generation: u64,
    status: PulseStatus,
    reconnect_attempts: u8,
    reconnect_pending: bool,
    backoff: BackoffConfig,
    reconcile_pending: bool,
}

impl Default for PulseController {
    fn default() -> Self {
        Self {
            generation: 0,
            server_generation: 0,
            status: PulseStatus::default(),
            reconnect_attempts: 0,
            reconnect_pending: false,
            backoff: BackoffConfig::default(),
            reconcile_pending: false,
        }
    }
}

impl PulseController {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn server_generation(&self) -> u64 {
        self.server_generation
    }

    pub fn connect_started(&mut self) {
        self.generation = self.generation.saturating_add(1).max(1);
        self.status.state = PulseState::Connecting;
    }

    pub fn ready(&mut self, captured_generation: u64) -> bool {
        if captured_generation != self.generation {
            return false;
        }
        self.status.state = PulseState::Ready;
        self.reconnect_attempts = 0;
        self.reconnect_pending = false;
        true
    }

    pub fn server_restarted(&mut self) {
        self.generation = self.generation.saturating_add(1).max(1);
        self.server_generation = self.server_generation.saturating_add(1).max(1);
        self.status = PulseStatus::default();
        self.reconcile_pending = false;
        self.reconnect_pending = true;
    }

    pub fn reconcile(
        &mut self,
        endpoints: Vec<AudioEndpointSnapshot>,
        streams: Vec<AudioStreamSnapshot>,
        captured_generation: u64,
        captured_server_generation: u64,
    ) -> bool {
        if captured_generation != self.generation
            || captured_server_generation != self.server_generation
        {
            return false;
        }
        self.status = PulseStatus {
            state: PulseState::Ready,
            endpoints,
            streams,
        };
        self.reconcile_pending = false;
        true
    }

    pub fn subscription_event(&mut self, captured_generation: u64) -> bool {
        if captured_generation != self.generation || self.reconcile_pending {
            return false;
        }
        self.reconcile_pending = true;
        true
    }

    pub fn take_reconcile_intent(&mut self, captured_generation: u64) -> Option<PulseIntent> {
        if captured_generation != self.generation || !self.reconcile_pending {
            return None;
        }
        self.reconcile_pending = false;
        Some(PulseIntent::Reconcile)
    }

    pub fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<PulseIntent> {
        self.validate_ready(action.generation, action.server_generation)?;
        if action.percent > 150 {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "audio volume must be in 0..=150",
            ));
        }
        self.validate_target(action.target)?;
        Ok(PulseIntent::SetVolume(action))
    }

    pub fn set_mute(&self, action: AudioMuteAction) -> FlameResult<PulseIntent> {
        self.validate_ready(action.generation, action.server_generation)?;
        self.validate_target(action.target)?;
        Ok(PulseIntent::SetMute(action))
    }

    pub fn next_reconnect_delay_ms(&mut self) -> Option<u64> {
        next_reconnect_delay_ms(
            self.backoff.initial_ms,
            self.backoff.maximum_ms,
            self.backoff.maximum_attempts,
            &mut self.reconnect_attempts,
            &mut self.reconnect_pending,
        )
    }

    #[must_use]
    pub fn snapshot(&self) -> AudioSnapshot {
        let default_sink = self
            .status
            .endpoints
            .iter()
            .find(|endpoint| endpoint.is_default);
        AudioSnapshot {
            availability: if self.status.state == PulseState::Ready {
                ServiceAvailability::Available
            } else {
                ServiceAvailability::Unavailable
            },
            generation: self.generation,
            server_generation: self.server_generation,
            sink_name: default_sink.map_or_else(String::new, |sink| sink.name.clone()),
            volume_percent: default_sink.map_or(0, |sink| sink.volume_percent),
            muted: default_sink.is_some_and(|sink| sink.muted),
            ..AudioSnapshot::default()
        }
        .with_items(self.status.endpoints.clone(), self.status.streams.clone())
    }

    fn validate_ready(&self, generation: u64, server_generation: u64) -> FlameResult<()> {
        if generation != self.generation || server_generation != self.server_generation {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "stale PulseAudio generation",
            ));
        }
        if self.status.state != PulseState::Ready {
            return Err(FlameError::new(
                ErrorCode::Unavailable,
                "PulseAudio/PipeWire-Pulse is unavailable",
            ));
        }
        Ok(())
    }

    fn validate_target(&self, target: AudioTarget) -> FlameResult<()> {
        let found = match target {
            AudioTarget::Endpoint { id, kind } => self
                .status
                .endpoints
                .iter()
                .any(|endpoint| endpoint.id == id && endpoint.kind == kind),
            AudioTarget::Stream { id } => self.status.streams.iter().any(|stream| stream.id == id),
        };
        if found {
            Ok(())
        } else {
            Err(FlameError::new(
                ErrorCode::NotFound,
                "audio target no longer exists",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_server_callback_is_rejected_after_restart() {
        let mut pulse = PulseController::default();
        pulse.connect_started();
        let generation = pulse.generation();
        let server_generation = pulse.server_generation();
        pulse.server_restarted();
        assert!(!pulse.reconcile(Vec::new(), Vec::new(), generation, server_generation));
    }

    #[test]
    fn server_restart_uses_bounded_reconnect_backoff() {
        let mut pulse = PulseController::default();
        pulse.server_restarted();
        assert_eq!(
            (0..=8)
                .map(|_| pulse.next_reconnect_delay_ms())
                .collect::<Vec<_>>(),
            [
                Some(300),
                Some(600),
                Some(1_200),
                Some(2_400),
                Some(4_800),
                Some(8_000),
                Some(8_000),
                Some(8_000),
                None,
            ]
        );
    }

    #[test]
    fn subscription_burst_coalesces_to_one_reconcile() {
        let mut pulse = PulseController::default();
        pulse.connect_started();
        let generation = pulse.generation();
        assert!(pulse.subscription_event(generation));
        assert!(!pulse.subscription_event(generation));
        assert_eq!(
            pulse.take_reconcile_intent(generation),
            Some(PulseIntent::Reconcile)
        );
    }

    #[test]
    fn target_actions_reject_stale_server_and_unknown_id() {
        let mut pulse = PulseController::default();
        pulse.connect_started();
        let generation = pulse.generation();
        assert!(pulse.ready(generation));
        assert!(pulse.reconcile(
            vec![AudioEndpointSnapshot {
                id: 4,
                kind: flamewm_api::system::AudioEndpointKind::Sink,
                name: "sink".to_owned(),
                description: String::new(),
                volume_percent: 100,
                muted: false,
                is_default: true,
            }],
            Vec::new(),
            generation,
            pulse.server_generation(),
        ));
        let stale = pulse.set_volume(AudioVolumeAction {
            target: AudioTarget::Endpoint {
                id: 4,
                kind: flamewm_api::system::AudioEndpointKind::Sink,
            },
            percent: 100,
            generation,
            server_generation: 1,
        });
        assert_eq!(
            stale.expect_err("stale server").code,
            ErrorCode::StaleRevision
        );
        let missing = pulse.set_mute(AudioMuteAction {
            target: AudioTarget::Stream { id: 9 },
            muted: true,
            generation,
            server_generation: pulse.server_generation(),
        });
        assert_eq!(
            missing.expect_err("missing stream").code,
            ErrorCode::NotFound
        );
    }
}
