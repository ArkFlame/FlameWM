/// Clock display format selected by the `lockClockFormat` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockFormat {
    H24,
    H12,
}

impl ClockFormat {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "24h" => Some(Self::H24),
            "12h" => Some(Self::H12),
            _ => None,
        }
    }
}

/// Formats `hour` (0-23) and `minute` (0-59). Out-of-range inputs are clamped
/// so a bad clock value can never panic the lock screen.
pub fn format_clock(hour: u8, minute: u8, format: ClockFormat) -> String {
    let hour = hour.min(23);
    let minute = minute.min(59);
    match format {
        ClockFormat::H24 => format!("{hour:02}:{minute:02}"),
        ClockFormat::H12 => {
            let display = match hour % 12 {
                0 => 12,
                value => value,
            };
            let suffix = if hour < 12 { "AM" } else { "PM" };
            format!("{display}:{minute:02} {suffix}")
        }
    }
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
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
];

/// Formats a Gregorian date as e.g. "Monday, 12 October 2026".
/// Returns `None` for an invalid month or day.
pub fn format_date(year: i32, month: u8, day: u8) -> Option<String> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let weekday = day_of_week(year, month, day);
    Some(format!(
        "{}, {} {} {}",
        WEEKDAYS[weekday],
        day,
        MONTHS[usize::from(month - 1)],
        year
    ))
}

/// Sakamoto's algorithm; returns 0 for Sunday through 6 for Saturday.
fn day_of_week(year: i32, month: u8, day: u8) -> usize {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let m = i32::from(month);
    let d = i32::from(day);
    let value = (y + y / 4 - y / 100 + y / 400 + OFFSETS[(m - 1) as usize] + d) % 7;
    value.rem_euclid(7) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_24_hour_clock_with_padding() {
        assert_eq!(format_clock(0, 5, ClockFormat::H24), "00:05");
        assert_eq!(format_clock(23, 59, ClockFormat::H24), "23:59");
    }

    #[test]
    fn formats_12_hour_clock_with_meridiem() {
        assert_eq!(format_clock(0, 0, ClockFormat::H12), "12:00 AM");
        assert_eq!(format_clock(12, 30, ClockFormat::H12), "12:30 PM");
        assert_eq!(format_clock(13, 7, ClockFormat::H12), "1:07 PM");
        assert_eq!(format_clock(11, 59, ClockFormat::H12), "11:59 AM");
    }

    #[test]
    fn clamps_out_of_range_clock_values() {
        assert_eq!(format_clock(99, 99, ClockFormat::H24), "23:59");
    }

    #[test]
    fn parses_only_exact_format_names() {
        assert_eq!(ClockFormat::parse("24h"), Some(ClockFormat::H24));
        assert_eq!(ClockFormat::parse("12h"), Some(ClockFormat::H12));
        assert_eq!(ClockFormat::parse("12H"), None);
        assert_eq!(ClockFormat::parse(""), None);
    }

    #[test]
    fn formats_known_dates_with_weekday() {
        // 2026-10-12 is a Monday.
        assert_eq!(
            format_date(2026, 10, 12).as_deref(),
            Some("Monday, 12 October 2026")
        );
        // 2000-01-01 was a Saturday.
        assert_eq!(
            format_date(2000, 1, 1).as_deref(),
            Some("Saturday, 1 January 2000")
        );
    }

    #[test]
    fn rejects_invalid_dates() {
        assert_eq!(format_date(2026, 0, 1), None);
        assert_eq!(format_date(2026, 13, 1), None);
        assert_eq!(format_date(2026, 1, 0), None);
    }
}
