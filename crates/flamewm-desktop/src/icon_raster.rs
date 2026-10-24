use flamewm_integrations_linux::icons::{IconOrigin, Rgba8Raster};
use flamewm_ui_x11::RuntimeImage;

pub(crate) const ICON_TILE_PX: u32 = 44;

/// Half-opacity ghost pixels while preserving straight-alpha RGB and the
/// original icon cache. This keeps the drag preview visually consistent with
/// the 50% label and independent of renderer node-group alpha support.
pub(crate) fn fade_ghost_pixels(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[3] = ((u16::from(pixel[3]) + 1) / 2) as u8;
    }
}

/// Flame-red fallback raster (brand 239,64,72) painted synchronously on a
/// cache miss; the worker result overwrites it. Not a placeholder shape: a
/// real RGBA8 raster in the tile's selected semantic slot.
pub(crate) fn flame_fallback_raster() -> Rgba8Raster {
    let pixels = [239_u8, 64, 72, 255].repeat(ICON_TILE_PX as usize * ICON_TILE_PX as usize);
    Rgba8Raster {
        source: "flamewm-fallback".to_owned(),
        width: ICON_TILE_PX,
        height: ICON_TILE_PX,
        pixels,
        origin: IconOrigin::PackagedFallback,
        fallback_reason: None,
    }
}

pub(crate) fn runtime_image(raster: Rgba8Raster) -> RuntimeImage {
    RuntimeImage {
        source: raster.source,
        width: raster.width,
        height: raster.height,
        pixels: raster.pixels,
    }
}
