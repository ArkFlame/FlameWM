//! Pure policy behind the external-facing Wayland protocols (toplevel listing, gamma control).
//!
//! The protocol wiring lives in the compositor binary; these units hold the decisions so they can
//! be tested without a display, a seat or a GPU.

pub mod gamma;
pub mod toplevel;
