//! Desktop wallpaper placement: the shared owner lives in `flamewm_image_core::wallpaper`.

pub use flamewm_image_core::wallpaper::{compose, parse};

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::background::WallpaperFit;
    use flamewm_image_core::{RgbaImage, wallpaper::BACKGROUND};

    /// 4x2 image: red encodes the column (0, 60, 120, 180), green encodes the row (0, 100).
    fn columns_image() -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                pixels.extend_from_slice(&[x * 60, y * 100, 0, 255]);
            }
        }
        RgbaImage {
            width: 4,
            height: 2,
            pixels,
        }
    }

    fn at(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * image.width + x) * 4) as usize;
        image.pixels[index..index + 4].try_into().expect("pixel")
    }

    #[test]
    fn parse_maps_wire_values_and_defaults_to_fill() {
        assert_eq!(parse("fill"), WallpaperFit::Fill);
        assert_eq!(parse("fit"), WallpaperFit::Fit);
        assert_eq!(parse("stretch"), WallpaperFit::Stretch);
        assert_eq!(parse("center"), WallpaperFit::Center);
        assert_eq!(parse("tile"), WallpaperFit::Tile);
        assert_eq!(parse(""), WallpaperFit::Fill);
        assert_eq!(parse("Tile"), WallpaperFit::Fill);
        assert_eq!(parse("zoom"), WallpaperFit::Fill);
    }

    #[test]
    fn fill_crops_the_overflow_evenly_without_stretching() {
        // 4x2 into 2x2 covers by height; the middle two columns survive unscaled.
        let image = compose(&columns_image(), 2, 2, WallpaperFit::Fill).expect("fill");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(at(&image, 0, 0), [60, 0, 0, 255]);
        assert_eq!(at(&image, 1, 0), [120, 0, 0, 255]);
        assert_eq!(at(&image, 0, 1), [60, 100, 0, 255]);
        assert_eq!(at(&image, 1, 1), [120, 100, 0, 255]);
    }

    #[test]
    fn fit_letterboxes_in_the_desktop_background_colour() {
        // 4x2 into 4x4 keeps the aspect ratio: one black bar above and below.
        let image = compose(&columns_image(), 4, 4, WallpaperFit::Fit).expect("fit");
        assert_eq!(at(&image, 0, 0), BACKGROUND);
        assert_eq!(at(&image, 3, 3), BACKGROUND);
        assert_eq!(at(&image, 2, 1), [120, 0, 0, 255]);
        assert_eq!(at(&image, 2, 2), [120, 100, 0, 255]);
    }

    #[test]
    fn stretch_fills_the_whole_target_and_differs_from_cover() {
        let stretched = compose(&columns_image(), 2, 2, WallpaperFit::Stretch).expect("stretch");
        let covered = compose(&columns_image(), 2, 2, WallpaperFit::Fill).expect("fill");
        assert_eq!((stretched.width, stretched.height), (2, 2));
        assert_ne!(stretched.pixels, covered.pixels);
    }

    #[test]
    fn center_keeps_natural_size_and_crops_when_larger() {
        let smaller = compose(&columns_image(), 6, 4, WallpaperFit::Center).expect("center");
        assert_eq!(at(&smaller, 0, 0), BACKGROUND);
        assert_eq!(at(&smaller, 0, 1), BACKGROUND);
        assert_eq!(at(&smaller, 2, 1), [60, 0, 0, 255]);
        assert_eq!(at(&smaller, 5, 1), BACKGROUND);

        let cropped = compose(&columns_image(), 2, 2, WallpaperFit::Center).expect("center crop");
        assert_eq!(at(&cropped, 0, 0), [60, 0, 0, 255]);
        assert_eq!(at(&cropped, 1, 1), [120, 100, 0, 255]);
    }

    #[test]
    fn tile_repeats_the_natural_image_from_the_origin() {
        let tiled = compose(&columns_image(), 6, 3, WallpaperFit::Tile).expect("tile");
        assert_eq!(at(&tiled, 0, 0), [0, 0, 0, 255]);
        assert_eq!(at(&tiled, 4, 0), [0, 0, 0, 255]);
        assert_eq!(at(&tiled, 5, 0), [60, 0, 0, 255]);
        assert_eq!(at(&tiled, 4, 1), [0, 100, 0, 255]);
        assert_eq!(at(&tiled, 5, 2), [60, 0, 0, 255]);
    }

    #[test]
    fn zero_sized_inputs_compose_to_nothing() {
        assert!(compose(&columns_image(), 0, 2, WallpaperFit::Fill).is_none());
        assert!(compose(&columns_image(), 2, 0, WallpaperFit::Tile).is_none());
    }
}
