mod document;
mod geometry;
mod interaction;
mod style;
#[cfg(test)]
mod tests;

pub use document::*;
pub use geometry::*;
pub use interaction::*;
pub use style::*;

pub const FORMAT_MAGIC: [u8; 4] = *b"RWRB";
/// Current asset pixel encoding: straight (non-premultiplied) RGBA8.
/// Version 5 appends text-wrap bytes per style; version 4 is identical
/// RGBA8 without them (but with the image-treatment byte). Version 3 is
/// identical RGBA8 without that byte. Version 2 documents carry opaque
/// RGB8 assets; the codec upconverts them to RGBA8 with alpha=255 so PPM
/// build inputs keep working.
pub const FORMAT_VERSION: u16 = 5;
pub const FORMAT_VERSION_TREATMENT_LEGACY: u16 = 4;
pub const FORMAT_VERSION_RGBA8_LEGACY: u16 = 3;
pub const FORMAT_VERSION_RGB8_LEGACY: u16 = 2;
