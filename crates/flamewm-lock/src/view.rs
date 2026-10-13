//! Pure projection from lock state and settings to the text shown on screen.
use crate::clock::{format_clock, format_date};
use crate::native_time::LocalTime;
use crate::secret::PasswordBuffer;
use crate::settings::LockSettings;
use crate::state::Status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockView {
    pub clock: String,
    pub date: String,
    pub show_date: bool,
    pub username: String,
    pub show_username: bool,
    pub initial: String,
    pub mask: String,
    pub status: &'static str,
    /// The last attempt failed: the password field takes the red-tinted glass.
    pub failed: bool,
}

impl LockView {
    pub fn project(
        settings: &LockSettings,
        now: Option<LocalTime>,
        username: &str,
        password: &PasswordBuffer,
        status: Status,
    ) -> Self {
        let (clock, date) = match now {
            Some(time) => (
                format_clock(time.hour, time.minute, settings.clock_format),
                format_date(time.year, time.month, time.day).unwrap_or_default(),
            ),
            None => ("--:--".to_string(), String::new()),
        };
        Self {
            clock,
            date,
            show_date: settings.show_date,
            username: username.to_string(),
            show_username: settings.show_username,
            initial: initial_of(username),
            mask: password.masked(),
            status: status.text(),
            failed: status == Status::Failed,
        }
    }
}

/// Avatar letter: first character of the user name, upper-cased.
pub fn initial_of(username: &str) -> String {
    username
        .chars()
        .next()
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ClockFormat;

    fn sample_time() -> LocalTime {
        LocalTime {
            year: 2026,
            month: 10,
            day: 12,
            hour: 13,
            minute: 7,
        }
    }

    #[test]
    fn projects_clock_date_and_mask() {
        let settings = LockSettings::default();
        let mut password = PasswordBuffer::default();
        password.push('s');
        password.push('3');
        let view = LockView::project(
            &settings,
            Some(sample_time()),
            "alice",
            &password,
            Status::Failed,
        );
        assert_eq!(view.clock, "13:07");
        assert_eq!(view.date, "Monday, 12 October 2026");
        assert_eq!(view.mask, "\u{2022}\u{2022}");
        assert!(!view.mask.contains('s'));
        assert_eq!(view.initial, "A");
        assert_eq!(view.status, "Authentication failed");
    }

    #[test]
    fn honours_12_hour_clock_and_visibility_flags() {
        let settings = LockSettings {
            clock_format: ClockFormat::H12,
            show_date: false,
            show_username: false,
            ..LockSettings::default()
        };
        let view = LockView::project(
            &settings,
            Some(sample_time()),
            "bob",
            &PasswordBuffer::default(),
            Status::Idle,
        );
        assert_eq!(view.clock, "1:07 PM");
        assert!(!view.show_date && !view.show_username);
        assert_eq!(view.mask, "");
        assert_eq!(view.status, "");
    }

    #[test]
    fn missing_clock_shows_placeholder_without_date() {
        let view = LockView::project(
            &LockSettings::default(),
            None,
            "carol",
            &PasswordBuffer::default(),
            Status::Authenticating,
        );
        assert_eq!(view.clock, "--:--");
        assert_eq!(view.date, "");
        assert_eq!(view.status, "Authenticating\u{2026}");
    }

    #[test]
    fn initial_is_empty_for_empty_user() {
        assert_eq!(initial_of(""), "");
        assert_eq!(initial_of("zed"), "Z");
    }
}
