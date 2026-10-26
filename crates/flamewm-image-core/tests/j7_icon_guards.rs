//! J7 icon/alpha ratchet, G1/G2/G5 surface half: canonical Flame surfaces
//! are ARGB-validated with no blend-over-black and no `XCreateSimpleWindow`.
//!
//! `flamewm-image-core` is the shared RGBA8 owner: straight alpha, never a
//! magenta color-key. `flamewm-render-x11` stays the sole native surface
//! owner; this test pins the shared contract from the image side.

use flamewm_image_core::{ImageSourceFormat, RgbaImage};

#[test]
fn rgba_alpha_decides_transparency_not_magenta() {
    let magenta = RgbaImage::from_rgba8(1, 1, vec![255, 0, 255, 255]).expect("magenta pixel");
    assert!(magenta.is_fully_opaque());
    assert_eq!(magenta.alpha_at(0, 0), Some(255));
    let clear = RgbaImage::from_rgba8(1, 1, vec![10, 20, 30, 0]).expect("clear pixel");
    assert!(!clear.is_fully_opaque());
    assert_eq!(clear.alpha_at(0, 0), Some(0));
}

#[test]
fn opaque_rgb8_upconverts_without_color_key() {
    let image = RgbaImage::from_rgb8_opaque(1, 1, &[255, 0, 255]).expect("opaque magenta");
    assert_eq!(image.pixels, vec![255, 0, 255, 255]);
    assert!(image.is_fully_opaque());
}

#[test]
fn ppm_format_decodes_opaque_with_alpha_255() {
    let bytes = b"P6\n1 1\n255\n\xff\0\xff".to_vec();
    let image =
        flamewm_image_core::decode_bytes(&bytes, ImageSourceFormat::Ppm).expect("PPM decodes");
    assert_eq!(image.pixels, vec![255, 0, 255, 255]);
    assert!(image.is_fully_opaque());
}
