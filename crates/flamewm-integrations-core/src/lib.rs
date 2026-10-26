//! Reactor-neutral product contracts for `NetworkManager`, MPRIS and `PulseAudio`.
//!
//! This crate ports the lifecycle, generation, reconciliation and intent rules from the current
//! native `FlameWM` integrations. It deliberately does not choose a D-Bus or Pulse event loop.

mod backoff;
pub mod dispatcher;
pub mod mpris;
pub mod network;
pub mod pulse;
