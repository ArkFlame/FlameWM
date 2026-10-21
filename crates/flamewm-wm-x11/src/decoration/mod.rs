//! Retired decoration facades.
//!
//! J16: the live frame owner is `crate::frame` (model/input/layout/
//! geometry/chrome/resources/session/controller). The old `decoration::`
//! manager/paint/pointer-zone/interaction/layout/model/shape/identity
//! production paths are deleted. `cache` (glyph/raster/title-measure
//! catalog icon conversion (`cache`) and the decoration-policy decision
//! (`policy`) remain.

pub mod cache;
pub mod policy;
