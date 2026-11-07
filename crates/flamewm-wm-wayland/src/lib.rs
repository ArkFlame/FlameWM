//! The compositor's Wayland modules, exposed as a library so tests can drive them.
//!
//! The binary in `main.rs` is the real entry point; this surface exists for the protocol tests,
//! which need the same `SessionLockManagerState` and handler code the compositor installs.
//!
//! Only `session_lock` is public: it is the module whose observable effect (an advertised
//! global) is testable without a seat, a GPU or a running compositor.

pub mod lock_global;
/// Pure policy for the external protocols (toplevel listing, gamma ramps).
pub mod protocols;
