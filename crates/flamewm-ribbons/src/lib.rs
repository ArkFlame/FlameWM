//! Deterministic "Ribbons" screensaver: a pure simulation plus a CPU rasterizer.
//!
//! Multi-monitor: `Config::span_rects` in global coordinates. The simulation runs on the
//! union bounds of all rects; `Ribbons::covers` tells the output which pixels lie inside a
//! rect, and `Rasterizer::render` paints the L-shaped gaps black. Alternatively, create one
//! `Ribbons` per output with a single rect for independent per-output animation.
#![forbid(unsafe_code)]

mod colour;
mod config;
mod curve;
mod profile;
mod raster;
mod rng;
mod sim;

pub use colour::palette;
pub use config::{Config, Rect};
pub use raster::{Quality, Rasterizer};
pub use sim::{DT, HISTORY_CAPACITY, Ribbons, Sample, Stream};
