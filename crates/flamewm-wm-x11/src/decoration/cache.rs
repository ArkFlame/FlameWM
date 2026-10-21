//! Catalog icon conversion for the WM loop.
//!
//! Resolution and decoding belong to `IconService`; this module only converts
//! the worker-produced straight RGBA8 raster into the cached ARGB32 form.

/// Convert a worker-produced straight RGBA8 catalog raster to ARGB32.
/// Resolution and decoding belong to `IconService`, never the WM event loop.
#[must_use]
pub fn catalog_icon_image(
    raster: &flamewm_integrations_linux::icons::Rgba8Raster,
) -> Option<crate::chrome::IconImage> {
    if raster.width == 0 || raster.height == 0 {
        return None;
    }
    let pixel_count = usize::try_from(raster.width)
        .ok()?
        .checked_mul(usize::try_from(raster.height).ok()?)?;
    if raster.pixels.len() != pixel_count.saturating_mul(4) {
        return None;
    }
    // Straight RGBA8 -> cached ARGB32 `IconImage` (pure memory convert).
    // Miss keeps the empty slot, never brand artwork.
    let argb = raster
        .pixels
        .chunks_exact(4)
        .map(|pixel| {
            u32::from(pixel[3]) << 24
                | u32::from(pixel[0]) << 16
                | u32::from(pixel[1]) << 8
                | u32::from(pixel[2])
        })
        .collect::<Vec<u32>>();
    Some(crate::chrome::IconImage {
        width: raster.width,
        height: raster.height,
        argb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_catalog_raster_converts_to_argb_without_resolving() {
        let raster = flamewm_integrations_linux::icons::Rgba8Raster {
            source: "worker".to_owned(),
            width: 1,
            height: 1,
            pixels: vec![0x11, 0x22, 0x33, 0x80],
            origin: flamewm_integrations_linux::icons::IconOrigin::SystemTheme,
            fallback_reason: None,
        };
        let image = catalog_icon_image(&raster).expect("valid worker raster");
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.argb, vec![0x8011_2233]);
    }
}
