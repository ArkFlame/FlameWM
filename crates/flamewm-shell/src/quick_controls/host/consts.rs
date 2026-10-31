use super::*;

/// Pre-commit debug probe; the final geometry commit limit stays fixed
/// (refusals leave state untouched, no limit raise).
pub(crate) const POPUP_TRANSITION: DebugEventId = DebugEventId("shell.popup.transition");

pub(crate) const AUDIO_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr"));

pub(crate) const NETWORK_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr"));

pub(crate) const CALENDAR_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-calendar.rwr"));

pub(crate) const BATTERY_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr"));

pub(crate) const WEATHER_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-weather.rwr"));

pub(crate) const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";
