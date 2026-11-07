//! `_NET_WM_PING` liveness for a window the user asked to close.
//!
//! Armed only by a close request: the WM sends `WM_DELETE_WINDOW` and, when the client lists
//! `_NET_WM_PING` in `WM_PROTOCOLS`, a ping. One reactor deadline covers the wait; there is no
//! periodic timer. A window that does not pong within [`PING_TIMEOUT`] is marked unresponsive, and
//! its next close request kills it (`XKillClient`).

use super::*;
use std::time::{Duration, Instant};

/// How long a closing client has to answer its `_NET_WM_PING`.
pub(super) const PING_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct PingState {
    /// Clients awaiting a pong, with the instant the ping deadline expires.
    pending: HashMap<Window, Instant>,
    /// Clients that missed their pong; the next close request force-kills them.
    unresponsive: HashSet<Window>,
}

impl PingState {
    pub(super) fn is_unresponsive(&self, client: Window) -> bool {
        self.unresponsive.contains(&client)
    }

    /// Records a pong: the client is alive, so it is neither pending nor unresponsive.
    pub(super) fn pong(&mut self, client: Window) {
        self.pending.remove(&client);
        self.unresponsive.remove(&client);
    }

    pub(super) fn forget(&mut self, client: Window) {
        self.pending.remove(&client);
        self.unresponsive.remove(&client);
    }

    /// Arms the deadline for `client`. A ping already pending keeps its earlier deadline.
    pub(super) fn arm(&mut self, client: Window, now: Instant) {
        self.pending.entry(client).or_insert(now + PING_TIMEOUT);
    }

    /// Moves every expired pending client to unresponsive and returns the time to the next
    /// deadline, or `None` when nothing is pending.
    pub(super) fn expire(&mut self, now: Instant) -> Option<Duration> {
        let expired: Vec<Window> = self
            .pending
            .iter()
            .filter(|(_, deadline)| now >= **deadline)
            .map(|(client, _)| *client)
            .collect();
        for client in expired {
            self.pending.remove(&client);
            self.unresponsive.insert(client);
        }
        self.pending
            .values()
            .map(|deadline| deadline.saturating_duration_since(now))
            .min()
    }
}

impl<C: Connection> Wm<'_, C> {
    /// Sends `_NET_WM_PING` to `client` and arms its deadline. Caller checks the protocol.
    pub(super) fn send_ping(&mut self, client: Window) -> Result<(), ReplyError> {
        let event = ClientMessageEvent::new(
            32,
            client,
            self.atoms.wm_protocols,
            [self.atoms.net_wm_ping, CURRENT_TIME, client, 0, 0],
        );
        self.conn
            .send_event(false, client, EventMask::NO_EVENT, event)?;
        self.pings.arm(client, Instant::now());
        Ok(())
    }

    /// Hook for the reactor: expires overdue pings and returns the wait until the next one.
    pub(crate) fn ping_wait_timeout(&mut self, now: Instant) -> Option<Duration> {
        self.pings.expire(now)
    }
}

#[cfg(test)]
mod ping_tests {
    use super::*;

    #[test]
    fn pong_before_deadline_clears_pending_and_unresponsive() {
        let mut state = PingState::default();
        let t0 = Instant::now();
        state.arm(7, t0);
        state.pong(7);
        assert_eq!(state.expire(t0 + PING_TIMEOUT * 2), None);
        assert!(!state.is_unresponsive(7));
    }

    #[test]
    fn missed_pong_marks_unresponsive_only_after_timeout() {
        let mut state = PingState::default();
        let t0 = Instant::now();
        state.arm(7, t0);
        assert_eq!(state.expire(t0), Some(PING_TIMEOUT));
        assert!(!state.is_unresponsive(7));
        assert_eq!(state.expire(t0 + PING_TIMEOUT), None);
        assert!(state.is_unresponsive(7));
    }

    #[test]
    fn rearming_keeps_the_original_deadline() {
        let mut state = PingState::default();
        let t0 = Instant::now();
        state.arm(7, t0);
        state.arm(7, t0 + Duration::from_secs(3));
        assert_eq!(state.expire(t0 + PING_TIMEOUT), None);
        assert!(state.is_unresponsive(7));
    }

    #[test]
    fn forget_drops_all_state_for_destroyed_window() {
        let mut state = PingState::default();
        let t0 = Instant::now();
        state.arm(7, t0);
        state.expire(t0 + PING_TIMEOUT);
        state.forget(7);
        assert!(!state.is_unresponsive(7));
        assert_eq!(state.expire(t0 + PING_TIMEOUT * 3), None);
    }
}
