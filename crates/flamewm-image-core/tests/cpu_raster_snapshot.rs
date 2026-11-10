//! Bit-exact snapshot of CPU raster ops on a deterministic input set.
//!
//! The expected digests were captured from the pre-optimisation code. Any
//! change to blur, frosted backdrop or scale output must keep them identical.
//! Exception: `blur_r12_96x40` and `frosted_r9_tint` sit in the 2x downscale tier
//! (radius above 6, see `blur.rs`). Their digests were re-captured after that change; against the
//! previous output on this white-noise fixture the mean abs diff is 1.39 and 0.22 (of 255).

use flamewm_image_core::blur::{BlurScratch, frosted_backdrop_with, gaussian_blur_rgba_with};
use flamewm_image_core::scale::{ScaleFilter, scale_rgba};
use flamewm_image_core::wallpaper::{self, WallpaperFit};
use flamewm_image_core::{Rect, RgbaImage};

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Deterministic LCG; no external RNG dependency.
fn pattern(width: u32, height: u32, seed: u32) -> RgbaImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for _ in 0..(width as usize * height as usize * 4) {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        pixels.push((state >> 24) as u8);
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn blur_digest(width: u32, height: u32, radius: f32) -> u64 {
    let mut image = pattern(width, height, 7);
    let mut scratch = BlurScratch::new();
    gaussian_blur_rgba_with(&mut scratch, &mut image.pixels, width, height, radius)
        .expect("blur accepts valid input");
    fnv1a(&image.pixels)
}

fn frosted_digest(radius: f32, tint: Option<[u8; 4]>) -> u64 {
    let src = pattern(131, 77, 11);
    let crop = Rect {
        x: -9,
        y: 5,
        width: 97,
        height: 61,
    };
    let mut scratch = BlurScratch::new();
    let out = frosted_backdrop_with(&mut scratch, &src, crop, radius, tint);
    fnv1a(&out.pixels)
}

fn scale_digest(filter: ScaleFilter, width: u32, height: u32) -> u64 {
    let src = pattern(53, 41, 3);
    let out = scale_rgba(&src, width, height, filter).expect("non-zero scale");
    fnv1a(&out.pixels)
}

fn wallpaper_digest(fit: WallpaperFit) -> u64 {
    let natural = pattern(37, 23, 5);
    let out = wallpaper::compose(&natural, 101, 67, fit).expect("non-zero compose");
    fnv1a(&out.pixels)
}

fn cases() -> Vec<(&'static str, u64)> {
    vec![
        ("wallpaper_fill", wallpaper_digest(WallpaperFit::Fill)),
        ("wallpaper_fit", wallpaper_digest(WallpaperFit::Fit)),
        ("wallpaper_stretch", wallpaper_digest(WallpaperFit::Stretch)),
        ("wallpaper_center", wallpaper_digest(WallpaperFit::Center)),
        ("wallpaper_tile", wallpaper_digest(WallpaperFit::Tile)),
        ("blur_r0.5_33x17", blur_digest(33, 17, 0.5)),
        ("blur_r3_64x48", blur_digest(64, 48, 3.0)),
        ("blur_r12_96x40", blur_digest(96, 40, 12.0)),
        ("blur_r33_101x67", blur_digest(101, 67, 33.0)),
        ("blur_r64_128x90", blur_digest(128, 90, 64.0)),
        ("frosted_r0", frosted_digest(0.0, None)),
        (
            "frosted_r9_tint",
            frosted_digest(9.0, Some([10, 20, 30, 128])),
        ),
        ("frosted_r40", frosted_digest(40.0, None)),
        (
            "scale_nearest_37x29",
            scale_digest(ScaleFilter::Nearest, 37, 29),
        ),
        (
            "scale_bilinear_131x97",
            scale_digest(ScaleFilter::Bilinear, 131, 97),
        ),
        ("scale_area_19x13", scale_digest(ScaleFilter::Area, 19, 13)),
    ]
}

#[test]
fn frosted_crop_that_overflows_usize_returns_transparent_pixel() {
    let src = pattern(4, 4, 1);
    let crop = Rect {
        x: 0,
        y: 0,
        width: u32::MAX,
        height: u32::MAX,
    };
    let mut scratch = BlurScratch::new();
    let out = frosted_backdrop_with(&mut scratch, &src, crop, 8.0, None);
    assert_eq!((out.width, out.height), (1, 1));
    assert_eq!(out.pixels, vec![0, 0, 0, 0]);
}

#[test]
fn one_pixel_frosted_crop_keeps_its_source_colour_at_zero_radius() {
    let src = pattern(4, 4, 2);
    let crop = Rect {
        x: 2,
        y: 1,
        width: 1,
        height: 1,
    };
    let mut scratch = BlurScratch::new();
    let out = frosted_backdrop_with(&mut scratch, &src, crop, 0.0, None);
    let index = (4 + 2) * 4;
    assert_eq!((out.width, out.height), (1, 1));
    assert_eq!(out.pixels, src.pixels[index..index + 4]);
}

#[test]
fn cpu_raster_outputs_match_pinned_snapshot() {
    // Digits are kept as captured (no separators) so they can be compared by eye.
    #[allow(clippy::unreadable_literal)]
    let expected: &[(&str, u64)] = &[
        ("wallpaper_fill", 0x77d53703472ea41d),
        ("wallpaper_fit", 0xb1cc21fa7dfa19f0),
        ("wallpaper_stretch", 0x26f01a94b9fd4e4e),
        ("wallpaper_center", 0x0ce0e0d522587d1c),
        ("wallpaper_tile", 0xce62a962f7d4402c),
        ("blur_r0.5_33x17", 0x93a11cf02d6aa338),
        ("blur_r3_64x48", 0x8c4acb436519d83e),
        ("blur_r12_96x40", 0x1a926f5f7463c6c3),
        ("blur_r33_101x67", 0x8173ef7dd12acf92),
        ("blur_r64_128x90", 0xdf7df2cb77316f84),
        ("frosted_r0", 0x2cda3c14a5335fbf),
        ("frosted_r9_tint", 0x26c7b9ea9b95f5e1),
        ("frosted_r40", 0xf13185019baf2187),
        ("scale_nearest_37x29", 0xb47a4f638e756373),
        ("scale_bilinear_131x97", 0xf056851dfcdb9b89),
        ("scale_area_19x13", 0x7f136691936c5577),
    ];
    let actual = cases();
    assert_eq!(actual.len(), expected.len(), "snapshot case count");
    let mut changed = Vec::new();
    for ((name, digest), (exp_name, exp_digest)) in actual.iter().zip(expected) {
        assert_eq!(name, exp_name);
        if digest != exp_digest {
            changed.push(format!(
                "{name}: got {digest:#018x} want {exp_digest:#018x}"
            ));
        }
    }
    assert!(
        changed.is_empty(),
        "snapshot changed:\n{}",
        changed.join("\n")
    );
}
