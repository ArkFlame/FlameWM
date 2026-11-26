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
    StatusView {
        views: views(snapshot),
        popovers: StatusPopovers {
            audio: audio::popover(snapshot),
            media: media::popover(snapshot),
            network: network::popover(snapshot),
        },
    }
}

/// Status icons and visibility only, without the popup models. The taskbar tray
/// projection reads nothing but `views`, so a system snapshot change no longer
/// allocates the network access-point rows and joined media detail string for
/// popovers that may not even be open.
#[must_use]
pub fn views(snapshot: &SystemSnapshot) -> StatusViews {
    let base = StatusViews::from_snapshot(snapshot);
    StatusViews {
        audio: audio::apply(base.audio, snapshot),
        media: media::apply(base.media, snapshot),
        network: network::apply(base.network, snapshot),
    }
}
