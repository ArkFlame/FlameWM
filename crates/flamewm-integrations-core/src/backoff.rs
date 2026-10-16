/// Shared exponential reconnect schedule used by the network and pulse controllers.
///
/// Advances `attempts` and clears `pending` once `maximum_attempts` is reached or the
/// reconnect is no longer pending. Returns the delay for this attempt, or `None` when
/// no further attempt is allowed.
pub(crate) fn next_reconnect_delay_ms(
    initial_ms: u64,
    maximum_ms: u64,
    maximum_attempts: u8,
    attempts: &mut u8,
    pending: &mut bool,
) -> Option<u64> {
    if !*pending || *attempts >= maximum_attempts {
        *pending = false;
        return None;
    }
    let shift = u32::from((*attempts).min(7));
    let delay = initial_ms.saturating_mul(1_u64 << shift).min(maximum_ms);
    *attempts = attempts.saturating_add(1);
    if *attempts >= maximum_attempts {
        *pending = false;
    }
    Some(delay)
}
