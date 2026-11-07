use super::*;

/// What a release outside the active popup does to the quick-control popups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutsideReleaseDecision {
    /// Not an outside release: the normal routing continues.
    Ignore,
    /// Close the popup; the release is consumed.
    Close,
    /// The tray volume node toggles an open audio popup closed. The release is
    /// consumed, so the same click cannot reopen it on this turn.
    ToggleTrayVolumeClosed,
}

/// Pure decision for release handling. Non-Escape keyboard releases never
/// close popups (Escape is handled before this point). A release on the tray
/// volume node is an outside release for the popup, but only closes an open
/// audio popup; with no audio popup open it falls through to the open path.
pub(crate) fn outside_release_decision(
    action: &str,
    is_release: bool,
    is_keyboard: bool,
    inside: bool,
    audio_popup_open: bool,
) -> OutsideReleaseDecision {
    if !is_release || is_keyboard {
        return OutsideReleaseDecision::Ignore;
    }
    if action != OUTSIDE_RELEASE_ACTION && inside {
        return OutsideReleaseDecision::Ignore;
    }
    if action == TRAY_VOLUME_ACTION {
        return if audio_popup_open {
            OutsideReleaseDecision::ToggleTrayVolumeClosed
        } else {
            OutsideReleaseDecision::Ignore
        };
    }
    OutsideReleaseDecision::Close
}

/// Window in which the opener release that follows an outside-press dismissal
/// is still treated as the same click.
pub(crate) const DISMISS_LATCH_TTL: Duration = Duration::from_millis(300);

/// Popup group closed by the latest outside-press dismissal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DismissLatch {
    pub(crate) group: &'static str,
    pub(crate) at: Instant,
}

/// Popup group an opener action toggles; `None` for non-opener actions.
pub(crate) fn opener_group(action: &str) -> Option<&'static str> {
    match action {
        "start.toggle" => Some("start"),
        TRAY_VOLUME_ACTION => Some("audio"),
        "network.open" => Some("network"),
        "battery.open" => Some("battery"),
        "weather.open" => Some("weather"),
        "clock.open" => Some("calendar"),
        "media.open" | "media.toggle" => Some("media"),
        _ => None,
    }
}

/// Popup group a quick-control kind occupies; matches its opener's `opener_group`.
pub(crate) fn quick_popup_group(kind: QuickControlKind) -> &'static str {
    match kind {
        QuickControlKind::Audio => "audio",
        QuickControlKind::Network => "network",
        QuickControlKind::Calendar => "calendar",
        QuickControlKind::Battery => "battery",
        QuickControlKind::Weather => "weather",
    }
}

/// True when an observed root press lands outside every quick popup. `title` is
/// the `WM_NAME` of the top-level window under the pointer.
pub(crate) fn raw_press_is_outside_quick_popups(title: Option<&str>) -> bool {
    !title.is_some_and(is_quick_popup_title)
}

/// Consumes the opener release that is the same click as the dismissal that set
/// `latch`: true means the opener must be a no-op. The latch survives unrelated
/// events and expires after `DISMISS_LATCH_TTL`.
pub(crate) fn latch_consumes_opener(
    latch: &mut Option<DismissLatch>,
    action: &str,
    now: Instant,
) -> bool {
    let Some(pending) = *latch else {
        return false;
    };
    if now.saturating_duration_since(pending.at) > DISMISS_LATCH_TTL {
        *latch = None;
        return false;
    }
    if opener_group(action) == Some(pending.group) {
        *latch = None;
        return true;
    }
    false
}
