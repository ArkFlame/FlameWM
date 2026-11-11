//! Failure policy for lock-screen steps: keep the lock engaged and retry deferred steps next turn.

/// Runs one lock-screen step that must not end the loop. A failure is logged and reported
/// as `false`; the caller keeps its state and the grabs stay held.
pub(super) fn keep_lock_step(what: &str, step: impl FnOnce() -> Result<(), String>) -> bool {
    match step() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("flamewm-lock: {what}, keeping the lock engaged: {error}");
            false
        }
    }
}

/// Applies a deferred step. Success clears `pending`; a failure is logged and leaves it set
/// so a later turn retries. It never ends the loop or touches the grabs.
pub(super) fn settle(pending: &mut bool, what: &str, apply: impl FnOnce() -> Result<(), String>) {
    match apply() {
        Ok(()) => *pending = false,
        Err(error) => {
            *pending = true;
            eprintln!("flamewm-lock: {what}, retrying next turn: {error}");
        }
    }
}
