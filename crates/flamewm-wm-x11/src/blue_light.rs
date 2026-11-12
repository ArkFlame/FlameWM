//! Blue light filter on X11: one `RandR` gamma upload per output.
//!
//! X11 exposes the gamma ramp through `RandR` `SetCrtcGamma`, so the filter is a real output
//! calibration: every pixel the server scans out is warmed. The schedule needs a wake when its
//! window boundary is crossed; the WM runtime asks [`next_refresh_delay`] for that deadline, so an
//! idle desktop with a schedule arms one timer instead of polling.
//!
//! The ramp maths is `flamewm_api::blue_light`; this module only decides *when* and *where* to
//! upload it.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use flamewm_api::blue_light::{BlueLightConfig, resolve_at};
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

/// What the last uploaded ramp covered, so an unchanged intensity never round-trips.
#[derive(Clone, PartialEq, Eq)]
struct Applied {
    intensity: u8,
}

thread_local! {
    static APPLIED: RefCell<Option<Applied>> = const { RefCell::new(None) };
    /// Committed configuration, refreshed at the single settings sync point.
    static CONFIG: Cell<BlueLightConfig> = const { Cell::new(DEFAULTS) };
}

/// The product defaults, restated as a `const` so the thread-local starts without a call.
const DEFAULTS: BlueLightConfig = BlueLightConfig {
    intensity: flamewm_api::blue_light::DEFAULT_INTENSITY,
    night_shift: false,
    schedule: false,
    start_minutes: flamewm_api::blue_light::DEFAULT_START_MINUTES,
    end_minutes: flamewm_api::blue_light::DEFAULT_END_MINUTES,
};

/// Records the committed filter configuration (called from the settings sync point).
pub(crate) fn set_config(config: BlueLightConfig) {
    CONFIG.with(|slot| slot.set(config));
}

/// The committed filter configuration.
pub(crate) fn config() -> BlueLightConfig {
    CONFIG.with(Cell::get)
}

/// Minutes after local midnight.
pub(crate) fn now_minutes() -> u32 {
    use chrono::{Local, Timelike};
    let now = Local::now();
    now.hour() * 60 + now.minute()
}

/// Applies the filter for the current minute; returns whether a ramp was uploaded.
pub(crate) fn apply(
    conn: &RustConnection,
    root: Window,
    config: BlueLightConfig,
) -> Result<bool, x11rb::errors::ReplyError> {
    apply_at(conn, root, config, now_minutes())
}

/// Applies the filter for `now`; the clock is injected so the schedule is testable.
///
/// This runs on every WM loop turn, so an unchanged intensity returns before any round trip: the
/// crtc walk happens only when the ramp must actually be uploaded.
pub(crate) fn apply_at(
    conn: &RustConnection,
    root: Window,
    config: BlueLightConfig,
    now: u32,
) -> Result<bool, x11rb::errors::ReplyError> {
    let intensity = resolve_at(config, now).intensity;
    if APPLIED.with_borrow(|slot| slot.as_ref().is_some_and(|a| a.intensity == intensity)) {
        return Ok(false);
    }
    let crtcs = crtcs_for_root(conn, root)?;
    APPLIED.with_borrow_mut(|slot| *slot = Some(Applied { intensity }));
    for crtc in crtcs {
        let Ok(size) = gamma_size(conn, crtc) else {
            continue;
        };
        if size == 0 {
            // A virtual or nested server may report no ramp; the filter simply cannot apply there.
            continue;
        }
        let (red, green, blue) = flamewm_api::blue_light::gamma_ramp(size, intensity);
        let uploaded = conn
            .randr_set_crtc_gamma(crtc, &red, &green, &blue)
            .map_err(x11rb::errors::ReplyError::ConnectionError);
        let uploaded = uploaded.and_then(x11rb::cookie::VoidCookie::check);
        if let Err(error) = uploaded {
            // A read-only gamma is a property of the server, not a WM failure.
            eprintln!("flamewm: blue light filter not applied to crtc {crtc}: {error}");
        }
    }
    Ok(true)
}

/// Forgets the last upload so the next apply walks the crtcs again and re-uploads the same ramp.
///
/// Called when the output topology changed: the same intensity must reach the new crtc set, and the
/// cached crtc ids may no longer be valid. An idle build has no topology signal to hook, so the
/// forget is also what keeps a hotplug from leaving an unwarmed output.
pub(crate) fn invalidate() {
    APPLIED.with_borrow_mut(|slot| *slot = None);
}

/// Entries in one crtc's gamma ramp, or 0 when the crtc has none.
fn gamma_size(conn: &RustConnection, crtc: u32) -> Result<usize, x11rb::errors::ReplyError> {
    Ok(usize::from(
        conn.randr_get_crtc_gamma_size(crtc)?.reply()?.size,
    ))
}

