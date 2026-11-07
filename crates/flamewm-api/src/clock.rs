//! Wall-clock timestamps since the Unix epoch.

use std::time::{SystemTime, UNIX_EPOCH};

/// Whole seconds since the Unix epoch, or 0 when the clock is before it.
#[must_use]
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Nanoseconds since the Unix epoch, or 0 when the clock is before it.
#[must_use]
pub fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}
