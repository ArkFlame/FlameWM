//! Settings and output-geometry change detection, plus control snapshot folding.

use super::*;

pub(super) fn typography_changed(previous: &LockSettings, next: &LockSettings) -> bool {
    previous.font_family != next.font_family
        || previous.font_bold != next.font_bold
        || previous.font_size_offset != next.font_size_offset
}

pub(super) fn background_changed(previous: &LockSettings, next: &LockSettings) -> bool {
    previous.background_color != next.background_color
        || previous.background_image != next.background_image
        || previous.background_mode != next.background_mode
        || previous.share_wallpaper != next.share_wallpaper
        || previous.blur_percent != next.blur_percent
        || previous.dim_percent != next.dim_percent
}

pub(super) fn output_rects_need_rebuild(current: &[Rect], next: &[Rect]) -> bool {
    current != next
}

pub(super) fn latest_control_snapshots(
    signals: impl IntoIterator<Item = ControlSignal>,
) -> (
    Option<flamewm_api::settings::SettingsSnapshot>,
    Option<DisplaySnapshot>,
) {
    let mut settings = None;
    let mut displays = None;
    for signal in signals {
        match signal {
            ControlSignal::SettingsSnapshotChanged { snapshot } => settings = Some(snapshot),
            ControlSignal::DisplaysSnapshotChanged { snapshot } => displays = Some(snapshot),
            _ => {}
        }
    }
    (settings, displays)
}
