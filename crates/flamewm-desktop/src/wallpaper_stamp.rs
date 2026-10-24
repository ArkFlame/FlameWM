use std::sync::Mutex;

use crate::wallpaper_loader::WallpaperRaster;

/// Identity of the wallpaper last written to the document. The pixel buffer
/// pointer and length change whenever the loader replaces the raster, so an
/// unchanged raster skips the full-buffer clone without a content compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WallpaperStamp {
    source: String,
    width: u32,
    height: u32,
    pixels_ptr: usize,
    pixels_len: usize,
}

/// Last image projected by [`project_background`]. Cleared when the wallpaper is
/// not an image so a later image re-sends.
pub(crate) static WALLPAPER_STAMP: Mutex<Option<WallpaperStamp>> = Mutex::new(None);

pub(crate) fn wallpaper_stamp(raster: &WallpaperRaster) -> WallpaperStamp {
    WallpaperStamp {
        source: raster.source.clone(),
        width: raster.width,
        height: raster.height,
        pixels_ptr: raster.pixels.as_ptr() as usize,
        pixels_len: raster.pixels.len(),
    }
}

/// Compares a cached stamp against `raster` without allocating.
pub(crate) fn wallpaper_matches(stamp: &WallpaperStamp, raster: &WallpaperRaster) -> bool {
    stamp.source == raster.source
        && stamp.width == raster.width
        && stamp.height == raster.height
        && stamp.pixels_ptr == raster.pixels.as_ptr() as usize
        && stamp.pixels_len == raster.pixels.len()
}
