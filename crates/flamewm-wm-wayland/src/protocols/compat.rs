//! Client-compatibility protocols: primary selection, cursor shape, xdg-activation and idle inhibit.
//!
//! Each protocol's handler lives here so the shared `state.rs`/`handlers.rs` files only gain
//! registration lines. Globals that need no per-object state are created in `State::new`.

use std::time::Duration;

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState,
};
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use smithay::{
    delegate_cursor_shape, delegate_idle_inhibit, delegate_primary_selection,
    delegate_xdg_activation,
};

use crate::state::State;

/// A token is honored only this long after the client was given it.
pub const ACTIVATION_TOKEN_MAX_AGE: Duration = Duration::from_secs(10);

/// Focus-steal policy for `xdg-activation-v1`: focus moves only for a token that was created from
/// a user input event (`has_input_serial`) and is still fresh. Anything else is ignored.
pub fn activation_allowed(has_input_serial: bool, age: Duration) -> bool {
    has_input_serial && age <= ACTIVATION_TOKEN_MAX_AGE
}

/// Set of keys (inhibiting surfaces) that each hold one idle inhibition. Idle is inhibited while
/// any key is present, so repeated `inhibit` calls from one surface count once.
pub struct InhibitSet<K> {
    keys: Vec<K>,
}

impl<K> Default for InhibitSet<K> {
    fn default() -> Self {
        Self { keys: Vec::new() }
    }
}

impl<K: PartialEq> InhibitSet<K> {
    pub fn inhibit(&mut self, key: K) {
        if !self.keys.contains(&key) {
            self.keys.push(key);
        }
    }

    pub fn uninhibit(&mut self, key: &K) {
        self.keys.retain(|held| held != key);
    }

    // shortcut: no idle timer exists in the compositor yet, so only tests read this;
    // revisit when the idle/lock timer lands and consumes it.
    #[cfg_attr(not(test), expect(dead_code, reason = "no idle timer consumer yet"))]
    pub fn is_inhibited(&self) -> bool {
        !self.keys.is_empty()
    }
}

/// Protocol state that must outlive `State::new`: the primary-selection and activation globals
/// keep per-object data, the idle inhibitors are read by the idle timer.
pub struct CompatState {
    pub primary_selection: PrimarySelectionState,
    pub xdg_activation: XdgActivationState,
    pub idle_inhibitors: InhibitSet<WlSurface>,
}

impl CompatState {
    /// Creates the globals. Cursor-shape and idle-inhibit managers keep no state here, so their
    /// return values are dropped (the global lives in the display).
    pub fn new(display: &smithay::reexports::wayland_server::DisplayHandle) -> Self {
        let _cursor_shape = CursorShapeManagerState::new::<State>(display);
        let _idle_inhibit = IdleInhibitManagerState::new::<State>(display);
        Self {
            primary_selection: PrimarySelectionState::new::<State>(display),
            xdg_activation: XdgActivationState::new::<State>(display),
            idle_inhibitors: InhibitSet::default(),
        }
    }
}

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.compat.primary_selection
    }
}

delegate_primary_selection!(State);

// cursor-shape requires the tablet seat hook; the compositor has no tablet tool cursors yet, so the
// trait's default (ignore tool images) is the intended behavior.
impl TabletSeatHandler for State {}
delegate_cursor_shape!(State);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.compat.xdg_activation
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        let age = token_data.timestamp.elapsed();
        if !activation_allowed(token_data.serial.is_some(), age) {
            return;
        }
        // Single use: a token that focused a window cannot be replayed.
        self.compat.xdg_activation.remove_token(&token);
        if let Some(id) = self.window_for_surface(&surface) {
            self.set_focus(Some(id));
        }
    }
}

delegate_xdg_activation!(State);

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.compat.idle_inhibitors.inhibit(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.compat.idle_inhibitors.uninhibit(&surface);
    }
}

delegate_idle_inhibit!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_needs_input_serial_and_freshness() {
        assert!(activation_allowed(true, Duration::ZERO));
        assert!(activation_allowed(true, ACTIVATION_TOKEN_MAX_AGE));
        assert!(!activation_allowed(false, Duration::ZERO));
        assert!(!activation_allowed(
            true,
            ACTIVATION_TOKEN_MAX_AGE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn idle_inhibit_counts_surfaces_not_calls() {
        let mut set = InhibitSet::<u32>::default();
        assert!(!set.is_inhibited());
        set.inhibit(1);
        set.inhibit(1);
        set.inhibit(2);
        set.uninhibit(&1);
        assert!(set.is_inhibited(), "surface 2 still inhibits");
        set.uninhibit(&2);
        assert!(!set.is_inhibited());
        set.uninhibit(&9);
        assert!(!set.is_inhibited(), "unknown key is a no-op");
    }
}
