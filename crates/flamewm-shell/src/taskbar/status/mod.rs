pub mod audio;
pub mod battery;
pub mod media;
pub mod network;

use flamewm_api::system::SystemSnapshot;
use flamewm_shell_core::{StatusPopovers, StatusViews};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusView {
    pub views: StatusViews,
    pub popovers: StatusPopovers,
}

#[must_use]
pub fn project(snapshot: &SystemSnapshot) -> StatusView {
    let mut views = StatusViews::from_snapshot(snapshot);
    views.audio = audio::project(snapshot);
    views.media = media::project(snapshot);
    views.network = network::project(snapshot);
    StatusView {
        views,
        popovers: StatusPopovers {
            audio: audio::popover(snapshot),
            media: media::popover(snapshot),
            network: network::popover(snapshot),
        },
    }
}
