//! Advertising `ext-session-lock-v1`.
//!
//! The `FlameWM` lock UI is an ordinary Wayland client, so the only way it can paint over a locked
//! session is for the compositor to offer `ext_session_lock_manager_v1`. This module owns that
//! one call so the compositor and its test install the global the same way.

use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::session_lock::SessionLockManagerState;

/// The interface name a lock client looks for. Used by the protocol test; the compositor only
/// needs the global itself.
#[allow(dead_code)]
pub const SESSION_LOCK_INTERFACE: &str = "ext_session_lock_manager_v1";

/// Creates the session-lock global on `display`, admitting every client.
///
/// A single-seat session has nobody to arbitrate between, and the protocol itself refuses a
/// second lock while one is held, so the filter admits all clients.
#[must_use]
pub fn install<D>(display: &DisplayHandle) -> SessionLockManagerState
where
    D: smithay::wayland::session_lock::SessionLockHandler
        + smithay::reexports::wayland_server::Dispatch<
            smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            (),
        >
        + smithay::reexports::wayland_server::GlobalDispatch<
            smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            smithay::wayland::session_lock::SessionLockManagerGlobalData,
        >
        + smithay::reexports::wayland_server::Dispatch<
            smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
            smithay::wayland::session_lock::SessionLockState,
        >
        + 'static,
{
    SessionLockManagerState::new::<D, _>(display, |_| true)
}