/// Crtcs currently scanning out on this screen.
pub(crate) fn crtcs_for_root(
    conn: &RustConnection,
    root: Window,
) -> Result<Vec<u32>, x11rb::errors::ReplyError> {
    let resources = conn.randr_get_screen_resources_current(root)?.reply()?;
    let mut crtcs = Vec::new();
    for output in resources.outputs {
        let Ok(info) = conn
            .randr_get_output_info(output, resources.config_timestamp)?
            .reply()
        else {
            continue;
        };
        if info.crtc != 0 && !crtcs.contains(&info.crtc) {
            crtcs.push(info.crtc);
        }
    }
    Ok(crtcs)
}

/// Delay until the next schedule boundary, or `None` when nothing has to wake.
///
/// Only a scheduled window can flip the filter without a user action, so an always-on filter or a
/// disabled schedule arms nothing. The delay is capped at an hour so a clock step or a time-zone
/// change cannot leave the filter stale until the next midnight.
pub(crate) fn next_refresh_delay(config: BlueLightConfig, now_minutes: u32) -> Option<Duration> {
    if !config.night_shift || !config.schedule {
        return None;
    }
    let start = config.start_minutes;
    let end = config.end_minutes;
    if start == end {
        return None;
    }
    let seconds = u64::from(seconds_to_boundary(now_minutes, start, end));
    Some(Duration::from_secs(seconds.clamp(1, 3_600)))
}

/// Seconds until either boundary of an active schedule is crossed, floored at one second.
fn seconds_to_boundary(now_minutes: u32, start: u32, end: u32) -> u32 {
    let day = flamewm_api::blue_light::MINUTES_PER_DAY;
    let now = now_minutes % day;
    // Distance forward in the day to each boundary; 0 means "right now", which is a real crossing.
    let forward = |target: u32| (target + day - now) % day;
    let minutes = forward(start).min(forward(end));
    // The clock's second hand is unknown here, so wait a full minute when the boundary is now.
    (minutes.max(1)) * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduled() -> BlueLightConfig {
        BlueLightConfig {
            intensity: 60,
            night_shift: true,
            schedule: true,
            start_minutes: 22 * 60,
            end_minutes: 6 * 60,
        }
    }

    /// Only a scheduled window can flip on its own, so only it arms a wake.
    #[test]
    fn a_wake_is_armed_only_for_an_enabled_schedule() {
        assert!(next_refresh_delay(scheduled(), 12 * 60).is_some());
        assert_eq!(
            next_refresh_delay(
                BlueLightConfig {
                    schedule: false,
                    ..scheduled()
                },
                12 * 60
            ),
            None
        );
        assert_eq!(
            next_refresh_delay(
                BlueLightConfig {
                    night_shift: false,
                    ..scheduled()
                },
                12 * 60
            ),
            None
        );
        assert_eq!(
            next_refresh_delay(
                BlueLightConfig {
                    start_minutes: 9 * 60,
                    end_minutes: 9 * 60,
                    ..scheduled()
                },
                12 * 60
            ),
            None,
            "an empty window never flips"
        );
    }

    /// The armed delay lands on the boundary and never exceeds the one-hour safety cap.
    #[test]
    fn the_armed_delay_lands_on_a_boundary_within_the_cap() {
        // 21:00 with a 22:00 start: one hour, exactly the cap.
        assert_eq!(
            next_refresh_delay(scheduled(), 21 * 60),
            Some(Duration::from_secs(3_600))
        );
        // 05:59 with a 06:00 end: one minute.
        assert_eq!(
            next_refresh_delay(scheduled(), 5 * 60 + 59),
            Some(Duration::from_secs(60))
        );
        // 12:00: the next boundary is the 22:00 start, ten hours away, capped.
        assert_eq!(
            next_refresh_delay(scheduled(), 12 * 60),
            Some(Duration::from_secs(3_600))
        );
        for now in [0, 5 * 60, 12 * 60, 21 * 60 + 59, 23 * 60] {
            let delay = next_refresh_delay(scheduled(), now).expect("scheduled");
            assert!(delay >= Duration::from_secs(1));
            assert!(delay <= Duration::from_secs(3_600));
        }
    }

    #[test]
    fn the_boundary_distance_wraps_over_midnight() {
        assert_eq!(seconds_to_boundary(22 * 60, 22 * 60, 6 * 60), 60);
        assert_eq!(seconds_to_boundary(21 * 60, 22 * 60, 6 * 60), 3_600);
        assert_eq!(seconds_to_boundary(3 * 60, 22 * 60, 6 * 60), 3 * 3_600);
        assert_eq!(seconds_to_boundary(5 * 60 + 59, 22 * 60, 6 * 60), 60);
    }
}
