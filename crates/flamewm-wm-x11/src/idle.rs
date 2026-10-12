//! Idle-to-lock trigger. The WM reads MIT-SCREEN-SAVER idle time on a single
//! reactor timer that exists only while `lockTimeoutMinutes` is non-zero.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use calloop::RegistrationToken;
use flamewm_reactor::Reactor;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::xproto::Window;
use x11rb::rust_connection::RustConnection;

/// Idle check period while the lock timeout is armed.
pub(crate) const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

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

    /// Called once per reactor turn. Keeps the timer registered only while the
    /// timeout is armed, and runs an idle check when the timer has fired.
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
        if self.timer.is_none() {
            self.start_timer(reactor);
            return;
        }
        if !self.due.replace(false) {
            return;
        }
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
    }

    fn start_timer(&mut self, reactor: &Rc<RefCell<Reactor>>) {
        let due = Rc::clone(&self.due);
        match reactor
            .borrow_mut()
            .register_timer(IDLE_CHECK_INTERVAL, true, move || due.set(true))
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
    fn u16_max_timeout_does_not_overflow() {
        let threshold = u64::from(u16::MAX) * MINUTE_MS;
        assert_eq!(idle_decision(u16::MAX, threshold - 1, false), IdleDecision::Nothing);
        assert_eq!(idle_decision(u16::MAX, threshold, false), IdleDecision::RequestLock);
        assert_eq!(idle_decision(u16::MAX, u64::from(u32::MAX), true), IdleDecision::Nothing);
        assert_eq!(idle_decision(u16::MAX, 0, true), IdleDecision::Rearm);
    }
}
