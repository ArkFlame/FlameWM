use std::time::Duration;

/// Failed attempts allowed before any delay applies.
pub const FREE_ATTEMPTS: u32 = 3;
/// Upper bound for the retry delay.
pub const MAX_DELAY: Duration = Duration::from_secs(30);

/// Delay before the next attempt is accepted after `failures` consecutive
/// failed authentications: 0s for the first three, then 1s, 2s, 4s, ... capped
/// at [`MAX_DELAY`].
pub fn delay_after_failures(failures: u32) -> Duration {
    if failures < FREE_ATTEMPTS {
        return Duration::ZERO;
    }
    let shift = failures - FREE_ATTEMPTS;
    if shift >= 5 {
        return MAX_DELAY;
    }
    Duration::from_secs(1 << shift).min(MAX_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_delay_within_free_attempts() {
        assert_eq!(delay_after_failures(0), Duration::ZERO);
        assert_eq!(delay_after_failures(2), Duration::ZERO);
    }

    #[test]
    fn doubles_then_caps() {
        assert_eq!(delay_after_failures(3), Duration::from_secs(1));
        assert_eq!(delay_after_failures(4), Duration::from_secs(2));
        assert_eq!(delay_after_failures(5), Duration::from_secs(4));
        assert_eq!(delay_after_failures(7), Duration::from_secs(16));
        assert_eq!(delay_after_failures(8), MAX_DELAY);
        assert_eq!(delay_after_failures(u32::MAX), MAX_DELAY);
    }
}
