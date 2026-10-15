//! Native target/presenter convergence (J07).
//! Transactional geometry commit + explicit [`PresentationState`] machine.

pub mod coverage;
pub mod cursor;
pub mod image;
pub(crate) mod rounded;
pub mod surface_format;
pub mod target;
pub mod xresource;

pub use target::{PresentationState, presenter_error};
