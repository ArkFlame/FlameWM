use flamewm_api::system::{
    AudioSnapshot, MediaSnapshot, NetworkKind, NetworkSnapshot, PlaybackState, ServiceAvailability,
    SystemAction, SystemSnapshot,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

pub type SystemActionHandler = Box<dyn FnMut(SystemAction, SystemSnapshot) -> FlameResult<()>>;

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
        if update == self.snapshot.network {
            return false;
        }
        self.snapshot.network = update;
        self.bump_revision();
        true
    }

    pub fn update_media(&mut self, update: MediaSnapshot) -> bool {
        if update.generation < self.snapshot.media.generation {
            return false;
        }
        if update == self.snapshot.media {
            return false;
        }
        self.snapshot.media = update;
        self.bump_revision();
        true
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
        if update == self.snapshot.audio {
            return false;
        }
        self.snapshot.audio = update;
        self.bump_revision();
        true
    }

    pub fn network_owner_lost(&mut self) {
        let generation = self.snapshot.network.generation.saturating_add(1);
        let update = NetworkSnapshot {
            availability: ServiceAvailability::Unavailable,
            generation,
            kind: NetworkKind::Unavailable,
            ..NetworkSnapshot::default()
        };
        if update != self.snapshot.network {
            self.snapshot.network = update;
            self.bump_revision();
        }
    }

    pub fn mpris_owner_lost(&mut self) {
        let generation = self.snapshot.media.generation.saturating_add(1);
        let update = MediaSnapshot {
            availability: ServiceAvailability::Unavailable,
            generation,
            playback: PlaybackState::Unavailable,
            ..MediaSnapshot::default()
        };
        if update != self.snapshot.media {
            self.snapshot.media = update;
            self.bump_revision();
        }
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
        if update != self.snapshot.audio {
            self.snapshot.audio = update;
            self.bump_revision();
        }
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
                if action.percent > 150 {
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

    fn bump_revision(&mut self) {
        self.snapshot.revision = self.snapshot.revision.saturating_add(1).max(1);
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
        assert!(!bypasses_revision_fence(&SystemAction::SetWifiEnabled(true)));
    }

    #[test]
    fn stale_revision_still_rejects_non_audio_actions() {
        let mut service = SystemService::default();
        service.set_action_handler(Box::new(|_, _| Ok(())));
        let stale = service.snapshot.revision.wrapping_add(1);
        let result = service.perform_action(SystemAction::SetWifiEnabled(true), stale);
        assert!(result.is_err());
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

/// Volume and mute writes carry their own PulseAudio generation fences, so the
/// snapshot-wide revision is not required for them.
#[must_use]
pub fn bypasses_revision_fence(action: &SystemAction) -> bool {
    matches!(action, SystemAction::SetVolume(_) | SystemAction::SetMute(_))
}
