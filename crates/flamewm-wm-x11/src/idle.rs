//! Idle-to-lock trigger. The WM reads MIT-SCREEN-SAVER idle time on a single
//! reactor one-shot timer that exists only while `lockTimeoutMinutes` is
//! non-zero. Each check arms the next one for the time remaining until the
//! threshold, bounded by a max re-check so suspend/resume and input resets
//! are picked up.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use calloop::RegistrationToken;
use flamewm_reactor::Reactor;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

/// Lower bound on the delay to the next idle check.
const MIN_IDLE_RECHECK: Duration = Duration::from_secs(1);
/// Upper bound on the delay to the next idle check; also used while a lock
/// request is latched, so input resets and resume are observed within this.
const MAX_IDLE_RECHECK: Duration = Duration::from_secs(30);

static LIVE_LOCK_TIMEOUT_MINUTES: AtomicU16 = AtomicU16::new(0);

/// Publishes the live `lockTimeoutMinutes` value (0 means manual lock only).
pub(crate) fn set_live_lock_timeout(minutes: u16) {
    LIVE_LOCK_TIMEOUT_MINUTES.store(minutes, Ordering::Relaxed);
}

fn live_lock_timeout() -> u16 {
    LIVE_LOCK_TIMEOUT_MINUTES.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleDecision {
    Nothing,
    RequestLock,
    Rearm,
}

/// Pure idle policy. `lock_requested` latches one lock per idle episode; it
/// is cleared (`Rearm`) only once input drops idle time below the threshold.
pub(crate) fn idle_decision(timeout_minutes: u16, idle_ms: u64, lock_requested: bool) -> IdleDecision {
    let threshold_ms = u64::from(timeout_minutes) * 60_000;
    if timeout_minutes == 0 || idle_ms < threshold_ms {
        if lock_requested {
            IdleDecision::Rearm
        } else {
            IdleDecision::Nothing
        }
    } else if lock_requested {
        IdleDecision::Nothing
    } else {
        IdleDecision::RequestLock
    }
}

/// Pure deadline policy: delay until the next idle check. Armed for the time
/// remaining until the threshold, clamped to `[MIN_IDLE_RECHECK, MAX_IDLE_RECHECK]`.
/// A latched request or a disabled timeout uses the max re-check.
pub(crate) fn next_check_delay(timeout_minutes: u16, idle_ms: u64, lock_requested: bool) -> Duration {
    if timeout_minutes == 0 || lock_requested {
        return MAX_IDLE_RECHECK;
    }
    let threshold_ms = u64::from(timeout_minutes) * 60_000;
    Duration::from_millis(threshold_ms.saturating_sub(idle_ms)).clamp(MIN_IDLE_RECHECK, MAX_IDLE_RECHECK)
}

fn query_idle_ms(conn: &RustConnection, root: Window) -> Result<u64, Box<dyn std::error::Error>> {
    let reply = conn.screensaver_query_info(root)?.reply()?;
    Ok(u64::from(reply.ms_since_user_input))
}

pub(crate) struct IdleLock {
    timer: Option<RegistrationToken>,
    due: Rc<Cell<bool>>,
    lock_requested: bool,
    disabled: bool,
}

impl IdleLock {
    pub(crate) fn new() -> Self {
        Self {
            timer: None,
            due: Rc::new(Cell::new(false)),
            lock_requested: false,
            disabled: false,
        }
    }

    /// Called once per reactor turn. Keeps a one-shot timer armed only while the
    /// timeout is set. When it fires, runs an idle check and arms the next one
    /// at the computed deadline.
    pub(crate) fn turn(&mut self, reactor: &Rc<RefCell<Reactor>>, conn: &RustConnection, root: Window) {
        let timeout = live_lock_timeout();
        if timeout == 0 {
            self.stop_timer(reactor);
            self.due.set(false);
            self.lock_requested = false;
            return;
        }
        if self.disabled {
            self.stop_timer(reactor);
            return;
        }
        if self.timer.is_none() && !self.due.get() {
            self.start_timer(reactor, MIN_IDLE_RECHECK);
            return;
        }
        if !self.due.replace(false) {
            return;
        }
        // The one-shot has fired and its source is gone; forget the token.
        self.timer = None;
        let idle_ms = match query_idle_ms(conn, root) {
            Ok(idle_ms) => idle_ms,
            Err(error) => {
                eprintln!("flamewm: idle lock disabled, MIT-SCREEN-SAVER query failed: {error}");
                self.disabled = true;
                self.stop_timer(reactor);
                return;
            }
        };
        match idle_decision(timeout, idle_ms, self.lock_requested) {
            IdleDecision::Nothing => {}
            IdleDecision::Rearm => self.lock_requested = false,
            IdleDecision::RequestLock => {
                self.lock_requested = true;
                if let Err(error) = flamewm_x11_desktop::start_lock_screen() {
                    eprintln!("flamewm: idle lock could not start the lock screen: {error}");
                }
            }
        }
        let delay = next_check_delay(timeout, idle_ms, self.lock_requested);
        self.start_timer(reactor, delay);
    }

    fn start_timer(&mut self, reactor: &Rc<RefCell<Reactor>>, delay: Duration) {
        let due = Rc::clone(&self.due);
        match reactor
            .borrow_mut()
            .register_timer(delay, false, move || due.set(true))
        {
            Ok(token) => self.timer = Some(token),
            Err(error) => {
                eprintln!("flamewm: idle lock timer not registered: {error}");
                self.disabled = true;
            }
        }
    }

    fn stop_timer(&mut self, reactor: &Rc<RefCell<Reactor>>) {
        if let Some(token) = self.timer.take() {
            let _ = reactor.borrow_mut().remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE_MS: u64 = 60_000;

    #[test]
    fn zero_timeout_never_requests_lock() {
        assert_eq!(idle_decision(0, 0, false), IdleDecision::Nothing);
        assert_eq!(idle_decision(0, u64::MAX / 2, false), IdleDecision::Nothing);
    }

    #[test]
    fn zero_timeout_rearms_a_pending_request() {
        assert_eq!(idle_decision(0, 10 * MINUTE_MS, true), IdleDecision::Rearm);
    }

    #[test]
    fn threshold_boundary_is_inclusive() {
        assert_eq!(idle_decision(1, MINUTE_MS - 1, false), IdleDecision::Nothing);
        assert_eq!(idle_decision(1, MINUTE_MS, false), IdleDecision::RequestLock);
    }

    #[test]
    fn request_is_latched_until_activity() {
        assert_eq!(idle_decision(1, 2 * MINUTE_MS, true), IdleDecision::Nothing);
        assert_eq!(idle_decision(1, 0, true), IdleDecision::Rearm);
        assert_eq!(idle_decision(1, 0, false), IdleDecision::Nothing);
    }

    #[test]
    fn rearm_then_idle_again_requests_once_more() {
        assert_eq!(idle_decision(1, 0, true), IdleDecision::Rearm);
        assert_eq!(idle_decision(1, 0, false), IdleDecision::Nothing);
        assert_eq!(idle_decision(1, MINUTE_MS, false), IdleDecision::RequestLock);
    }

    #[test]
    fn deadline_is_remaining_time_within_bounds() {
        // 1 min threshold, 40 s idle -> 20 s remaining, inside the bounds.
        assert_eq!(next_check_delay(1, 40_000, false), Duration::from_secs(20));
        // Nearly idle -> clamped up to the minimum re-check.
        assert_eq!(next_check_delay(1, 59_500, false), MIN_IDLE_RECHECK);
        // Fresh input on a long timeout -> clamped down to the max re-check.
        assert_eq!(next_check_delay(10, 0, false), MAX_IDLE_RECHECK);
    }

    #[test]
    fn deadline_at_or_past_threshold_uses_minimum() {
        assert_eq!(next_check_delay(1, MINUTE_MS, false), MIN_IDLE_RECHECK);
        assert_eq!(next_check_delay(1, 5 * MINUTE_MS, false), MIN_IDLE_RECHECK);
    }

    #[test]
    fn deadline_latched_or_disabled_uses_max_recheck() {
        assert_eq!(next_check_delay(1, 5 * MINUTE_MS, true), MAX_IDLE_RECHECK);
        assert_eq!(next_check_delay(0, 5 * MINUTE_MS, false), MAX_IDLE_RECHECK);
    }

    #[test]
    fn deadline_never_exceeds_max_recheck_for_large_timeouts() {
        assert_eq!(next_check_delay(u16::MAX, 0, false), MAX_IDLE_RECHECK);
        assert_eq!(next_check_delay(u16::MAX, u64::MAX, false), MIN_IDLE_RECHECK);
    }

    #[test]
    fn u16_max_timeout_does_not_overflow() {
        let threshold = u64::from(u16::MAX) * MINUTE_MS;
        assert_eq!(idle_decision(u16::MAX, threshold - 1, false), IdleDecision::Nothing);
        assert_eq!(idle_decision(u16::MAX, threshold, false), IdleDecision::RequestLock);
        assert_eq!(idle_decision(u16::MAX, u64::from(u32::MAX), true), IdleDecision::Nothing);
        assert_eq!(idle_decision(u16::MAX, 0, true), IdleDecision::Rearm);
    }
}
