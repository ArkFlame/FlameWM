//! Typed weather feed facade shared by the shell (publisher), the control host (cache and
//! fan-out) and the desktop widget (subscriber).
//!
//! The shell owns the fetch, the refresh schedule and the settings. The control host stores
//! the last published payload and assigns its revision; it never interprets the payload.
//! `payload` is the encoding owned by `flamewm_shell_core::weather::WeatherFeed`; only that
//! crate encodes or decodes it.

/// Upper bound for one published payload (seven forecast days fit well below this).
pub const MAX_WEATHER_FEED_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WeatherFeedSnapshot {
    /// Advances by one each time the payload changes; `0` means nothing was ever published.
    pub revision: u64,
    pub payload: String,
}
