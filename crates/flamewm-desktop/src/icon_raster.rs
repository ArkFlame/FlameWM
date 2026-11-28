use flamewm_integrations_linux::icons::{IconOrigin, Rgba8Raster};

pub(crate) const ICON_TILE_PX: u32 = 44;

/// Half-opacity ghost pixels while preserving straight-alpha RGB and the
/// original icon cache. This keeps the drag preview visually consistent with
/// the 50% label and independent of renderer node-group alpha support.
pub(crate) fn fade_ghost_pixels(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[3] = u16::from(pixel[3]).div_ceil(2) as u8;
    }
}

/// Flame-red fallback raster (brand 239,64,72) painted synchronously on a
/// cache miss; the worker result overwrites it. Not a placeholder shape: a
/// real RGBA8 raster in the tile's selected semantic slot.
pub(crate) fn flame_fallback_raster() -> Rgba8Raster {
    static CACHED: std::sync::OnceLock<Rgba8Raster> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            let pixels =
                [239_u8, 64, 72, 255].repeat(ICON_TILE_PX as usize * ICON_TILE_PX as usize);
            Rgba8Raster {
                source: "flamewm-fallback".to_owned(),
                width: ICON_TILE_PX,
                height: ICON_TILE_PX,
                pixels,
                origin: IconOrigin::PackagedFallback,
                fallback_reason: None,
            }
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_raster_is_cached_and_clones_are_independent() {
        let first = flame_fallback_raster();
        let second = flame_fallback_raster();
        assert_eq!(first.pixels, second.pixels);
        assert_eq!(first.source, second.source);
        assert_eq!(first.width, second.width);
        assert_eq!(first.height, second.height);
        assert_eq!(first.origin, second.origin);

        let mut faded = first.clone();
        fade_ghost_pixels(&mut faded.pixels);
        assert_ne!(
            faded.pixels, first.pixels,
            "fade must touch the local clone"
        );
        assert_eq!(flame_fallback_raster().pixels, first.pixels);
    }
}
