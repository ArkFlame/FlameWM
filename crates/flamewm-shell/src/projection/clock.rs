//! Taskbar clock projection: display settings, time text and date helpers.

use chrono::{DateTime, Datelike, Local};
use flamewm_platform::settings::ClockFormat;
use flamewm_ui_x11::UiDocumentAccess;

/// Taskbar clock presentation from `taskbarClockFormat`, `taskbarShowSeconds`
/// and `taskbarShowDate`. Defaults match `ProductSettings::default()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDisplay {
    pub format: ClockFormat,
    pub show_seconds: bool,
    pub show_date: bool,
}

impl Default for ClockDisplay {
    fn default() -> Self {
        Self {
            format: ClockFormat::TwentyFour,
            show_seconds: false,
            show_date: true,
        }
    }
}

/// Time text for the taskbar clock: `HH:MM` or `HH:MM:SS` in 24h, `H:MM AM/PM`
/// or `H:MM:SS AM/PM` in 12h.
#[must_use]
pub fn clock_time_text(display: ClockDisplay, now: DateTime<Local>) -> String {
    let pattern = match (display.format, display.show_seconds) {
        (ClockFormat::TwentyFour, false) => "%H:%M",
        (ClockFormat::TwentyFour, true) => "%H:%M:%S",
        (ClockFormat::Twelve, false) => "%-I:%M %p",
        (ClockFormat::Twelve, true) => "%-I:%M:%S %p",
    };
    now.format(pattern).to_string()
}

/// Panel rebuild paths carry no settings; the settings projection re-applies the
/// configured clock on the next settings pass and on every clock tick.
pub fn project_clock(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    project_clock_at(document, Local::now(), ClockDisplay::default())
}

pub fn project_clock_at(
    document: &mut impl UiDocumentAccess,
    now: DateTime<Local>,
    display: ClockDisplay,
) -> Result<(), String> {
    document.text("clock-time", clock_time_text(display, now))?;
    document.text("clock-date", now.format("%-d/%-m/%y").to_string())?;
    document.visible("clock-date", display.show_date)?;
    Ok(())
}

#[must_use]
pub fn local_date() -> (i32, u8, u8) {
    let now = Local::now();
    (now.year(), now.month() as u8, now.day() as u8)
}

pub(super) fn month_name(month: u8) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("Unknown")
}

#[cfg(test)]
mod clock_display_tests {
    use super::{clock_time_text, ClockDisplay, ClockFormat};
    use chrono::{Local, TimeZone};

    fn at(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 12, hour, minute, second)
            .single()
            .expect("unambiguous local time")
    }

    fn display(format: ClockFormat, show_seconds: bool) -> ClockDisplay {
        ClockDisplay {
            format,
            show_seconds,
            show_date: true,
        }
    }

    #[test]
    fn twenty_four_hour_clock_omits_seconds_by_default() {
        let text = clock_time_text(display(ClockFormat::TwentyFour, false), at(14, 5, 9));
        assert_eq!(text, "14:05");
    }

    #[test]
    fn twenty_four_hour_clock_shows_seconds_when_enabled() {
        let text = clock_time_text(display(ClockFormat::TwentyFour, true), at(14, 5, 9));
        assert_eq!(text, "14:05:09");
    }

    #[test]
    fn twelve_hour_clock_uses_unpadded_hour_and_meridiem() {
        let text = clock_time_text(display(ClockFormat::Twelve, false), at(14, 5, 9));
        assert_eq!(text, "2:05 PM");
    }

    #[test]
    fn twelve_hour_midnight_and_noon_map_to_twelve() {
        assert_eq!(
            clock_time_text(display(ClockFormat::Twelve, false), at(0, 30, 0)),
            "12:30 AM"
        );
        assert_eq!(
            clock_time_text(display(ClockFormat::Twelve, false), at(12, 0, 0)),
            "12:00 PM"
        );
    }

    #[test]
    fn twelve_hour_clock_shows_seconds_when_enabled() {
        let text = clock_time_text(display(ClockFormat::Twelve, true), at(9, 7, 3));
        assert_eq!(text, "9:07:03 AM");
    }

    #[test]
    fn default_display_matches_product_defaults() {
        let defaults = ClockDisplay::default();
        assert_eq!(defaults.format, ClockFormat::TwentyFour);
        assert!(!defaults.show_seconds);
        assert!(defaults.show_date);
    }
}
