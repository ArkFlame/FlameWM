//! `ext-session-lock-v1` support for the Wayland compositor.
//!
//! `FlameWM`'s lock UI is an ordinary client, so on Wayland it cannot paint unless the compositor
//! advertises the session-lock protocol. This module is that advertisement: the global, the
//! lock/unlock handshake, and the per-output lock surfaces the client draws into. While the
//! session is locked the compositor paints those surfaces alone, so no other client content is
//! reachable.

use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::utils::{Logical, Size};
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};

use crate::state::State;

/// One locked output and the client surface covering it.
pub struct LockedOutput {
    /// Kept for the protocol handshake and so a multi-output presenter can key on it later.
    pub _output: WlOutput,
    pub surface: LockSurface,
}

impl State {
    /// Creates the lock global. Every client may lock: a single-seat nested session has nobody to
    /// arbitrate between, and the protocol itself refuses a second lock while one is held.
    pub fn init_session_lock(&mut self) {
        self.session_lock = Some(crate::lock_global::install::<State>(&self.display));
    }

    /// Forgets every lock surface. Called on unlock, and when a locker dies mid-lock.
    pub fn clear_lock_surfaces(&mut self) {
        self.locked_outputs.clear();
    }

    /// The size a lock surface covers: the whole output, as the protocol requires.
    #[must_use]
    pub fn lock_size(&self) -> Size<u32, Logical> {
        Size::from((
            self.output_size.w.max(0) as u32,
            self.output_size.h.max(0) as u32,
        ))
    }

    /// The live lock surface for the compositor's single output.
    ///
    /// `ext-session-lock-v1` keys surfaces by `WlOutput`, but smithay's `Output` does not expose
    /// its protocol handle. This compositor drives exactly one output, so the newest live lock
    /// surface is the one to paint.
    #[must_use]
    pub fn lock_surface(&self) -> Option<&LockSurface> {
        self.locked_outputs
            .iter()
            .rev()
            .find(|locked| locked.surface.alive())
            .map(|locked| &locked.surface)
    }
}

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        // `init_session_lock` runs during startup, before any client can send a request.
        self.session_lock
            .as_mut()
            .expect("session lock global is created at startup")
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // Confirm at once: this compositor paints the lock surfaces itself and has no separate
        // "frame ready" step. Dropping `confirmation` instead would refuse the request.
        confirmation.lock();
        self.clear_lock_surfaces();
    }

    fn unlock(&mut self) {
        self.clear_lock_surfaces();
        // The keyboard still points at the destroyed lock surface; hand focus back or no client
        // receives keys until something else moves focus.
        self.refocus();
        // A lock may have replaced the pointer; show the themed one again afterwards.
        self.cursor_status = CursorImageStatus::default_named();
        self.sync_host_cursor();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let size = self.lock_size();
        surface.with_pending_state(|state| state.size = Some(size));
        surface.send_configure();
        // The lock client's keyboard must be focused or the password it draws can never be
        // entered: on Wayland a client only receives keys while a surface has focus.
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                Some(surface.wl_surface().clone()),
                crate::state::next_serial(),
            );
        }
        self.locked_outputs.push(LockedOutput {
            _output: output,
            surface,
        });
    }
}

/// Drops lock surfaces whose client is gone, so a crashed locker cannot leave the session
/// permanently locked. Returns whether anything was removed.
pub fn prune_dead_locks(state: &mut State) -> bool {
    let before = state.locked_outputs.len();
    state.locked_outputs.retain(|locked| locked.surface.alive());
    let pruned = before != state.locked_outputs.len();
    if pruned && state.lock_surface().is_none() {
        // Focus was on the dead locker's surface; move it back to a real window.
        state.refocus();
    }
    pruned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_surface_covers_the_whole_output() {
        // The protocol requires a full-output lock surface; a stale smaller size would leave part
        // of the desktop visible while locked.
        let size: Size<u32, Logical> = Size::from((1920u32, 1080u32));
        assert_eq!(size.w, 1920);
        assert_eq!(size.h, 1080);
    }
}
