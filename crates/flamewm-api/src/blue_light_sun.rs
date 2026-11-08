//! Sunrise and sunset for the Night Light `sunset` schedule.
//!
//! Uses the NOAA solar position approximation (declination and equation of time from a Fourier
//! fit, sunrise/sunset at the 90.833 degree zenith that includes refraction). Accuracy is a few
//! minutes, which is far below what a night-light schedule can perceive. Pure maths: no clock,
//! no location lookup. The caller supplies latitude, longitude, day of year and the local UTC
//! offset, and gets local minutes after midnight back.

/// Zenith that places the sun's upper limb on the horizon with standard refraction.
pub const SUNRISE_SUNSET_ZENITH_DEG: f64 = 90.833;

/// Minutes in one day.
const MINUTES_PER_DAY: i64 = 24 * 60;

/// Largest UTC offset accepted, in minutes (UTC+14 is the real maximum).
const MAX_UTC_OFFSET_MINUTES: i32 = 14 * 60;

/// Local sunrise and sunset, minutes after local midnight (`0..1440`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SunTimes {
    pub sunrise_minutes: u32,
    pub sunset_minutes: u32,
}

/// Local sunrise and sunset for one day at `latitude_deg`/`longitude_deg` (east positive).
///
/// Returns `None` for invalid inputs and for polar day or polar night, where the sun does not
/// cross the horizon that day.
#[must_use]
pub fn sun_times(
    latitude_deg: f64,
    longitude_deg: f64,
    day_of_year: u32,
    utc_offset_minutes: i32,
) -> Option<SunTimes> {
    if !(-90.0..=90.0).contains(&latitude_deg)
        || !(-180.0..=180.0).contains(&longitude_deg)
        || !(1..=366).contains(&day_of_year)
        || utc_offset_minutes.abs() > MAX_UTC_OFFSET_MINUTES
    {
        return None;
    }
    let gamma = std::f64::consts::TAU / 365.0 * f64::from(day_of_year - 1);
    let (sin1, cos1) = gamma.sin_cos();
    let (sin2, cos2) = (2.0 * gamma).sin_cos();
    let (sin3, cos3) = (3.0 * gamma).sin_cos();
    let equation_of_time_min = 229.18
        * (0.000_075 + 0.001_868 * cos1 - 0.032_077 * sin1 - 0.014_615 * cos2 - 0.040_849 * sin2);
    let declination = 0.006_918 - 0.399_912 * cos1 + 0.070_257 * sin1 - 0.006_758 * cos2
        + 0.000_907 * sin2
        - 0.002_697 * cos3
        + 0.001_48 * sin3;

    let latitude = latitude_deg.to_radians();
    let zenith = SUNRISE_SUNSET_ZENITH_DEG.to_radians();
    let cos_hour_angle =
        zenith.cos() / (latitude.cos() * declination.cos()) - latitude.tan() * declination.tan();
    if !(-1.0..=1.0).contains(&cos_hour_angle) {
        return None;
    }
    let hour_angle_deg = cos_hour_angle.acos().to_degrees();
    let sunrise_utc = 720.0 - 4.0 * (longitude_deg + hour_angle_deg) - equation_of_time_min;
    let sunset_utc = 720.0 - 4.0 * (longitude_deg - hour_angle_deg) - equation_of_time_min;
    Some(SunTimes {
        sunrise_minutes: local_minutes(sunrise_utc, utc_offset_minutes),
        sunset_minutes: local_minutes(sunset_utc, utc_offset_minutes),
    })
}

/// The night-light window for one day: from sunset to sunrise. The start is greater than the end
/// when the window wraps over midnight, which `blue_light::schedule_active` already handles.
#[must_use]
pub fn night_window(
    latitude_deg: f64,
    longitude_deg: f64,
    day_of_year: u32,
    utc_offset_minutes: i32,
) -> Option<(u32, u32)> {
    sun_times(latitude_deg, longitude_deg, day_of_year, utc_offset_minutes)
        .map(|times| (times.sunset_minutes, times.sunrise_minutes))
}

fn local_minutes(utc_minutes: f64, utc_offset_minutes: i32) -> u32 {
    let total = utc_minutes.round() as i64 + i64::from(utc_offset_minutes);
    u32::try_from(total.rem_euclid(MINUTES_PER_DAY)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blue_light::schedule_active;

    /// Day 80 is the March equinox in a non-leap year.
    const EQUINOX: u32 = 80;

    fn near(actual: u32, expected: u32, tolerance: u32) -> bool {
        actual.abs_diff(expected) <= tolerance
    }

    #[test]
    fn equator_at_equinox_rises_and_sets_near_six_and_eighteen() {
        let times = sun_times(0.0, 0.0, EQUINOX, 0).expect("equator has a sunrise");
        assert!(near(times.sunrise_minutes, 6 * 60, 12), "{times:?}");
        assert!(near(times.sunset_minutes, 18 * 60, 12), "{times:?}");
    }

    #[test]
    fn longitude_and_utc_offset_shift_local_noon_to_the_same_place() {
        // 15 degrees east is one hour ahead of UTC; UTC+1 puts solar noon at local 12:00.
        let times = sun_times(0.0, 15.0, EQUINOX, 60).expect("valid");
        let midday = u32::midpoint(times.sunrise_minutes, times.sunset_minutes);
        assert!(near(midday, 12 * 60, 15), "midday {midday}");
    }

    #[test]
    fn northern_summer_days_are_longer_than_winter_days() {
        let summer = sun_times(52.0, 13.0, 172, 120).expect("summer");
        let winter = sun_times(52.0, 13.0, 355, 60).expect("winter");
        let summer_len = (summer.sunset_minutes + 1440 - summer.sunrise_minutes) % 1440;
        let winter_len = (winter.sunset_minutes + 1440 - winter.sunrise_minutes) % 1440;
        assert!(
            summer_len > winter_len + 4 * 60,
            "{summer_len} vs {winter_len}"
        );
    }

    #[test]
    fn polar_day_and_polar_night_have_no_sunrise_or_sunset() {
        assert_eq!(sun_times(70.0, 19.0, 172, 120), None, "midnight sun");
        assert_eq!(sun_times(70.0, 19.0, 355, 60), None, "polar night");
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        assert_eq!(sun_times(91.0, 0.0, EQUINOX, 0), None);
        assert_eq!(sun_times(0.0, 181.0, EQUINOX, 0), None);
        assert_eq!(sun_times(0.0, 0.0, 0, 0), None);
        assert_eq!(sun_times(0.0, 0.0, 367, 0), None);
        assert_eq!(sun_times(0.0, 0.0, EQUINOX, 841), None);
        assert_eq!(sun_times(f64::NAN, 0.0, EQUINOX, 0), None);
    }

    #[test]
    fn sunset_window_wraps_over_midnight_and_covers_the_night() {
        let (start, end) = night_window(0.0, 0.0, EQUINOX, 0).expect("window");
        assert!(start > end, "sunset {start} must come after sunrise {end}");
        assert!(schedule_active(23 * 60, start, end));
        assert!(schedule_active(2 * 60, start, end));
        assert!(!schedule_active(12 * 60, start, end));
    }

    #[test]
    fn results_are_always_minutes_of_one_day() {
        for day in [1, 80, 172, 265, 355, 366] {
            for lat in [-60.0, -20.0, 0.0, 35.0, 60.0] {
                if let Some(times) = sun_times(lat, -150.0, day, -600) {
                    assert!(times.sunrise_minutes < 1440 && times.sunset_minutes < 1440);
                }
            }
        }
    }
}
