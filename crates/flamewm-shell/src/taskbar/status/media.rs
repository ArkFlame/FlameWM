use flamewm_api::system::{PlaybackState, ServiceAvailability, SystemSnapshot};
use flamewm_shell_core::{MediaPopoverModel, MediaView};

use super::super::intent::StatusIntent;

#[must_use]
pub fn project(snapshot: &SystemSnapshot) -> MediaView {
    let mut view = flamewm_shell_core::StatusViews::from_snapshot(snapshot).media;
    view.visible = snapshot.media.availability == ServiceAvailability::Available
        && matches!(
            snapshot.media.playback,
            PlaybackState::Playing | PlaybackState::Paused
        );
    view
}

#[must_use]
pub fn popover(snapshot: &SystemSnapshot) -> Option<MediaPopoverModel> {
    if snapshot.media.availability != ServiceAvailability::Available
        || !matches!(
            snapshot.media.playback,
            PlaybackState::Playing | PlaybackState::Paused
        )
    {
        return None;
    }
    flamewm_shell_core::StatusPopovers::from_snapshot(snapshot).media
}

#[must_use]
pub fn intent(snapshot: &SystemSnapshot) -> Option<StatusIntent> {
    (snapshot.media.availability == ServiceAvailability::Available
        && matches!(
            snapshot.media.playback,
            PlaybackState::Playing | PlaybackState::Paused
        ))
    .then_some(StatusIntent::Media)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(availability: ServiceAvailability, playback: PlaybackState) -> SystemSnapshot {
        let mut system = SystemSnapshot::default();
        system.media.availability = availability;
        system.media.playback = playback;
        system
    }

    #[test]
    fn indicator_hidden_without_player_and_shown_when_player_plays() {
        let none = snapshot(ServiceAvailability::Unavailable, PlaybackState::Unavailable);
        assert!(!project(&none).visible);
        assert!(intent(&none).is_none());

        let playing = snapshot(ServiceAvailability::Available, PlaybackState::Playing);
        assert!(project(&playing).visible);
        assert_eq!(intent(&playing), Some(StatusIntent::Media));
    }

    #[test]
    fn indicator_hidden_when_player_stopped() {
        let stopped = snapshot(ServiceAvailability::Available, PlaybackState::Stopped);
        assert!(!project(&stopped).visible);
    }
}
