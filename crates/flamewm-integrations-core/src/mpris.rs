use std::collections::BTreeMap;

use flamewm_api::system::{MediaSnapshot, PlaybackState, ServiceAvailability};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub bus_name: String,
    pub identity: String,
    pub track_title: String,
    pub artist: String,
    pub playback: PlaybackState,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaIntent {
    Play { bus_name: String },
    Pause { bus_name: String },
    PlayPause { bus_name: String },
    Next { bus_name: String },
    Previous { bus_name: String },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MprisController {
    generation: u64,
    players: BTreeMap<String, PlayerInfo>,
    active_bus_name: Option<String>,
    reconcile_pending: bool,
}

impl MprisController {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn player_appeared(&mut self, player: PlayerInfo) {
        self.generation = self.generation.saturating_add(1).max(1);
        self.players.insert(player.bus_name.clone(), player);
        self.reconcile_pending = true;
    }

    pub fn player_vanished(&mut self, bus_name: &str) -> bool {
        let removed = self.players.remove(bus_name).is_some();
        if removed {
            self.generation = self.generation.saturating_add(1).max(1);
            self.reconcile_pending = true;
        }
        removed
    }

    pub fn properties_changed(&mut self, player: PlayerInfo, captured_generation: u64) -> bool {
        if captured_generation != self.generation || !self.players.contains_key(&player.bus_name) {
            return false;
        }
        self.players.insert(player.bus_name.clone(), player);
        self.reconcile_pending = true;
        true
    }

    pub fn reconcile(&mut self) -> bool {
        if !self.reconcile_pending {
            return false;
        }
        self.reconcile_pending = false;
        self.active_bus_name = pick_active(self.players.values());
        true
    }

    #[must_use]
    pub fn snapshot(&self) -> MediaSnapshot {
        let Some(bus) = self.active_bus_name.as_ref() else {
            return MediaSnapshot {
                availability: if self.players.is_empty() {
                    ServiceAvailability::Unavailable
                } else {
                    ServiceAvailability::Available
                },
                generation: self.generation,
                ..MediaSnapshot::default()
            };
        };
        let Some(player) = self.players.get(bus) else {
            return MediaSnapshot::default();
        };
        MediaSnapshot {
            availability: ServiceAvailability::Available,
            generation: self.generation,
            active_bus_name: player.bus_name.clone(),
            identity: player.identity.clone(),
            title: player.track_title.clone(),
            artist: player.artist.clone(),
            playback: player.playback,
            can_play: player.can_play,
            can_pause: player.can_pause,
            can_next: player.can_next,
            can_previous: player.can_previous,
        }
    }

    pub fn request(&self, intent: MediaIntent, caller_generation: u64) -> FlameResult<MediaIntent> {
        if caller_generation != self.generation {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "stale MPRIS generation",
            ));
        }
        let bus = match &intent {
            MediaIntent::Play { bus_name }
            | MediaIntent::Pause { bus_name }
            | MediaIntent::PlayPause { bus_name }
            | MediaIntent::Next { bus_name }
            | MediaIntent::Previous { bus_name } => bus_name,
        };
        let Some(player) = self.players.get(bus) else {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "MPRIS player no longer exists",
            ));
        };
        let supported = match &intent {
            MediaIntent::Play { .. } => player.can_play,
            MediaIntent::Pause { .. } => player.can_pause,
            MediaIntent::PlayPause { .. } => player.can_play || player.can_pause,
            MediaIntent::Next { .. } => player.can_next,
            MediaIntent::Previous { .. } => player.can_previous,
        };
        if !supported {
            return Err(FlameError::new(
                ErrorCode::Unsupported,
                "MPRIS player does not support requested action",
            ));
        }
        Ok(intent)
    }
}

#[must_use]
pub fn pick_active<'a>(players: impl Iterator<Item = &'a PlayerInfo>) -> Option<String> {
    let mut playing: Option<&str> = None;
    let mut paused: Option<&str> = None;
    let mut remaining: Option<&str> = None;
    for player in players {
        let target = match player.playback {
            PlaybackState::Playing => &mut playing,
            PlaybackState::Paused => &mut paused,
            PlaybackState::Stopped | PlaybackState::Unavailable => &mut remaining,
        };
        let should_replace = match *target {
            Some(current) => player.bus_name.as_str() < current,
            None => true,
        };
        if should_replace {
            *target = Some(player.bus_name.as_str());
        }
    }
    playing.or(paused).or(remaining).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus: &str, playback: PlaybackState) -> PlayerInfo {
        PlayerInfo {
            bus_name: bus.to_owned(),
            identity: bus.to_owned(),
            track_title: String::new(),
            artist: String::new(),
            playback,
            can_play: true,
            can_pause: true,
            can_next: true,
            can_previous: true,
        }
    }

    #[test]
    fn playing_player_wins_then_lexicographic_bus_name() {
        let players = [
            player("org.mpris.MediaPlayer2.zeta", PlaybackState::Playing),
            player("org.mpris.MediaPlayer2.alpha", PlaybackState::Paused),
            player("org.mpris.MediaPlayer2.beta", PlaybackState::Playing),
        ];
        assert_eq!(
            pick_active(players.iter()).as_deref(),
            Some("org.mpris.MediaPlayer2.beta")
        );
    }

    #[test]
    fn appearing_player_becomes_snapshot_and_vanishing_player_clears_it() {
        let mut controller = MprisController::default();
        assert_eq!(
            controller.snapshot().availability,
            ServiceAvailability::Unavailable
        );

        controller.player_appeared(player("org.mpris.MediaPlayer2.a", PlaybackState::Playing));
        assert!(controller.reconcile());
        let shown = controller.snapshot();
        assert_eq!(shown.availability, ServiceAvailability::Available);
        assert_eq!(shown.active_bus_name, "org.mpris.MediaPlayer2.a");

        assert!(controller.player_vanished("org.mpris.MediaPlayer2.a"));
        assert!(controller.reconcile());
        assert_eq!(
            controller.snapshot().availability,
            ServiceAvailability::Unavailable
        );
    }

    #[test]
    fn vanished_active_player_is_reconciled_away() {
        let mut controller = MprisController::default();
        controller.player_appeared(player("org.mpris.MediaPlayer2.a", PlaybackState::Playing));
        let _ = controller.reconcile();
        assert!(controller.player_vanished("org.mpris.MediaPlayer2.a"));
        let _ = controller.reconcile();
        assert!(controller.snapshot().active_bus_name.is_empty());
    }
}
