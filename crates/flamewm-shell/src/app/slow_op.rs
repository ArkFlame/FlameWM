//! Slow-operation diagnostics and popup refusal classification for the shell loop.

use super::*;

/// C07: one shell operation owns exactly one total span; nested totals must
/// not double-own. `operation_total_active` on the `ShellLoop` event turn
/// marks the outer owner; inner paths check it and skip their own total.
pub(crate) const SHELL_SLOW_TOTAL_NS: u128 = 16_670_000;
pub(crate) const SHELL_SLOW_COOLDOWN_MS: u64 = 500;

/// C07 slow-operation note: when `started`->now exceeds one 60Hz frame,
/// emit `shell.operation.slow` debug with a 500ms per-op cooldown.
pub(crate) fn shell_slow_note(op: &'static str, started: Instant) {
    fn last_for(op: &str) -> Option<&'static std::sync::atomic::AtomicU64> {
        use std::sync::atomic::AtomicU64;
        static START: AtomicU64 = AtomicU64::new(0);
        static CONTEXT: AtomicU64 = AtomicU64::new(0);
        static WORKSPACE: AtomicU64 = AtomicU64::new(0);
        match op {
            "start" => Some(&START),
            "context" => Some(&CONTEXT),
            "workspace" => Some(&WORKSPACE),
            _ => None,
        }
    }
    if started.elapsed().as_nanos() <= SHELL_SLOW_TOTAL_NS {
        return;
    }
    let wall_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if let Some(last) = last_for(op) {
        let prev = last.load(std::sync::atomic::Ordering::Relaxed);
        if wall_ms.saturating_sub(prev) < SHELL_SLOW_COOLDOWN_MS {
            return;
        }
        last.store(wall_ms, std::sync::atomic::Ordering::Relaxed);
    }
    flamewm_debug::emit(
        flamewm_debug::DebugEventId("shell.operation.slow"),
        Duration::from_millis(SHELL_SLOW_COOLDOWN_MS),
        || format!("op={op} slow"),
    );
}

/// Popup measurement refusal: layout pending, anchor pending, or transient
/// pointer-grab contention. Callers swallow these (state untouched).
pub(crate) fn is_popup_refusal(error: &str) -> bool {
    error == PopupRefusal::PendingLayout.to_string()
        || error == PopupRefusal::PendingAnchor.to_string()
        || error == PopupRefusal::PointerGrabRefused.to_string()
}
